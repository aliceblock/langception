//! Windows OCR language-pack detection for the first-run wizard and the
//! Engines tab (ticket 06). Windows.Media.Ocr silently produces nothing for
//! a language whose pack is not installed — "nothing translates" with no
//! visible cause — so the wizard probes exactly which of the four Source
//! Languages are usable before the user wastes a session on a black box.
//!
//! A pack counts as installed only when an OCR engine can actually be
//! created for it (the same call the translation pipeline makes), not merely
//! when a recognizer language is listed.

use serde::Serialize;
use windows::Media::Ocr::OcrEngine as WinOcrEngine;

/// One Source Language's OCR availability on this machine.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OcrPackStatus {
    /// Config code: "en" | "ja" | "zh" | "ko".
    pub code: &'static str,
    /// Human-readable language name for the wizard UI.
    pub name: &'static str,
    /// Whether an OCR engine can be created for this language.
    pub installed: bool,
    /// The installed pack that would be used (empty when none).
    pub pack_tag: String,
}

/// Probes every Source Language. MUST run off the main thread (WinRT calls
/// block); the caller also needs COM initialized — done here, mirroring
/// [`crate::ocr`].
pub fn detect_ocr_packs() -> Result<Vec<OcrPackStatus>, String> {
    crate::ocr::init_com();

    let languages = WinOcrEngine::AvailableRecognizerLanguages()
        .map_err(|e| format!("Windows OCR is unavailable: {e}"))?;
    let count = languages
        .Size()
        .map_err(|e| format!("Windows OCR is unavailable: {e}"))?;
    let mut available: Vec<String> = Vec::with_capacity(count as usize);
    for i in 0..count {
        if let Ok(lang) = languages.GetAt(i) {
            if let Ok(tag) = lang.LanguageTag() {
                available.push(tag.to_string());
            }
        }
    }

    Ok(SOURCE_LANGUAGES
        .iter()
        .map(|&(code, name)| {
            let pack_tag = best_pack_tag(&available, code);
            OcrPackStatus {
                code,
                name,
                installed: !pack_tag.is_empty(),
                pack_tag,
            }
        })
        .collect())
}

/// The four Source Languages in wizard order (EN first: it is the default).
const SOURCE_LANGUAGES: &[(&str, &str)] = &[
    ("en", "English"),
    ("ja", "Japanese"),
    ("zh", "Chinese"),
    ("ko", "Korean"),
];

/// The installed pack that would actually serve `code` (mirrors the OCR
/// engine's own fallback: exact tag first, then any pack with the same
/// primary subtag — Simplified Chinese preferred over Traditional), or empty
/// when creating an engine for every candidate failed.
fn best_pack_tag(available: &[String], code: &str) -> String {
    for tag in available {
        if tag.eq_ignore_ascii_case(code) && crate::ocr::try_create(tag).is_ok() {
            return tag.clone();
        }
    }
    let primary = crate::ocr::primary_subtag(code);
    let mut candidates: Vec<&String> = available
        .iter()
        .filter(|tag| crate::ocr::primary_subtag(tag) == primary)
        .collect();
    candidates.sort_by_key(|tag| crate::ocr::pack_preference(tag));
    for tag in candidates {
        if crate::ocr::try_create(tag).is_ok() {
            return tag.clone();
        }
    }
    String::new()
}
