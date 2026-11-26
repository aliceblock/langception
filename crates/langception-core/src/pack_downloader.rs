//! The Model Pack downloader (ADR-0008, deferred-model-packs ticket 03):
//! the transport half of the pack feature — fetch a pack's archive
//! following the manifest's URL order, verify it, and hand it to the
//! assembly ([`crate::model_packs::install_pack_archive`]) that lands it
//! in the active models directory. The pure half (manifest, required
//! packs, status, assembly) lives in [`crate::model_packs`]; this module
//! adds only the network and the download lifecycle. No UI here — the
//! Engines tab is ticket 04.
//!
//! The pieces and their contracts:
//!
//! - **Transport boundary** ([`PackTransport`]): one HTTP GET per URL,
//!   the body as an async chunk stream. Production speaks reqwest
//!   ([`HttpPackTransport`]); the tests script canned responses — the
//!   same seam shape as the Google leg chain's transport, so CI never
//!   touches the network.
//! - **Fallback order**: the manifest's URLs are tried in order (the
//!   GitHub Releases asset first, pinned upstream mirrors after). A
//!   source that cannot be reached, answers an HTTP error, drops the
//!   connection mid-stream, or delivers bytes that fail their pins, is
//!   a failed source and the next URL is tried automatically — each
//!   attempt downloads from zero (no byte-resume by design). The pack
//!   is rejected WHOLE only when every source failed: nothing lands in
//!   the active directory ([`crate::model_packs::install_pack_archive`]
//!   verifies every file's size and hash before anything moves), and
//!   the failed state carries the last source's verdict for the Retry.
//! - **Staging**: archive bytes stream into a residue-named temp file
//!   beside the active models directory (the module-wide
//!   [`crate::model_packs`] residue contract: dot-prefixed so the
//!   sidecar never sees it, pid-tagged so a concurrent assembly's sweep
//!   never deletes an in-flight download), and the temp is removed on
//!   every exit path — success, failure, cancel, panic. The active
//!   directory only ever receives verified files, atomically.
//! - **Lifecycle**: owned by the core, never by a window. [`start`]
//!   spawns a tokio task; closing the Settings window drops nothing.
//!   A returning window re-attaches by subscribing again and querying
//!   [`state`] — the truth, not a replay.
//! - **Emit discipline** (the post-v0.2.0 loop-event broadcast rule):
//!   one broadcast send per event, never one per window. The
//!   window-existence half of the rule lives at the shell seam (ticket
//!   04): the shell forwards an event to the webview only while a
//!   consuming window exists. Core-side, an event channel with zero
//!   receivers is a no-op send, and the download continues regardless.
//!
//! [`start`]: PackDownloader::start
//! [`state`]: PackDownloader::state

use crate::model_packs::{
    install_pack_archive, pack_status, residue_dir_sibling, Manifest, PackId, PackSpec,
    PackStatus,
};
use async_trait::async_trait;
use serde::Serialize;
use std::collections::BTreeMap;
use std::fs;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{broadcast, watch};

/// How many events a slow consumer may fall behind before it starts
/// receiving `Lagged` (and must re-sync through [`PackDownloader::state`],
/// the same re-attach a reopened window does). Progress events are
/// step-gated, so the steady state is a handful of events per pack.
const EVENT_CAPACITY: usize = 256;

// ---- the transport boundary ----

/// One source's answer to the archive fetch: the HTTP status, the
/// advertised total size when the source declares one (display-only —
/// the manifest's pins are the only truth), and the body as an async
/// chunk stream.
pub struct PackResponse {
    pub status: u16,
    pub content_length: Option<u64>,
    pub body: Box<dyn PackBody>,
}

impl PackResponse {
    /// Whether the fetch answered with a success status.
    fn is_success(&self) -> bool {
        self.status / 100 == 2
    }
}

/// The archive's byte stream, one chunk at a time. A chunked body keeps
/// the seam free of any concrete stream type: the tests script
/// `Vec<u8>` chunks, production wraps reqwest's.
#[async_trait]
pub trait PackBody: Send {
    /// The next chunk of the archive; `None` ends the body. An error
    /// means the source connection failed mid-transfer — the downloader
    /// treats it like any other transport failure and falls to the next
    /// source.
    async fn next_chunk(&mut self) -> std::io::Result<Option<Vec<u8>>>;
}

/// The seam under the downloader: production speaks reqwest, the tests
/// script responses per call. Same shape as the Google leg chain's
/// transport (ticket 02 of the rate-limit-resilience feature).
#[async_trait]
pub trait PackTransport: Send + Sync {
    /// Fetches one URL. Every failure to even get an answer — DNS,
    /// refused, TLS, timeout — is a [`PackFetchError`]; an HTTP answer
    /// with any status (success or not) is a [`PackResponse`], because
    /// the status is the caller's routing decision.
    async fn fetch(&self, url: &str) -> Result<PackResponse, PackFetchError>;
}

/// One source was unreachable — the transfer never started. `detail`
/// carries the plain reason for the failed-state message.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("cannot reach {url}: {detail}")]
pub struct PackFetchError {
    pub url: String,
    pub detail: String,
}

/// The production transport: reqwest, an honest User-Agent, redirects
/// followed (the GitHub Releases asset redirects to its storage), a
/// connect timeout so an unreachable host is diagnosed in seconds — and
/// deliberately NO overall timeout: a slow-but-alive multi-minute
/// transfer must not be killed mid-flight, and the user's Cancel is the
/// abort control (ADR-0008 rejected pause/resume; a hung transfer is
/// canceled, not timed out).
pub struct HttpPackTransport {
    client: reqwest::Client,
}

impl HttpPackTransport {
    pub fn new() -> Result<Self, String> {
        let client = reqwest::Client::builder()
            .user_agent(format!(
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) Langception/{}",
                env!("CARGO_PKG_VERSION")
            ))
            .connect_timeout(Duration::from_secs(30))
            .build()
            .map_err(|e| format!("failed to build the download client: {e}"))?;
        Ok(Self { client })
    }
}

struct ReqwestBody {
    response: reqwest::Response,
}

#[async_trait]
impl PackBody for ReqwestBody {
    async fn next_chunk(&mut self) -> std::io::Result<Option<Vec<u8>>> {
        match self.response.chunk().await {
            Ok(Some(chunk)) => Ok(Some(chunk.to_vec())),
            Ok(None) => Ok(None),
            Err(e) => Err(std::io::Error::other(format!(
                "the source connection failed mid-transfer: {e}"
            ))),
        }
    }
}

#[async_trait]
impl PackTransport for HttpPackTransport {
    async fn fetch(&self, url: &str) -> Result<PackResponse, PackFetchError> {
        let response = self
            .client
            .get(url)
            .send()
            .await
            .map_err(|e| PackFetchError {
                url: url.to_string(),
                detail: e.to_string(),
            })?;
        let status = response.status().as_u16();
        let content_length = response.content_length();
        Ok(PackResponse {
            status,
            content_length,
            body: Box::new(ReqwestBody { response }),
        })
    }
}

// ---- events and state ----

