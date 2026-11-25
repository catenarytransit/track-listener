use crate::{audio::{duration_ms, pcm_is_silent}, pipeline::SharedStore, store::CompressionJob};
use anyhow::{Context, Result, ensure};
use std::{process::Stdio, time::Duration};
use tokio::{process::Command, time::{sleep, timeout}};

const POLL_INTERVAL: Duration = Duration::from_secs(2);
const RETRY_INTERVAL: Duration = Duration::from_secs(30);

pub(crate) async fn encode(job: &CompressionJob) -> Result<()> {
    let child = Command::new("ffmpeg")
        .args(["-nostdin", "-hide_banner", "-loglevel", "error", "-y", "-i"])
        .arg(&job.source)
        .args(["-map", "0:a:0", "-vn", "-sn", "-dn", "-c:a", "aac", "-b:a", "32k",
            "-ac", "1", "-ar", "16000", "-threads", "1", "-movflags", "+faststart", "-f", "mp4"])
        .arg(&job.temporary)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .context("launch FFmpeg archive encoder")?;
    let output = timeout(Duration::from_secs(60), child.wait_with_output()).await
        .context("archive encoding timed out")??;
    ensure!(output.status.success(), "FFmpeg archive encoding failed: {}",
        String::from_utf8_lossy(&output.stderr));
    Ok(())
}

// Decode historical WAVs and MP4s completely. An empty, broken or truncated
// recording is not evidence of silence and must never trigger deletion.
async fn check_silence(job: &CompressionJob) -> Result<bool> {
    let child = Command::new("ffmpeg")
        .args(["-nostdin", "-hide_banner", "-loglevel", "error", "-xerror", "-err_detect", "explode", "-i"])
        .arg(&job.source)
        .args(["-map","0:a:0","-vn","-sn","-dn","-ac","1","-ar","16000",
            "-c:a","pcm_s16le","-f","s16le","pipe:1"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn().context("launch FFmpeg silence scanner")?;
    let output=timeout(Duration::from_secs(60),child.wait_with_output()).await
        .context("silence scan timed out")??;
    ensure!(output.status.success() && output.stderr.is_empty(), "silence scan decode failed: {}",
        String::from_utf8_lossy(&output.stderr));
    ensure!(!output.stdout.is_empty() && output.stdout.len()%2==0,"empty or incomplete decoded PCM");
    // WAV durations are rounded to milliseconds. AAC may include frame padding.
    let decoded_ms=duration_ms(output.stdout.len()/2);
    let tolerance=if job.format=="mp4" { 100 } else { 1 };
    ensure!(decoded_ms.abs_diff(job.duration_ms)<=tolerance,"decoded duration does not match clip metadata");
    Ok(pcm_is_silent(&output.stdout))
}

fn ffmpeg_unavailable(e:&anyhow::Error)->bool {
    e.downcast_ref::<std::io::Error>().is_some_and(|io|
        matches!(io.kind(),std::io::ErrorKind::NotFound|std::io::ErrorKind::PermissionDenied))
}

pub(crate) async fn compress_pass(store: &SharedStore) -> Result<()> {
    let archive = store.clone();
    let jobs = tokio::task::spawn_blocking(move || {
        archive.lock().unwrap().compression_jobs()
    }).await??;
    // At most one FFmpeg encoder and one temporary compressed file at a time.
    // The store mutex is not held during FFmpeg, so capture, ASR and HTTP
    // continue independently. The database is the durable work list.
    for job in jobs {
        if !job.source.is_file() { continue; }
        if !job.silence_checked {
            match check_silence(&job).await {
                Ok(true) => {
                    let archive=store.clone();
                    let id=job.id.clone();
                    let result=tokio::task::spawn_blocking(move||archive.lock().unwrap().delete_silent_clip(&id)).await?;
                    match result {
                        Ok(true)=>tracing::info!(clip=%job.id,"deleted silent recording and transcript"),
                        Ok(false)=>{},
                        Err(e)=>tracing::warn!(clip=%job.id,error=%e,"silent clip cleanup failed"),
                    }
                    continue;
                }
                Ok(false) => {
                    let archive=store.clone();
                    let id=job.id.clone();
                    tokio::task::spawn_blocking(move||archive.lock().unwrap().mark_silence_checked(&id)).await??;
                }
                Err(e) => {
                    if ffmpeg_unavailable(&e) {return Err(e);}
                    if job.source.is_file() {
                        tracing::warn!(clip=%job.id,error=%e,"cannot verify silence; preserving recording and transcript");
                    }
                    continue;
                }
            }
        }
        if job.format!="wav" {continue;}
        let encoded = encode(&job).await;
        if let Err(e) = encoded {
            let _ = tokio::fs::remove_file(&job.temporary).await;
            // The WAV remains playable; try again on a later sweep. A missing
            // executable is a global problem, so avoid logging once per clip.
            if ffmpeg_unavailable(&e) {
                return Err(e);
            }
            if job.source.is_file() {
                tracing::warn!(clip=%job.id, error=%e, "archive compression failed; retaining WAV");
            }
            continue;
        }
        let archive = store.clone();
        let id = job.id.clone();
        let result = tokio::task::spawn_blocking(move || {
            let result = archive.lock().unwrap().complete_compression(&job);
            // On failure or concurrent eviction there may be an unpublished
            // temporary file left over. Startup reconciliation also removes it.
            let _ = std::fs::remove_file(&job.temporary);
            result
        }).await?;
        match result {
            Ok(true) => tracing::debug!(clip=%id, "archive clip compressed to MP4"),
            Ok(false) => {},
            Err(e) => tracing::warn!(clip=%id, error=%e, "archive compression publication failed"),
        }
    }
    Ok(())
}

pub async fn run(store: SharedStore) {
    loop {
        let delay = match compress_pass(&store).await {
            Ok(()) => POLL_INTERVAL,
            Err(e) => {
                tracing::warn!(error=%e, "archive compression sweep failed; retrying");
                RETRY_INTERVAL
            }
        };
        sleep(delay).await;
    }
}
