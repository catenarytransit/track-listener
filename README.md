# Track Listener (Rust) v0.2

A self-hosted CPU-only radio transcription and recording archive. All backend application code is Rust. The browser UI uses Svelte 5 and Vite. FFmpeg is the native audio decoder; Sherpa-ONNX links native C/C++ inference libraries through its Rust bindings. No Python interpreter, packages, scripts or sidecar are required.

## Architecture

- One independent Tokio capture task and FFmpeg process per enabled radio channel. PLS playlists are resolved with reqwest; alternative FileN entries and bounded reconnect backoff are supported. A 20-second decoded audio read timeout handles stalled receivers.
- One Silero VAD per channel, with independent state. FFmpeg supplies mono 16 kHz signed 16-bit PCM. VAD emits speech clips (bounded by `max_clip_seconds`).
- Shared bounded crossbeam queue feeds dedicated Rust OS threads running the English Parakeet TDT 0.6B v2 INT8 model on CPU via `sherpa-onnx` 2.0.3. Model copies are per worker; ONNX threads are configured per copy. A full queue records an error and retains the audio when budget permits instead of silently dropping the archive entry.
- SQLite (`rusqlite`, bundled SQLite, WAL mode) stores one row per clip and one row per recognized sentence. New archive audio starts as WAV. In disk mode a separate asynchronous FFmpeg worker encodes completed files to mono 32 kbit/s AAC in an audio-only MP4 container; RAM mode keeps WAV. A common store mutex serializes archive updates and SQLite writes; blocking storage work is moved off the async HTTP and capture tasks.
- Axum serves the REST API, frontend and byte-range-capable WAV and MP4 playback. Svelte supports channel/time/text filtering, live polling, pagination, per-sentence playback, full clip playback, and storage/queue status. The Appearance selector supports System (default), Light, and Dark themes; the selected preference is stored locally in the browser and System follows OS appearance changes.

## Quick start (Linux)

Install stable Rust (edition 2024), Node.js 22+, FFmpeg, curl and bzip2. On Debian/Ubuntu:

```sh
sudo apt-get update
sudo apt-get install -y build-essential pkg-config cmake curl bzip2 ffmpeg libssl-dev
./scripts/download-models.sh
(cd web && npm install && npm run build)
cargo run --release -- --config config.toml
```

Open http://127.0.0.1:4582. The first Cargo build obtains native Sherpa-ONNX libraries and dependencies; the separate shell script downloads the ASR and VAD models. After installation, inference does not require Internet access, although network radio feeds do. No GPU or CUDA needed. For custom native library locations, consult https://docs.rs/sherpa-onnx/2.0.3/sherpa_onnx/ (e.g. `SHERPA_ONNX_LIB_DIR`). The default static linking feature is used. The upstream model bundle is about 630 MiB on disk; inference uses additional RAM.

On Unix, the executable automatically sets its nice value to +19 before creating the Tokio runtime or starting any capture, recognition or compression workers. On Linux, this is the lowest normal CPU scheduling priority and is inherited by newly created threads and FFmpeg child processes. Track Listener can still use spare CPU capacity, but yields to higher-priority workloads under contention; it is not a hard CPU limit. No sudo or additional container capabilities are needed. If the operating system rejects the change, the service logs a warning and continues. Non-Unix builds leave scheduling priority unchanged. This applies to the executable, not the preceding Cargo build.

## Docker

```sh
./scripts/download-models.sh
docker compose up --build
```

Compose mounts `data/` persistently, `models/` read-only, and `config.docker.toml` as the container config. The published port is restricted to the host loopback. The Docker build is multi-stage (Node frontend, Rust backend, Debian runtime).

## Demo / archive-only operation

Demo does not connect to radio or load models. It generates clearly labeled tone fixtures, not real transmissions or speech. It uses separate `data/demo.sqlite3` and `data/demo-clips/` paths. A no-default-features build does not link Sherpa-ONNX, useful for testing the core and UI:

```sh
cargo run --no-default-features -- --demo
# or view an existing archive without recording:
cargo run --no-default-features -- --no-capture
```

Run only one service instance per database/archive directory, and stop it before changing storage configuration. Relative paths are resolved against the working directory. Back up your existing database and audio directory before switching versions. Existing SQLite archives migrate automatically by adding an audio format column. The worker also converts retained WAV files from previous versions, including in archive-only mode. Back up the archive before upgrading; older releases cannot play compressed files.

## Configure

Edit `config.toml` (or `config.docker.toml` for Compose):