/// One pack download event, broadcast exactly once per occurrence on the
/// downloader's channel (the emit discipline in the module docs). The
/// wire shape is what the Engines tab reads (ticket 04).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "event", rename_all = "kebab-case")]
pub enum PackEvent {
    /// A source answered successfully and its body is streaming. A
    /// fallback to a later mirror announces its own attempt (progress
    /// restarts from zero with it — no byte-resume by design).
    #[serde(rename_all = "camelCase")]
    Started {
        pack: PackId,
        total_bytes: Option<u64>,
    },
    /// The step-gated byte count: fired when the received count crosses
    /// a progress step, not per chunk.
    #[serde(rename_all = "camelCase")]
    Progress {
        pack: PackId,
        received_bytes: u64,
        total_bytes: Option<u64>,
    },
    /// The pack installed and verifies against its pins — disk truth
    /// ([`PackStatus::Ready`]) from here on.
    Finished { pack: PackId },
    /// The download failed (every source, or the archive failed its
    /// pins); `message` is display-ready. The state stays
    /// [`PackDownloadState::Failed`] until a Retry ([`start`]) succeeds.
    #[serde(rename_all = "camelCase")]
    Failed { pack: PackId, message: String },
    /// The user canceled; zero residue on disk, the pack reports
    /// not-downloaded again.
    Canceled { pack: PackId },
}

/// The downloader's per-pack activity state. This is DOWNLOAD activity,
/// deliberately distinct from the on-disk [`PackStatus`]: `NotDownloading`
/// means "no download in flight" — the UI combines it with the pack's
/// disk status (ready → ships/ready, missing → not downloaded, stale →
/// update pending). It survives only while it means something: `Failed`
/// persists so the Engines tab can show Retry, everything else clears
/// when the download ends.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "kebab-case")]
pub enum PackDownloadState {
    /// No download in flight for this pack.
    NotDownloading,
    #[serde(rename_all = "camelCase")]
    Downloading {
        received_bytes: u64,
        total_bytes: Option<u64>,
    },
    /// The last download failed; `message` is display-ready for the
    /// failed-state row and its Retry.
    #[serde(rename_all = "camelCase")]
    Failed { message: String },
}

/// What [`PackDownloader::start`] decided. Not errors — honest outcomes
/// the caller surfaces (or ignores): a selection that runs while a
/// download is already going, or a pack whose files already verify.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartOutcome {
    /// A download task started.
    Started,
    /// A download for this pack is already in flight; the request is a
    /// no-op (one download per pack at a time).
    AlreadyDownloading,
    /// The pack's files already verify against the manifest — nothing
    /// to download (ADR-0008: never re-download what is ready).
    AlreadyReady,
}

// ---- the downloader ----

/// Owns the Model Pack downloads (ADR-0008: the lifecycle belongs to the
/// core, independent of any window). One download per pack at a time;
/// events broadcast once on an internal channel that any number of
/// windows subscribe to.
pub struct PackDownloader {
    transport: Arc<dyn PackTransport>,
    manifest: Arc<Manifest>,
    active_dir: PathBuf,
    /// The progress step override; `None` derives from the source's
    /// declared size (see [`adaptive_step`]).
    progress_step: Option<u64>,
    events: broadcast::Sender<PackEvent>,
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    /// Per-pack cancel flags for the in-flight downloads. An entry
    /// exists exactly while its task runs; [`PackDownloader::finish`]
    /// removes it and settles the final state under the same lock, so
    /// start/cancel/state transitions and their events never interleave
    /// across tasks.
    active: BTreeMap<PackId, watch::Sender<bool>>,
    states: BTreeMap<PackId, PackDownloadState>,
}

impl PackDownloader {
    /// Builds a downloader over `transport` for the packs pinned in
    /// `manifest`, installing into `active_dir`. Must run inside a tokio
    /// runtime ([`start`] spawns tasks).
    ///
    /// [`start`]: PackDownloader::start
    pub fn new(
        transport: Arc<dyn PackTransport>,
        manifest: Arc<Manifest>,
        active_dir: impl Into<PathBuf>,
    ) -> Self {
        let (events, _) = broadcast::channel(EVENT_CAPACITY);
        Self {
            transport,
            manifest,
            active_dir: active_dir.into(),
            progress_step: None,
            events,
            inner: Mutex::new(Inner::default()),
        }
    }

    /// Overrides the progress step (bytes per Progress event) — the
    /// tests pin the progress sequence with tiny steps; production
    /// leaves this unset and lets [`adaptive_step`] decide.
    #[cfg(test)]
    pub(crate) fn with_progress_step(mut self, step: u64) -> Self {
        self.progress_step = Some(step.max(1));
        self
    }

    /// Subscribes one consumer to the event stream. Subscribing is the
    /// re-attach: a window opened mid-download receives the events from
    /// now on and reads [`state`] for where things stand (no replay —
    /// the state query is the truth).
    pub fn subscribe(&self) -> broadcast::Receiver<PackEvent> {
        self.events.subscribe()
    }

    /// The download state of one pack (the truth a returning window
    /// re-attaches to).
    pub fn state(&self, pack: PackId) -> PackDownloadState {
        self.inner
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .states
            .get(&pack)
            .cloned()
            .unwrap_or(PackDownloadState::NotDownloading)
    }

    /// The manifest this downloader downloads against — the shell reads
    /// the pack specs from it for the Engines tab's statuses and the
    /// delete-unused cleanup (one manifest, one truth; the shell never
    /// re-reads the bundle for pack work).
    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    /// Starts downloading one pack (Missing or Stale on disk — a Ready
    /// pack is never re-downloaded) and returns immediately; the
    /// download runs on its own task and reports through
    /// [`subscribe`]/[`state`]. Must run inside a tokio runtime.
    ///
    /// [`subscribe`]: PackDownloader::subscribe
    /// [`state`]: PackDownloader::state
    pub fn start(self: &Arc<Self>, pack: PackId) -> StartOutcome {
        // The disk check stays outside the lock (hashing tens of MB);
        // the active-map check under the lock is the single-flight gate.
        if pack_status(self.manifest.pack(pack), &self.active_dir) == PackStatus::Ready {
            return StartOutcome::AlreadyReady;
        }
        let (cancel, cancel_rx) = watch::channel(false);
        {
            let mut inner = self.inner.lock().unwrap_or_else(|p| p.into_inner());
            if inner.active.contains_key(&pack) {
                return StartOutcome::AlreadyDownloading;
            }
            inner.active.insert(pack, cancel);
            inner.states.insert(
                pack,
                PackDownloadState::Downloading {
                    received_bytes: 0,
                    total_bytes: None,
                },
            );
        }
        let downloader = Arc::clone(self);
        tokio::spawn(downloader.run(pack, cancel_rx));
        StartOutcome::Started
    }

    /// Cancels the in-flight download of one pack: the transfer aborts
    /// — the cancel flag is selected against the source fetch and every
    /// chunk read, so a parked or stalled transfer answers instantly —
    /// the staged temp is removed, and the state returns to
    /// not-downloaded. One boundary remains by design: once the last
    /// source's archive has fully arrived and verified, the install
    /// itself is atomic and short, and a cancel landing inside it
    /// completes as Finished rather than discarding verified bytes.
    /// `false` when nothing is in flight.
    pub fn cancel(&self, pack: PackId) -> bool {
        let inner = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        match inner.active.get(&pack) {
            Some(cancel) => {
                let _ = cancel.send(true);
                true
            }
            None => false,
        }
    }

