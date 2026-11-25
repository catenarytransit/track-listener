use crate::{audio::{RATE,duration_ms}, config::Config, pipeline::{Job,Pipeline,SharedStore}, sentences::split};
use anyhow::{Context, Result, ensure};
use sherpa_onnx::{OfflineRecognizer, OfflineRecognizerConfig, VadModelConfig, VoiceActivityDetector};
use std::{path::Path, thread};

fn file(path:&Path)->Result<String> {
    ensure!(path.is_file() && path.metadata()?.len()>0, "missing model file: {}; run scripts/download-models.sh",path.display());
    Ok(path.to_str().context("model path must be UTF-8")?.to_owned())
}
pub fn recognizer(cfg:&Config)->Result<OfflineRecognizer> {
    let dir=&cfg.recognition.model_dir;
    let mut c=OfflineRecognizerConfig::default();
    c.model_config.transducer.encoder=Some(file(&dir.join("encoder.int8.onnx"))?);
    c.model_config.transducer.decoder=Some(file(&dir.join("decoder.int8.onnx"))?);
    c.model_config.transducer.joiner=Some(file(&dir.join("joiner.int8.onnx"))?);
    c.model_config.tokens=Some(file(&dir.join("tokens.txt"))?);
    c.model_config.model_type=Some("nemo_transducer".into());
    c.model_config.provider=Some("cpu".into());
    c.model_config.num_threads=cfg.recognition.threads;
    c.decoding_method=Some("greedy_search".into());
    OfflineRecognizer::create(&c).context("create Parakeet recognizer")
}
pub fn vad(cfg:&Config)->Result<VoiceActivityDetector> {
    let mut c=VadModelConfig::default();
    c.silero_vad.model=Some(file(&cfg.recognition.vad_model)?);
    c.silero_vad.threshold=cfg.recognition.vad_threshold;
    c.silero_vad.min_speech_duration=cfg.recognition.min_speech_seconds;
    c.silero_vad.min_silence_duration=cfg.recognition.min_silence_seconds;
    c.silero_vad.max_speech_duration=cfg.audio.max_clip_seconds as f32;
    c.silero_vad.window_size=512;
    c.sample_rate=RATE as i32;
    c.num_threads=1; c.provider=Some("cpu".into());
    VoiceActivityDetector::create(&c,cfg.audio.max_clip_seconds as f32+15.0).context("create Silero VAD")
}
fn decode(model:&OfflineRecognizer, job:&Job)->Result<Vec<crate::sentences::Sentence>> {
    let stream=model.create_stream();
    stream.accept_waveform(RATE as i32,&job.samples);
    model.decode(&stream);
    let r=stream.get_result().context("no recognition result")?;
    Ok(split(&r.text,&r.tokens,r.timestamps.as_deref().unwrap_or(&[]),r.durations.as_deref().unwrap_or(&[]),duration_ms(job.samples.len())))
}
pub fn start_workers(p:&mut Pipeline,cfg:&Config,store:SharedStore)->Result<()> {
    // Load all models before starting workers; fail fast on missing/invalid models.
    let mut models=Vec::new();
    for _ in 0..cfg.recognition.workers { models.push(recognizer(cfg)?); }
    for (i,model) in models.into_iter().enumerate() {
        let queue=p.queue.clone(); let store=store.clone();
        let model_name=cfg.recognition.model_dir.file_name().and_then(|n|n.to_str()).unwrap_or("parakeet").to_string();
        p.workers.push(thread::Builder::new().name(format!("asr-{i}")).spawn(move || {
            while let Ok(job)=queue.recv() {
                let result=decode(&model,&job);
                let write=match result {
                    Ok(s)=>store.lock().unwrap().finish(&job.id,&s,&model_name,None),
                    Err(e)=>{tracing::warn!(clip=%job.id,%e,"recognition failed");store.lock().unwrap().finish(&job.id,&[],&model_name,Some(&e.to_string()))},
                };
                if let Err(e)=write { tracing::error!(clip=%job.id,%e,"could not save recognition result"); }
            }
        })?);
    }
    Ok(())
}
