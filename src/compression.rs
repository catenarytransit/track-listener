use crate::{pipeline::SharedStore, store::CompressionJob};
use anyhow::{Context, Result, ensure};
use std::{process::Stdio, time::Duration};
use tokio::{process::Command, time::{sleep, timeout}};

const POLL_INTERVAL: Duration = Duration::from_secs(2);
const RETRY_INTERVAL: Duration = Duration::from_secs(30);

async fn encode(job: &CompressionJob) -> Result<()> {
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
        let encoded = encode(&job).await;
        if let Err(e) = encoded {
            let _ = tokio::fs::remove_file(&job.temporary).await;
            // The WAV remains playable; try again on a later sweep. A missing
            // executable is a global problem, so avoid logging once per clip.
            if e.downcast_ref::<std::io::Error>().is_some_and(|io|
                matches!(io.kind(), std::io::ErrorKind::NotFound | std::io::ErrorKind::PermissionDenied)) {
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