    /// The download task: try the manifest's sources in order, stage the
    /// first complete archive, verify-install it. Every exit path ends
    /// in [`finish`] — the task owns its active entry for exactly its
    /// own lifetime.
    ///
    /// [`finish`]: PackDownloader::finish
    async fn run(self: Arc<Self>, pack: PackId, mut cancel: watch::Receiver<bool>) {
        let spec: &PackSpec = self.manifest.pack(pack);
        let temp = download_temp_path(&self.active_dir, pack);
        let _guard = TempGuard(temp.clone());
        // The temp stages beside the active directory; create the parent
        // when the engine has never run yet (the active directory itself
        // is created by the install).
        if let Some(parent) = temp.parent() {
            let _ = fs::create_dir_all(parent);
        }

        let mut last_error: Option<String> = None;
        let mut fetched = false;
        let mut canceled = *cancel.borrow();

        for url in &spec.urls {
            if canceled {
                break;
            }
            // The cancel flag is selected against the fetch too, so a
            // cancel during a stalled connect lands instantly instead of
            // waiting out the connection.
            let mut response = tokio::select! {
                biased;
                _ = cancel.changed() => {
                    canceled = true;
                    break;
                }
                response = self.transport.fetch(url) => match response {
                    Ok(response) => response,
                    Err(error) => {
                        last_error = Some(error.to_string());
                        continue;
                    }
                },
            };
            if !response.is_success() {
                last_error = Some(format!("the source answered HTTP {}", response.status));
                continue;
            }
            // This source answered: announce the attempt. A later
            // mirror's attempt re-announces with its own total; progress
            // restarts from zero with it.
            let total = response.content_length;
            self.begin_attempt(pack, total);
            let step = self.progress_step.unwrap_or_else(|| adaptive_step(total));
            let mut on_progress = |received: u64| self.progress(pack, received, total);
            match stream_to_temp(
                response.body.as_mut(),
                &temp,
                &mut cancel,
                step,
                &mut on_progress,
            )
            .await
            {
                Err(StreamError::Canceled) => {
                    canceled = true;
                    break;
                }
                Err(StreamError::Io(error)) => {
                    last_error = Some(format!("the transfer failed mid-stream: {error}"));
                    continue;
                }
                Ok(()) => {}
            }
            // The archive fully arrived: verify-install it. A source
            // whose bytes fail their pins is a failed source — the next
            // one may still serve good bytes; the temp is rewritten from
            // zero by the next attempt.
            match install_pack_archive(&temp, pack, spec, &self.active_dir) {
                Ok(()) => {
                    fetched = true;
                    break;
                }
                Err(error) => {
                    last_error = Some(error.to_string());
                    continue;
                }
            }
        }
        // A cancel landing before the install still counts: nothing has
        // landed. Once the install ran, its Finished verdict stands —
        // see [`PackDownloader::cancel`].
        if !fetched && !canceled {
            canceled = *cancel.borrow();
        }

        if canceled {
            self.finish(
                pack,
                PackEvent::Canceled { pack },
                PackDownloadState::NotDownloading,
            );
            return;
        }
        if fetched {
            self.finish(
                pack,
                PackEvent::Finished { pack },
                PackDownloadState::NotDownloading,
            );
            return;
        }
        let message = match last_error {
            Some(error) => format!("the {} pack download failed: {error}", pack.display_name()),
            None => format!(
                "the {} pack has no download source left to try",
                pack.display_name()
            ),
        };
        self.finish(
            pack,
            PackEvent::Failed {
                pack,
                message: message.clone(),
            },
            PackDownloadState::Failed { message },
        );
    }

    /// Announces one source's attempt: the state flips to downloading
    /// with the source's declared total, one `Started` event.
    fn begin_attempt(&self, pack: PackId, total: Option<u64>) {
        let mut inner = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        inner.states.insert(
            pack,
            PackDownloadState::Downloading {
                received_bytes: 0,
                total_bytes: total,
            },
        );
        let _ = self.events.send(PackEvent::Started {
            pack,
            total_bytes: total,
        });
    }

    /// Step-gated progress: state and event move together.
    fn progress(&self, pack: PackId, received: u64, total: Option<u64>) {
        let mut inner = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        inner.states.insert(
            pack,
            PackDownloadState::Downloading {
                received_bytes: received,
                total_bytes: total,
            },
        );
        let _ = self.events.send(PackEvent::Progress {
            pack,
            received_bytes: received,
            total_bytes: total,
        });
    }

    /// Ends the task: the active entry dies with it, the final state and
    /// its event land under one lock (start/cancel racing this task see
    /// either the old world or the new one, never a torn half).
    fn finish(&self, pack: PackId, event: PackEvent, state: PackDownloadState) {
        let mut inner = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        inner.active.remove(&pack);
        inner.states.insert(pack, state);
        let _ = self.events.send(event);
    }
}

/// Bytes per Progress event when the downloader has no override: one
/// percent of a declared total (bounded below by 1 so tiny bodies still
/// report), or one MiB against a source that declares nothing.
fn adaptive_step(total: Option<u64>) -> u64 {
    match total {
        Some(total) => (total / 100).max(1),
        None => 1024 * 1024,
    }
}

/// Where one pack's archive stages: a residue-named sibling of the
/// active models directory ([`residue_dir_sibling`] — the module-wide
/// residue contract: dot-prefixed so the sidecar never sees it,
/// pid-tagged so a concurrent assembly's sweep never deletes an
/// in-flight download), pack-named so two packs never collide.
fn download_temp_path(active_dir: &Path, pack: PackId) -> PathBuf {
    residue_dir_sibling(active_dir, &format!("pack-{}-download", pack.as_str()))
}

/// Removes the staged temp on every exit path — success (the install
/// consumed it), failure, cancel, panic. The guard makes "zero residue"
/// a property of the code's shape, not of its discipline.
struct TempGuard(PathBuf);

impl Drop for TempGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

enum StreamError {
    /// The user canceled; the abort is clean and the caller reports it.
    Canceled,
    /// The transfer or the staging write failed mid-stream — a transport
    /// failure, so the next source may still serve.
    Io(std::io::Error),
}

