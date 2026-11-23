//! The translation loop: capture → change detection → read → multi-line
//! merge (classic path only, ticket 23) → Excluded Block filter →
//! per-block stability window → glossary short-circuit → translate
//! (LLM engines get per-block prompt guidance: relevant Glossary entries +
//! the Genderless instruction, ticket 11) → genderless scrub (ticket 08) →
//! events, across every active Capture Zone.
//!
//! The read stage has two forms (ticket 12): the classic OCR pass feeding
//! per-block translation, or — when a fused engine is attached — ONE
//! vision-LLM call that reads every Text Block and translates it in the
//! same answer, bypassing both the OcrEngine and TranslateEngine stages.
//! On the classic path the OCR output first runs through the multi-line
//! merge ([`crate::merge::merge_lines`], ticket 23): consecutive lines
//! that visually form one paragraph become one Text Block before tracking,
//! so a multi-line box is one pill and one engine call. The fused path is
//! untouched — the vision model already reads the zone as a whole.
//! Everything downstream of the read is shared: the frame checksum still
//! decides whether the read runs at all, and each block's dHash still gates
//! when its result fires, so an unchanged screen costs capture + hashing
//! only and an unchanged block is never re-emitted — in fused mode the
//! block's translation rides in its pending change instead of being
//! requested from a Translate Engine on fire. Pixel hashing alone cannot
//! see that a re-read found the SAME text, though (ticket 22): jitter
//! around unchanged glyphs changes a block's dHash and would re-call the
//! engine with identical text — so the shared fire path also compares the
//! read's trimmed text against the block's last shown translation and
//! drops exact repeats before any engine call or event.
//!
//! All collaborators are ports (`capture`, `ocr`, `translate`, `clock`) plus
//! the watched zone list, the paused flag, and the active Excluded Blocks,
//! which arrive through watch channels so the shell can change them while
//! the loop runs without losing per-block tracking state.
//!
//! Translation Context (ticket 44) rides the same pattern: each zone keeps
//! a rolling buffer of its last X successful translations (source → Thai),
//! and the buffer rides into every LLM prompt the zone fires through the
//! same per-block guidance the Glossary uses, rendered as a "Recent
//! translations" continuity section ([`crate::prompt`]). Only actual
//! emissions record into it — a failed translation and a suppressed
//! identical re-read (ticket 22) leave the buffer alone, and a Glossary-pinned
//! display counts (it is what the user saw). The window X arrives through
//! its own watch channel so a settings change applies on the very next
//! translation without a loop restart; 0 (the default) records nothing and
//! injects nothing.
//!
//! Change detection (ticket 05) is what makes continuous translation cheap
//! enough to leave running. Every tick captures the region and checksums
//! its pixels: when the checksum matches the last OCR'd frame, nothing on
//! screen changed and OCR is skipped entirely — that tick costs capture +
//! hashing only, inside the ≤ 50 ms budget. When pixels changed, OCR runs,
//! and each Text Block's bounding box is dHashed: blocks whose pixels are
//! unchanged are skipped without a new translation (even if their OCR text
//! jitters), and a changed block must hold its new pixels stable for
//! [`LoopConfig::stability`] before it is translated. Animations never hold
//! still, so they debounce into silence instead of spamming re-translations.
//!
//! Every pass also measures itself (ticket 47): capture, the read stage,
//! the translation work, and the pass total ride the same event stream as a
//! [`LoopEvent::Timing`] — a few clock reads per pass, no behavior change —
//! so the Timing Debug window can show which stage is slow in real time.
//! The same event carries the pass's read trace (ticket 48): every Text
//! Block the read produced with its words and the decision the loop made
//! about it, built at the decision points themselves.

use crate::glossary::Glossary;
use crate::hash::{block_dhash, frame_checksum};
use crate::merge::merge_lines;
use crate::ports::{
    CaptureSource, CapturedImage, Clock, ExcludedBlock, FusedEngine, OcrEngine, Region,
    SourceLanguage, TextBlock, TranslateEngine, Zone,
};
use crate::sample::{sample_block_style, SampledStyle};
use async_trait::async_trait;
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, watch};

/// Which path produced a translation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Via {
    /// From the active Translate Engine.
    Engine,
    /// From a Glossary whole-block match (engine never called).
    Glossary,
}

/// Which pipeline stage an error came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    Capture,
    Ocr,
    Translate,
    /// The fused vision call (ticket 12) — it replaces the OCR and
    /// Translate stages, so its failures carry their own stage name.
    Fused,
}

impl Stage {
    /// The stage's snake_case name — the spelling the serialized events use
    /// (and the app log's tag for stage failures, ticket 27).
    pub fn name(&self) -> &'static str {
        match self {
            Stage::Capture => "capture",
            Stage::Ocr => "ocr",
            Stage::Translate => "translate",
            Stage::Fused => "fused",
        }
    }
}

/// Where a pipeline pass came from: one of the active Capture Zones, or a
/// One-shot Capture (a single pass that creates no zone and has no tracking).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EventOrigin {
    Zone { id: String, name: String },
    OneShot,
}

/// What the loop decided about one Text Block a read pass produced (ticket
/// 48), rendered by the Timing Debug window so "why didn't this
/// translate?" is answerable on screen. Traces are built exactly where the
/// decisions happen — [`Self::process_zone`], [`Self::fire_read`],
/// [`Self::fire_matured`], [`Self::capture_once`] — so they cannot drift
/// from the branch actually taken.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(tag = "disposition", rename_all = "snake_case")]
pub enum ReadDisposition {
    /// Shown: the Translate Engine answered this pass's own read.
    TranslatedEngine,
    /// Shown: a Glossary whole-block match pinned it (engine never called).
    TranslatedGlossary,
    /// Shown: the fused vision call answered read + translation together
    /// (ticket 12) — a Glossary pin on the answer doesn't change that the
    /// read itself was fused.
    TranslatedFused,
    /// Shown: a fast-path matured fire ([`Self::fire_matured`]) off the
    /// read cached in the pending — no OCR ran this pass. Where the shown
    /// Thai came from (engine / glossary) rides on the `Translated` event.
    TranslatedCached,
    /// Suppressed by the shared fire path's dedup: the trimmed text equals
    /// the block's last SHOWN text, so nothing was re-requested or
    /// re-emitted — the In-place Overlay already displays it (ticket 22).
    Suppressed,
    /// A change holding still inside the stability window (ticket 05), not
    /// yet matured. Carries how long the pixels have held against the
    /// configured window, so the UI can show the time remaining.
    Waiting { elapsed_ms: u64, total_ms: u64 },
    /// Pixels identical to what the currently shown translation was
    /// rendered for (`fired_hash` match) — including a pending change that
    /// reverted to the shown pixels, which this cancels.
    Unchanged,
    /// An Excluded Block covers it (ticket 04) — never translated, never
    /// shown, and its tracking state is dropped.
    Excluded,
    /// The read text trimmed to nothing — whitespace noise, dropped
    /// before tracking.
    Empty,
    /// Missing from this pass's read; `misses` counts consecutive passes
    /// missing it, toward [`REMOVAL_MISSES`] (two in a row remove it).
    Gone { misses: u32 },
}

