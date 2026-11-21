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
use std::collections::HashMap;
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

/// Everything the Results Window needs, streamed per pipeline tick.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum LoopEvent {
    Translated {
        block: TextBlock,
        translation: String,
        via: Via,
        at_ms: u64,
        origin: EventOrigin,
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

fn block_key(block: &TextBlock) -> (i32, i32) {
    (block.x / BLOCK_GRID_PX, block.y / BLOCK_GRID_PX)
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
            return;
        }
        for zone in &zones {
            let state = self.translated.remove(&zone.id).unwrap_or_default();
            let origin = EventOrigin::Zone {
                id: zone.id.clone(),
                name: zone.name.clone(),
            };
            let state = self
                .process_region(&zone.region, state, origin, &format!("zone \"{}\"", zone.name))
                .await;
            self.translated.insert(zone.id.clone(), state);
        }
        // Drop tracking state of zones that no longer exist, so a redrawn
        // zone (same id pattern, new id) starts fresh.
        self.translated.retain(|id, _| zones.iter().any(|z| &z.id == id));
    }

    /// One OCR+translate pass over `region` — the One-shot Capture: events
    /// carry [`EventOrigin::OneShot`], no tracking state is consulted or
    /// retained, and the running loop's state is untouched.
    pub async fn capture_once(&self, region: Region) {
        let _ = self
            .process_region(&region, HashMap::new(), EventOrigin::OneShot, "One-shot capture")
            .await;
    }

    /// The pipeline for one screen region. `state` is the region's block-key
    /// → last-text map (empty for a fresh region); the updated map is
    /// returned. `label` names the region in error events.
    async fn process_region(
        &self,
        region: &Region,
        mut state: HashMap<(i32, i32), String>,
        origin: EventOrigin,
        label: &str,
    ) -> HashMap<(i32, i32), String> {
        let image = match self.capture.capture(*region).await {
            Ok(image) => image,
            Err(e) => {
                self.emit(Stage::Capture, format!("{label}: {e}"));
                return state;
            }
        };

        let blocks = match self.ocr.recognize(&image, self.config.source_language).await {
            Ok(blocks) => blocks,
            Err(e) => {
                self.emit(Stage::Ocr, format!("{label}: {e}"));
                return state;
            }
        };

        for block in blocks {
            let text = block.text.trim().to_string();
            if text.is_empty() {
                continue;
            }
            let key = block_key(&block);
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
        state
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
