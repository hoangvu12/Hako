//! The upload engine: a serial queue draining one clip at a time (each clip's
//! own multipart upload already runs 4 parts in parallel), with byte-accurate
//! throttled progress, mid-flight cancel, and presign-on-success. Transient
//! failures are retried inside OpenDAL's `RetryLayer`; only terminal failures
//! reach the `error` state and the user re-triggers from there (Medal's model).
//!
//! Concurrency = 1 clip at a time (decided in the plan's §10): gentlest on the
//! uplink during gameplay. The worker is spawned once at startup and owns the
//! receiver + an `AppHandle` for DB access and event emission.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use tokio::io::AsyncReadExt;
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};

use super::{config_dir, operator, providers};
use crate::commands::{LibraryState, SettingsState};
use crate::events;
use crate::library::db::cloud_status;

/// Multipart part size streamed to the provider.
const CHUNK: usize = 8 * 1024 * 1024; // 8 MiB
/// Parts uploaded in parallel within a single clip's upload. Serial (1) on
/// purpose: with parallel parts the writer buffers the whole (small) clip before
/// any bytes leave, so progress sticks at 0% and every request fires silently
/// inside `close()`. Serial makes each `write()` block until its part is truly
/// uploaded — honest byte progress, gentler on the uplink (the design goal), and
/// friendlier to lightweight S3-compatible backends that dislike parallel part
/// PUTs. The cross-clip queue is already serial, so this only caps intra-clip
/// fan-out.
const CONCURRENT: usize = 1;
/// Min interval between progress event/DB writes (the renderer doesn't need more).
const PROGRESS_THROTTLE: Duration = Duration::from_millis(250);
/// Presigned read-URL lifetime. Stored as a refreshable convenience (cloud-only
/// playback), never as source of truth — we re-presign on demand when it lapses.
const PRESIGN_TTL: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// One unit of work for the upload worker.
pub struct UploadJob {
    pub clip_id: i64,
    pub provider_id: String,
}

/// Managed state for the upload engine. Holds the queue sender plus the small
/// in-flight bookkeeping (which clips are queued, which were asked to cancel).
pub struct CloudState {
    tx: UnboundedSender<UploadJob>,
    inner: Arc<Mutex<Inner>>,
}

#[derive(Default)]
struct Inner {
    /// Clips currently queued or uploading — dedupes double-enqueues.
    queued: HashSet<i64>,
    /// Clips the user asked to cancel — consumed by the worker.
    cancels: HashSet<i64>,
    /// Clips currently being re-downloaded (cloud → local) for editing —
    /// dedupes double-clicks of "Download to edit".
    downloads: HashSet<i64>,
}

