//! The translation loop: capture → change detection → OCR → Excluded Block
//! filter → per-block stability window → glossary short-circuit → translate
//! → events, across every active Capture Zone.
//!
//! All collaborators are ports (`capture`, `ocr`, `translate`, `clock`) plus
//! the watched zone list, the paused flag, and the active Excluded Blocks,
//! which arrive through watch channels so the shell can change them while
//! the loop runs without losing per-block tracking state.
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

use crate::glossary::Glossary;
use crate::hash::{block_dhash, frame_checksum};
use crate::ports::{
    CaptureSource, Clock, ExcludedBlock, OcrEngine, Region, SourceLanguage, TextBlock,
    TranslateEngine, Zone,
};
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
}

/// Where a pipeline pass came from: one of the active Capture Zones, or a
/// One-shot Capture (a single pass that creates no zone and has no tracking).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EventOrigin {
    Zone { id: String, name: String },
    OneShot,
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
}

/// A zone's tracking state. `last_frame` is the checksum of the last OCR'd
/// frame; a tick whose capture checksums identically skips OCR and only
/// matures pendings and advances removals.
#[derive(Debug)]
struct ZoneState {
    region: Region,
    last_frame: Option<u64>,
    blocks: HashMap<(i32, i32), BlockState>,
}

impl ZoneState {
    fn fresh(region: Region) -> Self {
        Self {
            region,
            last_frame: None,
            blocks: HashMap::new(),
        }
    }
}