/// One read's trace entry (ticket 48): the words the read stage saw plus
/// the decision made about them. The words reuse the trimmed text the pass
/// already allocated — where the loop needs its own copy still (a pending's
/// cached read, the dedup record), the trace clones it; everywhere else it
/// moves in. Built at the decision points, so it rides every `Timing`
/// event whether or not the debug window is open.
#[derive(Debug, Clone, Serialize)]
pub struct BlockTrace {
    /// The block's trimmed read text — the words OCR read. Empty for an
    /// [`ReadDisposition::Empty`] read.
    pub text: String,
    pub disposition: ReadDisposition,
}

impl From<Via> for ReadDisposition {
    /// The classic fire path's route → its trace disposition (ticket 48).
    /// Shared by `fire_read` and the one-shot pass so the engine/glossary
    /// split has exactly one spelling.
    fn from(via: Via) -> Self {
        match via {
            Via::Engine => Self::TranslatedEngine,
            Via::Glossary => Self::TranslatedGlossary,
        }
    }
}

/// Everything the display layer needs, streamed per pipeline tick.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum LoopEvent {
    Translated {
        /// The block's display identity ([`block_key_string`]) so the
        /// In-place Overlay can update its element in place instead of
        /// recreating it. Ignored by the Results Window.
        key: String,
        block: TextBlock,
        translation: String,
        via: Via,
        at_ms: u64,
        origin: EventOrigin,
        /// Auto Colors (ticket 14): the block's text and pill colors
        /// sampled from the frame it fired on. `Some` only when
        /// [`LoopConfig::auto_colors`] is on AND sampling succeeded;
        /// `None` means the overlay falls back to the manual Text Style.
        style: Option<SampledStyle>,
    },
    /// Text Blocks that vanished from a region's latest pass: the overlay
    /// clears their translations (stale text must not linger on screen
    /// after the source text is gone). Keys match [`LoopEvent::Translated`].
    BlocksRemoved {
        origin: EventOrigin,
        keys: Vec<String>,
        at_ms: u64,
    },
    Error {
        stage: Stage,
        message: String,
        at_ms: u64,
    },
    /// Per-pass stage timings (ticket 47): how long each stage of ONE
    /// pipeline pass took — capture (screenshot + checksum / change
    /// detection), the read (OCR + translate on the classic path, or the
    /// single fused call replacing both), and the pass total. Always
    /// collected (a few clock reads per pass, no behavior change) and
    /// rendered only by the Timing Debug window; the forwarder drops the
    /// event when that window is closed. Every pass emits — including
    /// unchanged ticks, whose `blocks_translated` is 0 — because capture
    /// itself can be the slow stage on a large zone.
    Timing {
        origin: EventOrigin,
        /// Capture + frame checksum + change detection.
        capture_ms: u64,
        /// The classic OCR read; `None` when this pass never read
        /// (unchanged frame took the fast path) or read through the fused
        /// call.
        ocr_ms: Option<u64>,
        /// Time spent producing translations during this pass — Translate
        /// Engine calls and their Glossary short-circuits, matured
        /// fast-path fires included (their engine time is this pass's
        /// cost). `None` when the stage never ran: the fused path (the
        /// translation rides the fused call's answer) and classic passes
        /// where no fire was attempted (ticket 50) alike — so the debug
        /// window's rolling summary counts real translate-stage passes
        /// only. `Some(0)` is honest: a Glossary-pinned fire ran and
        /// genuinely cost ~nothing.
        translate_ms: Option<u64>,
        /// The fused vision call (ticket 12); `None` on the classic path.
        fused_ms: Option<u64>,
        /// Pass start to pass end — the stage durations above plus the
        /// pass's own hashing and bookkeeping, so they sum to it within
        /// rounding.
        total_ms: u64,
        /// Translations actually shown during this pass (suppressed
        /// deduped reads and failures do not count).
        blocks_translated: u32,
        /// True when this pass skipped the read entirely — the fast path,
        /// whose capture checksum matched the last OCR'd frame (ticket 48):
        /// the row is marked "OCR skipped — frame unchanged" and `reads`
        /// carries only its matured fast-path fires. A failed capture or
        /// read is not a skip — the pass never got that far.
        read_skipped: bool,
        /// The per-block read trace (ticket 48): every Text Block this
        /// pass's read produced, the words it read, and the decision the
        /// loop made about each. Fast-path passes list only their matured
        /// fires; failed reads and failed translations leave no entry —
        /// the `Error` event naming the failure rides the same stream.
        reads: Vec<BlockTrace>,
        at_ms: u64,
    },
}

/// The loop's fixed inputs: in which language to read, how often, and how
/// long a change must hold still. The watched regions (Capture Zones) and
/// the paused flag are dynamic and arrive via watch channels.
#[derive(Debug, Clone)]
pub struct LoopConfig {
    pub source_language: SourceLanguage,
    pub cadence: Duration,
    /// How long a changed Text Block must hold its pixels stable before
    /// OCR+translate fire (ticket 05; debounces animations, whose pixels
    /// never stop changing). 0 fires on the detecting pass itself.
    pub stability: Duration,
    /// Genderless Mode (ticket 08): scrub gendered Thai politeness
    /// particles and first-person pronouns from ENGINE output before it
    /// is displayed. Read at loop start like the language — the shell
    /// restarts the loop when the toggle or the active Profile's override
    /// changes. Glossary output is the user's own pinned Thai and is
    /// never scrubbed.
    pub genderless: bool,
    /// Auto Colors (ticket 14): sample each fired block's text and pill
    /// colors from the captured frame and attach them to its `Translated`
    /// event. Read at loop start like `genderless` — the shell restarts
    /// the loop when the Text Style toggle changes.
    pub auto_colors: bool,
    /// Custom base instruction for the per-block translation prompt
    /// (ticket 37, Settings → Engines → Prompts); `None` or blank = the
    /// built-in default. Read at loop start like the language — the shell
    /// restarts the loop when the field changes. The raw config string is
    /// fine here: the prompt module normalizes (trims, whitespace-only =
    /// default).
    pub translation_prompt_override: Option<String>,
    /// The same override for the fused screen-reading prompt (ticket 37),
    /// handed to the fused engine's guidance.
    pub fused_prompt_override: Option<String>,
    /// The System Prompt Preset (ticket 42) the two overrides are the
    /// verbatim wording of — a built-in's code or a saved preset's name —
    /// as the shell resolved it from the config. Labels only: it rides into
    /// both guidances so the prompt module's version tag can name the
    /// preset. `None` for the built-in default and for hand-written text.
    pub prompt_preset: Option<String>,
}

/// A Text Block is identified by its on-screen position, quantized so
/// sub-pixel OCR jitter does not count as a "new" block. Its bounding box
/// is quantized to the same grid before hashing, so box jitter inside one
/// cell hashes the same pixels.
const BLOCK_GRID_PX: i32 = 4;

/// How many consecutive OCR passes must miss a block before it counts as
/// removed from the screen (Windows OCR flakiness guard; see `misses`).
const REMOVAL_MISSES: u32 = 2;

fn block_key(block: &TextBlock) -> (i32, i32) {
    (block.x / BLOCK_GRID_PX, block.y / BLOCK_GRID_PX)
}

