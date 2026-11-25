use anyhow::{Context, Result, ensure};
use serde::Deserialize;
use std::{collections::HashSet, net::IpAddr, path::{Path, PathBuf}};

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub server: Server,
    pub audio: Audio,
    pub recognition: Recognition,
    pub channels: Vec<Channel>,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Server {
    pub host: IpAddr,
    pub port: u16,
    pub database: PathBuf,
    pub web_dist: PathBuf,
}
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Mode { Ram, Disk }
impl Mode { pub fn as_str(self) -> &'static str { match self { Self::Ram => "ram", Self::Disk => "disk" } } }
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Audio {
    pub mode: Mode,
    pub max_mib: u64,
    pub path: PathBuf,
    pub max_age_hours: u64,
    /// Zero retains transcript rows indefinitely, independently of audio eviction.
    #[serde(default)]
    pub transcript_max_age_hours: u64,
    pub queue_max_clips: usize,
    pub max_clip_seconds: u32,
}
impl Audio { pub fn max_bytes(&self) -> u64 { self.max_mib * 1024 * 1024 } }
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Recognition {
    pub model_dir: PathBuf,
    pub vad_model: PathBuf,
    pub threads: i32,
    pub workers: usize,
    pub vad_threshold: f32,
    pub min_speech_seconds: f32,
    pub min_silence_seconds: f32,
    #[serde(default = "default_hotwords")]
    pub hotwords: Vec<String>,
    #[serde(default = "default_hotwords_score")]
    pub hotwords_score: f32,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Channel {
    pub id: String,
    pub name: String,
    pub url: String,
    #[serde(default = "enabled")]
    pub enabled: bool,
}
fn enabled() -> bool { true }
fn default_hotwords() -> Vec<String> { ["highball", "C.P."].into_iter().map(String::from).collect() }
fn default_hotwords_score() -> f32 { 0.75 }
impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        let config: Self = toml::from_str(&std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?)?;
        config.validate()?;
        Ok(config)
    }
    pub fn validate(&self) -> Result<()> {
        let a = &self.audio; let r = &self.recognition;
        ensure!(a.max_mib <= (i64::MAX as u64 / 1048576), "audio budget overflow");
        ensure!(a.max_age_hours <= 876000 && a.transcript_max_age_hours <= 876000, "age limit out of range");
        ensure!(a.transcript_max_age_hours == 0 || a.max_age_hours == 0 || a.transcript_max_age_hours > a.max_age_hours,
            "transcript_max_age_hours must exceed max_age_hours when both limits are enabled");
        ensure!((1..=300).contains(&a.max_clip_seconds), "max_clip_seconds must be 1..300");
        ensure!((1..=4096).contains(&a.queue_max_clips), "queue_max_clips must be 1..4096");
        ensure!((1..=128).contains(&r.threads) && (1..=32).contains(&r.workers), "invalid recognition thread/worker count");
        ensure!(r.vad_threshold > 0.0 && r.vad_threshold < 1.0, "invalid VAD threshold");
        ensure!(r.min_speech_seconds > 0.0 && r.min_speech_seconds < a.max_clip_seconds as f32 && r.min_silence_seconds > 0.0 && r.min_silence_seconds <= 10.0, "invalid VAD durations");
        ensure!(r.hotwords_score.is_finite() && (0.0..=5.0).contains(&r.hotwords_score), "hotwords_score must be between 0 and 5");
        ensure!(r.hotwords.len() <= 100 && r.hotwords.iter().all(|w| !w.trim().is_empty()
            && w.len() <= 100 && !w.chars().any(|c| c.is_control() || matches!(c,'/'|':'|'\u{2581}'))),
            "invalid hotwords (at most 100 phrases, each 1..100 bytes; no control characters, slash, colon or tokenizer marker)");
        let mut ids = HashSet::new();
        for c in &self.channels {
            ensure!(!c.id.is_empty() && c.id.len() <= 100 && !c.name.trim().is_empty() && ids.insert(&c.id), "empty or duplicate channel id/name: {}", c.id);
            let u = reqwest::Url::parse(&c.url)?;
            ensure!(matches!(u.scheme(), "http" | "https"), "only HTTP(S) feeds supported");
        }
        Ok(())
    }
}