impl CloudState {
    /// Create the state and the matching receiver. Pass the receiver to
    /// [`spawn_worker`]; manage the state.
    pub fn new() -> (CloudState, UnboundedReceiver<UploadJob>) {
        let (tx, rx) = mpsc::unbounded_channel();
        (
            CloudState {
                tx,
                inner: Arc::new(Mutex::new(Inner::default())),
            },
            rx,
        )
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        // The lock is only ever held for set ops, never across an await, so a
        // poisoned lock would mean a panic mid-mutation: recover the guard.
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Mark a clip queued; returns false if it was already queued/in-flight (so
    /// the caller skips a duplicate job).
    fn mark_queued(&self, clip_id: i64) -> bool {
        self.lock().queued.insert(clip_id)
    }

    fn unmark_queued(&self, clip_id: i64) {
        self.lock().queued.remove(&clip_id);
    }

    /// Number of clips queued or uploading — drives the toast's "+N" badge.
    pub fn queued_count(&self) -> usize {
        self.lock().queued.len()
    }

    /// Whether a live worker job currently owns this clip (queued or in-flight).
    /// False for a persisted row with no backing job — e.g. one interrupted by an
    /// app restart — which cancel must terminate directly rather than signal.
    pub fn is_tracked(&self, clip_id: i64) -> bool {
        self.lock().queued.contains(&clip_id)
    }

    /// Request cancellation of a clip's upload (queued or in-flight).
    pub fn request_cancel(&self, clip_id: i64) {
        self.lock().cancels.insert(clip_id);
    }

    fn is_cancel_requested(&self, clip_id: i64) -> bool {
        self.lock().cancels.contains(&clip_id)
    }

    fn clear_cancel(&self, clip_id: i64) {
        self.lock().cancels.remove(&clip_id);
    }

    /// Claim the download slot for a clip; returns false if one is already in
    /// flight (so the caller rejects the duplicate request).
    pub fn begin_download(&self, clip_id: i64) -> bool {
        self.lock().downloads.insert(clip_id)
    }

    /// Release a clip's download slot (any outcome).
    pub fn end_download(&self, clip_id: i64) {
        self.lock().downloads.remove(&clip_id);
    }

    /// Enqueue a job onto the worker. Returns an error only if the worker is gone.
    fn send(&self, job: UploadJob) -> Result<(), String> {
        self.tx
            .send(job)
            .map_err(|_| "upload worker is not running".to_string())
    }
}

/// Spawn the single draining worker. Processes jobs strictly one at a time.
pub fn spawn_worker(app: AppHandle, mut rx: UnboundedReceiver<UploadJob>) {
    tauri::async_runtime::spawn(async move {
        while let Some(job) = rx.recv().await {
            wait_for_background_window(&app, "cloud upload").await;
            run_job(&app, job).await;
        }
        tracing::warn!("cloud upload worker channel closed");
    });
}

async fn wait_for_background_window(app: &AppHandle, what: &str) {
    let mut logged = false;
    while crate::commands::pause_background_work(app) {
        if !logged {
            tracing::info!("{what}: paused while gaming");
            logged = true;
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
    if logged {
        tracing::info!("{what}: resumed after gaming paused/stopped");
    }
}

// --- event payloads --------------------------------------------------------

#[derive(Clone, Serialize)]
struct ProgressPayload {
    clip_id: i64,
    provider_id: String,
    sent: u64,
    total: u64,
    bytes_per_sec: u64,
}

#[derive(Clone, Serialize)]
struct StatusPayload {
    clip_id: i64,
    provider_id: String,
    status: String,
    error: Option<String>,
}

fn emit_status(
    app: &AppHandle,
    clip_id: i64,
    provider_id: &str,
    status: &str,
    error: Option<&str>,
) {
    let _ = app.emit(
        events::CLOUD_UPLOAD_STATUS,
        StatusPayload {
            clip_id,
            provider_id: provider_id.to_string(),
            status: status.to_string(),
            error: error.map(str::to_string),
        },
    );
}

/// Outcome of the byte-streaming phase.
enum StreamOutcome {
    Done,
    Canceled,
    Failed(String),
}

/// Process one job end to end: resolve clip + provider, stream the bytes, then
/// finalize (done / canceled / error) in the DB and to the renderer.
async fn run_job(app: &AppHandle, job: UploadJob) {
    let UploadJob {
        clip_id,
        provider_id,
    } = job;
    let cloud = app.state::<CloudState>();

    // Always release the queued slot when this job ends, whatever the outcome.
    let _guard = QueuedGuard {
        cloud: app.state::<CloudState>(),
        clip_id,
    };

    // Resolve the clip's file + creation time (under the library lock, briefly).
    let clip = {
        let lib = app.state::<LibraryState>();
        let guard = match lib.0.lock() {
            Ok(g) => g,
            Err(_) => return,
        };
        guard.get(clip_id)
    };
    let clip = match clip {
        Ok(Some(c)) => c,
        Ok(None) => {
            fail(app, clip_id, &provider_id, "clip no longer exists");
            return;
        }
        Err(e) => {
            fail(app, clip_id, &provider_id, &e);
            return;
        }
    };

    // Honor a cancel requested while still queued (before any bytes move).
    if cloud.is_cancel_requested(clip_id) {
        cloud.clear_cancel(clip_id);
        cancel(app, clip_id, &provider_id);
        return;
    }

    // Build the operator from the stored config + keyring secrets.
    let op = match build_op(app, &provider_id) {
        Ok(op) => op,
        Err(e) => {
            fail(app, clip_id, &provider_id, &e);
            return;
        }
    };
    let remote = operator::remote_key(
        &provider_config_kind(app, &provider_id),
        clip.created_unix_ms,
        &clip.path,
    );

    // Transition the row to `uploading` and tell the UI.
    if let Ok(lib) = app.state::<LibraryState>().0.lock() {
        let _ = lib.cloud_mark_uploading(clip_id, &provider_id, &remote);
    }
    emit_status(app, clip_id, &provider_id, cloud_status::UPLOADING, None);

    // Race the upload against a cancel watcher. `stream_upload` also checks the
    // cancel flag cooperatively between chunks, but a request hung *inside* an
    // OpenDAL call (e.g. multipart-init stalled at 0%) never reaches a checkpoint
    // — so we also lose the `select!` to the watcher, which drops the upload
    // future and aborts its in-flight reqwest calls. `biased` polls the upload
    // first so progress isn't starved by the poll loop.
    let outcome = tokio::select! {
        biased;
        out = stream_upload(app, &op, &clip.path, &remote, clip_id, &provider_id) => out,
        _ = wait_for_cancel(&cloud, clip_id) => StreamOutcome::Canceled,
    };
    match outcome {
        StreamOutcome::Done => {
            // Presign a read URL (best-effort; null when unsupported/expired).
            let url = operator::presign_get(&op, &remote, PRESIGN_TTL)
                .await
                .ok()
                .flatten();
            if let Ok(lib) = app.state::<LibraryState>().0.lock() {
                let _ = lib.cloud_mark_done(clip_id, &provider_id, url.as_deref());
            }
            emit_status(app, clip_id, &provider_id, cloud_status::DONE, None);
            tracing::info!("cloud upload done: clip {clip_id} → {provider_id}:{remote}");
            // Opt-in retention: reclaim local space now that this clip is safe in
            // the cloud (no-op unless cloud_free_up_space_enabled).
            super::retention::maybe_free_up_space(app);
        }
        StreamOutcome::Canceled => {
            cloud.clear_cancel(clip_id);
            cancel(app, clip_id, &provider_id);
        }
        StreamOutcome::Failed(msg) => fail(app, clip_id, &provider_id, &msg),
    }
}

/// Resolves when the user requests this clip's cancellation. Raced against the
/// upload in [`run_job`] so a *hung* request (one that never reaches a
/// cooperative checkpoint) is still aborted: losing the `select!` drops the
/// upload future, cancelling its in-flight reqwest calls. Polls because cancels
/// live in a sync set; the interval is cheap relative to an upload.
async fn wait_for_cancel(cloud: &CloudState, clip_id: i64) {
    while !cloud.is_cancel_requested(clip_id) {
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// Stream the local file through the OpenDAL writer (transparent multipart),
/// counting bytes for throttled progress and checking for cancellation between
/// chunks.
async fn stream_upload(
    app: &AppHandle,
    op: &opendal::Operator,
    local_path: &str,
    remote: &str,
    clip_id: i64,
    provider_id: &str,
) -> StreamOutcome {
    let cloud = app.state::<CloudState>();

    let total = std::fs::metadata(local_path).map(|m| m.len()).unwrap_or(0);
    let mut file = match tokio::fs::File::open(local_path).await {
        Ok(f) => f,
        Err(e) => return StreamOutcome::Failed(format!("open clip file: {e}")),
    };
    // Opening the writer kicks off the provider's multipart init — the request
    // that stalls at 0% on a misconfigured S3-compatible endpoint. Log around it
    // so the file log shows whether a hang is here vs. in the chunk uploads.
    tracing::info!("cloud upload: opening writer for clip {clip_id} ({total} bytes) → {remote}");
    let mut writer = match op
        .writer_with(remote)
        .chunk(CHUNK)
        .concurrent(CONCURRENT)
        .await
    {
        Ok(w) => w,
        Err(e) => return StreamOutcome::Failed(operator::friendly_error(&e)),
    };
    tracing::info!("cloud upload: writer ready for clip {clip_id}, streaming bytes");

    let mut buf = vec![0u8; CHUNK];
    let mut sent: u64 = 0;
    let started = Instant::now();
    let mut last_emit = Instant::now();

    loop {
        let n = match file.read(&mut buf).await {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) => {
                let _ = writer.abort().await;
                return StreamOutcome::Failed(format!("read clip file: {e}"));
            }
        };
        if let Err(e) = writer.write(buf[..n].to_vec()).await {
            let _ = writer.abort().await;
            return StreamOutcome::Failed(operator::friendly_error(&e));
        }
        sent += n as u64;

        if cloud.is_cancel_requested(clip_id) {
            let _ = writer.abort().await;
            return StreamOutcome::Canceled;
        }

        if last_emit.elapsed() >= PROGRESS_THROTTLE {
            emit_progress(app, clip_id, provider_id, sent, total, started);
            persist_progress(app, clip_id, provider_id, sent);
            last_emit = Instant::now();
        }
    }

    // All parts buffered/uploaded; `close()` flushes the last part + completes the
    // multipart upload (the provider may take a while here — e.g. a backend that
    // assembles parts on finalize). Log around it so a stall here is unambiguous.
    tracing::info!(
        "cloud upload: finalizing clip {clip_id} ({sent} bytes streamed), completing multipart"
    );
    if let Err(e) = writer.close().await {
        return StreamOutcome::Failed(operator::friendly_error(&e));
    }
    tracing::info!(
        "cloud upload: finalized clip {clip_id} in {:.1}s",
        started.elapsed().as_secs_f64()
    );
    // Final 100% tick so the UI lands exactly on total.
    emit_progress(app, clip_id, provider_id, sent, total, started);
    StreamOutcome::Done
}

fn emit_progress(
    app: &AppHandle,
    clip_id: i64,
    provider_id: &str,
    sent: u64,
    total: u64,
    started: Instant,
) {
    let secs = started.elapsed().as_secs_f64();
    let bytes_per_sec = if secs > 0.0 {
        (sent as f64 / secs) as u64
    } else {
        0
    };
    let _ = app.emit(
        events::CLOUD_UPLOAD_PROGRESS,
        ProgressPayload {
            clip_id,
            provider_id: provider_id.to_string(),
            sent,
            total,
            bytes_per_sec,
        },
    );
}

fn persist_progress(app: &AppHandle, clip_id: i64, provider_id: &str, sent: u64) {
    if let Ok(lib) = app.state::<LibraryState>().0.lock() {
        let _ = lib.cloud_set_progress(clip_id, provider_id, sent as i64);
    }
}

fn fail(app: &AppHandle, clip_id: i64, provider_id: &str, msg: &str) {
    if let Ok(lib) = app.state::<LibraryState>().0.lock() {
        let _ = lib.cloud_mark_failed(clip_id, provider_id, cloud_status::ERROR, msg);
    }
    emit_status(app, clip_id, provider_id, cloud_status::ERROR, Some(msg));
    tracing::warn!("cloud upload failed: clip {clip_id} → {provider_id}: {msg}");
}

fn cancel(app: &AppHandle, clip_id: i64, provider_id: &str) {
    if let Ok(lib) = app.state::<LibraryState>().0.lock() {
        let _ = lib.cloud_mark_failed(clip_id, provider_id, cloud_status::CANCELED, "canceled");
    }
    emit_status(app, clip_id, provider_id, cloud_status::CANCELED, None);
    tracing::info!("cloud upload canceled: clip {clip_id} → {provider_id}");
}

/// Build an operator for a provider id (config from disk + secrets from keyring).
pub(super) fn build_op(app: &AppHandle, provider_id: &str) -> Result<opendal::Operator, String> {
    let dir = config_dir(app)?;
    let cfg = providers::find_provider(&dir, provider_id)
        .ok_or_else(|| format!("no such provider: {provider_id}"))?;
    let secrets = providers::get_secrets(provider_id)?;
    operator::build_operator(&cfg, &secrets)
}

/// The `ProviderKind` for a provider id (for remote-key derivation). Falls back
/// to a bare S3 kind if the config vanished — the upload itself would already
/// have failed in [`build_op`], so this only affects the key string.
fn provider_config_kind(app: &AppHandle, provider_id: &str) -> providers::ProviderKind {
    config_dir(app)
        .ok()
        .and_then(|dir| providers::find_provider(&dir, provider_id))
        .map(|c| c.kind)
        .unwrap_or(providers::ProviderKind::S3 {
            endpoint: String::new(),
            region: String::new(),
            bucket: String::new(),
            prefix: String::new(),
        })
}

/// Releases a clip's queued slot when a job finishes (any outcome / early return).
struct QueuedGuard<'a> {
    cloud: tauri::State<'a, CloudState>,
    clip_id: i64,
}

impl Drop for QueuedGuard<'_> {
    fn drop(&mut self) {
        self.cloud.unmark_queued(self.clip_id);
    }
}

// --- enqueue (shared by the command + the auto-upload hook) ----------------

/// Reset a clip's row to `queued`, dedupe against in-flight work, emit a status
/// event, and hand the job to the worker. Shared by [`cloud_upload_clip`] and
/// [`maybe_auto_upload`]. Returns `Ok(())` (a no-op) if the clip is already
/// queued or uploading.
fn enqueue(app: &AppHandle, clip_id: i64, provider_id: String) -> Result<(), String> {
    // Look up the clip to size the row (and confirm it exists), then enqueue it.
    {
        let lib = app.state::<LibraryState>();
        let guard = lib.0.lock().map_err(|_| "library poisoned")?;
        let rec = guard.get(clip_id)?.ok_or("no such clip")?;
        guard.cloud_enqueue(clip_id, &provider_id, rec.size_bytes)?;
    }

    let cloud = app.state::<CloudState>();
    cloud.clear_cancel(clip_id); // a fresh request overrides a stale cancel
    if !cloud.mark_queued(clip_id) {
        // Already queued/in-flight — the existing job will run; don't double up.
        return Ok(());
    }
    emit_status(app, clip_id, &provider_id, cloud_status::QUEUED, None);
    cloud.send(UploadJob {
        clip_id,
        provider_id,
    })
}

/// Auto-upload entry point, called right after a clip row is inserted (manual
/// save and auto-clip paths). Enqueues the clip to the default provider when
/// `cloud_auto_upload` is on. Best-effort and non-fatal: any misconfiguration
/// (cloud subsystem absent, no default provider, enqueue error) is logged and
/// swallowed — auto-upload must never break the save path.
pub fn maybe_auto_upload(app: &AppHandle, clip_id: i64) {
    // Cloud subsystem may not be initialized (e.g. early-startup failure).
    if app.try_state::<CloudState>().is_none() {
        return;
    }

    let (enabled, provider) = match app.try_state::<SettingsState>() {
        Some(state) => match state.0.lock() {
            Ok(s) => (s.cloud_auto_upload, s.cloud_default_provider.clone()),
            Err(_) => return,
        },
        None => return,
    };
    if !enabled {
        return;
    }

    let provider_id = match provider.filter(|p| !p.trim().is_empty()) {
        Some(p) => p,
        None => {
            tracing::warn!(
                "cloud auto-upload is on but no default provider is set; skipping clip {clip_id}"
            );
            return;
        }
    };

    match enqueue(app, clip_id, provider_id) {
        Ok(()) => tracing::info!("cloud auto-upload: enqueued clip {clip_id}"),
        Err(e) => tracing::warn!("cloud auto-upload enqueue failed for clip {clip_id}: {e}"),
    }
}

// --- commands --------------------------------------------------------------

/// Enqueue a clip for upload. `provider_id` defaults to the configured
/// `cloud_default_provider`. Resets the row to `queued` and emits a status event.
#[tauri::command]
pub fn cloud_upload_clip(
    app: AppHandle,
    clip_id: i64,
    provider_id: Option<String>,
) -> Result<(), String> {
    // Resolve the target provider.
    let provider_id = match provider_id {
        Some(p) if !p.trim().is_empty() => p,
        _ => app
            .state::<SettingsState>()
            .0
            .lock()
            .ok()
            .and_then(|s| s.cloud_default_provider.clone())
            .filter(|p| !p.trim().is_empty())
            .ok_or("no cloud provider selected (set a default in Settings)")?,
    };

    enqueue(&app, clip_id, provider_id)
}

/// Cancel a clip's queued or in-flight upload. For a live job this signals the
/// worker (which hard-aborts via the `select!` in `run_job`). For a row with no
/// backing job — e.g. one left `uploading` by a previous run — it terminates the
/// DB row and emits `canceled` directly, so the toast always clears.
#[tauri::command]
pub fn cloud_cancel_upload(app: AppHandle, clip_id: i64) -> Result<(), String> {
    let cloud = app.state::<CloudState>();
    cloud.request_cancel(clip_id);
    if cloud.is_tracked(clip_id) {
        return Ok(()); // a live worker job will observe the flag and finalize.
    }

    // No live job: finalize any lingering non-terminal rows ourselves.
    let stale: Vec<String> = {
        let lib = app.state::<LibraryState>();
        let guard = lib.0.lock().map_err(|_| "library poisoned")?;
        let rows = guard.cloud_status(Some(clip_id)).unwrap_or_default();
        let providers: Vec<String> = rows
            .into_iter()
            .filter(|r| r.status == cloud_status::QUEUED || r.status == cloud_status::UPLOADING)
            .map(|r| r.provider_id)
            .collect();
        for provider_id in &providers {
            let _ =
                guard.cloud_mark_failed(clip_id, provider_id, cloud_status::CANCELED, "canceled");
        }
        providers
    };
    for provider_id in &stale {
        emit_status(&app, clip_id, provider_id, cloud_status::CANCELED, None);
    }
    cloud.clear_cancel(clip_id);
    Ok(())
}

/// Cloud-upload rows for one clip, or all rows when `clip_id` is `None`.
#[tauri::command]
pub fn cloud_upload_status(
    app: AppHandle,
    clip_id: Option<i64>,
) -> Result<Vec<crate::library::db::CloudUpload>, String> {
    app.state::<LibraryState>()
        .0
        .lock()
        .map_err(|_| "library poisoned")?
        .cloud_status(clip_id)
}