/// A block's identity for the display layer, shared by
/// [`LoopEvent::Translated`] and [`LoopEvent::BlocksRemoved`].
fn block_key_string(block: &TextBlock) -> String {
    let (gx, gy) = block_key(block);
    format!("{gx},{gy}")
}

/// The block's bounding box as a region — the one place a `TextBlock`
/// rect becomes the `Region` the pixel-working helpers take.
fn block_region(block: &TextBlock) -> Region {
    Region {
        x: block.x,
        y: block.y,
        width: block.width,
        height: block.height,
    }
}

/// The block's bounding box snapped to the block grid, so an OCR box that
/// jitters a few pixels between passes still hashes the same rect of
/// pixels. Width and height quantize too (with a one-cell floor) so box
/// shape is absorbed as well; [`block_dhash`] clamps whatever pokes past
/// the region edge.
fn quantized_region(block: &TextBlock) -> Region {
    Region {
        x: block.x / BLOCK_GRID_PX * BLOCK_GRID_PX,
        y: block.y / BLOCK_GRID_PX * BLOCK_GRID_PX,
        width: (block.width as i32).div_euclid(BLOCK_GRID_PX).max(1) as u32 * BLOCK_GRID_PX as u32,
        height: (block.height as i32).div_euclid(BLOCK_GRID_PX).max(1) as u32
            * BLOCK_GRID_PX as u32,
    }
}

/// One tracked Text Block's change-detection state.
#[derive(Debug, Default)]
struct BlockState {
    /// dHash of the pixels the block's current translation was shown for.
    /// `None` = never translated. A pass whose block hashes to this value
    /// is skipped — even when its OCR text jitters.
    fired_hash: Option<u64>,
    /// Ticket 22: the trimmed text of the last read that was successfully
    /// translated and shown. A matured read whose trimmed text equals this
    /// is suppressed on the shared fire path — pixel jitter around
    /// unchanged glyphs (animation, antialiasing) must not re-call the
    /// engine or re-emit the event for a translation the In-place Overlay
    /// already displays. Recorded only on success: a failed fire records
    /// nothing, so the read is retried when the pixels change again.
    /// Cleared with the rest of the state when the block is removed, so
    /// reappearance re-translates.
    last_fired_text: Option<String>,
    /// A pixel change observed but not yet fired: still inside the
    /// stability window, or pixels that keep changing (an animation). The
    /// change's rect and text are from the last OCR'd frame that showed
    /// them, so a window that matures on a later fast-path tick (pixels
    /// identical, OCR skipped) can fire with that cached read.
    pending: Option<PendingChange>,
    /// Consecutive passes that did not see this block; two in a row remove
    /// it (Windows OCR intermittently returns empty results on a healthy
    /// image). A pass that sees the block resets the count.
    misses: u32,
}

impl BlockState {
    /// The last words this block was seen with — the words its gone-row
    /// trace entry carries (ticket 48): a pending change's cached read if
    /// one is holding, else the text of the translation last shown.
    /// Copies, not moves — the loop keeps its state. Empty when the block
    /// was never read to text (its only fire failed before anything was
    /// shown).
    fn last_known_text(&self) -> String {
        self.pending
            .as_ref()
            .map(|p| p.text.clone())
            .or_else(|| self.last_fired_text.clone())
            .unwrap_or_default()
    }
}

/// A change holding in the stability window.
#[derive(Debug)]
struct PendingChange {
    hash: u64,
    /// Clock ms when this hash was first seen. The window restarts whenever
    /// the pixels change to a different hash — animation jitter resets it
    /// every tick and never fires.
    since_ms: u64,
    rect: TextBlock,
    text: String,
    /// The translation that fires with this change when the read came from
    /// a fused call (ticket 12): the vision model answered it together with
    /// the block, so firing displays it directly. `None` = classic OCR
    /// read: firing calls the Translate Engine with `text` as usual.
    translation: Option<String>,
}

/// A zone's tracking state. `last_frame` is the checksum of the last OCR'd
/// frame; a tick whose capture checksums identically skips OCR and only
/// matures pendings and advances removals. `recent` is the Translation
/// Context buffer (ticket 44): the zone's last successful translations,
/// oldest first, trimmed to the live window.
#[derive(Debug)]
struct ZoneState {
    region: Region,
    last_frame: Option<u64>,
    blocks: HashMap<(i32, i32), BlockState>,
    recent: Vec<(String, String)>,
}

impl ZoneState {
    fn fresh(region: Region) -> Self {
        Self {
            region,
            last_frame: None,
            blocks: HashMap::new(),
            recent: Vec::new(),
        }
    }
}

/// Stage durations and the read trace accumulated across one pipeline pass
/// (tickets 47/48), then shipped as [`LoopEvent::Timing`]. The read stage
/// fills `ocr_ms` or `fused_ms`; every Translate Engine call this pass
/// made — matured fast-path fires included — adds to `translate_ms` and
/// counts a shown block; every read decision pushes a [`BlockTrace`].
/// Purely observational: nothing in the loop reads this struct, it only
/// rides along.
#[derive(Debug, Default)]
struct PassTimings {
    capture_ms: u64,
    ocr_ms: Option<u64>,
    fused_ms: Option<u64>,
    translate_ms: u64,
    /// The stage ran this pass: at least one fire attempt entered
    /// [`Self::translate_and_emit`] — an engine call or a Glossary
    /// short-circuit (ticket 50). A pass with zero fire attempts keeps it
    /// false, so `emit_timing` reports `translate_ms: None` instead of a
    /// lying `Some(0)`.
    translate_ran: bool,
    blocks_translated: u32,
    /// The fast path sets this instead of reading (ticket 48).
    read_skipped: bool,
    reads: Vec<BlockTrace>,
}

impl PassTimings {
    /// Records one read's trace entry (ticket 48) — the single line every
    /// decision point pushes, so the entry shape lives in one place.
    fn trace(&mut self, text: String, disposition: ReadDisposition) {
        self.reads.push(BlockTrace { text, disposition });
    }
}

/// Evicts a Translation Context buffer's oldest pairs until it holds at
/// most `window` (ticket 44) — the one eviction invariant, shared by the
/// record path and every read. A window of 0 empties the buffer outright:
/// with the feature off nothing is kept, so re-enabling it later starts
/// from what is translated next, never from a stale backlog.
fn trim_recent(buffer: &mut Vec<(String, String)>, window: usize) {
    if window == 0 {
        buffer.clear();
    } else {
        buffer.drain(..buffer.len().saturating_sub(window));
    }
}

/// Trims a zone's Translation Context buffer (ticket 44) to the live window
/// and returns the pairs to inject into this pass's prompts — oldest first.
fn recent_context(state: &mut ZoneState, window: usize) -> Vec<(String, String)> {
    trim_recent(&mut state.recent, window);
    state.recent.clone()
}

/// Records one displayed translation in the zone's rolling buffer (ticket
/// 44). Only the fire path calls this, and only on a shown translation —
/// engine failures and suppressed identical re-reads never reach it, and
/// the recorded value is the post-scrub Thai that actually went on screen.
fn record_recent(buffer: &mut Vec<(String, String)>, window: usize, pair: (String, String)) {
    if window == 0 {
        return;
    }
    buffer.push(pair);
    trim_recent(buffer, window);
}

