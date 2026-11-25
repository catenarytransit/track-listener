use crate::{config::{Channel, Config}, store::Store};
use serde::Serialize;
use std::sync::{Arc, Mutex};
use crossbeam_channel::{Receiver, Sender};

pub type SharedStore = Arc<Mutex<Store>>;
pub type Statuses = Arc<Mutex<Vec<ChannelStatus>>>;
#[derive(Clone, Debug, Serialize)]
pub struct ChannelStatus {
    pub id: String, pub name: String, pub state: String, pub detail: Option<String>,
    pub dropped_clips: u64, pub errors: u64, pub last_audio_ms: Option<i64>,
}
impl ChannelStatus {
    pub fn new(ch: &Channel, demo: bool, capture: bool) -> Self {
        Self { id:ch.id.clone(), name:ch.name.clone(), state:if demo {"demo"} else if capture && ch.enabled {"starting"} else {"disabled"}.into(), detail:None,dropped_clips:0,errors:0,last_audio_ms:None }
    }
}
#[derive(Debug)]
pub struct Job { pub id: String, pub samples: Vec<f32> }

pub struct Pipeline {
    pub statuses: Statuses,
    pub queue: Receiver<Job>,
    pub sender: Option<Sender<Job>>,
    pub capture_tasks: Vec<tokio::task::JoinHandle<()>>,
    pub workers: Vec<std::thread::JoinHandle<()>>,
}
impl Pipeline {
    pub fn new(cfg: &Config, demo: bool, capture: bool) -> Self {
        let (tx,rx)=crossbeam_channel::bounded(cfg.audio.queue_max_clips);
        Self { statuses: Arc::new(Mutex::new(cfg.channels.iter().map(|ch| ChannelStatus::new(ch,demo,capture)).collect())),queue:rx,sender:Some(tx),capture_tasks:Vec::new(),workers:Vec::new() }
    }
    pub async fn stop(mut self) {
        for t in &self.capture_tasks { t.abort(); }
        for t in self.capture_tasks.drain(..) { let _=t.await; }
        self.sender.take();
        // All sender clones now dropped. Workers drain pending jobs before exit.
        let workers=std::mem::take(&mut self.workers);
        let _=tokio::task::spawn_blocking(move || { for w in workers { let _=w.join(); } }).await;
    }
}
