//! Opt-in smoke test for the thin Windows adapters (GDI capture + Windows
//! OCR). Deliberately NOT part of the automated suite — it depends on a real
//! display and installed OCR language packs (spec: Testing Decisions).
//!
//! Run it on a machine with a live desktop session:
//! `cargo test -p langception --test manual_smoke -- --ignored --nocapture`
//!
//! The captured region defaults to `0,0 800x300` and can be pointed at any
//! screen area via `LANGCEPTION_SMOKE_REGION="x,y,width,height"` (physical
//! px) — e.g. at a Capture Zone's region from config.toml.

use langception_core::ports::{CaptureSource, OcrEngine, Region, SourceLanguage};

#[test]
#[ignore]
#[cfg(windows)]
fn capture_and_ocr_smoke() {
    let region = std::env::var("LANGCEPTION_SMOKE_REGION")
        .ok()
        .and_then(|text| parse_region(&text))
        .unwrap_or(Region { x: 0, y: 0, width: 800, height: 300 });

    let capture = langception::capture::GdiCaptureSource::new();
    let image = tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(capture.capture(region))
        .expect("GDI capture should work in a desktop session");
    assert_eq!(image.width, region.width);
    assert_eq!(image.height, region.height);
    assert_eq!(
        image.bgra.len(),
        (region.width * region.height * 4) as usize
    );
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

fn parse_region(text: &str) -> Option<Region> {
    let parts: Vec<i32> = text
        .split(',')
        .map(|p| p.trim().parse().ok())
        .collect::<Option<_>>()?;
    if parts.len() != 4 {
        return None;
    }
    Some(Region {
        x: parts[0],
        y: parts[1],
        width: parts[2].max(1) as u32,
        height: parts[3].max(1) as u32,
    })
}