/// Runs the Auto-translate loop against injected ports and streams
/// [`LoopEvent`]s to the display.
pub struct Orchestrator {
    capture: Arc<dyn CaptureSource>,
    ocr: Arc<dyn OcrEngine>,
    translate: Arc<dyn TranslateEngine>,
    /// The fused vision engine (ticket 12). `Some` = every read is ONE
    /// model call over the whole frame; `ocr` and `translate` then sit
    /// unused (they are always built so the pipeline shape stays uniform).
    fused: Option<Arc<dyn FusedEngine>>,
    clock: Arc<dyn Clock>,
    config: LoopConfig,
    glossary: Glossary,
    zones_rx: watch::Receiver<Vec<Zone>>,
    paused_rx: watch::Receiver<bool>,
    /// The shell's picker suspension (ticket 46): true while the Capture
    /// Zone picker is open with Capture Visibility on. Distinct from the
    /// user pause — the tray label and Results Window state stay "running"
    /// while it is in force; the loop just holds still like a paused one.
    suspended_rx: watch::Receiver<bool>,
    excluded_rx: watch::Receiver<Vec<ExcludedBlock>>,
    /// Translation Context window (ticket 44): the live number of recent
    /// translations each zone's prompts carry. Read per pass, so a change
    /// applies on the next translation without a loop restart; 0 = off
    /// (the private default channel parks there).
    context_rx: watch::Receiver<u32>,
    events: mpsc::UnboundedSender<LoopEvent>,
    /// Zone id → its tracking state (last frame checksum + per-block dHash
    /// state). State for removed zones is pruned per tick, so a redrawn
    /// zone (same id pattern, new id) starts fresh.
    zones: HashMap<String, ZoneState>,
    /// The Excluded Block list as of the last tick. When it changes, every
    /// zone's frame checksum is invalidated so the next pass OCRs even an
    /// unchanged screen — clearing an exclusion must translate its block
    /// again, and that block's pixels may not have changed to announce it.
    exclusions_seen: Vec<ExcludedBlock>,
}

