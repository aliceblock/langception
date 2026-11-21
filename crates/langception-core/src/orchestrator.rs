//! The translation loop: capture → OCR → per-block dedup → glossary
//! short-circuit → translate → events, across every active Capture Zone.
//!
//! All collaborators are ports (`capture`, `ocr`, `translate`, `clock`), so
//! the whole pipeline is exercised with fakes at this seam — see
//! `tests/orchestrator_tests.rs`. The zone list and the paused flag arrive
//! through watch channels so the shell can change them while the loop runs
//! without losing per-block dedup state.

use crate::glossary::Glossary;
use crate::ports::{
    CaptureSource, Clock, OcrEngine, Region, SourceLanguage, TextBlock, TranslateEngine, Zone,
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
/// One-shot Capture (a single pass that creates no zone and has no dedup).
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

/// The loop's fixed inputs: in which language to read, and how often.
/// The watched regions (Capture Zones) and the paused flag are dynamic and
/// arrive via watch channels.
#[derive(Debug, Clone)]
pub struct LoopConfig {
    pub source_language: SourceLanguage,
    pub cadence: Duration,
}

/// A Text Block is identified by its on-screen position, quantized so
/// sub-pixel OCR jitter does not count as a "new" block. Ticket 05 replaces
/// this text-hash dedup with per-block image hashing.
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
    events: mpsc::UnboundedSender<LoopEvent>,
    /// Zone id → (block key → last translated (trimmed) text). Unchanged
    /// text is never re-translated; a failed block is also recorded so a
    /// dead engine is not hammered every tick (retried when the text changes
    /// or the zone is redrawn). State for removed zones is pruned per tick.
    translated: HashMap<String, HashMap<(i32, i32), String>>,
    /// Zone id → block keys that vanished from the latest pass, with how
    /// many consecutive passes have missed them. Windows OCR intermittently
    /// returns empty results on a healthy image, so a block is only reported
    /// as removed ([`LoopEvent::BlocksRemoved`]) after TWO consecutive
    /// misses — a one-tick glitch must not clear a visible translation.
    misses: HashMap<String, HashMap<(i32, i32), u32>>,
}

