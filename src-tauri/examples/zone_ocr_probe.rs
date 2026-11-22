//! Throwaway diagnostic probe, stage 2: runs the REAL Windows pipeline
//! (GDI capture → Windows OCR) against a region while the user keeps text
//! visible there — the two Windows-dependent links the fake-tested
//! orchestrator cannot cover.
//!
//! `cargo run -p langception --example zone_ocr_probe -- --region x,y,w,h [--seconds 30]`
//! Defaults to the profile zone from the bug report (501,1069 1809x369).
//!
//! Prints: installed OCR recognizer languages (a missing pack makes the
//! pipeline error on every pass), then every recognized Text Block and
//! every error.

use langception::capture::GdiCaptureSource;
use langception::ocr::WindowsOcrEngine;
use langception_core::ports::{CaptureSource, OcrEngine, Region, SourceLanguage};

fn parse_region(text: &str) -> Option<Region> {
    let parts: Vec<&str> = text.split(',').collect();
    if parts.len() != 4 {
        return None;
    }
    Some(Region {
        x: parts[0].trim().parse().ok()?,
        y: parts[1].trim().parse().ok()?,
        width: parts[2].trim().parse().ok()?,
        height: parts[3].trim().parse().ok()?,
    })
}

fn main() {
    let mut args = std::env::args().skip(1);
    let mut region = Region {
        x: 501,
        y: 1069,
        width: 1809,
        height: 369,
    };
    let mut seconds: u64 = 30;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--region" => {
                if let Some(text) = args.next() {
                    if let Some(parsed) = parse_region(&text) {
                        region = parsed;
                    }
                }
            }
            "--seconds" => {
                if let Some(text) = args.next() {
                    if let Ok(value) = text.parse() {
                        seconds = value;
                    }
                }
            }
            _ => {}
        }
    }

    // 1. Which OCR languages does this Windows actually have?
    println!("installed OCR recognizer languages: {:?}", langception::ocr::available_recognizer_languages());

    // 2. The real capture+OCR pipeline against the region.
    let capture = GdiCaptureSource::new();
    let ocr = WindowsOcrEngine::new(SourceLanguage::En);
    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
    println!(
        "\nsampling region ({},{}) {}x{} for {seconds}s — keep game text visible there …",
        region.x, region.y, region.width, region.height
    );
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(seconds);
    let mut saw_text = false;
    let mut pass = 0u32;
    while std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(700));
        pass += 1;
        let image = match runtime.block_on(capture.capture(region)) {
            Ok(image) => image,
            Err(e) => {
                println!("[pass {pass}] capture FAILED: {}", e.message);
                continue;
            }
        };
        match runtime.block_on(ocr.recognize(&image, SourceLanguage::En)) {
            Ok(blocks) if blocks.is_empty() => {
                println!("[pass {pass}] {}x{} → 0 blocks (no text visible?)", image.width, image.height)
            }
            Ok(blocks) => {
                for block in &blocks {
                    saw_text = true;
                    println!(
                        "[pass {pass}] block at ({},{}) {}x{}: {:?}",
                        block.x, block.y, block.width, block.height, block.text
                    );
                }
            }
            Err(e) => println!("[pass {pass}] OCR FAILED: {}", e.message),
        }
    }
    if saw_text {
        println!("\nRESULT: capture+OCR WORK on this region — bug is in shell wiring or display.");
    } else {
        println!("\nRESULT: no text ever recognized — capture/OCR side reproduced (or no text was visible).");
    }
}
