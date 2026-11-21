//! Opt-in smoke test for the thin Windows adapters (GDI capture + Windows
//! OCR). Deliberately NOT part of the automated suite — it depends on a real
//! display and installed OCR language packs (spec: Testing Decisions).
//!
//! Run it on a machine with a live desktop session:
//! `cargo test -p langception --test manual_smoke -- --ignored --nocapture`

use langception_core::ports::{CaptureSource, OcrEngine, Region, SourceLanguage};

#[test]
#[ignore]
#[cfg(windows)]
fn capture_and_ocr_smoke() {
    let region = Region { x: 0, y: 0, width: 800, height: 300 };

    let capture = langception::capture::GdiCaptureSource::new();
    let image = tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(capture.capture(region))
        .expect("GDI capture should work in a desktop session");
    assert_eq!(image.width, 800);
    assert_eq!(image.height, 300);
    assert_eq!(image.bgra.len(), (800 * 300 * 4) as usize);
    println!("captured {}x{} BGRA OK", image.width, image.height);

    let ocr = langception::ocr::WindowsOcrEngine::new(SourceLanguage::En);
    let blocks = tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(ocr.recognize(&image, SourceLanguage::En))
        .expect("Windows OCR should run with an installed English pack");
    println!("OCR blocks: {} (0 is fine — depends what is on screen)", blocks.len());
    for block in &blocks {
        println!("  {:?} at {},{}", block.text, block.x, block.y);
    }
}