```toml
[audio]
mode = "disk" # or "ram"
max_mib = 512
path = "data/clips"
max_age_hours = 0 # zero disables age-based audio eviction
transcript_max_age_hours = 0 # zero keeps transcripts indefinitely
queue_max_clips = 64
max_clip_seconds = 25

[recognition]
model_dir = "models/sherpa-onnx-nemo-parakeet-tdt-0.6b-v2-int8"
vad_model = "models/silero_vad.onnx"
threads = 4
workers = 1
vad_threshold = 0.35
min_speech_seconds = 0.20
min_silence_seconds = 0.65
hotwords = ["highball", "C.P."]
hotwords_score = 0.75 # set to 0 or use hotwords = [] to restore greedy decoding
```

The archive budget applies to total retained audio bytes across all channels (actual MP4 file sizes after conversion). Oldest audio clips are evicted first; late-arriving clips cannot evict newer ones to make space for themselves. A zero-byte budget retains transcripts only. Age expiration is also applied periodically and at archive access. SQLite transcripts survive audio eviction and are retained indefinitely by default. Optionally set `transcript_max_age_hours` to an age in hours; if both age limits are enabled, the transcript limit must exceed the audio limit. Expired transcript rows are deleted only after audio is gone and recognition is no longer pending. For example, set `max_age_hours = 168` and `transcript_max_age_hours = 2160` for seven-day audio and 90-day transcript age limits; the audio byte budget may evict clips sooner. At 32 kbit/s, AAC payloads use roughly one eighth of the space of 16 kHz mono PCM WAV before container overhead. Actual savings and coverage depend on clip lengths and encoder output. The worker encodes one file at a time outside the store lock, publishes the MP4 by atomic rename, updates the retained byte count, and removes the WAV only after successful publication. Failed encodes leave the WAV available for playback and retry. An in-progress MP4 temporary file uses additional disk space outside the retained payload budget. RAM mode does not survive restarts. This is a payload budget, not a hard limit on process RSS: inference models, queued float samples, temporary encoding buffers, in-flight HTTP responses, and SQLite caches need additional memory. Disk mode does not cap the SQLite database itself. The queue has a separate bounded clip count.

Four feeds are preconfigured under California: Orange County, Metrolink San Fernando Valley, Santa Barbara and Caltrain. The optional `region` field groups channels in the web navigation. Existing configurations keep the California grouping for those four IDs; other channels without a region appear under Unassigned. Add more with:

```toml
[[channels]]
id = "extra"
name = "Extra dispatch channel"
region = "California"
url = "https://example.org/radio.pls"
enabled = true
```

Direct HTTP(S) audio URLs are accepted as well as `.pls` URLs. Disable a channel with `enabled = false`. A playlist may use alternative stream URLs; the capture task cycles through them on disconnect. Only use authorized radio feeds and retention policies.

## API

| Method | Path | Purpose |
|---|---|---|
| GET | `/health` | Process liveness (not upstream feed health) |
| GET | `/api/channels` | Per-channel region, connection state, errors, dropped jobs and last audio time |
| GET | `/api/status` | Retention stats and queue status |
| GET | `/api/clips` | Paginated transcript/clip list |
| GET, HEAD | `/api/clips/{id}/audio` | WAV or AAC/MP4 audio with HTTP Range seeking and format-specific Content-Type; 404 after audio eviction |

Clip filters: `channel`, `q` (literal case-insensitive ASCII substring), `since`, `until` (Unix milliseconds), `limit` (1–100), `before_ms` and `before_id` (together from `next_cursor`). Sorting is newest-first by timestamp and UUID. `start_ms`/`end_ms` for sentences are offsets into the clip; `started_ms` for the clip is UTC Unix milliseconds. Sentence alignment uses model token offsets only when the normalized token text matches the transcript. Unknown offsets are null. Capture timestamps are receiver-side estimates anchored to the arrival of the first decoded frame; upstream buffering, transport delay and FFmpeg buffering can shift them. Sentence boundaries are inferred from model punctuation.

## Tests and status

```sh
cargo test --no-default-features # core tests (FFmpeg required), no ASR native libraries
cargo test --all-features       # also compile the native recognition integration
(cd web && npm install && npm run build)
```

The included tests cover WAV serialization, PLS parsing, sentence alignment and fallback, oldest-first eviction, age/byte limits, zero-budget handling, restart recovery, search and pagination, and HTTP byte-range responses. CI also exercises the demo API and compiles the native backend and frontend. These tests have been authored but **not run** in the creation environment: it has no Rust toolchain and no access to dependency download servers. No lockfile could be generated; generate and commit `Cargo.lock` and `web/package-lock.json` after the first successful build. Native recognition, live feed reception, frontend compilation and Docker build remain unverified. The previous attempt to access the four playlists timed out. The new implementation is not a claim that those feeds are currently online.

