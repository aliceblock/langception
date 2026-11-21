//! Ports: the seams the translation loop depends on. Windows adapters and the
//! Tauri shell implement these; all pipeline behavior is tested against them
//! with fakes.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use thiserror::Error;

/// A rectangular screen region in physical pixels (origin may be negative on
/// multi-monitor setups — coordinates are relative to the primary monitor's
/// top-left).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Region {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

/// A captured screen region as 8-bit BGRA, top-down rows, stride `width * 4`.
#[derive(Debug, Clone)]
pub struct CapturedImage {
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
}

/// A Capture Zone (CONTEXT.md): a user-drawn rectangular screen region
/// captured and translated repeatedly. Several zones are watched at once;
/// each tracks its Text Blocks independently. The `id` is stable across
/// renames and restarts so the loop can keep per-zone tracking state.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Zone {
    pub id: String,
    pub name: String,
    pub region: Region,
}

/// One contiguous run of text found by an OCR engine, with its bounding box.
/// The unit of tracking, display, and re-translation.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TextBlock {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub text: String,
}

/// The language Langception expects to read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SourceLanguage {
    En,
    Ja,
    Zh,
    Ko,
}

impl SourceLanguage {
    /// Language code used by translate engines.
    pub fn code(self) -> &'static str {
        match self {
            SourceLanguage::En => "en",
            SourceLanguage::Ja => "ja",
            SourceLanguage::Zh => "zh",
            SourceLanguage::Ko => "ko",
        }
    }
}

/// The only target language in scope for the MVP.
pub const TARGET_LANGUAGE: &str = "th";

#[derive(Debug, Clone, Error)]
#[error("{message}")]
pub struct CaptureError {
    pub message: String,
}

#[derive(Debug, Clone, Error)]
#[error("{message}")]
pub struct OcrError {
    pub message: String,
}

#[derive(Debug, Clone, Error)]
#[error("{message}")]
pub struct TranslateError {
    pub message: String,
}

macro_rules! error_new {
    ($($name:ident),*) => {
        $(impl $name {
            pub fn new(message: impl Into<String>) -> Self {
                Self { message: message.into() }
            }
        })*
    };
}
error_new!(CaptureError, OcrError, TranslateError);

/// What the app watches: here, a fixed configured region.
#[async_trait]
pub trait CaptureSource: Send + Sync {
    async fn capture(&self, region: Region) -> Result<CapturedImage, CaptureError>;
}

/// A pluggable component that turns a captured image region into Text Blocks.
#[async_trait]
pub trait OcrEngine: Send + Sync {
    async fn recognize(
        &self,
        image: &CapturedImage,
        source: SourceLanguage,
    ) -> Result<Vec<TextBlock>, OcrError>;
}

/// A pluggable component that translates text from a Source Language to a target.
#[async_trait]
pub trait TranslateEngine: Send + Sync {
    async fn translate(
        &self,
        text: &str,
        source: SourceLanguage,
        target: &str,
    ) -> Result<String, TranslateError>;
}

/// Time abstraction for the loop: pacing between ticks and event timestamps.
#[async_trait]
pub trait Clock: Send + Sync {
    async fn sleep(&self, duration: Duration);
    fn now_ms(&self) -> u64;
}