impl Orchestrator {
    pub fn new(
        capture: Arc<dyn CaptureSource>,
        ocr: Arc<dyn OcrEngine>,
        translate: Arc<dyn TranslateEngine>,
        clock: Arc<dyn Clock>,
        config: LoopConfig,
        glossary: Glossary,
        zones_rx: watch::Receiver<Vec<Zone>>,
        paused_rx: watch::Receiver<bool>,
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
            events,
            translated: HashMap::new(),
            misses: HashMap::new(),
        }
    }

    /// One pipeline pass over every active zone: capture, OCR, dedup,
    /// glossary short-circuit, translate, emit. Zones are processed in
    /// order, each with its own tracking state. Never panics; every failure
    /// becomes an [`LoopEvent::Error`] — the app must never fail silently.
    pub async fn tick(&mut self) {
        let zones = self.zones_rx.borrow().clone();
        if zones.is_empty() {
            self.translated.clear(); // no zones → no state worth keeping
            self.misses.clear();
            return;
        }
        for zone in &zones {
            let state = self.translated.remove(&zone.id).unwrap_or_default();
            let origin = EventOrigin::Zone {
                id: zone.id.clone(),
                name: zone.name.clone(),
            };
            let misses = self.misses.remove(&zone.id).unwrap_or_default();
            let (state, misses) = self
                .process_region(
                    &zone.region,
                    state,
                    misses,
                    origin,
                    &format!("zone \"{}\"", zone.name),
                )
                .await;
            self.translated.insert(zone.id.clone(), state);
            self.misses.insert(zone.id.clone(), misses);
        }
        // Drop tracking state of zones that no longer exist, so a redrawn
        // zone (same id pattern, new id) starts fresh.
        self.translated
            .retain(|id, _| zones.iter().any(|z| &z.id == id));
        self.misses
            .retain(|id, _| zones.iter().any(|z| &z.id == id));
    }

    /// One OCR+translate pass over `region` — the One-shot Capture: events
    /// carry [`EventOrigin::OneShot`], no tracking state is consulted or
    /// retained, and the running loop's state is untouched.
    pub async fn capture_once(&self, region: Region) {
        let _ = self
            .process_region(
                &region,
                HashMap::new(),
                HashMap::new(),
                EventOrigin::OneShot,
                "One-shot capture",
            )
            .await;
    }

    /// The pipeline for one screen region. `state` is the region's block-key
    /// → last-text map and `misses` its consecutive-miss counts (both empty
    /// for a fresh region); the updated pair is returned. `label` names the
    /// region in error events.
    async fn process_region(
        &self,
        region: &Region,
        mut state: HashMap<(i32, i32), String>,
        mut misses: HashMap<(i32, i32), u32>,
        origin: EventOrigin,
        label: &str,
    ) -> (HashMap<(i32, i32), String>, HashMap<(i32, i32), u32>) {
        let image = match self.capture.capture(*region).await {
            Ok(image) => image,
            Err(e) => {
                self.emit(Stage::Capture, format!("{label}: {e}"));
                return (state, misses);
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
                return (state, misses);
            }
        };

        let mut seen: HashSet<(i32, i32)> = HashSet::new();
        for block in blocks {
            let text = block.text.trim().to_string();
            if text.is_empty() {
                continue;
            }
            let key = block_key(&block);
            seen.insert(key);
            if state.get(&key).is_some_and(|last| *last == text) {
                continue; // unchanged block: never re-translate
            }

            // OCR boxes are relative to the captured region; report screen
            // coordinates so display modes can position on screen.
            let screen_block = TextBlock {
                x: block.x + region.x,
                y: block.y + region.y,
                width: block.width,
                height: block.height,
                text: block.text.clone(),
            };

            let translated = match self.glossary.lookup(&text) {
                Some(thai) => Ok((thai, Via::Glossary)),
                None => self
                    .translate
                    .translate(
                        &text,
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
                            // The region-relative key, exactly what the
                            // dedup state and BlocksRemoved use — the
                            // overlay's element identity must match both.
                            key: block_key_string(&block),
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
            state.insert(key, text);
        }

        // Blocks seen before but not in this pass are candidates for removal
        // (gone from the screen). Only computed after a successful OCR pass
        // — a failed capture/OCR keeps whatever was last seen (nothing
        // provably disappeared). Two consecutive passes must miss a block
        // before it is reported: Windows OCR intermittently returns empty
        // results on a healthy image, and a one-tick glitch must not clear
        // a visible translation.
        // Keys that came back lose their miss count; still-missing keys keep counting.
        misses.retain(|key, _| !seen.contains(key));
        let mut gone: Vec<(i32, i32)> = Vec::new();
        for key in state.keys() {
            if seen.contains(key) {
                continue;
            }
            let count = misses.entry(*key).or_insert(0);
            *count += 1;
            if *count >= REMOVAL_MISSES {
                gone.push(*key);
            }
        }
        let removed: Vec<String> = gone.iter().map(|&(gx, gy)| format!("{gx},{gy}")).collect();
        for key in &gone {
            misses.remove(key);
        }
        if !removed.is_empty() {
            self.events
                .send(LoopEvent::BlocksRemoved {
                    origin,
                    keys: removed,
                    at_ms: self.clock.now_ms(),
                })
                .ok();
        }
        (state, misses)
    }

    /// Runs the loop forever, pacing ticks with the clock and holding still
    /// (without losing tracking state) while paused. The caller stops it by
    /// aborting the task.
    pub async fn run(mut self) {
        loop {
            if *self.paused_rx.borrow() {
                if self.paused_rx.wait_for(|paused| !*paused).await.is_err() {
                    return; // sender dropped: the shell is gone
                }
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
