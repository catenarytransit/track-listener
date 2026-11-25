use crate::{audio::{duration_ms, now_ms, wav}, config::{Audio, Channel, Mode}, sentences::Sentence};
use anyhow::{Context, Result, ensure};
use bytes::Bytes;
use rusqlite::{Connection, OptionalExtension, params, named_params};
use serde::{Deserialize, Serialize};
use std::{collections::{HashMap, HashSet}, fs, path::{Path, PathBuf}, time::Duration};
use uuid::Uuid;

pub struct Store { db: Connection, cfg: Audio, ram: HashMap<String, Bytes> }
#[derive(Debug, Serialize)]
pub struct SentenceRow { ordinal: i64, text: String, start_ms: Option<i64>, end_ms: Option<i64> }
#[derive(Debug, Serialize)]
pub struct Clip {
    pub id: String, pub channel_id: String, pub channel_name: String,
    pub started_ms: i64, pub duration_ms: i64, pub audio_bytes: i64,
    pub audio_mode: Option<String>, pub audio_available: bool,
    pub status: String, pub model: Option<String>, pub error: Option<String>,
    pub sentences: Vec<SentenceRow>,
}
#[derive(Debug, Default, Deserialize)]
pub struct Filters {
    pub channel: Option<String>, pub q: Option<String>, pub since: Option<i64>, pub until: Option<i64>,
    pub before_ms: Option<i64>, pub before_id: Option<String>, pub limit: Option<usize>,
}
impl Filters {
    pub fn validate(&self) -> Result<()> {
        ensure!(self.before_ms.is_some() == self.before_id.is_some(), "both cursor fields required");
        ensure!(self.since.zip(self.until).is_none_or(|(a,b)| a < b), "since must precede until");
        ensure!((1..=100).contains(&self.limit.unwrap_or(50)), "limit must be 1..100");
        ensure!(self.q.as_ref().is_none_or(|q| q.len() <= 200), "search too long");
        Ok(())
    }
}
#[derive(Debug, Serialize)]
pub struct Cursor { before_ms: i64, before_id: String }
#[derive(Debug, Serialize)]
pub struct Page { pub items: Vec<Clip>, pub next_cursor: Option<Cursor> }
#[derive(Debug, Serialize)]
pub struct Stats {
    pub clips: i64, pub used_bytes: i64, pub retained_clips: i64, pub oldest_ms: Option<i64>,
    pub max_bytes: u64, pub mode: &'static str, pub max_age_hours: u64, pub transcript_max_age_hours: u64, pub sentences: i64,
}
// Only one encoder runs per archive; UUID-based paths stay inside its directory.
#[derive(Debug)]
pub struct CompressionJob {
    pub id: String,
    pub source: PathBuf,
    pub temporary: PathBuf,
}
#[derive(Debug)]
struct Archived { id: String, mode: Option<String>, bytes: i64, started: i64 }
impl Store {
    pub fn open(path: &Path, cfg: Audio) -> Result<Self> {
        if let Some(p) = path.parent().filter(|p| !p.as_os_str().is_empty()) { fs::create_dir_all(p)?; }
        fs::create_dir_all(&cfg.path)?;
        let db = Connection::open(path)?;
        db.busy_timeout(Duration::from_secs(5))?;
        db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;
            CREATE TABLE IF NOT EXISTS clips (
                id TEXT PRIMARY KEY, channel_id TEXT NOT NULL, channel_name TEXT NOT NULL,
                started_ms INTEGER NOT NULL, duration_ms INTEGER NOT NULL,
                audio_bytes INTEGER NOT NULL DEFAULT 0, audio_mode TEXT,
                audio_format TEXT NOT NULL DEFAULT 'wav',
                status TEXT NOT NULL, model TEXT, error TEXT
            );
            CREATE TABLE IF NOT EXISTS sentences (
                clip_id TEXT NOT NULL REFERENCES clips(id) ON DELETE CASCADE,
                ordinal INTEGER NOT NULL, text TEXT NOT NULL, start_ms INTEGER, end_ms INTEGER,
                PRIMARY KEY(clip_id, ordinal)
            );
            CREATE INDEX IF NOT EXISTS clips_time ON clips(started_ms DESC, id DESC);
            CREATE INDEX IF NOT EXISTS clips_channel_time ON clips(channel_id, started_ms DESC, id DESC);
            CREATE INDEX IF NOT EXISTS archive_time ON clips(started_ms, id) WHERE audio_bytes>0;")?;
        // Existing archives have no format column; their retained payloads are WAV.
        let has_format = {
            let mut stmt = db.prepare("PRAGMA table_info(clips)")?;
            let names = stmt.query_map([], |r| r.get::<_, String>(1))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            names.iter().any(|name| name == "audio_format")
        };
        if !has_format {
            db.execute_batch("ALTER TABLE clips ADD COLUMN audio_format TEXT NOT NULL DEFAULT 'wav';")?;
        }
        db.execute_batch("CREATE INDEX IF NOT EXISTS disk_wav_time ON clips(started_ms,id)
            WHERE audio_mode='disk' AND audio_format='wav' AND audio_bytes>0;")?;
        let mut s = Self { db, cfg, ram: HashMap::new() };
        s.reconcile()?;
        Ok(s)
    }
    fn path(&self, id: &str, extension: &str) -> Result<PathBuf> {
        Ok(self.cfg.path.join(format!("{}.{extension}", Uuid::parse_str(id)?)))
    }
    fn archived(&self) -> Result<Vec<Archived>> {
        let mut stmt = self.db.prepare("SELECT id,audio_mode,audio_bytes,started_ms FROM clips WHERE audio_bytes>0 ORDER BY started_ms,id")?;
        let rows = stmt.query_map([], |r| Ok(Archived { id:r.get(0)?, mode:r.get(1)?, bytes:r.get(2)?, started:r.get(3)? }))?.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }
    fn clear_metadata(&self, id: &str) -> Result<()> {
        self.db.execute("UPDATE clips SET audio_bytes=0,audio_mode=NULL,audio_format='wav' WHERE id=?1", [id])?;
        Ok(())
    }
    fn remove_if_exists(path: &Path) -> Result<()> {
        match fs::remove_file(path) { Ok(()) => Ok(()), Err(e) if e.kind()==std::io::ErrorKind::NotFound => Ok(()), Err(e) => Err(e.into()) }
    }
    fn evict(&mut self, row: &Archived) -> Result<()> {
        if row.mode.as_deref()==Some("ram") { self.ram.remove(&row.id); }
        else {
            Self::remove_if_exists(&self.path(&row.id, "wav")?)?;
            Self::remove_if_exists(&self.path(&row.id, "mp4")?)?;
        }
        self.clear_metadata(&row.id)
    }
    fn reconcile(&mut self) -> Result<()> {
        self.db.execute_batch("UPDATE clips SET status='interrupted',error='Process stopped before recognition completed' WHERE status='pending';
            UPDATE clips SET audio_bytes=0,audio_mode=NULL,audio_format='wav' WHERE audio_mode='ram';")?;
        let mut known = HashSet::new();
        for row in self.archived()? {
            let mut recovered = false;
            if self.cfg.mode == Mode::Disk {
                // An MP4 is published only after FFmpeg succeeds. Prefer it if a
                // crash occurred between rename, SQLite update and WAV deletion.
                for (format, minimum) in [("mp4", 1), ("wav", 44)] {
                    let p = self.path(&row.id, format)?;
                    if !p.is_file() { continue; }
                    let n = p.metadata()?.len();
                    if n >= minimum && n <= i64::MAX as u64 {
                        self.db.execute("UPDATE clips SET audio_bytes=?1,audio_format=?2 WHERE id=?3",
                            params![n as i64,format,row.id])?;
                        known.insert(p);
                        recovered = true;
                        break;
                    }
                }
            }
            if !recovered { self.evict(&row)?; }
        }
        // Only delete files in the dedicated archive directory with our UUID filenames.
        for entry in fs::read_dir(&self.cfg.path)? {
            let p = entry?.path();
            if p.is_file() && matches!(p.extension().and_then(|e| e.to_str()),Some("wav"|"mp4"|"tmp"|"encoding"))
                && p.file_stem().and_then(|s| s.to_str()).is_some_and(|s| Uuid::parse_str(s).is_ok()) && !known.contains(&p) {
                Self::remove_if_exists(&p)?;
            }
        }
        self.prune()
    }
    pub fn prune(&mut self) -> Result<()> {
        let cutoff = (self.cfg.max_age_hours > 0).then(|| now_ms() - (self.cfg.max_age_hours as i64 * 3_600_000));
        let mut total: i64 = self.db.query_row("SELECT coalesce(sum(audio_bytes),0) FROM clips WHERE audio_bytes>0", [], |r| r.get(0))?;
        // Walk only the expired/over-budget prefix of the archive index.
        loop {
            let oldest = self.db.query_row("SELECT id,audio_mode,audio_bytes,started_ms FROM clips WHERE audio_bytes>0 ORDER BY started_ms,id LIMIT 1", [], |r| Ok(Archived{id:r.get(0)?,mode:r.get(1)?,bytes:r.get(2)?,started:r.get(3)?})).optional()?;
            let Some(row) = oldest else { break; };
            if total <= self.cfg.max_bytes() as i64 && !cutoff.is_some_and(|c| row.started < c) { break; }
            self.evict(&row)?;
            total -= row.bytes;
        }
        // Transcript retention is independent of the byte budget and audio age.
        // Never delete pending recognition jobs or clips with playable audio.
        if self.cfg.transcript_max_age_hours > 0 {
            let cutoff = now_ms() - self.cfg.transcript_max_age_hours as i64 * 3_600_000;
            self.db.execute("DELETE FROM clips WHERE audio_bytes=0 AND status<>'pending' AND started_ms<?1", [cutoff])?;
        }
        Ok(())
    }
    pub fn compression_jobs(&self) -> Result<Vec<CompressionJob>> {
        let mut stmt = self.db.prepare("SELECT id FROM clips
            WHERE audio_mode='disk' AND audio_format='wav' AND audio_bytes>0
            ORDER BY started_ms,id")?;
        let ids = stmt.query_map([], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        ids.into_iter().map(|id| Ok(CompressionJob {
            source: self.path(&id, "wav")?,
            temporary: self.path(&id, "encoding")?,
            id,
        })).collect()
    }
    // Called under the store lock only after the asynchronous encoder succeeds.
    // Eviction may have won the race; never recreate evicted audio.
    pub fn complete_compression(&mut self, job: &CompressionJob) -> Result<bool> {
        let kept: bool = self.db.query_row("SELECT EXISTS(SELECT 1 FROM clips
            WHERE id=?1 AND audio_mode='disk' AND audio_format='wav' AND audio_bytes>0)",
            [&job.id], |r| r.get(0))?;
        if !kept || !job.source.is_file() { return Ok(false); }
        let n = i64::try_from(job.temporary.metadata()?.len())?;
        ensure!(n > 0, "empty encoded clip");
        let destination = self.path(&job.id, "mp4")?;
        // Same-directory rename publishes a complete MP4 atomically.
        fs::rename(&job.temporary, &destination)?;
        if let Err(e) = self.db.execute("UPDATE clips SET audio_bytes=?1,audio_format='mp4' WHERE id=?2",
            params![n,job.id]) {
            // The original remains available if the metadata write fails.
            let _ = Self::remove_if_exists(&destination);
            return Err(e.into());
        }
        Self::remove_if_exists(&job.source)?;
        self.prune()?;
        Ok(true)
    }
    pub fn add_clip(&mut self, ch: &Channel, started: i64, samples: &[f32]) -> Result<String> {
        let id = Uuid::new_v4().to_string();
        self.db.execute("INSERT INTO clips(id,channel_id,channel_name,started_ms,duration_ms,status) VALUES(?1,?2,?3,?4,?5,'pending')", params![id,ch.id,ch.name,started,duration_ms(samples.len())])?;
        let expected = samples.len() as u64 * 2 + 44;
        if expected > self.cfg.max_bytes() { return Ok(id); }
        // Reserve budget before allocating or writing a new retained payload. A late
        // arrival may evict itself rather than newer clips. Transcription is independent.
        self.db.execute("UPDATE clips SET audio_bytes=?1,audio_mode=?2 WHERE id=?3", params![expected as i64,self.cfg.mode.as_str(),id])?;
        if let Err(e) = self.prune() {
            self.clear_metadata(&id)?;
            tracing::error!(%e, "archive prune failed; skipping audio retention");
            return Ok(id);
        }
        let kept: i64 = self.db.query_row("SELECT audio_bytes FROM clips WHERE id=?1", [&id], |r| r.get(0))?;
        if kept == 0 { return Ok(id); }
        let data = wav(samples);
        if self.cfg.mode == Mode::Ram { self.ram.insert(id.clone(), Bytes::from(data)); }
        else {
            let path = self.path(&id, "wav")?; let tmp = path.with_extension("tmp");
            let write = (|| -> Result<()> { fs::write(&tmp,&data)?; fs::rename(&tmp,&path)?; Ok(()) })();
            if let Err(e) = write {
                tracing::error!(%e, %id, "could not retain clip");
                self.clear_metadata(&id)?;
                let _ = Self::remove_if_exists(&tmp);
            }
        }
        Ok(id)
    }
    pub fn finish(&mut self, id: &str, sentences: &[Sentence], model: &str, error: Option<&str>) -> Result<()> {
        let tx = self.db.transaction()?;
        tx.execute("DELETE FROM sentences WHERE clip_id=?1",[id])?;
        for (i,s) in sentences.iter().enumerate() {
            tx.execute("INSERT INTO sentences(clip_id,ordinal,text,start_ms,end_ms) VALUES(?1,?2,?3,?4,?5)",params![id,i as i64,s.text,s.start_ms,s.end_ms])?;
        }
        tx.execute("UPDATE clips SET status=?1,model=?2,error=?3 WHERE id=?4",params![if error.is_some(){"error"}else{"ready"},model,error,id])?;
        tx.commit()?; Ok(())
    }
    pub fn get_audio(&mut self, id: &str) -> Result<Option<(Bytes, &'static str)>> {
        let Ok(uuid) = Uuid::parse_str(id) else { return Ok(None); };
        if uuid.to_string() != id { return Ok(None); }
        self.prune()?;
        let row: Option<(Option<String>, String)> = self.db.query_row(
            "SELECT audio_mode,audio_format FROM clips WHERE id=?1 AND audio_bytes>0",
            [id], |r| Ok((r.get(0)?,r.get(1)?))).optional()?;
        let Some((mode,format)) = row else { return Ok(None); };
        match mode.as_deref() {
            Some("ram") => Ok(self.ram.get(id).cloned().map(|data| (data,"audio/wav"))),
            Some("disk") => {
                let (extension,mime) = if format=="mp4" { ("mp4","audio/mp4") } else { ("wav","audio/wav") };
                match fs::read(self.path(id, extension)?) {
                    Ok(data) => Ok(Some((Bytes::from(data),mime))),
                    Err(e) if e.kind()==std::io::ErrorKind::NotFound => { self.clear_metadata(id)?; Ok(None) },
                    Err(e) => Err(e.into()),
                }
            },
            _ => Ok(None),
        }
    }
    pub fn list(&mut self, f: &Filters) -> Result<Page> {
        f.validate()?; self.prune()?;
        let limit = f.limit.unwrap_or(50);
        let mut stmt = self.db.prepare("SELECT id,channel_id,channel_name,started_ms,duration_ms,audio_bytes,audio_mode,status,model,error
            FROM clips WHERE (:channel IS NULL OR channel_id=:channel)
            AND (:q IS NULL OR EXISTS(SELECT 1 FROM sentences s WHERE s.clip_id=clips.id AND instr(lower(s.text),lower(:q))>0))
            AND (:since IS NULL OR started_ms>=:since) AND (:until IS NULL OR started_ms<:until)
            AND (:before_ms IS NULL OR (started_ms<:before_ms OR (started_ms=:before_ms AND id<:before_id)))
            ORDER BY started_ms DESC,id DESC LIMIT :limit")?;
        let mut items = stmt.query_map(named_params!{":channel":f.channel,":q":f.q,":since":f.since,":until":f.until,":before_ms":f.before_ms,":before_id":f.before_id,":limit":(limit+1) as i64}, |r| {
            let audio_bytes:i64=r.get(5)?;
            Ok(Clip{id:r.get(0)?,channel_id:r.get(1)?,channel_name:r.get(2)?,started_ms:r.get(3)?,duration_ms:r.get(4)?,audio_bytes,audio_mode:r.get(6)?,audio_available:audio_bytes>0,status:r.get(7)?,model:r.get(8)?,error:r.get(9)?,sentences:Vec::new()})
        })?.collect::<rusqlite::Result<Vec<_>>>()?;
        let more=items.len()>limit; items.truncate(limit);
        let mut sentences=self.db.prepare("SELECT ordinal,text,start_ms,end_ms FROM sentences WHERE clip_id=?1 ORDER BY ordinal")?;
        for c in &mut items {
            c.sentences=sentences.query_map([&c.id],|r|Ok(SentenceRow{ordinal:r.get(0)?,text:r.get(1)?,start_ms:r.get(2)?,end_ms:r.get(3)?}))?.collect::<rusqlite::Result<Vec<_>>>()?;
        }
        let next_cursor=if more {items.last().map(|c|Cursor{before_ms:c.started_ms,before_id:c.id.clone()})}else{None};
        Ok(Page{items,next_cursor})
    }
    pub fn stats(&mut self) -> Result<Stats> {
        self.prune()?;
        let (clips, used_bytes, retained_clips, oldest_ms) = self.db.query_row("SELECT count(*),coalesce(sum(audio_bytes),0),count(CASE WHEN audio_bytes>0 THEN 1 END),min(CASE WHEN audio_bytes>0 THEN started_ms END) FROM clips", [], |r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?)))?;
        let sentences=self.db.query_row("SELECT count(*) FROM sentences",[],|r|r.get(0)).context("count sentences")?;
        Ok(Stats{clips,used_bytes,retained_clips,oldest_ms,max_bytes:self.cfg.max_bytes(),mode:self.cfg.mode.as_str(),max_age_hours:self.cfg.max_age_hours,transcript_max_age_hours:self.cfg.transcript_max_age_hours,sentences})
    }
}
#[cfg(test)] mod tests {
    use super::*;
    use crate::audio::RATE;
    fn config(path:&Path,mode:Mode)->Audio { Audio{mode,max_mib:1,path:path.join("clips"),max_age_hours:0,transcript_max_age_hours:0,queue_max_clips:4,max_clip_seconds:25} }
    fn channel()->Channel { Channel{id:"test".into(),name:"Test".into(),url:"http://example.org".into(),enabled:true} }
    #[test] fn retention_preserves_text_and_evicts_oldest() {
        for mode in [Mode::Ram,Mode::Disk] {
            let d=tempfile::tempdir().unwrap(); let mut s=Store::open(&d.path().join("db"),config(d.path(),mode)).unwrap();
            let samples=vec![0.0;RATE*20]; let first=s.add_clip(&channel(),1,&samples).unwrap();
            s.finish(&first,&[Sentence{text:"Copy 100%.".into(),start_ms:Some(0),end_ms:Some(1000)}],"test",None).unwrap();
            let second=s.add_clip(&channel(),2,&samples).unwrap();
            assert!(s.get_audio(&first).unwrap().is_none()); assert!(s.get_audio(&second).unwrap().is_some());
            let stats=s.stats().unwrap(); assert!(stats.used_bytes <= stats.max_bytes as i64); assert_eq!(stats.sentences,1);
            let page=s.list(&Filters{q:Some("100%".into()),channel:Some("test".into()),..Default::default()}).unwrap(); assert_eq!(page.items.len(),1); assert_eq!(page.items[0].id,first);
        }
    }
    #[test] fn restart_and_zero_budget() {
        let d=tempfile::tempdir().unwrap(); let db=d.path().join("db"); let mut cfg=config(d.path(),Mode::Disk);
        let id={let mut s=Store::open(&db,cfg.clone()).unwrap();s.add_clip(&channel(),1,&[0.0;512]).unwrap()};
        let mut s=Store::open(&db,cfg.clone()).unwrap();assert!(s.get_audio(&id).unwrap().is_some()); assert_eq!(s.list(&Filters::default()).unwrap().items[0].status,"interrupted"); drop(s);
        cfg.max_mib=0; let mut s=Store::open(&db,cfg).unwrap(); assert!(s.get_audio(&id).unwrap().is_none()); assert_eq!(s.stats().unwrap().clips,1);
    }
    #[test] fn age_limit_and_late_clips() {
        let d=tempfile::tempdir().unwrap();let mut cfg=config(d.path(),Mode::Ram);cfg.max_age_hours=1;
        let mut s=Store::open(&d.path().join("db"),cfg).unwrap(); let old=s.add_clip(&channel(),now_ms()-7_200_000,&[0.0;512]).unwrap();assert!(s.get_audio(&old).unwrap().is_none());
        let samples=vec![0.0;RATE*20]; let new=s.add_clip(&channel(),now_ms(),&samples).unwrap();let late=s.add_clip(&channel(),now_ms()-10000,&samples).unwrap();assert!(s.get_audio(&late).unwrap().is_none());assert!(s.get_audio(&new).unwrap().is_some());
    }
    #[tokio::test]
    async fn compresses_existing_archive_and_recovers_after_restart() {
        let d=tempfile::tempdir().unwrap();
        let db=d.path().join("db");
        let cfg=config(d.path(),Mode::Disk);
        let id={
            let mut s=Store::open(&db,cfg.clone()).unwrap();
            let samples=vec![0.0; RATE*2];
            let id=s.add_clip(&channel(),now_ms(),&samples).unwrap();
            s.finish(&id,&[Sentence{text:"Radio test".into(),start_ms:Some(0),end_ms:Some(1000)}],"test",None).unwrap();
            id
        };
        let shared=std::sync::Arc::new(std::sync::Mutex::new(Store::open(&db,cfg.clone()).unwrap()));
        let original=shared.lock().unwrap().get_audio(&id).unwrap().unwrap();
        assert_eq!(original.1,"audio/wav");
        crate::compression::compress_pass(&shared).await.unwrap();
        {
            let mut s=shared.lock().unwrap();
            let (encoded,mime)=s.get_audio(&id).unwrap().unwrap();
            assert_eq!(mime,"audio/mp4");
            assert_eq!(&encoded[4..8],b"ftyp");
            assert!(encoded.len()<original.0.len());
            assert_eq!(s.stats().unwrap().used_bytes,encoded.len() as i64);
            assert!(s.compression_jobs().unwrap().is_empty());
            assert!(!s.path(&id,"wav").unwrap().exists());
            // Simulate a crash after publication, before the WAV was deleted
            // and before the format/byte count was committed to SQLite.
            fs::write(s.path(&id,"wav").unwrap(),&original.0).unwrap();
            fs::write(s.path(&id,"encoding").unwrap(),b"partial").unwrap();
            s.db.execute("UPDATE clips SET audio_format='wav',audio_bytes=?1 WHERE id=?2",
                params![original.0.len() as i64,id]).unwrap();
        }
        drop(shared);
        let mut s=Store::open(&db,cfg).unwrap();
        assert_eq!(s.get_audio(&id).unwrap().unwrap().1,"audio/mp4");
        assert!(!s.path(&id,"wav").unwrap().exists());
        assert!(!s.path(&id,"encoding").unwrap().exists());
        assert_eq!(s.list(&Filters{q:Some("Radio test".into()),..Default::default()}).unwrap().items.len(),1);
        s.cfg.max_mib=0;
        s.prune().unwrap();
        assert!(s.get_audio(&id).unwrap().is_none());
        assert!(!s.path(&id,"mp4").unwrap().exists());
        assert_eq!(s.stats().unwrap().sentences,1);
    }
    #[test]
    fn encoding_cannot_resurrect_evicted_audio() {
        let d=tempfile::tempdir().unwrap();
        let mut s=Store::open(&d.path().join("db"),config(d.path(),Mode::Disk)).unwrap();
        let id=s.add_clip(&channel(),now_ms(),&[0.0;512]).unwrap();
        let job=s.compression_jobs().unwrap().pop().unwrap();
        fs::write(&job.temporary,b"encoded").unwrap();
        s.cfg.max_mib=0;
        s.prune().unwrap();
        assert!(!s.complete_compression(&job).unwrap());
        assert!(s.get_audio(&id).unwrap().is_none());
        assert!(!s.path(&id,"mp4").unwrap().exists());
    }
    #[test]
    fn transcripts_outlive_audio_and_expire_independently() {
        let d=tempfile::tempdir().unwrap();
        let mut cfg=config(d.path(),Mode::Disk);
        cfg.max_age_hours=1;
        cfg.transcript_max_age_hours=24;
        let mut s=Store::open(&d.path().join("db"),cfg).unwrap();
        let now=now_ms();
        let recent=s.add_clip(&channel(),now-2*3_600_000,&[0.0;512]).unwrap();
        let old=s.add_clip(&channel(),now-25*3_600_000,&[0.0;512]).unwrap();
        let sentence=Sentence{text:"Copy".into(),start_ms:None,end_ms:None};
        assert!(s.get_audio(&recent).unwrap().is_none());
        // Old pending jobs must survive cleanup until their workers finish.
        assert_eq!(s.stats().unwrap().clips,2);
        s.finish(&recent,std::slice::from_ref(&sentence),"test",None).unwrap();
        s.finish(&old,std::slice::from_ref(&sentence),"test",None).unwrap();
        s.prune().unwrap();
        let page=s.list(&Filters{q:Some("Copy".into()),..Default::default()}).unwrap();
        assert_eq!(page.items.len(),1);
        assert_eq!(page.items[0].id,recent);
        assert!(!page.items[0].audio_available);
        assert_eq!(s.stats().unwrap().sentences,1);
    }
    #[test] fn cursor_orders_equal_timestamps() {
        let d=tempfile::tempdir().unwrap();let mut s=Store::open(&d.path().join("db"),config(d.path(),Mode::Ram)).unwrap();
        for _ in 0..3 {s.add_clip(&channel(),10,&[0.0;512]).unwrap();}
        let p=s.list(&Filters{limit:Some(1),..Default::default()}).unwrap();let c=p.next_cursor.unwrap();
        let rest=s.list(&Filters{before_ms:Some(c.before_ms),before_id:Some(c.before_id),..Default::default()}).unwrap();assert_eq!(rest.items.len(),2);assert!(rest.items.iter().all(|x|x.id != p.items[0].id));
    }
}