impl Orchestrator {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        capture: Arc<dyn CaptureSource>,
        ocr: Arc<dyn OcrEngine>,
        translate: Arc<dyn TranslateEngine>,
        clock: Arc<dyn Clock>,
        config: LoopConfig,
        glossary: Glossary,
        zones_rx: watch::Receiver<Vec<Zone>>,
        paused_rx: watch::Receiver<bool>,
        excluded_rx: watch::Receiver<Vec<ExcludedBlock>>,
        events: mpsc::UnboundedSender<LoopEvent>,
    ) -> Self {
        Self {
            capture,
            ocr,
            translate,
            fused: None,
            clock,
            config,
            glossary,
            zones_rx,
            paused_rx,
            // Without the builder the loop parks on a private false channel:
            // never suspended — the picker feature (ticket 46) is off.
            suspended_rx: watch::channel(false).1,
            excluded_rx,
            context_rx: watch::channel(0u32).1,
            events,
            zones: HashMap::new(),
            exclusions_seen: Vec::new(),
        }
    }

    /// Attaches the fused vision engine (ticket 12): from then on every
    /// zone read is ONE model call over the whole frame instead of an OCR
    /// pass feeding per-block translation. `None` (the default) keeps the
    /// classic pipeline.
    pub fn with_fused(mut self, fused: Option<Arc<dyn FusedEngine>>) -> Self {
        self.fused = fused;
        self
    }

    /// Points Translation Context (ticket 44) at a live watch channel: the
    /// shell sends the configured window at loop start and re-sends it on
    /// every settings change, and the loop applies the new value on the
    /// next translation without a restart. Without this builder the
    /// orchestrator parks on a private 0 channel — the feature off.
    pub fn with_context_rx(mut self, context_rx: watch::Receiver<u32>) -> Self {
        self.context_rx = context_rx;
        self
    }

    /// Points the picker suspension (ticket 46) at a live watch channel:
    /// the shell sends `true` when the Capture Zone picker opens with
    /// Capture Visibility on (the picker blankets the monitor, so masking
    /// it would blind the loop — ADR-0004) and `false` when it closes.
    /// Without this builder the orchestrator parks on a private false
    /// channel — the loop never suspends.
    pub fn with_suspended_rx(mut self, suspended_rx: watch::Receiver<bool>) -> Self {
        self.suspended_rx = suspended_rx;
        self
    }

    /// The Translation Context window in force right now (ticket 44).
    fn context_window(&self) -> usize {
        *self.context_rx.borrow() as usize
    }

    /// One pipeline pass over every active zone: capture, change detection,
    /// OCR, per-block stability, translate, emit. Zones are processed in
    /// order, each with its own tracking state. Never panics; every failure
    /// becomes an [`LoopEvent::Error`] — the app must never fail silently.
    pub async fn tick(&mut self) {
        let zones = self.zones_rx.borrow().clone();
        if zones.is_empty() {
            self.zones.clear(); // no zones → no state worth keeping
            return;
        }
        let exclusions = self.excluded_rx.borrow().clone();
        if exclusions != self.exclusions_seen {
            self.exclusions_seen = exclusions.clone();
            for state in self.zones.values_mut() {
                state.last_frame = None;
            }
        }
        for zone in &zones {
            let state = self
                .zones
                .remove(&zone.id)
                .unwrap_or_else(|| ZoneState::fresh(zone.region));
            // A zone dragged to a new rectangle starts fresh: its block
            // state belongs to the old region's pixels.
            let state = if state.region == zone.region {
                state
            } else {
                ZoneState::fresh(zone.region)
            };
            let state = self.process_zone(zone, state, &exclusions).await;
            self.zones.insert(zone.id.clone(), state);
        }
        // Drop tracking state of zones that no longer exist, so a redrawn
        // zone (same id pattern, new id) starts fresh.
        self.zones.retain(|id, _| zones.iter().any(|z| &z.id == id));
    }

    /// One OCR+translate pass over `region` — the One-shot Capture: events
    /// carry [`EventOrigin::OneShot`], everything found is translated
    /// immediately (no tracking state, no stability window — the user asked
    /// for what is on screen right now), and the running loop's state is
    /// untouched. With a fused engine attached the pass is one model call
    /// (ticket 12). Emits a [`LoopEvent::Timing`] like a zone pass (ticket
    /// 47) — every pass does, a failed capture included: its duration is
    /// the "which stage is slow" answer when capture hangs.
    pub async fn capture_once(&self, region: Region) {
        let origin = EventOrigin::OneShot;
        let label = "One-shot capture";
        let started = self.clock.now_ms();
        let mut timings = PassTimings::default();
        let image = match self.capture.capture(region).await {
            Ok(image) => image,
            Err(e) => {
                timings.capture_ms = self.clock.now_ms().saturating_sub(started);
                self.emit(Stage::Capture, format!("{label}: {e}"));
                self.emit_timing(origin, timings, started, true);
                return;
            }
        };
        timings.capture_ms = self.clock.now_ms().saturating_sub(started);
        let exclusions = self.excluded_rx.borrow().clone();
        if let Some(fused) = &self.fused {
            let read_started = self.clock.now_ms();
            let fused_blocks = match fused
                .recognize_and_translate(
                    &image,
                    self.config.source_language,
                    crate::ports::TARGET_LANGUAGE,
                    // A one-shot creates no Capture Zone, so there is no
                    // per-zone buffer to continue from (ticket 44).
                    &self.fused_guidance(Vec::new()),
                )
                .await
            {
                Ok(blocks) => blocks,
                Err(e) => {
                    timings.fused_ms = Some(self.clock.now_ms().saturating_sub(read_started));
                    self.emit(Stage::Fused, format!("{label}: {e}"));
                    self.emit_timing(origin, timings, started, false);
                    return;
                }
            };
            timings.fused_ms = Some(self.clock.now_ms().saturating_sub(read_started));
            for fused in fused_blocks {
                let text = fused.block.text.trim().to_string();
                if text.is_empty() {
                    timings.trace(text, ReadDisposition::Empty);
                    continue;
                }
                if exclusions
                    .iter()
                    .any(|e| e.covers(&screen_block(&fused.block, region)))
                {
                    timings.trace(text, ReadDisposition::Excluded);
                    continue;
                }
                self.emit_fused_block(
                    region,
                    &fused.block,
                    &fused.translation,
                    &origin,
                    self.sampled_style(&image, &fused.block),
                )
                .await;
                timings.blocks_translated += 1;
                timings.trace(text, ReadDisposition::TranslatedFused);
            }
            self.emit_timing(origin, timings, started, false);
            return;
        }
        let read_started = self.clock.now_ms();
        let blocks = match self
            .ocr
            .recognize(&image, self.config.source_language)
            .await
        {
            Ok(blocks) => blocks,
            Err(e) => {
                timings.ocr_ms = Some(self.clock.now_ms().saturating_sub(read_started));
                self.emit(Stage::Ocr, format!("{label}: {e}"));
                self.emit_timing(origin, timings, started, true);
                return;
            }
        };
        timings.ocr_ms = Some(self.clock.now_ms().saturating_sub(read_started));
        // Ticket 23: the same merge pass as the zone loop's classic read
        // (crate::merge) — a multi-line paragraph is one Text Block here too.
        for block in merge_lines(blocks, self.config.source_language) {
            let text = block.text.trim().to_string();
            if text.is_empty() {
                timings.trace(text, ReadDisposition::Empty);
                continue;
            }
            let screen_block = screen_block(&block, region);
            if exclusions.iter().any(|e| e.covers(&screen_block)) {
                timings.trace(text, ReadDisposition::Excluded);
                continue;
            }
            if let Some((_, via)) = self
                .translate_and_emit(
                    region,
                    &block,
                    &text,
                    &origin,
                    label,
                    // No zone context in a one-shot pass (ticket 44).
                    &[],
                    self.sampled_style(&image, &block),
                    &mut timings,
                )
                .await
            {
                timings.blocks_translated += 1;
                timings.trace(text, via.into());
            }
        }
        self.emit_timing(origin, timings, started, true);
    }

    /// One pass over one zone. Fast path: the capture is pixel-identical to
    /// the last OCR'd frame → matured pendings fire with their cached read,
    /// blocks that frame already missed keep missing, and OCR never runs.
    /// Otherwise OCR runs and every Text Block goes through the dHash
    /// pipeline. Never panics; every failure becomes a [`LoopEvent::Error`].
    /// Every pass also emits a [`LoopEvent::Timing`] (ticket 47) — unchanged
    /// ticks included (`blocks_translated: 0`), because capture itself can
    /// be the slow stage; a failed capture emits the error plus the timing
    /// it managed (capture duration up to the failure — a hanging capture
    /// is exactly when that number matters).
    async fn process_zone(
        &self,
        zone: &Zone,
        mut state: ZoneState,
        exclusions: &[ExcludedBlock],
    ) -> ZoneState {
        let origin = EventOrigin::Zone {
            id: zone.id.clone(),
            name: zone.name.clone(),
        };
        let label = format!("zone \"{}\"", zone.name);
        let started = self.clock.now_ms();
        let mut timings = PassTimings::default();
        let image = match self.capture.capture(zone.region).await {
            Ok(image) => image,
            Err(e) => {
                timings.capture_ms = self.clock.now_ms().saturating_sub(started);
                self.emit(Stage::Capture, format!("{label}: {e}"));
                self.emit_timing(origin, timings, started, self.fused.is_none());
                return state;
            }
        };
        timings.capture_ms = self.clock.now_ms().saturating_sub(started);

        let frame = frame_checksum(&image);
        if state.last_frame == Some(frame) {
            // Ticket 48: the fast path never reads — the timing row is
            // marked "OCR skipped — frame unchanged" and carries only the
            // matured fires' trace entries.
            timings.read_skipped = true;
            self.fire_matured(
                &mut state,
                zone.region,
                &origin,
                &label,
                &image,
                &mut timings,
            )
            .await;
            self.advance_removals(&mut state, &origin);
            self.emit_timing(origin, timings, started, self.fused.is_none());
            return state;
        }

        // Translation Context (ticket 44): the live window, read once per
        // pass. The fused call below injects the zone's buffer whole; each
        // classic fire re-reads it after the previous emission, so blocks
        // firing later in the same pass chain off the ones before them.
        let context_window = self.context_window();

        // The read stage, in one of its two forms (ticket 12): a fused
        // call answers blocks WITH translations, a classic OCR pass yields
        // blocks whose translation is requested per fire. Either way each
        // read is `(block, translation)` — `None` marks the OCR form.
        let reads: Vec<(TextBlock, Option<String>)> = match &self.fused {
            Some(fused) => {
                let read_started = self.clock.now_ms();
                let answered = fused
                    .recognize_and_translate(
                        &image,
                        self.config.source_language,
                        crate::ports::TARGET_LANGUAGE,
                        &self.fused_guidance(recent_context(&mut state, context_window)),
                    )
                    .await;
                timings.fused_ms = Some(self.clock.now_ms().saturating_sub(read_started));
                match answered {
                    Ok(blocks) => blocks
                        .into_iter()
                        .map(|fused| (fused.block, Some(fused.translation)))
                        .collect(),
                    Err(e) => {
                        self.emit(Stage::Fused, format!("{label}: {e}"));
                        // last_frame stays behind: the next pass retries
                        // even if the screen has not changed since.
                        self.emit_timing(origin, timings, started, false);
                        return state;
                    }
                }
            }
            None => {
                let read_started = self.clock.now_ms();
                let recognized = self
                    .ocr
                    .recognize(&image, self.config.source_language)
                    .await;
                timings.ocr_ms = Some(self.clock.now_ms().saturating_sub(read_started));
                let blocks = match recognized {
                    Ok(blocks) => blocks,
                    Err(e) => {
                        self.emit(Stage::Ocr, format!("{label}: {e}"));
                        // last_frame stays behind: the next pass retries OCR
                        // even if the screen has not changed since.
                        self.emit_timing(origin, timings, started, true);
                        return state;
                    }
                };
                // Ticket 23: merge consecutive OCR lines into one Text
                // Block before tracking (rationale in crate::merge).
                merge_lines(blocks, self.config.source_language)
                    .into_iter()
                    .map(|block| (block, None))
                    .collect()
            }
        };

        let now = self.clock.now_ms();
        let stable_ms = self.config.stability.as_millis() as u64;
        let mut seen: HashSet<(i32, i32)> = HashSet::new();
        for (block, fused_translation) in reads {
            let text = block.text.trim().to_string();
            if text.is_empty() {
                timings.trace(text, ReadDisposition::Empty);
                continue;
            }
            let key = block_key(&block);
            seen.insert(key);

            if exclusions
                .iter()
                .any(|e| e.covers(&screen_block(&block, zone.region)))
            {
                // Excluded Block (ticket 04): never translated, never shown.
                // Dropping its tracking state right away means clearing the
                // exclusion translates it again on the next read pass.
                state.blocks.remove(&key);
                timings.trace(text, ReadDisposition::Excluded);
                continue;
            }

            let hash = block_dhash(&image, quantized_region(&block));
            // Ticket 44: the context snapshot for this block's fire —
            // taken after any earlier emission this pass recorded, so a
            // dialogue's next line chains off the previous one even within
            // one frame.
            let recent = recent_context(&mut state, context_window);
            let entry = state.blocks.entry(key).or_default();
            entry.misses = 0;

            if entry.fired_hash == Some(hash) {
                // Pixels identical to what the current translation was shown
                // for: skip — a fused answer for an unchanged block is
                // dropped unread, and a pending change that reverted to the
                // shown pixels is cancelled too.
                entry.pending = None;
                timings.trace(text, ReadDisposition::Unchanged);
            } else if entry.pending.as_ref().is_some_and(|p| p.hash == hash) {
                // The same change as the previous pass: the block is holding
                // still. Fire once the stability window has elapsed...
                let since = entry
                    .pending
                    .as_ref()
                    .expect("pending hash matched above")
                    .since_ms;
                if now.saturating_sub(since) >= stable_ms {
                    entry.pending = None;
                    if let Some(pair) = self
                        .fire_read(
                            entry,
                            zone.region,
                            &block,
                            &text,
                            hash,
                            fused_translation.as_deref(),
                            &origin,
                            &label,
                            &image,
                            &recent,
                            // This pass read the frame itself — the words
                            // are fresh, not the pending's cached ones.
                            false,
                            &mut timings,
                        )
                        .await
                    {
                        record_recent(&mut state.recent, context_window, pair);
                    }
                } else {
                    // ...otherwise keep waiting, refreshing the cached read
                    // to this frame — the newest one a fast-path fire could
                    // use. The trace's copy of the words is the pass's one
                    // new allocation here: pending keeps its own text (the
                    // cached read a future fast-path fire needs).
                    let pending = entry.pending.as_mut().expect("pending hash matched above");
                    pending.rect = block.clone();
                    pending.text = text.clone();
                    pending.translation = fused_translation;
                    timings.trace(
                        text,
                        ReadDisposition::Waiting {
                            elapsed_ms: now.saturating_sub(since),
                            total_ms: stable_ms,
                        },
                    );
                }
            } else {
                // New or changed pixels: (re)start the stability window. A
                // zero-length window fires on the detecting pass itself.
                entry.pending = Some(PendingChange {
                    hash,
                    since_ms: now,
                    rect: block.clone(),
                    text: text.clone(),
                    translation: fused_translation.clone(),
                });
                if stable_ms == 0 {
                    entry.pending = None;
                    if let Some(pair) = self
                        .fire_read(
                            entry,
                            zone.region,
                            &block,
                            &text,
                            hash,
                            fused_translation.as_deref(),
                            &origin,
                            &label,
                            &image,
                            &recent,
                            false,
                            &mut timings,
                        )
                        .await
                    {
                        record_recent(&mut state.recent, context_window, pair);
                    }
                } else {
                    // The window just started for these pixels — elapsed 0
                    // of stable_ms. The local `text` moves into the trace;
                    // the pending already holds its own clone.
                    timings.trace(
                        text,
                        ReadDisposition::Waiting {
                            elapsed_ms: 0,
                            total_ms: stable_ms,
                        },
                    );
                }
            }
        }

        // Blocks seen before but not in this pass are candidates for removal
        // (gone from the screen). Only computed after a successful OCR pass
        // — a failed capture/OCR keeps whatever was last seen (nothing
        // provably disappeared). Two consecutive passes must miss a block
        // before it is reported: Windows OCR intermittently returns empty
        // results on a healthy image, and a one-tick glitch must not clear
        // a visible translation. A pending change dies with its block, so a
        // block that flickers away can still fire if it comes back.
        let mut gone: Vec<(i32, i32)> = Vec::new();
        for (key, entry) in &mut state.blocks {
            if seen.contains(key) {
                continue;
            }
            entry.misses += 1;
            timings.trace(
                entry.last_known_text(),
                ReadDisposition::Gone { misses: entry.misses },
            );
            if entry.misses >= REMOVAL_MISSES {
                gone.push(*key);
            }
        }
        self.report_removals(&mut state, gone, &origin);

        state.last_frame = Some(frame);
        self.emit_timing(origin, timings, started, self.fused.is_none());
        state
    }

    /// Fires every pending change whose stability window has elapsed, on a
    /// tick whose capture matched the last OCR'd frame. Only pendings that
    /// frame actually saw can fire (misses == 0): a block it no longer
    /// shows is not on screen, whatever its pending says. The cached
    /// rect/text are exactly what that frame read, so the fire is current —
    /// and Auto Colors samples from that same frame. Fired engine time
    /// accumulates into the pass's timings (ticket 47).
    async fn fire_matured(
        &self,
        state: &mut ZoneState,
        region: Region,
        origin: &EventOrigin,
        label: &str,
        image: &CapturedImage,
        timings: &mut PassTimings,
    ) {
        let now = self.clock.now_ms();
        let stable_ms = self.config.stability.as_millis() as u64;
        // Translation Context (ticket 44): the live window for this pass.
        let context_window = self.context_window();
        let matured: Vec<(i32, i32)> = state
            .blocks
            .iter()
            .filter(|(_, entry)| {
                entry.misses == 0
                    && entry
                        .pending
                        .as_ref()
                        .is_some_and(|p| now.saturating_sub(p.since_ms) >= stable_ms)
            })
            .map(|(key, _)| *key)
            .collect();
        for key in matured {
            // Ticket 44: snapshot after the previous matured fire recorded,
            // so chains built on the fast path continue line by line.
            let recent = recent_context(state, context_window);
            let Some(entry) = state.blocks.get_mut(&key) else {
                continue; // unreachable: the key came from this very map
            };
            let Some(pending) = entry.pending.take() else {
                continue; // unreachable: maturity was checked above
            };
            if let Some(pair) = self
                .fire_read(
                    entry,
                    region,
                    &pending.rect,
                    &pending.text,
                    pending.hash,
                    pending.translation.as_deref(),
                    origin,
                    label,
                    image,
                    &recent,
                    // The read is the pending's cached one — no OCR ran on
                    // this fast-path tick (ticket 48's translated-cached).
                    true,
                    timings,
                )
                .await
            {
                record_recent(&mut state.recent, context_window, pair);
            }
        }
    }

    /// Advances removal counting on a fast-path tick: the frame is
    /// pixel-identical to the last OCR'd one, so every block that frame
    /// already missed is missed again — a static screen cannot resurrect a
    /// block, and its stale pill must still clear.
    fn advance_removals(&self, state: &mut ZoneState, origin: &EventOrigin) {
        let mut gone: Vec<(i32, i32)> = Vec::new();
        for (key, entry) in &mut state.blocks {
            if entry.misses == 0 {
                continue;
            }
            entry.misses += 1;
            if entry.misses >= REMOVAL_MISSES {
                gone.push(*key);
            }
        }
        self.report_removals(state, gone, origin);
    }

    /// Drops the removed blocks' tracking state and reports them to the
    /// display. Dropping the state means the same pixels re-appearing later
    /// are translated again instead of being silently deduped.
    fn report_removals(&self, state: &mut ZoneState, gone: Vec<(i32, i32)>, origin: &EventOrigin) {
        if gone.is_empty() {
            return;
        }
        for key in &gone {
            state.blocks.remove(key);
        }
        let removed: Vec<String> = gone.iter().map(|&(gx, gy)| format!("{gx},{gy}")).collect();
        self.events
            .send(LoopEvent::BlocksRemoved {
                origin: origin.clone(),
                keys: removed,
                at_ms: self.clock.now_ms(),
            })
            .ok();
    }

    /// The prompt guidance for a fused call (ticket 12): ALL Glossary
    /// entries — a fused call sees the image, not one block's text, so
    /// there is nothing to filter entries by — plus the Genderless flag,
    /// the screen-reading prompt override (ticket 37), the preset both
    /// ride under (ticket 42), and the zone's Translation Context (ticket
    /// 44, empty for a one-shot pass).
    fn fused_guidance(
        &self,
        recent: Vec<(String, String)>,
    ) -> crate::ports::TranslationGuidance {
        crate::ports::TranslationGuidance {
            glossary: self.glossary.entries(),
            genderless: self.config.genderless,
            custom_base: self.config.fused_prompt_override.clone(),
            preset: self.config.prompt_preset.clone(),
            recent_translations: recent,
        }
    }

    /// Auto Colors (ticket 14): the per-block style override for a fired
    /// read, sampled from the very frame the block's pixels were checked
    /// on. `None` unless Auto Colors is on and the sample succeeded.
    fn sampled_style(&self, image: &CapturedImage, rect: &TextBlock) -> Option<SampledStyle> {
        if !self.config.auto_colors {
            return None;
        }
        sample_block_style(image, block_region(rect)).map(|block| SampledStyle::from_block(&block))
    }

    /// Sends one resolved translation event — the shared tail of every
    /// display path (glossary short-circuit, engine translate, fused
    /// read). `rect` is region-relative; the event carries screen
    /// coordinates.
    fn emit_translated(
        &self,
        region: Region,
        rect: &TextBlock,
        translation: String,
        via: Via,
        origin: &EventOrigin,
        style: Option<SampledStyle>,
    ) {
        self.events
            .send(LoopEvent::Translated {
                // The region-relative key, exactly what the dedup state and
                // BlocksRemoved use — the overlay's element identity must
                // match both.
                key: block_key_string(rect),
                block: screen_block(rect, region),
                translation,
                via,
                at_ms: self.clock.now_ms(),
                origin: origin.clone(),
                style,
            })
            .ok();
    }

    /// Fires one read through the block's tracking state: a fused read
    /// displays the translation the model already answered (ticket 12); a
    /// classic OCR read asks the Translate Engine
    /// ([`Self::translate_and_emit`]). Ticket 22: this is the shared fire
    /// path of both reading modes and the one place the dedup record is
    /// written. A read whose trimmed text equals the block's last SHOWN
    /// text is dropped before the engine is called or the fused answer is
    /// emitted — the In-place Overlay already displays that translation,
    /// and pixel jitter around unchanged glyphs must not re-request it.
    /// Otherwise `fired_hash` advances to `hash` unconditionally (a failed
    /// read still counts as handled, so a dead engine is not hammered
    /// every tick), while `last_fired_text` records the text only when a
    /// translation was actually shown — keeping "retried when the pixels
    /// change again" true for failures.
    ///
    /// Returns the `(source, shown Thai)` pair on an actual emission —
    /// exactly what Translation Context (ticket 44) records — and `None`
    /// for a suppressed read or a failed translation, which must leave the
    /// zone's buffer alone. Pairs are flattened onto one line: the prompt
    /// section renders one `- source → translation` line per pair, and a
    /// multi-line answer would break that shape. Actual emissions count
    /// into the pass's timings (ticket 47). Every decision here pushes its
    /// read-trace entry (ticket 48): a suppressed read names the text that
    /// matched — today's most silent path — and a shown read names its
    /// route (`cached` marks the fast-path matured fires, whose words are
    /// the pending's cached read rather than this pass's OCR). A FAILED
    /// translation pushes no entry — the `Error` event naming the failure
    /// rides the same stream.
    #[allow(clippy::too_many_arguments)]
    async fn fire_read(
        &self,
        entry: &mut BlockState,
        region: Region,
        rect: &TextBlock,
        text: &str,
        hash: u64,
        fused_translation: Option<&str>,
        origin: &EventOrigin,
        label: &str,
        image: &CapturedImage,
        recent: &[(String, String)],
        cached: bool,
        timings: &mut PassTimings,
    ) -> Option<(String, String)> {
        if entry.last_fired_text.as_deref() == Some(text) {
            // Suppressed: nothing shown, nothing recorded. The trace's
            // words are a copy — `last_fired_text` keeps the block's own.
            timings.trace(text.to_string(), ReadDisposition::Suppressed);
            return None;
        }
        let style = self.sampled_style(image, rect);
        let shown = match fused_translation {
            Some(thai) => Some((
                self.emit_fused_block(region, rect, thai, origin, style).await,
                ReadDisposition::TranslatedFused,
            )),
            None => {
                self.translate_and_emit(
                    region,
                    rect,
                    text,
                    origin,
                    label,
                    recent,
                    style,
                    timings,
                )
                .await
                .map(|(thai, via)| (thai, ReadDisposition::from(via)))
            }
        };
        entry.fired_hash = Some(hash);
        let (thai, disposition) = shown?;
        timings.blocks_translated += 1;
        entry.last_fired_text = Some(text.to_string());
        timings.trace(
            text.to_string(),
            if cached {
                ReadDisposition::TranslatedCached
            } else {
                disposition
            },
        );
        Some((one_line(text), one_line(&thai)))
    }

    /// Emits one fused read: the model's translation with the same output
    /// rules as the engine path — a whole-block Glossary match pins the
    /// displayed Thai (the model got every entry injected, but the pinned
    /// value is the deterministic answer), and the Genderless scrub runs
    /// belt-and-braces over the model's own text. Returns the shown Thai —
    /// what Translation Context (ticket 44) records.
    async fn emit_fused_block(
        &self,
        region: Region,
        rect: &TextBlock,
        thai: &str,
        origin: &EventOrigin,
        style: Option<SampledStyle>,
    ) -> String {
        let (translation, via) = match self.glossary.lookup(&rect.text) {
            Some(pinned) => (pinned, Via::Glossary),
            None => {
                let thai = if self.config.genderless {
                    crate::genderless::scrub(thai)
                } else {
                    thai.to_string()
                };
                (thai, Via::Engine)
            }
        };
        self.emit_translated(region, rect, translation.clone(), via, origin, style);
        translation
    }

    /// Glossary short-circuit, engine translate, and the resulting event
    /// for one block read. `rect` is region-relative; the emitted block
    /// carries screen coordinates. Returns the shown Thai with the route
    /// that produced it (`Via`) — the pair Translation Context (ticket 44)
    /// records, plus the trace's translated-engine/glossary split (ticket
    /// 48) — or `None` on engine failure, so the caller records no shown
    /// text and the read is retried when the pixels change again. A failed
    /// translation still counts as handled (the caller records the change)
    /// so a dead engine is not hammered every tick. The whole
    /// translation-production time (Glossary lookup, engine call, scrub)
    /// accumulates into the pass's translate stage (ticket 47), and
    /// entering here IS the stage running — the one place both the
    /// accumulator and the stage-ran flag (ticket 50) are set, so a pass
    /// that never fired reports no translate stage at all.
    #[allow(clippy::too_many_arguments)]
    async fn translate_and_emit(
        &self,
        region: Region,
        rect: &TextBlock,
        text: &str,
        origin: &EventOrigin,
        label: &str,
        recent: &[(String, String)],
        style: Option<SampledStyle>,
        timings: &mut PassTimings,
    ) -> Option<(String, Via)> {
        timings.translate_ran = true;
        let translate_started = self.clock.now_ms();
        let translated = match self.glossary.lookup(text) {
            Some(thai) => Ok((thai, Via::Glossary)),
            None => {
                // LLM guidance (ticket 11): the Glossary entries whose term
                // occurs in this block ride in the prompt so in-sentence
                // names translate per the Glossary, and Genderless Mode
                // rides as a prompt instruction for LLM engines. Non-LLM
                // engines discard the guidance (the port's default) and
                // keep relying on the output scrub below. The custom
                // translation-prompt override (ticket 37) rides the same
                // way — non-LLM engines never see prompts at all. The
                // preset the override is the wording of (ticket 42) rides
                // the same way, as does the zone's Translation Context
                // (ticket 44).
                let guidance = crate::ports::TranslationGuidance {
                    glossary: self.glossary.relevant_entries(text),
                    genderless: self.config.genderless,
                    custom_base: self.config.translation_prompt_override.clone(),
                    preset: self.config.prompt_preset.clone(),
                    recent_translations: recent.to_vec(),
                };
                self.translate
                    .translate_guided(
                        text,
                        self.config.source_language,
                        crate::ports::TARGET_LANGUAGE,
                        &guidance,
                    )
                    .await
                    .map(|thai| {
                        // Genderless Mode (ticket 08): engine output is
                        // scrubbed before display; the glossary path above
                        // short-circuits past this — a pinned translation is
                        // the user's own Thai, never rewritten. The prompt
                        // instruction (above) is the LLM mechanism; this
                        // rule-based scrub stays as the belt-and-braces
                        // guarantee for every engine (it is a no-op on
                        // already-genderless text).
                        let thai = if self.config.genderless {
                            crate::genderless::scrub(&thai)
                        } else {
                            thai
                        };
                        (thai, Via::Engine)
                    })
                    .map_err(|e| e.message)
            }
        };
        timings.translate_ms += self.clock.now_ms().saturating_sub(translate_started);
        match translated {
            Ok((translation, via)) => {
                self.emit_translated(region, rect, translation.clone(), via, origin, style);
                Some((translation, via))
            }
            Err(message) => {
                self.emit(Stage::Translate, format!("{label}: {message}"));
                None
            }
        }
    }

    /// Runs the loop forever, pacing ticks with the clock and holding still
    /// (without losing tracking state) while the user paused it or while
    /// the shell suspended it for the Capture Zone picker (ticket 46). The
    /// caller stops it by aborting the task.
    pub async fn run(mut self) {
        loop {
            if self.held() && self.wait_while_held().await.is_err() {
                return; // a sender dropped: the shell is gone
            }
            self.tick().await;
            self.clock.sleep(self.config.cadence).await;
        }
    }

    /// Whether either hold flag is in force right now: the user pause, or
    /// the picker suspension (ticket 46).
    fn held(&self) -> bool {
        *self.paused_rx.borrow() || *self.suspended_rx.borrow()
    }

    /// Waits until neither hold flag is in force — the same stillness the
    /// user pause provides, shared with the picker suspension. Each release
    /// re-checks BOTH flags: a pause set while the suspension released (or
    /// vice versa) keeps the loop held. Sender dropped = the shell is gone.
    async fn wait_while_held(&mut self) -> Result<(), tokio::sync::watch::error::RecvError> {
        loop {
            let paused = *self.paused_rx.borrow();
            let suspended = *self.suspended_rx.borrow();
            if !paused && !suspended {
                return Ok(());
            }
            tokio::select! {
                res = self.paused_rx.wait_for(|paused| !*paused), if paused => { res?; }
                res = self.suspended_rx.wait_for(|s| !*s), if suspended => { res?; }
                else => return Ok(()),
            }
        }
    }

    fn emit(&self, stage: Stage, message: String) {
        self.events
            .send(LoopEvent::Error {
                stage,
                message,
                at_ms: self.clock.now_ms(),
            })
            .ok();
    }

    /// Ships one pass's [`PassTimings`] as a [`LoopEvent::Timing`]
    /// (tickets 47/48). `total_ms` is measured pass start → pass end, so
    /// the stage durations plus the pass's own hashing and bookkeeping sum
    /// to it within rounding. The translate stage's presence needs both
    /// conditions (ticket 50): `classic` — on the fused path the stage
    /// never runs, the translation rides the fused call's answer — and
    /// `translate_ran` — a classic pass where no fire was attempted never
    /// entered it either. Both report `None`; a Glossary-pinned fire's
    /// `Some(0)` is the honest zero.
    fn emit_timing(
        &self,
        origin: EventOrigin,
        timings: PassTimings,
        started_ms: u64,
        classic: bool,
    ) {
        let now = self.clock.now_ms();
        self.events
            .send(LoopEvent::Timing {
                origin,
                capture_ms: timings.capture_ms,
                ocr_ms: timings.ocr_ms,
                translate_ms: (classic && timings.translate_ran).then_some(timings.translate_ms),
                fused_ms: timings.fused_ms,
                total_ms: now.saturating_sub(started_ms),
                blocks_translated: timings.blocks_translated,
                read_skipped: timings.read_skipped,
                reads: timings.reads,
                at_ms: now,
            })
            .ok();
    }
}

/// OCR boxes are relative to the captured region; the display layer needs
/// screen coordinates (positioning and exclusions work on the screen).
fn screen_block(block: &TextBlock, region: Region) -> TextBlock {
    TextBlock {
        x: block.x + region.x,
        y: block.y + region.y,
        width: block.width,
        height: block.height,
        text: block.text.clone(),
    }
}

/// Flattens text onto one line — the Translation Context record's shape
/// (ticket 44): the prompt section renders one `- source → translation`
/// line per pair, and a multi-line source or answer would break that
/// shape. Line breaks and their surrounding whitespace collapse to one
/// space; empty segments drop.
fn one_line(text: &str) -> String {
    text.split(['\r', '\n'])
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// The system clock as a [`Clock`] port implementation.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemClock;

#[async_trait]
impl Clock for SystemClock {
    async fn sleep(&self, duration: Duration) {
        tokio::time::sleep(duration).await;
    }

    fn now_ms(&self) -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0)
    }
}
