//! Windows OCR (`Windows.Media.Ocr`) — a thin adapter outside automated
//! tests (per the spec's testing decisions); exercised by the manual smoke
//! checklist.

use langception_core::ports::{CapturedImage, OcrEngine, OcrError, SourceLanguage, TextBlock};
use std::sync::{Arc, OnceLock};
use windows::Globalization::Language;
use windows::Graphics::Imaging::{BitmapPixelFormat, SoftwareBitmap};
use windows::Media::Ocr::OcrEngine as WinOcrEngine;
use windows::Security::Cryptography::CryptographicBuffer;

/// Recognizes text with the Windows OCR engine for one Source Language.
/// The underlying recognizer is created once (lazily) and reused.
pub struct WindowsOcrEngine {
    source: SourceLanguage,
    engine: Arc<OnceLock<Result<WinOcrEngine, String>>>,
}

impl WindowsOcrEngine {
    pub fn new(source: SourceLanguage) -> Self {
        Self {
            source,
            engine: Arc::new(OnceLock::new()),
        }
    }
}

#[async_trait::async_trait]
impl OcrEngine for WindowsOcrEngine {
    async fn recognize(
        &self,
        image: &CapturedImage,
        _source: SourceLanguage,
    ) -> Result<Vec<TextBlock>, OcrError> {
        let engine_cell = Arc::clone(&self.engine);
        let source = self.source;
        let (width, height) = (image.width, image.height);
        let bgra = image.bgra.clone();

        // WinRT calls need a COM-initialized thread; run them on the blocking
        // pool so the async runtime workers are never blocked or COM-bound.
        tauri::async_runtime::spawn_blocking(move || {
            let engine = resolve_engine(&engine_cell, source).map_err(OcrError::new)?;
            run_recognition(&engine, width, height, &bgra)
        })
        .await
        .map_err(|e| OcrError::new(format!("OCR task failed: {e}")))?
    }
}

fn resolve_engine(
    cell: &OnceLock<Result<WinOcrEngine, String>>,
    source: SourceLanguage,
) -> Result<WinOcrEngine, String> {
    let initialized = cell.get_or_init(|| init_ocr_engine(source.code()));
    match initialized {
        Ok(engine) => Ok(engine.clone()),
        Err(message) => Err(message.clone()),
    }
}

fn init_ocr_engine(code: &str) -> Result<WinOcrEngine, String> {
    init_com();

    // Primary-subtag match: config says "en"/"ja"/"zh"/"ko"; installed packs
    // use full tags like "en-US" or "zh-Hans".
    let primary = primary_subtag(code);
    let mut available: Vec<String> = Vec::new();
    match WinOcrEngine::AvailableRecognizerLanguages() {
        Ok(languages) => {
            let count = languages.Size().unwrap_or(0);
            for i in 0..count {
                if let Ok(lang) = languages.GetAt(i) {
                    if let Ok(tag) = lang.LanguageTag() {
                        available.push(tag.to_string());
                    }
                }
            }
        }
        Err(e) => return Err(format!("Windows OCR is unavailable: {e}")),
    }

    let mut last_error = format!(
        "No Windows OCR language pack for \"{code}\". Install it in Windows Settings → \
         Time & Language → Language & region, then stop/start translation."
    );

    // Exact tag first, then any installed pack with the same primary subtag.
    for tag in &available {
        if tag.eq_ignore_ascii_case(code) {
            if let Ok(engine) = try_create(tag) {
                return Ok(engine);
            }
        }
    }
    // Deterministic fallback order: for "zh" prefer Simplified (zh-Hans) over
    // Traditional (zh-Hant); otherwise installation order.
    let mut fallback: Vec<&String> = available
        .iter()
        .filter(|tag| primary_subtag(tag) == primary)
        .collect();
    fallback.sort_by_key(|tag| pack_preference(tag));
    for tag in &fallback {
        if let Ok(engine) = try_create(tag) {
            return Ok(engine);
        }
        last_error = format!("Windows OCR language pack \"{tag}\" could not be used: OCR engine creation failed");
    }

    Err(format!(
        "{last_error}. Installed OCR packs: {}. See the first-run wizard (ticket 06) for help.",
        if available.is_empty() {
            "none found".to_string()
        } else {
            available.join(", ")
        }
    ))
}

/// Creates one OCR engine for an installed pack tag. Shared with
/// `ocr_detect` (the wizard probes packs with the same call the pipeline
/// makes, so "installed" means "actually usable").
pub(crate) fn try_create(tag: &str) -> Result<WinOcrEngine, String> {
    let hstr = windows::core::HSTRING::from(tag);
    let language = Language::CreateLanguage(&hstr)
        .map_err(|e| format!("invalid language tag \"{tag}\": {e}"))?;
    WinOcrEngine::TryCreateFromLanguage(&language)
        .map_err(|e| format!("cannot create OCR engine for \"{tag}\": {e}"))
}