## Railroad vocabulary

The recognition configuration biases Parakeet toward `highball` and `C.P.` (control point) by default, including common casing, spacing and acronym variants. Edit `recognition.hotwords` in the TOML file to customize the list. The application prepares supported token sequences from the installed model vocabulary and enables modified beam search with a modest configurable score. No model retraining or extra download is required. This is a decoding preference, not a guarantee or a forced transcript replacement. Token segmentation is an approximation for older model archives without a BPE vocabulary, and modified beam search can be slower or less accurate than greedy decoding. Set `hotwords_score = 0` or `hotwords = []` to restore greedy search if quality degrades. Changes take effect after restart; previous transcripts are not rewritten. Sentence splitting preserves the dotted C.P. abbreviation.

## Silent recordings

New live captures also use an adaptive, independent noise floor per channel, estimated from recent Silero-idle windows. A DC-insensitive RMS gate rejects near-silence, low-level stationary hiss and isolated spikes before writing or queueing clips. It requires roughly 96 ms of above-floor audio (or all windows of a shorter capped tail), trims quiet leading/trailing audio with 200 ms of context, and adjusts the capture start timestamp. Interior pauses and original undenoised PCM are preserved. The adaptive RMS threshold is bounded between about -66 and -42 dBFS; until enough idle history is collected it uses a conservative low-level fallback. Profiles reset on reconnect. The original all-samples-within-eight-PCM-counts guard still applies at storage. Weak speech near the noise floor can be missed; actual thresholds need validation against live recordings.

For an archived clip longer than one second, an empty, punctuation-only or failed recognition triggers exactly one retry with `flexaudio-denoise` (offline RNNoise) on the existing ASR worker. A filtered 48 kHz mono buffer is denoised in memory, the library's delay is removed, and the result is converted back to the original 16 kHz sample count so transcript offsets still reference the original recording. No denoised file is written; a failed retry leaves the first result/error and original audio intact. Successfully recognized and shorter clips skip this extra processing.

The asynchronous archive worker still uses the stricter eight-PCM-count peak rule when checking previously stored WAV and AAC/MP4 recordings, including in `--no-capture` mode. If a complete, successful decode proves the clip is silent, its database row, sentence rows and audio file are removed. Non-silent historical clips are marked as checked in SQLite to avoid decoding them again. Missing, corrupt, truncated or already-evicted recordings cannot establish silence, so their transcripts are preserved. Back up your archive before upgrading if you want to retain even silent files.

## Operational limitations

- Recognition jobs are in-memory. Queued jobs interrupted by a crash are marked `interrupted` at next startup; audio on disk can remain playable. There is no durable job spool or restart retry in this version.
- No authentication or TLS is included. Keep the server private, or deploy behind an authenticated reverse proxy. Do not publish raw dispatch radio clips without authorization. Only one process should own an archive.
- Transcripts can contain errors and invented words. Do not use automated text for safety-critical railway operations. Verify against the original audio.
- Unlimited transcript retention can grow the SQLite database; configure a transcript age limit if appropriate. Multi-process deployments and durable recognition retry queues are future work.

## Layout

```
Cargo.toml
src/
  main.rs          CLI, service lifecycle, demo
  config.rs        TOML settings and validation
  capture.rs       FFmpeg, per-channel VAD, reconnect
  recognition.rs   CPU-only Parakeet, inference workers
  silence.rs       adaptive per-channel clip activity and edge trimming
  retry.rs         recognition fallback policy
  denoise.rs       in-memory RNNoise and sample-rate conversion
  pipeline.rs      shared state and bounded job queue
  audio.rs         PCM/WAV and UTC time helpers
  compression.rs   asynchronous AAC/MP4 archive encoder
  playlists.rs     PLS parsing and resolving
  sentences.rs     sentence segmentation/alignment
  store.rs         SQLite and archive retention
  api.rs           Axum API and audio range responses
web/               Svelte interface
scripts/           model download shell script
.github/workflows/ci.yml
```

Upstream references: [Sherpa-ONNX Rust API](https://docs.rs/sherpa-onnx/latest/sherpa_onnx/), [Parakeet model files](https://k2-fsa.github.io/sherpa/onnx/pretrained_models/offline-transducer/nemo-transducer-models.html), [Parakeet license/model card](https://huggingface.co/nvidia/parakeet-tdt-0.6b-v2). Model files are not redistributed in this archive.
