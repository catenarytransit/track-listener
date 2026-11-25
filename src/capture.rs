use crate::{audio::{RATE,duration_ms,now_ms}, config::{Config,Channel}, pipeline::{Job,Pipeline,SharedStore,Statuses}, playlists, recognition};
use anyhow::{Context, Result, bail};
use crossbeam_channel::Sender;
use sherpa_onnx::VoiceActivityDetector;
use std::{process::Stdio, sync::Arc, time::{Duration,Instant}};
use tokio::{io::AsyncReadExt, process::Command, time::{sleep,timeout}};

struct Capture {
    cfg:Arc<Config>, channel:Channel, index:usize, store:SharedStore, statuses:Statuses, tx:Sender<Job>,
    client:reqwest::Client, vad:VoiceActivityDetector,
}
impl Capture {
    fn state(&self,state:&str,detail:Option<String>) {
        let mut s=self.statuses.lock().unwrap(); let ch=&mut s[self.index];
        ch.state=state.into(); ch.detail=detail;
        if state=="reconnecting" {ch.errors+=1;}
    }
    async fn submit(&self, started:i64,samples:Vec<f32>) {
        let store=self.store.clone(); let ch=self.channel.clone(); let tx=self.tx.clone();
        let result=tokio::task::spawn_blocking(move || -> Result<bool> {
            let id=store.lock().unwrap().add_clip(&ch,started,&samples)?;
            if let Err(e)=tx.try_send(Job{id:id.clone(),samples}) {
                let msg=if e.is_full(){"recognition queue full"}else{"recognition workers disconnected"};
                store.lock().unwrap().finish(&id,&[],"not-transcribed",Some(msg))?;
                return Ok(false);
            }
            Ok(true)
        }).await;
        match result {
            Ok(Ok(true))=>{},
            other=>{
                self.statuses.lock().unwrap()[self.index].dropped_clips+=1;
                tracing::warn!(channel=%self.channel.id,?other,"clip not queued");
            }
        }
    }
    async fn drain(&self,origin:i64) {
        while let Some(s)=self.vad.front() {
            let start=s.start().max(0) as usize;
            let audio=s.samples().to_vec();
            drop(s);
            self.vad.pop();
            let cap=self.cfg.audio.max_clip_seconds as usize*RATE;
            for (i,chunk) in audio.chunks(cap).enumerate() {
                if !chunk.is_empty() {self.submit(origin+duration_ms(start+i*cap),chunk.to_vec()).await;}
            }
        }
    }
    async fn receive(&self,url:&reqwest::Url)->Result<()> {
        self.vad.reset();
        let mut child=Command::new("ffmpeg")
            .args(["-nostdin","-hide_banner","-loglevel","error","-rw_timeout","15000000","-protocol_whitelist","http,https,tcp,tls,crypto","-i",url.as_str(),"-vn","-sn","-dn","-ac","1","-ar","16000","-c:a","pcm_s16le","-f","s16le","pipe:1"])
            .stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::inherit()).kill_on_drop(true).spawn().context("launch ffmpeg")?;
        let mut pipe=child.stdout.take().context("ffmpeg stdout missing")?;
        let mut origin=None;
        let mut frame=[0u8;1024];
        let mut last_status=Instant::now();
        let reason=loop {
            match timeout(Duration::from_secs(20),pipe.read_exact(&mut frame)).await {
                Ok(Ok(_))=>{}, Ok(Err(e))=>break format!("audio stream ended: {e}"), Err(_)=>break "audio read timed out".into(),
            }
            if origin.is_none() {
                origin=Some(now_ms()-duration_ms(512));self.state("receiving",None);
                self.statuses.lock().unwrap()[self.index].last_audio_ms=Some(now_ms());
            }
            if last_status.elapsed()>=Duration::from_secs(1) {self.statuses.lock().unwrap()[self.index].last_audio_ms=Some(now_ms());last_status=Instant::now();}
            let mut samples=[0f32;512];
            for (s,pair) in samples.iter_mut().zip(frame.chunks_exact(2)) { *s=i16::from_le_bytes([pair[0],pair[1]]) as f32/32768.0; }
            self.vad.accept_waveform(&samples);
            self.drain(origin.unwrap()).await;
        };
        if let Some(origin)=origin { self.vad.flush();self.drain(origin).await; }
        let _=child.kill().await; let _=child.wait().await;
        bail!("{reason}")
    }
    async fn run(self) {
        let mut delay=2;
        loop {
            self.state("connecting",None);
            match playlists::resolve(&self.client,&self.channel.url).await {
                Ok(urls)=>{
                    for url in urls {
                        let started=Instant::now();
                        self.state("connecting",None);
                        if let Err(e)=self.receive(&url).await {self.state("reconnecting",Some(e.to_string()));tracing::warn!(channel=%self.channel.id,%e,"radio stream disconnected");}
                        if started.elapsed()>Duration::from_secs(60) {delay=2;}
                    }
                },
                Err(e)=>{self.state("reconnecting",Some(e.to_string()));tracing::warn!(channel=%self.channel.id,%e,"playlist resolution failed");}
            }
            sleep(Duration::from_secs(delay)).await;delay=(delay*2).min(60);
        }
    }
}
pub fn start(p:&mut Pipeline,cfg:Arc<Config>,store:SharedStore)->Result<()> {
    let client=reqwest::Client::builder().user_agent("TrackListener/0.2").timeout(Duration::from_secs(15)).redirect(reqwest::redirect::Policy::limited(5)).build()?;
    let mut captures=Vec::new();
    for (index,channel) in cfg.channels.iter().enumerate().filter(|(_,c)|c.enabled) {
        captures.push(Capture{cfg:cfg.clone(),channel:channel.clone(),index,store:store.clone(),statuses:p.statuses.clone(),tx:p.sender.as_ref().context("pipeline stopped")?.clone(),client:client.clone(),vad:recognition::vad(&cfg)?});
    }
    for cap in captures {p.capture_tasks.push(tokio::spawn(cap.run()));}
    Ok(())
}