/// Streams one source's body into the staged temp file, counting bytes,
/// firing the step-gated progress callback, and selecting the cancel
/// flag against every chunk read (a cancel answers instantly even while
/// the source is stalled mid-body — the parked read future is dropped,
/// not awaited out).
async fn stream_to_temp(
    body: &mut dyn PackBody,
    temp: &Path,
    cancel: &mut watch::Receiver<bool>,
    step: u64,
    on_progress: &mut (dyn FnMut(u64) + Send),
) -> Result<(), StreamError> {
    let file = fs::File::create(temp).map_err(StreamError::Io)?;
    let mut writer = BufWriter::new(file);
    // Each attempt writes from zero (no byte-resume): File::create
    // truncates whatever a failed earlier attempt left in the file.
    let mut received: u64 = 0;
    let mut next_emit = step;
    loop {
        let chunk = tokio::select! {
            biased;
            // A dropped flag sender means the manager is gone — the same
            // verdict as an explicit cancel for this task's purpose.
            _ = cancel.changed() => return Err(StreamError::Canceled),
            chunk = body.next_chunk() => chunk.map_err(StreamError::Io)?,
        };
        let Some(chunk) = chunk else {
            break;
        };
        received += chunk.len() as u64;
        writer.write_all(&chunk).map_err(StreamError::Io)?;
        if received >= next_emit {
            on_progress(received);
            next_emit = received.saturating_add(step);
        }
    }
    writer.flush().map_err(StreamError::Io)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    //! The transport contract, scripted end to end: fallback order,
    //! wrong-bytes rejection, clean cancel, the progress sequence, and
    //! the window-independent lifecycle. No network anywhere — the
    //! scripted transport serves canned chunks, and the archives are
    //! built in memory against a manifest parsed from a fixture JSON
    //! (same discipline as the model_packs tests).

    use super::*;
    use crate::model_packs::{pack_status, MANIFEST_VERSION, RESIDUE_PREFIX};
    use sha2::{Digest, Sha256};
    use std::collections::VecDeque;
    use std::io::Cursor;
    use tokio::sync::Notify;
    use tokio::time::{timeout, Duration};

    const PRIMARY: &str = "https://packs.example/releases/v0.3.0/pp-ocrv6-small.zip";
    const MIRROR: &str = "https://mirror.example/pp-ocrv6/small.zip";
    const THIRD: &str = "https://backup.example/small/pp-ocrv6-small.zip";

    const SMALL_DET: &[u8] = b"small-det-v1";
    const SMALL_REC: &[u8] = b"small-rec-v1";
    const SMALL_DICT: &[u8] = b"shared-dict-v1";
    /// The Small pack's files: (name, bytes) — the manifest pins these
    /// exact bytes, and the scripted archives carry them (or a tampered
    /// variant) at these names.
    const SMALL_FILES: [(&str, &[u8]); 3] = [
        ("small_det.onnx", SMALL_DET),
        ("small_rec.onnx", SMALL_REC),
        ("dict.txt", SMALL_DICT),
    ];

    const AWAIT: Duration = Duration::from_secs(5);

    fn hex(digest: &[u8]) -> String {
        let mut out = String::with_capacity(digest.len() * 2);
        for byte in digest {
            out.push_str(&format!("{byte:02x}"));
        }
        out
    }

    /// A manifest JSON file entry pinning the exact bytes a test chose.
    fn pinned(name: &str, bytes: &[u8]) -> String {
        format!(
            "{{\"name\": \"{name}\", \"sha256\": \"{}\", \"size\": {}}}",
            hex(&Sha256::digest(bytes)),
            bytes.len()
        )
    }

    /// The stale bytes a test pre-seeds into the active directory.
    fn stale(bytes: &[u8]) -> Vec<u8> {
        [bytes, b"-stale"].concat()
    }

    /// A full valid manifest: the Small pack pins [`SMALL_FILES`] and
    /// three sources (primary, mirror, backup); Medium, Korean, and the
    /// bundled baseline pin one file each so the manifest validates.
    fn fixture_manifest() -> Arc<Manifest> {
        let json = format!(
            r#"{{"manifest_version": {MANIFEST_VERSION},
            "bundled": {{"files": [{}]}},
            "packs": {{
                "small": {{"version": "1", "urls": ["{PRIMARY}", "{MIRROR}", "{THIRD}"], "files": [{}, {}, {}]}},
                "medium": {{"version": "1", "urls": ["https://packs.example/medium.zip"], "files": [{}]}},
                "korean": {{"version": "1", "urls": ["https://packs.example/korean.zip"], "files": [{}]}}
            }}}}"#,
            pinned("tiny.onnx", b"tiny"),
            pinned("small_det.onnx", SMALL_DET),
            pinned("small_rec.onnx", SMALL_REC),
            pinned("dict.txt", SMALL_DICT),
            pinned("medium.onnx", b"medium"),
            pinned("korean.onnx", b"korean"),
        );
        Arc::new(Manifest::parse(&json).unwrap())
    }

    /// The Small pack's archive as zip bytes; `tamper` flips the last
    /// byte of one entry — same length (the size pin still passes), so
    /// the hash is what rejects.
    fn small_archive(tamper: Option<&str>) -> Vec<u8> {
        let cursor = Cursor::new(Vec::new());
        let mut zip = zip::ZipWriter::new(cursor);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        for (name, bytes) in SMALL_FILES {
            let mut entry = bytes.to_vec();
            if tamper == Some(name) {
                let last = entry.len() - 1;
                entry[last] = entry[last].wrapping_add(1);
            }
            zip.start_file(name, options).unwrap();
            zip.write_all(&entry).unwrap();
        }
        let cursor = zip.finish().unwrap();
        cursor.into_inner()
    }

    /// Splits bytes into chunks of the given sizes (the remainder last),
    /// so the progress sequence and mid-stream failures are scriptable.
    fn chunked(bytes: &[u8], sizes: &[usize]) -> Vec<Vec<u8>> {
        let mut chunks = Vec::new();
        let mut at = 0;
        for size in sizes {
            let end = (at + size).min(bytes.len());
            if at >= bytes.len() {
                break;
            }
            chunks.push(bytes[at..end].to_vec());
            at = end;
        }
        if at < bytes.len() {
            chunks.push(bytes[at..].to_vec());
        }
        chunks
    }

    // ---- the scripted transport ----

    struct Gate {
        entered: Notify,
        release: Notify,
    }

    impl Gate {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                entered: Notify::new(),
                release: Notify::new(),
            })
        }
    }

    enum Step {
        /// The source cannot even be reached.
        Unreachable(&'static str),
        /// An HTTP answer with this status and an empty body.
        Status(u16),
        /// A success answer streaming these chunks.
        Body(Vec<Vec<u8>>),
        /// Chunks first, then a mid-stream connection failure.
        BodyThenError(Vec<Vec<u8>>, &'static str),
        /// Chunks with a gate: before yielding chunk `pause_after` + 1,
        /// notify `entered` once and park until `release` — the hang the
        /// cancel and state tests drive through.
        Gated(Vec<Vec<u8>>, Arc<Gate>, usize),
    }

    struct ScriptedTransport {
        steps: Mutex<VecDeque<Step>>,
        contacted: Mutex<Vec<String>>,
    }

    impl ScriptedTransport {
        fn new(steps: Vec<Step>) -> Arc<Self> {
            Self {
                steps: Mutex::new(steps.into()),
                contacted: Mutex::new(Vec::new()),
            }
            .into()
        }

        fn contacted(&self) -> Vec<String> {
            self.contacted.lock().unwrap().clone()
        }

        fn body_response(chunks: Vec<Vec<u8>>, error: Option<&'static str>) -> PackResponse {
            let content_length = chunks.iter().map(|c| c.len() as u64).sum::<u64>();
            PackResponse {
                status: 200,
                content_length: Some(content_length),
                body: Box::new(ScriptedBody {
                    chunks: chunks.into(),
                    error_after: error.map(str::to_string),
                    gate: None,
                    yielded: 0,
                }),
            }
        }
    }

    #[async_trait]
    impl PackTransport for ScriptedTransport {
        async fn fetch(&self, url: &str) -> Result<PackResponse, PackFetchError> {
            self.contacted.lock().unwrap().push(url.to_string());
            let step = self
                .steps
                .lock()
                .unwrap()
                .pop_front()
                .expect("the transport script ran out before the download finished");
            Ok(match step {
                Step::Unreachable(detail) => {
                    return Err(PackFetchError {
                        url: url.to_string(),
                        detail: detail.to_string(),
                    })
                }
                Step::Status(status) => PackResponse {
                    status,
                    content_length: None,
                    body: Box::new(ScriptedBody {
                        chunks: VecDeque::new(),
                        error_after: None,
                        gate: None,
                        yielded: 0,
                    }),
                },
                Step::Body(chunks) => Self::body_response(chunks, None),
                Step::BodyThenError(chunks, error) => {
                    Self::body_response(chunks, Some(error))
                }
                Step::Gated(chunks, gate, pause_after) => PackResponse {
                    status: 200,
                    content_length: Some(chunks.iter().map(|c| c.len() as u64).sum::<u64>()),
                    body: Box::new(ScriptedBody {
                        chunks: chunks.into(),
                        error_after: None,
                        gate: Some((gate, pause_after)),
                        yielded: 0,
                    }),
                },
            })
        }
    }

    struct ScriptedBody {
        chunks: VecDeque<Vec<u8>>,
        error_after: Option<String>,
        gate: Option<(Arc<Gate>, usize)>,
        yielded: usize,
    }

    #[async_trait]
    impl PackBody for ScriptedBody {
        async fn next_chunk(&mut self) -> std::io::Result<Option<Vec<u8>>> {
            if let Some((gate, pause_after)) = &self.gate {
                if self.yielded == *pause_after {
                    gate.entered.notify_one();
                    gate.release.notified().await;
                }
            }
            match self.chunks.pop_front() {
                Some(chunk) => {
                    self.yielded += 1;
                    Ok(Some(chunk))
                }
                None => match self.error_after.take() {
                    Some(error) => Err(std::io::Error::other(error)),
                    None => Ok(None),
                },
            }
        }
    }

    // ---- fixtures around the downloader ----

    /// A unique temp directory per test, removed on drop.
    struct Scratch {
        path: PathBuf,
    }

    impl Scratch {
        fn new(tag: &str) -> Self {
            let path =
                std::env::temp_dir().join(format!("pack-downloader-{}-{tag}", std::process::id()));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).unwrap();
            Self { path }
        }

        fn dir(&self, name: &str) -> PathBuf {
            let dir = self.path.join(name);
            fs::create_dir_all(&dir).unwrap();
            dir
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    fn downloader(
        transport: Arc<ScriptedTransport>,
        active: &Path,
        step: u64,
    ) -> (Arc<PackDownloader>, Arc<Manifest>) {
        let manifest = fixture_manifest();
        let downloader = Arc::new(
            PackDownloader::new(transport as Arc<dyn PackTransport>, Arc::clone(&manifest), active)
                .with_progress_step(step),
        );
        (downloader, manifest)
    }

    async fn next_event(rx: &mut broadcast::Receiver<PackEvent>) -> PackEvent {
        timeout(AWAIT, rx.recv())
            .await
            .expect("no event arrived within 5s")
            .expect("the event channel closed")
    }

    /// Lets a spawned download task run its course until the state
    /// reports `expected` (the current-thread test runtime only runs
    /// the task while the test awaits).
    async fn until_state(downloader: &PackDownloader, pack: PackId, expected: PackDownloadState) {
        timeout(AWAIT, async {
            loop {
                if downloader.state(pack) == expected {
                    return;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the download never reached the expected state");
    }

    /// Waits for the first event matching `want`, skipping the Started /
    /// Progress events a download emits before its verdict (a Finished
    /// while waiting for something else means the download took a
    /// different path than the test pins — say so instead of hanging).
    async fn until_event(
        rx: &mut broadcast::Receiver<PackEvent>,
        want: impl Fn(&PackEvent) -> bool,
    ) -> PackEvent {
        timeout(AWAIT, async {
            loop {
                let event = next_event(rx).await;
                if want(&event) {
                    return event;
                }
                assert!(
                    !matches!(event, PackEvent::Finished { .. }),
                    "the download finished before the expected event: {event:?}"
                );
            }
        })
        .await
        .expect("the expected event never arrived")
    }

    /// Waits for the pack's Finished event, skipping Started / Progress.
    async fn until_finished(rx: &mut broadcast::Receiver<PackEvent>) {
        until_event(rx, |event| {
            matches!(event, PackEvent::Finished { pack: PackId::Small })
        })
        .await;
    }

    /// No transient artifact may survive the download — no staged temp
    /// beside the active directory, nothing prefixed inside it.
    fn assert_no_residue(active: &Path) {
        let parent = active.parent().unwrap();
        let outside: Vec<String> = fs::read_dir(parent)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with(RESIDUE_PREFIX))
            .collect();
        assert!(outside.is_empty(), "staging residue left behind: {outside:?}");
        let inside: Vec<String> = fs::read_dir(active)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with(RESIDUE_PREFIX))
            .collect();
        assert!(inside.is_empty(), "prefixed residue left behind: {inside:?}");
    }

    /// The pack's disk status reads as fully missing (every file absent).
    fn assert_missing(manifest: &Manifest, active: &Path, pack: PackId) {
        let status = pack_status(manifest.pack(pack), active);
        let files: Vec<String> = manifest.pack(pack).files.iter().map(|f| f.name.clone()).collect();
        assert_eq!(status, PackStatus::Missing { files }, "{status:?}");
    }

    // ---- fallback order ----

    #[tokio::test]
    async fn the_primary_source_serves_and_the_mirrors_are_never_contacted() {
        let scratch = Scratch::new("primary-first");
        let active = scratch.dir("active");
        let transport = ScriptedTransport::new(vec![Step::Body(vec![small_archive(None)])]);
        let (downloader, manifest) = downloader(Arc::clone(&transport), &active, 1);

        let mut rx = downloader.subscribe();
        assert_eq!(downloader.start(PackId::Small), StartOutcome::Started);
        until_finished(&mut rx).await;
        assert_eq!(transport.contacted(), [PRIMARY], "the primary serves alone");
        assert_eq!(
            pack_status(manifest.pack(PackId::Small), &active),
            PackStatus::Ready
        );
        assert_eq!(
            downloader.state(PackId::Small),
            PackDownloadState::NotDownloading
        );
        assert_no_residue(&active);
    }

    #[tokio::test]
    async fn an_unreachable_primary_falls_through_to_the_next_source_in_order() {
        let scratch = Scratch::new("fallback-unreachable");
        let active = scratch.dir("active");
        let transport = ScriptedTransport::new(vec![
            Step::Unreachable("connection refused"),
            Step::Body(vec![small_archive(None)]),
        ]);
        let (downloader, manifest) = downloader(Arc::clone(&transport), &active, 1);

        let mut rx = downloader.subscribe();
        downloader.start(PackId::Small);
        until_finished(&mut rx).await;
        assert_eq!(
            transport.contacted(),
            [PRIMARY, MIRROR],
            "primary, then mirror"
        );
        assert_eq!(
            pack_status(manifest.pack(PackId::Small), &active),
            PackStatus::Ready
        );
        assert_no_residue(&active);
    }

    #[tokio::test]
    async fn an_http_error_status_falls_through_like_unreachability() {
        let scratch = Scratch::new("fallback-status");
        let active = scratch.dir("active");
        let transport = ScriptedTransport::new(vec![
            Step::Status(404),
            Step::Status(500),
            Step::Body(vec![small_archive(None)]),
        ]);
        let (downloader, _) = downloader(Arc::clone(&transport), &active, 1);

        let mut rx = downloader.subscribe();
        downloader.start(PackId::Small);
        until_finished(&mut rx).await;
        assert_eq!(
            transport.contacted(),
            [PRIMARY, MIRROR, THIRD],
            "every error status routes to the next source"
        );
    }

    #[tokio::test]
    async fn a_mid_stream_connection_failure_falls_through_to_the_next_source() {
        let scratch = Scratch::new("fallback-midstream");
        let active = scratch.dir("active");
        let transport = ScriptedTransport::new(vec![
            Step::BodyThenError(chunked(SMALL_DET, &[8]), "connection reset by peer"),
            Step::Body(vec![small_archive(None)]),
        ]);
        let (downloader, manifest) = downloader(Arc::clone(&transport), &active, 1);

        let mut rx = downloader.subscribe();
        downloader.start(PackId::Small);
        until_finished(&mut rx).await;
        assert_eq!(transport.contacted(), [PRIMARY, MIRROR]);
        assert_eq!(
            pack_status(manifest.pack(PackId::Small), &active),
            PackStatus::Ready,
            "the mirror's fresh download installs"
        );
        assert_no_residue(&active);
    }

    // ---- failure of the whole pack ----

    #[tokio::test]
    async fn every_source_failing_fails_the_pack_and_a_retry_then_succeeds() {
        let scratch = Scratch::new("all-fail-retry");
        let active = scratch.dir("active");
        let transport = ScriptedTransport::new(vec![
            Step::Unreachable("connection refused"),
            Step::Status(404),
            Step::Unreachable("connection refused"),
            // The Retry's script:
            Step::Body(vec![small_archive(None)]),
        ]);
        let (downloader, manifest) = downloader(Arc::clone(&transport), &active, 1);

        let mut rx = downloader.subscribe();
        downloader.start(PackId::Small);
        let PackEvent::Failed { message, .. } =
            until_event(&mut rx, |event| matches!(event, PackEvent::Failed { .. })).await
        else {
            unreachable!("until_event returned a non-Failed event")
        };
        assert!(
            message.contains(THIRD) && message.contains("connection refused"),
            "the failed-state message must name the last source's reason: {message}"
        );
        assert_eq!(
            downloader.state(PackId::Small),
            PackDownloadState::Failed { message },
            "the failed state persists for the Retry"
        );
        assert_missing(&manifest, &active, PackId::Small);
        assert_no_residue(&active);

        // Story 5: the failed state carries a Retry, and the retry works.
        assert_eq!(downloader.start(PackId::Small), StartOutcome::Started);
        until_finished(&mut rx).await;
        assert_eq!(
            pack_status(manifest.pack(PackId::Small), &active),
            PackStatus::Ready
        );
        assert_no_residue(&active);
    }

    // ---- hash rejection ----

    #[tokio::test]
    async fn a_source_delivering_wrong_bytes_fails_and_the_next_mirror_rescues() {
        let scratch = Scratch::new("wrong-bytes-rescue");
        let active = scratch.dir("active");
        // Pre-seed stale copies: the failing attempt must leave them
        // untouched until the mirror's good bytes replace them.
        for (name, bytes) in SMALL_FILES {
            fs::write(active.join(name), stale(bytes)).unwrap();
        }

        // The primary's archive fully arrives but one entry fails its
        // hash pin; the mirror serves the good archive.
        let transport = ScriptedTransport::new(vec![
            Step::Body(vec![small_archive(Some("small_rec.onnx"))]),
            Step::Body(vec![small_archive(None)]),
        ]);
        let (downloader, manifest) = downloader(Arc::clone(&transport), &active, 1);

        let mut rx = downloader.subscribe();
        downloader.start(PackId::Small);
        until_finished(&mut rx).await;
        assert_eq!(
            transport.contacted(),
            [PRIMARY, MIRROR],
            "a source failing its pins is a failed source — the next one is tried"
        );
        assert_eq!(
            pack_status(manifest.pack(PackId::Small), &active),
            PackStatus::Ready,
            "the mirror's good bytes install"
        );
        assert_no_residue(&active);
    }

    #[tokio::test]
    async fn every_source_failing_its_pins_rejects_the_whole_pack() {
        let scratch = Scratch::new("wrong-bytes-all");
        let active = scratch.dir("active");
        let manifest = fixture_manifest();
        for (name, bytes) in SMALL_FILES {
            fs::write(active.join(name), stale(bytes)).unwrap();
        }

        // Every source delivers same-size bytes that fail their pins —
        // the whole pack is rejected on the last hash verdict.
        let transport = ScriptedTransport::new(vec![
            Step::Body(vec![small_archive(Some("small_rec.onnx"))]),
            Step::Body(vec![small_archive(Some("dict.txt"))]),
            Step::Body(vec![small_archive(Some("small_det.onnx"))]),
        ]);
        let (downloader, _) = downloader(Arc::clone(&transport), &active, 1);

        let mut rx = downloader.subscribe();
        downloader.start(PackId::Small);
        let PackEvent::Failed { message, .. } =
            until_event(&mut rx, |event| matches!(event, PackEvent::Failed { .. })).await
        else {
            unreachable!("until_event returned a non-Failed event")
        };
        assert!(
            message.contains("fails its SHA-256 pin"),
            "the failed-state message carries the last source's hash verdict: {message}"
        );
        assert_eq!(transport.contacted(), [PRIMARY, MIRROR, THIRD]);
        // Nothing landed and nothing was deleted: the pre-seeded stale
        // copies stay (status Stale — present but failing their pins),
        // which the next successful install replaces.
        assert_eq!(
            pack_status(manifest.pack(PackId::Small), &active),
            PackStatus::Stale {
                files: SMALL_FILES
                    .iter()
                    .map(|(name, _)| name.to_string())
                    .collect()
            },
            "a rejected pack must not touch the active directory"
        );
        assert_no_residue(&active);
    }

    #[tokio::test]
    async fn a_corrupt_archive_is_rejected_whole() {
        let scratch = Scratch::new("corrupt-archive");
        let active = scratch.dir("active");
        let manifest = fixture_manifest();
        // Not-a-zip from every source: the last verdict names the
        // archive shape.
        let garbage = || Step::Body(vec![b"not a zip at all".to_vec()]);
        let transport = ScriptedTransport::new(vec![garbage(), garbage(), garbage()]);
        let (downloader, _) = downloader(Arc::clone(&transport), &active, 1);

        let mut rx = downloader.subscribe();
        downloader.start(PackId::Small);
        let PackEvent::Failed { message, .. } =
            until_event(&mut rx, |event| matches!(event, PackEvent::Failed { .. })).await
        else {
            unreachable!("until_event returned a non-Failed event")
        };
        assert!(message.contains("not a readable ZIP archive"), "was: {message}");
        assert_missing(&manifest, &active, PackId::Small);
        assert_no_residue(&active);
    }

    // ---- cancel ----

    #[tokio::test]
    async fn cancel_mid_stream_aborts_cleanly_with_zero_residue() {
        let scratch = Scratch::new("cancel-midstream");
        let active = scratch.dir("active");
        let archive = small_archive(None);
        let gate = Gate::new();
        let transport = ScriptedTransport::new(vec![Step::Gated(
            chunked(&archive, &[8, 8, archive.len() - 16]),
            Arc::clone(&gate),
            1, // park before the second chunk
        )]);
        let (cancel_downloader, cancel_manifest) =
            downloader(Arc::clone(&transport), &active, 1);

        let mut rx = cancel_downloader.subscribe();
        cancel_downloader.start(PackId::Small);
        timeout(AWAIT, gate.entered.notified())
            .await
            .expect("the download never delivered its first chunk");

        assert!(
            cancel_downloader.cancel(PackId::Small),
            "cancel reports the in-flight download"
        );
        gate.release.notify_one();
        until_event(&mut rx, |event| {
            matches!(event, PackEvent::Canceled { pack: PackId::Small })
        })
        .await;
        // Status back to not-downloaded, everywhere: state, disk, residue.
        assert_eq!(
            cancel_downloader.state(PackId::Small),
            PackDownloadState::NotDownloading
        );
        assert_missing(&cancel_manifest, &active, PackId::Small);
        assert_no_residue(&active);

        // Story 6: a later re-selection starts cleanly from scratch.
        let transport = ScriptedTransport::new(vec![Step::Body(vec![small_archive(None)])]);
        let (retry_downloader, retry_manifest) = downloader(transport, &active, 1);
        let mut rx = retry_downloader.subscribe();
        retry_downloader.start(PackId::Small);
        until_finished(&mut rx).await;
        assert_eq!(
            pack_status(retry_manifest.pack(PackId::Small), &active),
            PackStatus::Ready
        );
        assert_no_residue(&active);
    }

    #[tokio::test]
    async fn cancel_of_an_idle_pack_is_false_and_says_nothing() {
        let scratch = Scratch::new("cancel-idle");
        let active = scratch.dir("active");
        let (downloader, _) = downloader(ScriptedTransport::new(vec![]), &active, 1);

        let mut rx = downloader.subscribe();
        assert!(!downloader.cancel(PackId::Small));
        // No event followed the no-op cancel.
        timeout(Duration::from_millis(100), rx.recv())
            .await
            .expect_err("an idle cancel must not emit");
    }

    // ---- progress and events ----

    #[tokio::test]
    async fn the_progress_sequence_runs_started_increasing_progress_finished() {
        let scratch = Scratch::new("progress-sequence");
        let active = scratch.dir("active");
        let archive = small_archive(None);
        let transport = ScriptedTransport::new(vec![Step::Body(chunked(&archive, &[5, 7, 3]))]);
        let (downloader, _) = downloader(Arc::clone(&transport), &active, 1);

        let mut rx = downloader.subscribe();
        downloader.start(PackId::Small);

        let mut events = Vec::new();
        loop {
            let event = next_event(&mut rx).await;
            let done = event == PackEvent::Finished { pack: PackId::Small };
            events.push(event);
            if done {
                break;
            }
        }
        // Started announces the source's declared total; Progress is
        // strictly increasing; Finished closes.
        assert_eq!(
            events[0],
            PackEvent::Started {
                pack: PackId::Small,
                total_bytes: Some(archive.len() as u64)
            }
        );
        let mut last = 0u64;
        for event in &events[1..events.len() - 1] {
            let PackEvent::Progress {
                received_bytes,
                total_bytes,
                ..
            } = event
            else {
                panic!("expected only progress between start and finish, got {event:?}")
            };
            assert_eq!(*total_bytes, Some(archive.len() as u64));
            assert!(*received_bytes > last, "progress must increase: {events:?}");
            last = *received_bytes;
        }
        assert_eq!(
            last, archive.len() as u64,
            "the last progress carries the full size"
        );
        assert_eq!(
            events.last(),
            Some(&PackEvent::Finished { pack: PackId::Small })
        );
    }

    #[tokio::test]
    async fn progress_is_step_gated_not_per_chunk() {
        let scratch = Scratch::new("progress-gate");
        let active = scratch.dir("active");
        let archive = small_archive(None);
        let transport = ScriptedTransport::new(vec![Step::Body(chunked(
            &archive,
            &[4, 4, 4, 4, 4, archive.len() - 20],
        ))]);
        let (downloader, _) = downloader(Arc::clone(&transport), &active, 1_000);

        let mut rx = downloader.subscribe();
        downloader.start(PackId::Small);
        assert_eq!(
            next_event(&mut rx).await,
            PackEvent::Started {
                pack: PackId::Small,
                total_bytes: Some(archive.len() as u64)
            }
        );
        assert_eq!(
            next_event(&mut rx).await,
            PackEvent::Finished { pack: PackId::Small },
            "a step larger than the archive emits no progress at all"
        );
    }

    #[test]
    fn adaptive_step_is_one_percent_of_a_declared_total() {
        assert_eq!(adaptive_step(Some(1_000)), 10);
        assert_eq!(adaptive_step(Some(50)), 1, "tiny totals still report");
        assert_eq!(adaptive_step(None), 1024 * 1024);
    }

    #[tokio::test]
    async fn the_state_reports_live_progress_while_streaming() {
        let scratch = Scratch::new("state-live");
        let active = scratch.dir("active");
        let archive = small_archive(None);
        let gate = Gate::new();
        let transport = ScriptedTransport::new(vec![Step::Gated(
            chunked(&archive, &[8, 8, archive.len() - 16]),
            Arc::clone(&gate),
            1,
        )]);
        let (downloader, _) = downloader(Arc::clone(&transport), &active, 1);

        downloader.start(PackId::Small);
        timeout(AWAIT, gate.entered.notified())
            .await
            .expect("the download never delivered its first chunk");
        assert_eq!(
            downloader.state(PackId::Small),
            PackDownloadState::Downloading {
                received_bytes: 8,
                total_bytes: Some(archive.len() as u64)
            },
            "a reopened window reads where the download stands"
        );
        downloader.cancel(PackId::Small);
        gate.release.notify_one();
        until_state(&downloader, PackId::Small, PackDownloadState::NotDownloading).await;
    }

    // ---- the broadcast rule and the window-independent lifecycle ----

    #[tokio::test]
    async fn each_window_receives_every_event_exactly_once() {
        let scratch = Scratch::new("broadcast-once");
        let active = scratch.dir("active");
        let transport = ScriptedTransport::new(vec![Step::Body(chunked(
            &small_archive(None),
            &[5, 7, 3],
        ))]);
        let (downloader, _) = downloader(Arc::clone(&transport), &active, 1);

        let mut rx_a = downloader.subscribe();
        let mut rx_b = downloader.subscribe();
        downloader.start(PackId::Small);

        async fn drain(rx: &mut broadcast::Receiver<PackEvent>) -> Vec<PackEvent> {
            let mut events = Vec::new();
            loop {
                let event = next_event(rx).await;
                let done = event == PackEvent::Finished { pack: PackId::Small };
                events.push(event);
                if done {
                    break;
                }
            }
            events
        }
        let seen_a = drain(&mut rx_a).await;
        let seen_b = drain(&mut rx_b).await;
        assert!(seen_a.contains(&PackEvent::Finished { pack: PackId::Small }));
        assert_eq!(seen_a, seen_b, "two windows hear the same events once each");
        let started_count = seen_a
            .iter()
            .filter(|e| matches!(e, PackEvent::Started { .. }))
            .count();
        assert_eq!(started_count, 1, "one attempt, one Started event");
    }

    #[tokio::test]
    async fn the_download_outlives_the_settings_window() {
        let scratch = Scratch::new("outlives-window");
        let active = scratch.dir("active");
        let manifest = fixture_manifest();
        let archive = small_archive(None);
        let gate = Gate::new();
        let transport = ScriptedTransport::new(vec![Step::Gated(
            chunked(&archive, &[8, 8, archive.len() - 16]),
            Arc::clone(&gate),
            1, // parks before the second chunk
        )]);
        let (downloader, _) = downloader(Arc::clone(&transport), &active, 1);

        // The Settings window subscribes, starts the download, and closes
        // mid-download (dropping its receiver) while the body is parked.
        let rx = downloader.subscribe();
        downloader.start(PackId::Small);
        timeout(AWAIT, gate.entered.notified())
            .await
            .expect("the download never delivered its first chunk");
        drop(rx);

        // The download keeps going with ZERO subscribers and runs to the
        // end: the pack lands, ready, on disk — nothing needed a window.
        gate.release.notify_one();
        until_state(&downloader, PackId::Small, PackDownloadState::NotDownloading).await;
        assert_eq!(
            pack_status(manifest.pack(PackId::Small), &active),
            PackStatus::Ready,
            "the download finished after its window closed"
        );
        assert_no_residue(&active);

        // Reopening re-attaches to the truth: the fresh subscriber gets
        // no replay (the events already fired) and the state query says
        // where things stand.
        let mut rx = downloader.subscribe();
        assert_eq!(
            downloader.state(PackId::Small),
            PackDownloadState::NotDownloading
        );
        timeout(Duration::from_millis(100), rx.recv())
            .await
            .expect_err("a re-attached window must not receive a replay");
        assert_eq!(
            pack_status(manifest.pack(PackId::Small), &active),
            PackStatus::Ready
        );
        assert_no_residue(&active);
    }

    // ---- single-flight and disk-truth guards ----

    #[tokio::test]
    async fn a_second_start_while_downloading_is_declined_without_a_second_fetch() {
        let scratch = Scratch::new("single-flight");
        let active = scratch.dir("active");
        let archive = small_archive(None);
        let gate = Gate::new();
        let transport = ScriptedTransport::new(vec![Step::Gated(
            chunked(&archive, &[8, 8, archive.len() - 16]),
            Arc::clone(&gate),
            1,
        )]);
        let (downloader, _) = downloader(Arc::clone(&transport), &active, 1);

        downloader.start(PackId::Small);
        timeout(AWAIT, gate.entered.notified())
            .await
            .expect("the download never delivered its first chunk");

        assert_eq!(
            downloader.start(PackId::Small),
            StartOutcome::AlreadyDownloading,
            "one download per pack at a time"
        );
        assert_eq!(
            transport.contacted(),
            [PRIMARY],
            "the declined start must not touch the transport"
        );
        downloader.cancel(PackId::Small);
        gate.release.notify_one();
        until_state(&downloader, PackId::Small, PackDownloadState::NotDownloading).await;
    }

    #[tokio::test]
    async fn a_ready_pack_is_never_downloaded_again() {
        let scratch = Scratch::new("already-ready");
        let active = scratch.dir("active");
        let manifest = fixture_manifest();
        for (name, bytes) in SMALL_FILES {
            fs::write(active.join(name), bytes).unwrap();
        }
        let transport = ScriptedTransport::new(vec![Step::Body(vec![small_archive(None)])]);
        let (downloader, _) = downloader(Arc::clone(&transport), &active, 1);

        assert_eq!(downloader.start(PackId::Small), StartOutcome::AlreadyReady);
        assert!(
            transport.contacted().is_empty(),
            "a ready pack must not fetch anything"
        );
        assert_eq!(
            pack_status(manifest.pack(PackId::Small), &active),
            PackStatus::Ready
        );
    }

    #[tokio::test]
    async fn a_stale_pack_re_downloads_and_replaces_the_stale_files() {
        let scratch = Scratch::new("stale-refresh");
        let active = scratch.dir("active");
        let manifest = fixture_manifest();
        for (name, bytes) in SMALL_FILES {
            fs::write(active.join(name), stale(bytes)).unwrap();
        }
        let transport = ScriptedTransport::new(vec![Step::Body(vec![small_archive(None)])]);
        let (downloader, _) = downloader(Arc::clone(&transport), &active, 1);

        let mut rx = downloader.subscribe();
        assert_eq!(downloader.start(PackId::Small), StartOutcome::Started);
        until_finished(&mut rx).await;
        assert_eq!(
            pack_status(manifest.pack(PackId::Small), &active),
            PackStatus::Ready,
            "the update interplay: stale files re-download"
        );
    }

    #[tokio::test]
    async fn the_installed_directory_stays_flat_with_no_residue() {
        let scratch = Scratch::new("flat-install");
        let active = scratch.dir("active");
        // The bundled baseline arrived first (engine ran once): the pack
        // lands beside it, nothing nested, nothing transient.
        fs::write(active.join("tiny.onnx"), b"tiny").unwrap();
        let transport = ScriptedTransport::new(vec![Step::Body(vec![small_archive(None)])]);
        let (downloader, _) = downloader(Arc::clone(&transport), &active, 1);

        let mut rx = downloader.subscribe();
        downloader.start(PackId::Small);
        until_finished(&mut rx).await;

        let mut names: Vec<String> = fs::read_dir(&active)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(
            names,
            ["dict.txt", "small_det.onnx", "small_rec.onnx", "tiny.onnx"],
            "the sidecar's flat layout, exactly the model files"
        );
        assert_no_residue(&active);
    }

    // ---- the wire shape ----

    #[test]
    fn the_wire_shape_of_events_and_state_is_what_the_engines_tab_reads() {
        // PackId spells itself as the manifest's key, never "Small".
        for (id, spelling) in [
            (PackId::Small, "small"),
            (PackId::Medium, "medium"),
            (PackId::Korean, "korean"),
        ] {
            let json = serde_json::to_string(&PackEvent::Finished { pack: id }).unwrap();
            let value: serde_json::Value = serde_json::from_str(&json).unwrap();
            assert_eq!(value["event"], "finished");
            assert_eq!(value["pack"], spelling);
        }
        let json = serde_json::to_string(&PackEvent::Started {
            pack: PackId::Small,
            total_bytes: Some(140_000_000),
        })
        .unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["event"], "started");
        assert_eq!(value["totalBytes"], 140_000_000);

        let json = serde_json::to_string(&PackEvent::Progress {
            pack: PackId::Small,
            received_bytes: 70_000_000,
            total_bytes: Some(140_000_000),
        })
        .unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["event"], "progress");
        assert_eq!(value["receivedBytes"], 70_000_000);

        let json = serde_json::to_string(&PackEvent::Failed {
            pack: PackId::Small,
            message: "no".into(),
        })
        .unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["event"], "failed");
        assert_eq!(value["message"], "no");

        let json = serde_json::to_string(&PackEvent::Canceled { pack: PackId::Small }).unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["event"], "canceled");

        for (state, kind) in [
            (PackDownloadState::NotDownloading, "not-downloading"),
            (
                PackDownloadState::Downloading {
                    received_bytes: 1,
                    total_bytes: None,
                },
                "downloading",
            ),
            (
                PackDownloadState::Failed {
                    message: "no".into(),
                },
                "failed",
            ),
        ] {
            let value: serde_json::Value =
                serde_json::from_str(&serde_json::to_string(&state).unwrap()).unwrap();
            assert_eq!(value["state"], kind, "{kind}: {value}");
        }
        let value: serde_json::Value = serde_json::from_str(
            &serde_json::to_string(&PackDownloadState::Downloading {
                received_bytes: 1,
                total_bytes: Some(2),
            })
            .unwrap(),
        )
        .unwrap();
        assert_eq!(value["receivedBytes"], 1);
        assert_eq!(value["totalBytes"], 2);
    }

    #[test]
    fn the_staged_temp_stays_inside_the_residue_contract() {
        let active = Path::new("/somewhere/models");
        let temp = download_temp_path(active, PackId::Small);
        assert!(temp.starts_with(active.parent().unwrap()));
        assert!(
            temp.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with(RESIDUE_PREFIX),
            "the staged temp must carry the residue prefix"
        );
        assert!(
            temp.file_name()
                .unwrap()
                .to_string_lossy()
                .contains(&std::process::id().to_string()),
            "the staged temp must be pid-tagged (the assembly's sweep skips it)"
        );
    }
}
