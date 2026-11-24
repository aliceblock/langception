//! The shared OCR sidecar machinery: everything both bundled engines have
//! in common because they speak the same JSON-lines pipe protocol shape
//! (captured frame in as base64 PNG, text blocks with positions out) and
//! run as a long-lived child process (pp-ocrv6-sidecar feature ticket 04
//! extracted it from `paddle.rs`, which still owns the old engine's
//! launch arguments). The state machine: lazily spawned on the first OCR
//! need, reused across passes, restarted when the model selection changes
//! (the engine type supplies the key its launch arguments are built
//! from), killed when dropped — it never outlives the app. Everything
//! protocol-shaped here is testable offline: the reply parser (against
//! recorded fixtures), the request encoding, and the process state
//! machine (against the scripted `paddle_stub` binary — see
//! `tests/paddle_sidecar_tests.rs` and `tests/ppocr_sidecar_tests.rs`).
//! The real sidecars are exercised by the manual smoke checklists, like
//! every other live engine.

use crate::fused::bgra_to_png_bytes;
use crate::ports::{CapturedImage, OcrError, TextBlock};
use base64::Engine as _;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::Mutex as AsyncMutex;

/// Model loading is cold disk + CPU work (hundreds of MB of tensors); a
/// slow machine needs headroom without ever hanging the loop.
pub const INIT_TIMEOUT: Duration = Duration::from_secs(30);
/// One pass' det+rec inference on a screen region stays in the seconds
/// range on CPU; anything longer means the sidecar is wedged.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);

// ---- Pure protocol logic (unit-tested below, no process involved) ----

/// Encodes one OCR task: the captured frame as base64 PNG, the sidecar's
/// `image_base64` shape (no `data:` prefix — the engine auto-detects).
pub fn encode_task(image: &CapturedImage) -> Result<String, OcrError> {
    let png = bgra_to_png_bytes(image).map_err(|e| OcrError::new(e.message))?;
    Ok(serde_json::json!({
        "image_base64": base64::engine::general_purpose::STANDARD.encode(png),
    })
    .to_string())
}

#[derive(Debug, serde::Deserialize)]
struct SidecarReply {
    code: i32,
    #[serde(default)]
    data: serde_json::Value,
}

#[derive(Debug, serde::Deserialize)]
struct ReplyBlock {
    #[serde(rename = "box")]
    box_points: Vec<[i32; 2]>,
    #[serde(default)]
    text: String,
}

/// Parses one sidecar reply line into Text Blocks. `code` 100 carries the
/// recognized runs of text as four-point quads (top-left → top-right →
/// bottom-right → bottom-left, capture-space pixels); they become
/// axis-aligned bounding boxes, the same rect shape Windows OCR produces.
/// `code` 101 with "No text found" is the engine's NORMAL empty result
/// (blank region), not a failure. Any other code is an engine error whose
/// `data` is the human-readable message.
pub fn parse_reply(line: &str) -> Result<Vec<TextBlock>, OcrError> {
    let line = line.trim();
    if line.is_empty() {
        return Err(OcrError::new("sidecar sent an empty reply"));
    }
    let reply: SidecarReply = serde_json::from_str(line)
        .map_err(|e| OcrError::new(format!("unrecognized sidecar reply ({e}): {line}")))?;
    match reply.code {
        100 => {
            let items = reply
                .data
                .as_array()
                .ok_or_else(|| OcrError::new("sidecar reply code 100 without a data array"))?;
            let mut blocks = Vec::with_capacity(items.len());
            for item in items {
                let block: ReplyBlock = serde_json::from_value(item.clone())
                    .map_err(|e| OcrError::new(format!("sidecar data block unparsable: {e}")))?;
                if block.text.trim().is_empty() {
                    continue;
                }
                // The quad may be tilted (skewed text); the Text Block is the
                // axis-aligned hull, same as the Windows adapter's union.
                let (mut x0, mut y0, mut x1, mut y1) = (i32::MAX, i32::MAX, i32::MIN, i32::MIN);
                for [x, y] in &block.box_points {
                    x0 = x0.min(*x);
                    y0 = y0.min(*y);
                    x1 = x1.max(*x);
                    y1 = y1.max(*y);
                }
                if block.box_points.is_empty() || x1 <= x0 || y1 <= y0 {
                    continue;
                }
                blocks.push(TextBlock {
                    x: x0,
                    y: y0,
                    width: (x1 - x0) as u32,
                    height: (y1 - y0) as u32,
                    text: block.text,
                });
            }
            Ok(blocks)
        }
        101 if reply
            .data
            .as_str()
            .is_some_and(|message| message.starts_with("No text found")) =>
        {
            Ok(Vec::new())
        }
        _ => {
            let message = match reply.data.as_str() {
                Some(message) => message.to_string(),
                None => format!("sidecar error code {}", reply.code),
            };
            Err(OcrError::new(message))
        }
    }
}