/// Runs the Auto-translate loop against injected ports and streams
/// [`LoopEvent`]s to the display.
pub struct Orchestrator {
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
            clock,
            config,
            glossary,
            zones_rx,
            paused_rx,
            excluded_rx,
            events,
            zones: HashMap::new(),
            exclusions_seen: Vec::new(),
        }
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
    /// untouched.
    pub async fn capture_once(&self, region: Region) {
        let origin = EventOrigin::OneShot;
        let label = "One-shot capture";
        let image = match self.capture.capture(region).await {
            Ok(image) => image,
            Err(e) => {
                self.emit(Stage::Capture, format!("{label}: {e}"));
                return;
            }
        };
        let blocks = match self
            .ocr
            .recognize(&image, self.config.source_language)
            .await
        {
            Ok(blocks) => blocks,
            Err(e) => {
                self.emit(Stage::Ocr, format!("{label}: {e}"));
                return;
            }
        };
        let exclusions = self.excluded_rx.borrow().clone();
        for block in blocks {
            let text = block.text.trim().to_string();
            if text.is_empty() {
                continue;
            }
            let screen_block = screen_block(&block, region);
            if exclusions.iter().any(|e| e.covers(&screen_block)) {
                continue;
            }
            self.translate_and_emit(region, &block, &text, &origin, label)
                .await;
        }
    }

    /// One pass over one zone. Fast path: the capture is pixel-identical to
    /// the last OCR'd frame → matured pendings fire with their cached read,
    /// blocks that frame already missed keep missing, and OCR never runs.
    /// Otherwise OCR runs and every Text Block goes through the dHash
    /// pipeline. Never panics; every failure becomes a [`LoopEvent::Error`].
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
        let image = match self.capture.capture(zone.region).await {
            Ok(image) => image,
            Err(e) => {
                self.emit(Stage::Capture, format!("{label}: {e}"));
                return state;
            }
        };

        let frame = frame_checksum(&image);
        if state.last_frame == Some(frame) {
            self.fire_matured(&mut state, zone.region, &origin, &label)
                .await;
            self.advance_removals(&mut state, &origin);
            return state;
        }

        let blocks = match self
            .ocr
            .recognize(&image, self.config.source_language)
            .await
        {
            Ok(blocks) => blocks,
            Err(e) => {
                self.emit(Stage::Ocr, format!("{label}: {e}"));
                // last_frame stays behind: the next pass retries OCR even
                // if the screen has not changed since.
                return state;
            }
        };

        let now = self.clock.now_ms();
        let stable_ms = self.config.stability.as_millis() as u64;
        let mut seen: HashSet<(i32, i32)> = HashSet::new();
        for block in blocks {
            let text = block.text.trim().to_string();
            if text.is_empty() {
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
                // exclusion translates it again on the next OCR pass.
                state.blocks.remove(&key);
                continue;
            }

            let hash = block_dhash(&image, quantized_region(&block));
            let entry = state.blocks.entry(key).or_default();
            entry.misses = 0;

            if entry.fired_hash == Some(hash) {
                // Pixels identical to what the current translation was shown
                // for: skip, even when this pass's OCR text jitters. A
                // pending change that reverted to the shown pixels is
                // cancelled too.
                entry.pending = None;
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
                    self.translate_and_emit(zone.region, &block, &text, &origin, &label)
                        .await;
                    entry.fired_hash = Some(hash);
                } else {
                    // ...otherwise keep waiting, refreshing the cached read
                    // to this frame — the newest one a fast-path fire could
                    // use.
                    let pending = entry.pending.as_mut().expect("pending hash matched above");
                    pending.rect = block.clone();
                    pending.text = text;
                }
            } else {
                // New or changed pixels: (re)start the stability window. A
                // zero-length window fires on the detecting pass itself.
                entry.pending = Some(PendingChange {
                    hash,
                    since_ms: now,
                    rect: block.clone(),
                    text: text.clone(),
                });
                if stable_ms == 0 {
                    entry.pending = None;
                    self.translate_and_emit(zone.region, &block, &text, &origin, &label)
                        .await;
                    entry.fired_hash = Some(hash);
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
            if entry.misses >= REMOVAL_MISSES {
                gone.push(*key);
            }
        }
        self.report_removals(&mut state, gone, &origin);

        state.last_frame = Some(frame);
        state
    }

    /// Fires every pending change whose stability window has elapsed, on a
    /// tick whose capture matched the last OCR'd frame. Only pendings that
    /// frame actually saw can fire (misses == 0): a block it no longer
    /// shows is not on screen, whatever its pending says. The cached
    /// rect/text are exactly what that frame read, so the fire is current.
    async fn fire_matured(
        &self,
        state: &mut ZoneState,
        region: Region,
        origin: &EventOrigin,
        label: &str,
    ) {
        let now = self.clock.now_ms();
        let stable_ms = self.config.stability.as_millis() as u64;
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
            let Some(entry) = state.blocks.get_mut(&key) else {
                continue; // unreachable: the key came from this very map
            };
            let Some(pending) = entry.pending.take() else {
                continue; // unreachable: maturity was checked above
            };
            self.translate_and_emit(region, &pending.rect, &pending.text, origin, label)
                .await;
            entry.fired_hash = Some(pending.hash);
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

    /// Glossary short-circuit, engine translate, and the resulting event
    /// for one block read. `rect` is region-relative; the emitted block
    /// carries screen coordinates. A failed translation still counts as
    /// handled (the caller records the change) so a dead engine is not
    /// hammered every tick — it is retried when the pixels change again.
    async fn translate_and_emit(
        &self,
        region: Region,
        rect: &TextBlock,
        text: &str,
        origin: &EventOrigin,
        label: &str,
    ) {
        let screen_block = screen_block(rect, region);
        let translated = match self.glossary.lookup(text) {
            Some(thai) => Ok((thai, Via::Glossary)),
            None => self
                .translate
                .translate(
                    text,
                    self.config.source_language,
                    crate::ports::TARGET_LANGUAGE,
                )
                .await
                .map(|thai| (thai, Via::Engine))
                .map_err(|e| e.message),
        };
        match translated {
            Ok((translation, via)) => {
                self.events
                    .send(LoopEvent::Translated {
                        // The region-relative key, exactly what the dedup
                        // state and BlocksRemoved use — the overlay's
                        // element identity must match both.
                        key: block_key_string(rect),
                        block: screen_block,
                        translation,
                        via,
                        at_ms: self.clock.now_ms(),
                        origin: origin.clone(),
                    })
                    .ok();
            }
            Err(message) => self.emit(Stage::Translate, format!("{label}: {message}")),
        }
    }

    /// Runs the loop forever, pacing ticks with the clock and holding still
    /// (without losing tracking state) while paused. The caller stops it by
    /// aborting the task.
    pub async fn run(mut self) {
        loop {
            if *self.paused_rx.borrow()
                && self
                    .paused_rx
                    .wait_for(|paused| !*paused)
                    .await
                    .is_err()
            {
                return; // sender dropped: the shell is gone
            }
            self.tick().await;
            self.clock.sleep(self.config.cadence).await;
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