pub(crate) fn primary_subtag(tag: &str) -> String {
    tag.split(['-', '_']).next().unwrap_or(tag).to_ascii_lowercase()
}

/// The language tags of every installed OCR pack ("en-US", "ja-JP", …),
/// for diagnostics and the first-run wizard. Empty when the query fails or
/// COM is not initialized on the calling thread.
pub fn available_recognizer_languages() -> Vec<String> {
    init_com();
    let Ok(languages) = WinOcrEngine::AvailableRecognizerLanguages() else {
        return Vec::new();
    };
    let mut tags = Vec::new();
    if let Ok(count) = languages.Size() {
        for i in 0..count {
            if let Ok(lang) = languages.GetAt(i) {
                if let Ok(tag) = lang.LanguageTag() {
                    tags.push(tag.to_string());
                }
            }
        }
    }
    tags.sort();
    tags
}

/// Lower ranks first: Simplified Chinese packs before Traditional, then the rest.
pub(crate) fn pack_preference(tag: &str) -> u8 {
    let lower = tag.to_ascii_lowercase();
    if lower.contains("hans") {
        1
    } else if lower.contains("hant") {
        2
    } else {
        3
    }
}

fn run_recognition(
    engine: &WinOcrEngine,
    width: u32,
    height: u32,
    bgra: &[u8],
) -> Result<Vec<TextBlock>, OcrError> {
    let bitmap = software_bitmap_from_bgra(width, height, bgra)?;
    // `join()` blocks until the WinRT async operation completes. Windows OCR
    // is a local engine that always completes; slice 1 accepts the (never
    // observed) stall risk of no timeout here — revisit if OCR ever hangs.
    let result = engine
        .RecognizeAsync(&bitmap)
        .map_err(|e| OcrError::new(format!("Windows OCR failed to start: {e}")))?
        .join()
        .map_err(|e| OcrError::new(format!("Windows OCR failed: {e}")))?;

    let mut blocks = Vec::new();
    let lines = result
        .Lines()
        .map_err(|e| OcrError::new(format!("Windows OCR returned no lines: {e}")))?;
    let line_count = lines.Size().map_err(|e| OcrError::new(e.to_string()))?;
    for i in 0..line_count {
        let line = lines.GetAt(i).map_err(|e| OcrError::new(e.to_string()))?;
        let text = line
            .Text()
            .map_err(|e| OcrError::new(format!("Windows OCR line unreadable: {e}")))?
            .to_string();
        if text.trim().is_empty() {
            continue;
        }

        // A Text Block's bounding box is the union of its words' boxes.
        let words = line.Words().map_err(|e| OcrError::new(e.to_string()))?;
        let word_count = words.Size().map_err(|e| OcrError::new(e.to_string()))?;
        let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for j in 0..word_count {
            let word = words.GetAt(j).map_err(|e| OcrError::new(e.to_string()))?;
            let rect = word
                .BoundingRect()
                .map_err(|e| OcrError::new(e.to_string()))?;
            x0 = x0.min(rect.X);
            y0 = y0.min(rect.Y);
            x1 = x1.max(rect.X + rect.Width);
            y1 = y1.max(rect.Y + rect.Height);
        }
        if word_count == 0 {
            continue;
        }

        blocks.push(TextBlock {
            x: x0.round() as i32,
            y: y0.round() as i32,
            width: ((x1 - x0).round() as u32).max(1),
            height: ((y1 - y0).round() as u32).max(1),
            text,
        });
    }
    Ok(blocks)
}

fn software_bitmap_from_bgra(
    width: u32,
    height: u32,
    bgra: &[u8],
) -> Result<SoftwareBitmap, OcrError> {
    let buffer = CryptographicBuffer::CreateFromByteArray(bgra)
        .map_err(|e| OcrError::new(format!("failed to buffer captured pixels: {e}")))?;
    SoftwareBitmap::CreateCopyFromBuffer(
        &buffer,
        BitmapPixelFormat::Bgra8,
        width as i32,
        height as i32,
    )
    .map_err(|e| OcrError::new(format!("failed to build bitmap for OCR: {e}")))
}

/// Every WinRT entry point needs a COM-initialized thread. The blocking pool
/// reuses threads, so initialize once per thread (MTA: usable from any pool
/// thread; S_FALSE / RPC_E_CHANGED_MODE both mean "already initialized" and
/// are fine to ignore). Shared with `ocr_detect` (the wizard's pack probe),
/// which runs on the same kind of thread.
pub(crate) fn init_com() {
    thread_local! {
        static COM_INIT: std::sync::Once = const { std::sync::Once::new() };
    }
    COM_INIT.with(|cell| {
        cell.call_once(|| {
            use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};
            unsafe {
                let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            }
        })
    });
}
