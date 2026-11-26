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

use langception_core::ports::{CaptureSource, OcrEngine, Region, SourceLanguage, TranslateEngine};

#[test]
#[ignore]
#[cfg(windows)]
fn capture_and_ocr_smoke() {
    let region = std::env::var("LANGCEPTION_SMOKE_REGION")
        .ok()
        .and_then(|text| parse_region(&text))
        .unwrap_or(Region {
            x: 0,
            y: 0,
            width: 800,
            height: 300,
        });

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
    // LANGCEPTION_SMOKE_DUMP=path writes the raw BGRA frame for inspection
    // (raw width*height*4 bytes, top-down); convert e.g. with PIL:
    // Image.frombuffer("RGBA", (w, h), data, "raw", "BGRA").save(path)
    if let Ok(dump) = std::env::var("LANGCEPTION_SMOKE_DUMP") {
        std::fs::write(&dump, &image.bgra).expect("should write the capture dump");
        println!("dumped {} bytes to {}", image.bgra.len(), dump);
    }

    let ocr = langception::ocr::WindowsOcrEngine::new(SourceLanguage::En);
    let blocks = tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(ocr.recognize(&image, SourceLanguage::En))
        .expect("Windows OCR should run with an installed English pack");
    println!(
        "OCR blocks: {} (0 is fine — depends what is on screen)",
        blocks.len()
    );
    for block in &blocks {
        println!("  {:?} at {},{}", block.text, block.x, block.y);
    }
}

/// PP-OCRv6 sidecar smoke (pp-ocrv6-sidecar feature ticket 04; tiny-only
/// since deferred-model-packs ticket 05). The real sidecar is deliberately
/// outside the automated suite (spec: Testing Decisions) — its lifecycle
/// logic is stub-tested offline. Point `LANGCEPTION_PPOCRV6_DIR` at the
/// bundle `packaging/fetch-pp-ocrv6.ps1` stages
/// (`src-tauri/resources/pp-ocrv6/`: the sidecar exe beside Tiny's three
/// files and the models manifest — the exe itself comes from
/// `cargo build -p pp-ocrv6-sidecar`) or at the app's active models
/// directory (the bundle baseline plus any downloaded packs) to prove the
/// whole chain through the NEW engine: spawn, handshake, PNG task, reply
/// parse, reuse, and the (language, tier) restart. Same region handling as
/// the other smokes.
#[test]
#[ignore]
#[cfg(windows)]
fn pp_ocrv6_smoke() {
    use std::path::Path;

    let dir = std::env::var("LANGCEPTION_PPOCRV6_DIR").unwrap_or_else(|_| {
        // Default: the fetched-into-repo bundle the app itself resolves.
        let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
        manifest.join("resources").join("pp-ocrv6").display().to_string()
    });
    let models = Path::new(&dir);
    assert!(
        models.join("ppocrv6_tiny_dict.txt").is_file(),
        "fetched models missing: {} (run packaging/fetch-pp-ocrv6.ps1)",
        models.display()
    );
    // The exe: beside the models (release layout), else the workspace
    // target dir beside the app exe — this test binary lives one level
    // deeper (target/debug/deps), so climb one parent for the dev layout
    // the shell's resolver sees.
    let exe_beside_models = models.join(langception_core::ppocr::SIDECAR_EXE);
    let program = if exe_beside_models.is_file() {
        exe_beside_models
    } else {
        let exe_dir = Path::new(&std::env::current_exe().unwrap())
            .parent()
            .and_then(|deps| deps.parent())
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf();
        let exe = exe_dir.join(langception_core::ppocr::SIDECAR_EXE);
        assert!(
            exe.is_file(),
            "sidecar exe missing: {} (run cargo build -p pp-ocrv6-sidecar)",
            exe.display()
        );
        exe
    };

    let region = std::env::var("LANGCEPTION_SMOKE_REGION")
        .ok()
        .and_then(|text| parse_region(&text))
        .unwrap_or(Region {
            x: 0,
            y: 0,
            width: 800,
            height: 300,
        });

    // ONE runtime for capture + every pass, matching how the app drives
    // the engine (see capture_and_ocr_smoke).
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let image = runtime
        .block_on(langception::capture::GdiCaptureSource::new().capture(region))
        .expect("GDI capture should work in a desktop session");

    let ocr = langception_core::ppocr::PpOcrv6Engine::new(
        program,
        models,
        langception_core::ppocr::ModelTier::Tiny,
    );
    // Pass 1 spawns (model load included), pass 2 must reuse the live
    // sidecar (marginal inference only) — both on TINY, the tier the
    // bundle actually ships. Pass 3 switches the Source Language to
    // Japanese — tiny has no kana, so the substituted small set must be
    // in the models dir (a downloaded Model Pack); this smoke never
    // downloads, so against a fresh tiny-only bundle the pass is skipped
    // with a note instead of failing. Point LANGCEPTION_PPOCRV6_DIR at
    // the app's active models directory (or any dir holding the small
    // files) to run it.
    let small_set_present = models.join("pp-ocrv6_small_det.onnx").is_file()
        && models.join("pp-ocrv6_small_rec.onnx").is_file()
        && models.join("ppocrv6_dict.txt").is_file();
    let run_pass = |label: &str, language: SourceLanguage| {
        let started = std::time::Instant::now();
        let blocks = runtime
            .block_on(ocr.recognize(&image, language))
            .expect("the sidecar should recognize a live desktop capture");
        println!(
            "{label}: {} blocks in {:?} (0 is fine — depends what is on screen)",
            blocks.len(),
            started.elapsed()
        );
        for block in &blocks {
            println!("  {:?} at {},{}", block.text, block.x, block.y);
        }
    };
    run_pass("pass 1 (en, fresh spawn)", SourceLanguage::En);
    run_pass("pass 2 (en, reuse)", SourceLanguage::En);
    if small_set_present {
        run_pass(
            "pass 3 (ja, restart on the substituted small set)",
            SourceLanguage::Ja,
        );
    } else {
        println!(
            "pass 3 (ja, restart) SKIPPED: {} has no small set — the substituted \
             Japanese models are a Model Pack this smoke never downloads",
            models.display()
        );
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

/// Live Translate Engine smoke (ticket 09): the keyed engine runs only
/// when its env var carries a real key —
/// `LANGCEPTION_SMOKE_AZURE_KEY` (+ `_REGION` for a regional resource).
/// Parses and error classification are already covered offline; this
/// proves the live endpoint answers and the Thai arrives.
#[test]
#[ignore]
#[cfg(windows)]
fn translate_engine_smoke() {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let azure_key = std::env::var("LANGCEPTION_SMOKE_AZURE_KEY").ok();
    if azure_key.is_none() {
        println!("no engine keys in env — nothing to smoke");
        return;
    }

    if let Some(key) = azure_key {
        let region = std::env::var("LANGCEPTION_SMOKE_AZURE_REGION").unwrap_or_default();
        let engine = langception_core::azure::AzureTranslateEngine::new(&key, &region)
            .expect("Azure engine should build");
        let thai = runtime
            .block_on(engine.translate("Hello", SourceLanguage::En, "th"))
            .expect("Azure should translate a one-word request");
        assert!(!thai.is_empty(), "Azure returned an empty translation");
        println!("Azure OK: Hello → {thai}");
    }
}
