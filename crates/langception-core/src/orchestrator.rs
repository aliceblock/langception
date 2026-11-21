//! The translation loop: capture → OCR → per-block dedup → glossary
//! short-circuit → translate → events.
//!
//! All collaborators are ports (`capture`, `ocr`, `translate`, `clock`), so
//! the whole pipeline is exercised with fakes at this seam — see
//! `tests/orchestrator_tests.rs`.

use crate::glossary::Glossary;
use crate::ports::{CaptureSource, Clock, OcrEngine, Region, SourceLanguage, TextBlock, TranslateEngine};
use async_trait::async_trait;
use serde::Serialize;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

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

/// Everything the Results Window needs, streamed per pipeline tick.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum LoopEvent {
    Translated {
        block: TextBlock,
        translation: String,
        via: Via,
        at_ms: u64,
    },
    Error {
        stage: Stage,
        message: String,
        at_ms: u64,
    },
}

/// The loop's fixed inputs: what to watch, in which language, how often.
#[derive(Debug, Clone)]
pub struct LoopConfig {
    pub region: Region,
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
    events: mpsc::UnboundedSender<LoopEvent>,
    /// Block key → last translated (trimmed) text. Unchanged text is never
    /// re-translated; a failed block is also recorded so a dead engine is not
    /// hammered every tick (retried when the text changes or the loop restarts).
    translated: HashMap<(i32, i32), String>,
}

impl Orchestrator {
    pub fn new(
        capture: Arc<dyn CaptureSource>,
        ocr: Arc<dyn OcrEngine>,
        translate: Arc<dyn TranslateEngine>,
        clock: Arc<dyn Clock>,
        config: LoopConfig,
        glossary: Glossary,
        events: mpsc::UnboundedSender<LoopEvent>,
    ) -> Self {
        Self {
            capture,
            ocr,
            translate,
            clock,
            config,
            glossary,
            events,
            translated: HashMap::new(),
        }
    }

    /// One pipeline pass: capture, OCR, dedup, glossary short-circuit,
    /// translate, emit. Never panics; every failure becomes an
    /// [`LoopEvent::Error`] — the app must never fail silently.
    pub async fn tick(&mut self) {
        let image = match self.capture.capture(self.config.region).await {
            Ok(image) => image,
            Err(e) => return self.emit(Stage::Capture, e.message),
        };

        let blocks = match self.ocr.recognize(&image, self.config.source_language).await {
            Ok(blocks) => blocks,
            Err(e) => return self.emit(Stage::Ocr, e.message),
        };

        for block in blocks {
            let text = block.text.trim().to_string();
            if text.is_empty() {
                continue;
            }
            let key = block_key(&block);
            if self.translated.get(&key).is_some_and(|last| *last == text) {
                continue; // unchanged block: never re-translate
            }

            if let Some(thai) = self.glossary.lookup(&text) {
                self.events
                    .send(LoopEvent::Translated {
                        block,
                        translation: thai,
                        via: Via::Glossary,
                        at_ms: self.clock.now_ms(),
                    })
                    .ok();
            } else {
                match self
                    .translate
                    .translate(
                        &text,
                        self.config.source_language,
                        crate::ports::TARGET_LANGUAGE,
                    )
                    .await
                {
                    Ok(thai) => {
                        self.events
                            .send(LoopEvent::Translated {
                                block,
                                translation: thai,
                                via: Via::Engine,
                                at_ms: self.clock.now_ms(),
                            })
                            .ok();
                    }
                    Err(e) => self.emit(Stage::Translate, e.message),
                }
            }
            self.translated.insert(key, text);
        }
    }

    /// Runs the loop forever, pacing ticks with the clock. The caller stops
    /// it by aborting the task.
    pub async fn run(mut self) {
        loop {
            self.tick().await;
            self.clock.sleep(self.config.cadence).await;
        }
    }

    fn emit(&mut self, stage: Stage, message: String) {
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