// ---- The sidecar process (integration-tested against paddle_stub) ----

/// How many stderr lines are kept for error messages: the sidecars
/// colorize and log a lot; the tail is enough to diagnose a crash.
const STDERR_TAIL_LINES: usize = 8;

type StderrTail = Mutex<Vec<String>>;

/// A callback fired when the sidecar is found dead between passes and is
/// respawned. The shell wires it to the app log: an unexpected exit is
/// worth one visible line (a crash hints at a broken bundle, AV
/// interference, or OOM) without turning a healed hiccup into a warning
/// banner. Invoked on the caller's async context, briefly.
pub type RestartNotice = Arc<dyn Fn(String) + Send + Sync>;

/// The base command every sidecar launches through: piped stdio, killed
/// whenever its handle drops (the process never outlives the engine), and
/// — on Windows — no console flash for a GUI app's child.
pub fn base_command(program: &Path) -> Command {
    let mut command = Command::new(program);
    command
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    command
}

struct SidecarProcess {
    /// Owned so [`Command::kill_on_drop`] applies whenever the sidecar is
    /// replaced (model switch) or dropped (request failure, engine drop,
    /// app exit).
    child: Child,
    stdin: ChildStdin,
    lines: Lines<BufReader<ChildStdout>>,
    stderr_tail: Arc<StderrTail>,
}

impl SidecarProcess {
    async fn spawn(mut command: Command, init_timeout: Duration) -> Result<Self, String> {
        let mut child = command
            .spawn()
            .map_err(|e| format!("failed to launch the sidecar: {e}"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "sidecar stdout was not piped".to_string())?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| "sidecar stderr was not piped".to_string())?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| "sidecar stdin was not piped".to_string())?;

        // Drain stderr in the background: the engines log verbosely, and an
        // unread full pipe would stall the engine mid-init.
        let stderr_tail: Arc<StderrTail> = Arc::new(Mutex::new(Vec::new()));
        tokio::spawn(drain_stderr(stderr, Arc::clone(&stderr_tail)));

        let mut lines = BufReader::new(stdout).lines();
        // The engine prints a version banner, the pipe-mode line, then the
        // readiness marker once its models are loaded; scan until it, so
        // unrelated banner lines cannot desynchronize the handshake.
        let deadline = Instant::now() + init_timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            match tokio::time::timeout(remaining, lines.next_line()).await {
                Err(_elapsed) => {
                    return Err(format!(
                        "the sidecar did not finish loading its models within {init_timeout:?}"
                    ));
                }
                Ok(Err(e)) => {
                    return Err(format!(
                        "the sidecar crashed during init: {e} (stderr: {})",
                        tail_of(&stderr_tail)
                    ));
                }
                // EOF: the process exited before announcing readiness.
                Ok(Ok(None)) => {
                    return Err(format!(
                        "the sidecar exited during init (stderr: {})",
                        tail_of(&stderr_tail)
                    ));
                }
                Ok(Ok(Some(line))) => {
                    if line.contains("OCR init completed") {
                        break;
                    }
                }
            }
        }
        Ok(Self {
            child,
            stdin,
            lines,
            stderr_tail,
        })
    }

    /// Sends one task line and parses the reply line. Any transport
    /// failure (write, EOF, timeout) means the process is suspect: the
    /// caller must not reuse this sidecar — dropping it kills it, and the
    /// next call spawns fresh.
    async fn request(
        &mut self,
        payload: &str,
        request_timeout: Duration,
    ) -> Result<Vec<TextBlock>, OcrError> {
        let write = async {
            self.stdin.write_all(payload.as_bytes()).await?;
            self.stdin.write_all(b"\n").await?;
            self.stdin.flush().await?;
            Ok::<(), std::io::Error>(())
        };
        if let Err(e) = write.await {
            return Err(OcrError::new(format!(
                "failed to send the task to the sidecar: {e} (stderr: {})",
                tail_of(&self.stderr_tail)
            )));
        }
        let reply = match tokio::time::timeout(request_timeout, self.lines.next_line()).await {
            Err(_elapsed) => {
                return Err(OcrError::new(format!(
                    "the sidecar did not reply within {request_timeout:?}"
                )));
            }
            Ok(Err(e)) => {
                return Err(OcrError::new(format!(
                    "the sidecar died mid-pass: {e} (stderr: {})",
                    tail_of(&self.stderr_tail)
                )));
            }
            // EOF: the process exited between init and this task.
            Ok(Ok(None)) => {
                return Err(OcrError::new(format!(
                    "the sidecar exited before replying (stderr: {})",
                    tail_of(&self.stderr_tail)
                )));
            }
            Ok(Ok(Some(line))) => line,
        };
        parse_reply(&reply)
    }
}

