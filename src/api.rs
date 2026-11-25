use crate::{pipeline::{Job,SharedStore,Statuses},store::{Filters,Page,Store}};
use axum::{Router, Json, body::Body, extract::{Path,Query,State}, http::{HeaderMap,Method,StatusCode,header}, response::{IntoResponse,Response}, routing::get};
use bytes::Bytes;
use serde_json::{Value,json};
use std::path::PathBuf;
use tower_http::{services::{ServeDir,ServeFile},trace::TraceLayer};

#[derive(Clone)]
pub struct AppState {pub store:SharedStore,pub channels:Statuses,pub queue:crossbeam_channel::Receiver<Job>,pub demo:bool,pub capture_enabled:bool}
#[derive(Debug)]
struct ApiError(StatusCode,String);
impl IntoResponse for ApiError {fn into_response(self)->Response{(self.0,Json(json!({"detail":self.1}))).into_response()}}
fn internal(e:impl std::fmt::Display)->ApiError {tracing::error!(%e,"API storage failure");ApiError(StatusCode::INTERNAL_SERVER_ERROR,"Storage operation failed".into())}
async fn db<T:Send+'static>(s:SharedStore,op:impl FnOnce(&mut Store)->anyhow::Result<T>+Send+'static)->Result<T,ApiError>{
    tokio::task::spawn_blocking(move || {let mut store=s.lock().map_err(|_|anyhow::anyhow!("store lock poisoned"))?;op(&mut store)}).await.map_err(internal)?.map_err(internal)
}
async fn channels(State(s):State<AppState>)->Json<Vec<crate::pipeline::ChannelStatus>> {Json(s.channels.lock().unwrap().clone())}
async fn status(State(s):State<AppState>)->Result<Json<Value>,ApiError>{
    let archive=db(s.store.clone(),Store::stats).await?;
    Ok(Json(json!({"archive":archive,"queue":{"depth":s.queue.len(),"capacity":s.queue.capacity()},"demo":s.demo,"capture_enabled":s.capture_enabled})))
}
async fn clips(State(s):State<AppState>,Query(f):Query<Filters>)->Result<Json<Page>,ApiError>{
    f.validate().map_err(|e|ApiError(StatusCode::BAD_REQUEST,e.to_string()))?;
    Ok(Json(db(s.store,move|store|store.list(&f)).await?))
}
fn range(value:&str,size:usize)->Option<(usize,usize)>{
    if size==0{return None;}
    let (a,b)=value.trim().strip_prefix("bytes=")?.split_once('-')?;
    if a.is_empty(){let suffix=b.parse::<usize>().ok()?;if suffix==0{return None;}Some((size.saturating_sub(suffix),size-1))}
    else{let start=a.parse::<usize>().ok()?;let end=if b.is_empty(){size-1}else{b.parse::<usize>().ok()?.min(size-1)};if start>=size||end<start{None}else{Some((start,end))}}
}
fn audio_response(data:Bytes,mime:&str,id:&str,headers:&HeaderMap,head:bool)->Response{
    let len=data.len();
    // Different validators for the temporary WAV and the final MP4 prevent
    // a conditional range request from splicing different representations.
    let etag=format!("\"{id}-{mime}\"");
    let mut response=Response::builder().header(header::ACCEPT_RANGES,"bytes").header(header::CACHE_CONTROL,"no-store").header(header::CONTENT_TYPE,mime).header(header::ETAG,etag.as_str());
    let requested_range=headers.get(header::RANGE).filter(|_|headers.get(header::IF_RANGE).is_none_or(|v|v.as_bytes()==etag.as_bytes()));
    let (status,start,end)=if let Some(raw)=requested_range{
        if let Some((a,b))=raw.to_str().ok().and_then(|v|range(v,len)){(StatusCode::PARTIAL_CONTENT,a,b+1)}
        else{return response.status(StatusCode::RANGE_NOT_SATISFIABLE).header(header::CONTENT_RANGE,format!("bytes */{len}")).body(Body::empty()).unwrap();}
    }else{(StatusCode::OK,0,len)};
    if status==StatusCode::PARTIAL_CONTENT{response=response.header(header::CONTENT_RANGE,format!("bytes {start}-{}/{len}",end-1));}
    response.status(status).header(header::CONTENT_LENGTH,(end-start).to_string()).body(if head{Body::empty()}else{Body::from(data.slice(start..end))}).unwrap()
}
async fn audio(State(s):State<AppState>,Path(id):Path<String>,method:Method,headers:HeaderMap)->Result<Response,ApiError>{
    let clip_id=id.clone();
    let (data,mime)=db(s.store,move|store|store.get_audio(&id)).await?.ok_or_else(||ApiError(StatusCode::NOT_FOUND,"Audio expired or unavailable; transcript may remain".into()))?;
    Ok(audio_response(data,mime,&clip_id,&headers,method==Method::HEAD))
}
async fn health(State(s):State<AppState>)->Json<Value>{Json(json!({"ok":true,"capture_enabled":s.capture_enabled}))}
pub fn router(state:AppState,web:PathBuf)->Router{
    let app=Router::new().route("/api/channels",get(channels)).route("/api/status",get(status)).route("/api/clips",get(clips)).route("/api/clips/{id}/audio",get(audio).head(audio)).route("/health",get(health));
    let app=if web.join("index.html").is_file(){app.route_service("/",ServeFile::new(web.join("index.html"))).nest_service("/assets",ServeDir::new(web.join("assets")))}else{app.route("/",get(||async{(StatusCode::SERVICE_UNAVAILABLE,"Frontend not built: cd web && npm install && npm run build")}))};
    app.layer(TraceLayer::new_for_http()).with_state(state)
}
#[cfg(test)] mod tests{
    use super::*;
    #[test]fn ranges(){assert_eq!(range("bytes=0-2",10),Some((0,2)));assert_eq!(range("bytes=-3",10),Some((7,9)));assert_eq!(range("bytes=8-",10),Some((8,9)));assert_eq!(range("bytes=8-99",10),Some((8,9)));assert_eq!(range("bytes=10-",10),None);assert_eq!(range("bytes=-0",10),None);assert_eq!(range("bytes=1-2,4-5",10),None);}
    #[test]fn changed_representation_ignores_old_if_range(){
        let mut h=HeaderMap::new();
        h.insert(header::RANGE,"bytes=2-4".parse().unwrap());
        h.insert(header::IF_RANGE,"\"test-audio/wav\"".parse().unwrap());
        let r=audio_response(Bytes::from_static(b"0123456789"),"audio/mp4","test",&h,true);
        assert_eq!(r.status(),200);
        assert_eq!(r.headers()[header::CONTENT_LENGTH],"10");
    }
    #[tokio::test]async fn partial_response(){let mut h=HeaderMap::new();h.insert(header::RANGE,"bytes=2-4".parse().unwrap());let r=audio_response(Bytes::from_static(b"0123456789"),"audio/mp4","test",&h,false);assert_eq!(r.status(),206);assert_eq!(r.headers()[header::CONTENT_TYPE],"audio/mp4");assert_eq!(r.headers()[header::CONTENT_RANGE],"bytes 2-4/10");assert_eq!(axum::body::to_bytes(r.into_body(),1024).await.unwrap(),Bytes::from_static(b"234"));}
}
