mod api;
mod audio;
mod compression;
mod config;
mod pipeline;
mod playlists;
mod sentences;
mod store;
#[cfg(feature="native-asr")] mod capture;
#[cfg(feature="native-asr")] mod recognition;
use anyhow::{Result,Context,bail};
use clap::Parser;
use std::{path::PathBuf,sync::{Arc,Mutex},time::Duration};
use config::Config;
use pipeline::Pipeline;
use store::Store;

#[derive(Parser,Debug)]
#[command(version,about="CPU-only railway radio transcription and clip archive")]
struct Args{
    #[arg(long,default_value="config.toml")] config:PathBuf,
    #[arg(long,conflicts_with="no_capture")] demo:bool,
    #[arg(long)] no_capture:bool,
}
fn seed_demo(cfg:&Config,store:&mut Store)->Result<()> {
    if store.stats()?.clips>0{return Ok(());}
    for (i,ch) in cfg.channels.iter().enumerate(){
        let samples:Vec<f32>=(0..audio::RATE*2).map(|t|(t as f32*440.0*std::f32::consts::TAU/audio::RATE as f32).sin()*0.08).collect();
        let id=store.add_clip(ch,audio::now_ms()-i as i64*73_000,&samples)?;
        store.finish(&id,&[sentences::Sentence{text:"Synthetic playback tone. No speech in this demo fixture.".into(),start_ms:Some(0),end_ms:Some(2000)}],"synthetic-demo",None)?;
    }
    Ok(())
}
async fn shutdown_signal(){
    #[cfg(unix)]{
        let mut term=tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).expect("SIGTERM handler");
        tokio::select!{_=tokio::signal::ctrl_c()=>{},_=term.recv()=>{}}
    }
    #[cfg(not(unix))] {let _=tokio::signal::ctrl_c().await;}
}
#[tokio::main]
async fn main()->Result<()> {
    tracing_subscriber::fmt().with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_|"track_listener=info,tower_http=info".into())).init();
    let args=Args::parse();let mut cfg=Config::load(&args.config)?;
    if args.demo {
        // Never mix synthetic fixtures with production data. Use an isolated archive.
        cfg.server.database=PathBuf::from("data/demo.sqlite3");cfg.audio.path=PathBuf::from("data/demo-clips");
    }
    let cfg=Arc::new(cfg);let live=!args.demo&&!args.no_capture;
    #[cfg(not(feature="native-asr"))]
    if live{bail!("built without native-asr; rebuild with default features or use --demo/--no-capture");}
    let listener=tokio::net::TcpListener::bind((cfg.server.host,cfg.server.port)).await.context("bind HTTP listener")?;
    let mut store=Store::open(&cfg.server.database,cfg.audio.clone())?;
    if args.demo{seed_demo(&cfg,&mut store)?;}
    let store=Arc::new(Mutex::new(store));
    let mut p=Pipeline::new(&cfg,args.demo,live);
    #[cfg(feature="native-asr")]
    if live{recognition::start_workers(&mut p,&cfg,store.clone())?;capture::start(&mut p,cfg.clone(),store.clone())?;}
    let compression=tokio::spawn(compression::run(store.clone()));
    let archive=store.clone();
    let cleanup=tokio::spawn(async move{loop{tokio::time::sleep(Duration::from_secs(30)).await;let archive=archive.clone();let r=tokio::task::spawn_blocking(move||archive.lock().unwrap().prune()).await;if let Ok(Err(e))=r{tracing::error!(%e,"retention cleanup failed");}}});
    let app=api::router(api::AppState{store,channels:p.statuses.clone(),queue:p.queue.clone(),demo:args.demo,capture_enabled:live},cfg.server.web_dist.clone());
    tracing::info!(address=%listener.local_addr()?,live,"Track Listener started");
    let result=axum::serve(listener,app).with_graceful_shutdown(shutdown_signal()).await;
    cleanup.abort();compression.abort();let _=compression.await;p.stop().await;result?;Ok(())
}