/// Builds the launch command for one model selection: the key identifies
/// the model set (the old engine's per-language config file; the new
/// sidecar's (Source Language, tier) pair) and the closure turns it into
/// the exact launch arguments each sidecar's contract fixes.
type LaunchFn<K> = Box<dyn Fn(&K) -> Result<Command, String> + Send + Sync>;

struct Session<K> {
    key: K,
    process: SidecarProcess,
}

/// The long-lived sidecar state machine shared by both bundled OCR
/// engines: lazily spawn, reuse across passes while `key` is unchanged
/// (a strict request→response dialogue with no concurrency — concurrent
/// callers serialize on the state lock instead of spawning rival
/// sidecars), respawn on death with a [`RestartNotice`], and restart
/// whenever the model selection (`key`) changes.
pub struct SidecarEngine<K> {
    /// A name for log/error lines, e.g. "PaddleOCR" or "PP-OCRv6".
    label: &'static str,
    launch: LaunchFn<K>,
    init_timeout: Duration,
    request_timeout: Duration,
    on_unexpected_exit: Option<RestartNotice>,
    state: AsyncMutex<Option<Session<K>>>,
}

impl<K: PartialEq + Send + 'static> SidecarEngine<K> {
    /// Production constructor (the shared timeout budgets).
    pub fn new(label: &'static str, launch: LaunchFn<K>) -> Self {
        Self::with_timeouts(label, launch, INIT_TIMEOUT, REQUEST_TIMEOUT)
    }

    /// The timeout knobs are constructor settings, not constants, so the
    /// stub tests can run fast while production keeps generous ones.
    pub fn with_timeouts(
        label: &'static str,
        launch: LaunchFn<K>,
        init_timeout: Duration,
        request_timeout: Duration,
    ) -> Self {
        Self {
            label,
            launch,
            init_timeout,
            request_timeout,
            on_unexpected_exit: None,
            state: AsyncMutex::new(None),
        }
    }

    /// Observes unexpected sidecar deaths (see [`RestartNotice`]).
    pub fn with_restart_notice(mut self, notice: RestartNotice) -> Self {
        self.on_unexpected_exit = Some(notice);
        self
    }

    /// One OCR pass through the live-or-spawned sidecar serving `key`.
    pub async fn recognize(
        &self,
        image: &CapturedImage,
        key: K,
    ) -> Result<Vec<TextBlock>, OcrError> {
        let payload = encode_task(image)?;
        // One lock for the whole pass (see the struct doc). A healthy
        // sidecar goes back into the slot for the next pass; a failed
        // request does not — dropping it kills the process, so the next
        // call spawns fresh instead of talking into a dead pipe. No
        // in-engine retry: a dead sidecar must surface through the loop's
        // error path, per ticket 31.
        let mut slot = self.state.lock().await;
        let mut session = self.ready_session(&mut slot, key).await.map_err(OcrError::new)?;
        match session
            .process
            .request(&payload, self.request_timeout)
            .await
        {
            Ok(blocks) => {
                *slot = Some(session);
                Ok(blocks)
            }
            Err(e) => Err(e),
        }
    }

    /// The live sidecar session serving `key`, spawning or restarting as
    /// needed. `slot` is held by the caller across the whole pass. A
    /// sidecar that visibly exited since the last pass is replaced (with
    /// a notice — see [`RestartNotice`]); a death the poll misses is
    /// caught by the request's EOF.
    async fn ready_session(
        &self,
        slot: &mut Option<Session<K>>,
        key: K,
    ) -> Result<Session<K>, String> {
        let session = match slot.take() {
            // Reuse the live sidecar while it serves the wanted model.
            Some(mut session) if session.key == key => match session.process.child.try_wait() {
                Ok(Some(status)) => {
                    if let Some(notice) = &self.on_unexpected_exit {
                        notice(format!(
                            "the {} sidecar exited unexpectedly ({status}) — \
                             restarting it for the next pass",
                            self.label
                        ));
                    }
                    None
                }
                _ => Some(session),
            },
            // A different model selection needs a restart: the docs' rule
            // is "close the old engine process, then start a new one".
            Some(session) => {
                drop(session);
                None
            }
            None => None,
        };
        match session {
            Some(session) => Ok(session),
            None => {
                let command =
                    (self.launch)(&key).map_err(|e| format!("{}: {e}", self.label))?;
                SidecarProcess::spawn(command, self.init_timeout)
                    .await
                    .map(|process| Session { key, process })
                    .map_err(|e| format!("{}: {e}", self.label))
            }
        }
    }
}

/// Keeps the last few stderr lines (ANSI-stripped) for error messages.
async fn drain_stderr(stderr: impl tokio::io::AsyncRead + Unpin, tail: Arc<StderrTail>) {
    let mut lines = BufReader::new(stderr).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        let mut tail = tail.lock().unwrap_or_else(|p| p.into_inner());
        if tail.len() >= STDERR_TAIL_LINES {
            tail.remove(0);
        }
        tail.push(scrub_ansi(&line));
    }
}

fn tail_of(tail: &StderrTail) -> String {
    let lines = tail.lock().unwrap_or_else(|p| p.into_inner());
    if lines.is_empty() {
        "nothing".to_string()
    } else {
        lines.join(" | ")
    }
}

/// Strips ANSI SGR sequences (`ESC [ … m`) from a log line — the engines
/// colorize their stderr and raw escapes would garble the app log.
fn scrub_ansi(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            // Swallow everything up to the sequence's final byte (an
            // ASCII letter); an unterminated sequence ends the line.
            for esc in chars.by_ref() {
                if esc.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(width: u32, height: u32) -> CapturedImage {
        CapturedImage {
            width,
            height,
            bgra: vec![90; (width * height * 4) as usize],
        }
    }

    #[test]
    fn task_is_a_base64_png_in_the_image_field() {
        let task = encode_task(&image(4, 4)).unwrap();
        let value: serde_json::Value = serde_json::from_str(&task).unwrap();
        let encoded = value["image_base64"].as_str().unwrap();
        let png = base64::engine::general_purpose::STANDARD.decode(encoded).unwrap();
        // PNG magic.
        assert_eq!(&png[..8], &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);
        assert_eq!(
            value.as_object().unwrap().len(),
            1,
            "no other fields ride along"
        );
    }

    #[test]
    fn success_reply_becomes_blocks_with_bounding_boxes() {
        let reply = include_str!("../fixtures/paddle/success.json");
        let blocks = parse_reply(reply).unwrap();
        assert_eq!(blocks.len(), 2);
        assert_eq!(
            blocks[0],
            TextBlock {
                x: 13,
                y: 5,
                width: 148, // 161 - 13
                height: 22, // 27 - 5
                text: "飞舞的因果交流".into(),
            }
        );
        assert_eq!(blocks[1].x, 210);
        assert_eq!(blocks[1].width, 138);
        assert_eq!(blocks[1].text, "第二行文本");
    }

    #[test]
    fn tilted_quad_becomes_the_axis_aligned_hull() {
        let reply = include_str!("../fixtures/paddle/success_skewed_quad.json");
        let blocks = parse_reply(reply).unwrap();
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].x, 101);
        assert_eq!(blocks[0].y, 52);
        assert_eq!(blocks[0].width, 195); // 296 - 101
        assert_eq!(blocks[0].height, 38); // 90 - 52
        assert_eq!(blocks[0].text, "tilted line");
    }

    #[test]
    fn no_text_found_is_an_empty_result_not_an_error() {
        let reply = include_str!("../fixtures/paddle/no_text.json");
        assert_eq!(parse_reply(reply).unwrap(), Vec::new());
    }

    #[test]
    fn engine_errors_carry_the_sidecars_message() {
        let reply = include_str!("../fixtures/paddle/bad_base64.json");
        let error = parse_reply(reply).unwrap_err();
        assert!(
            error.message.contains("Failed to decode base64"),
            "{}",
            error.message
        );
    }

    #[test]
    fn malformed_lines_never_panic() {
        assert!(parse_reply("").is_err());
        assert!(parse_reply("not json at all").is_err());
        assert!(parse_reply("{\"code\":100,\"data\":\"a string, not an array\"}").is_err());
        // Degenerate geometry (a point, not a box) is skipped, not fatal:
        assert_eq!(
            parse_reply("{\"code\":100,\"data\":[{\"box\":[[1,1]],\"text\":\"one point\"}]}")
                .unwrap(),
            Vec::new()
        );
        // Empty text is skipped the same way.
        assert_eq!(
            parse_reply("{\"code\":100,\"data\":[{\"box\":[[0,0],[9,0],[9,9],[0,9]],\"text\":\"  \"}]}")
                .unwrap(),
            Vec::new()
        );
    }

    #[test]
    fn unknown_codes_are_errors_with_the_message() {
        let error =
            parse_reply("{\"code\":401,\"data\":\"Json parse key image_base64 failed.\"}")
                .unwrap_err();
        assert!(error.message.contains("image_base64"));
    }

    #[test]
    fn ansi_codes_are_stripped_from_log_lines() {
        let scrubbed = scrub_ansi("\u{1b}[37m---    fused 0 elementwise_mul\u{1b}[0m");
        assert_eq!(scrubbed, "---    fused 0 elementwise_mul");
    }
}
