//! The PP-OCRv6 sidecar state machine (pp-ocrv6-sidecar feature ticket
//! 04), driven against the scripted `ocr_stub` binary — the engine
//! launches it with its own contract (`-models_path -language -tier`;
//! the stub keys its scripts on the language/tier pair). Covered here:
//! lazy spawn + reuse across passes,
//! restart on a tier change AND on a language change, crash detection
//! with recovery on the next call, the restart notice, init-crash
//! surfacing, and the request timeout. (Reply parsing and the launch
//! arguments have their own unit tests in `sidecar.rs` / `ppocr.rs`.)

use langception_core::ports::{CapturedImage, OcrEngine, SourceLanguage};
use langception_core::ppocr::{ModelTier, PpOcrv6Engine};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// CARGO_BIN_EXE_<name> is set for integration tests when the [[bin]] is
/// built — the scripted sidecar stand-in.
fn stub_exe() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_ocr_stub"))
}

fn ok_reply(text: &str) -> String {
    format!(
        "{{\"code\":100,\"data\":[{{\"box\":[[1,2],[11,2],[11,12],[1,12]],\"score\":0.9,\"text\":\"{text}\"}}]}}\n"
    )
}

/// Writes a stub script (directive line + reply lines) under the name a
/// (language, tier) launch reads, inside a fresh fake models dir.
fn fake_models(root: &Path, language: &str, tier: &str, script: &str) -> PathBuf {
    let models = root.join("models");
    fs::create_dir_all(&models).unwrap();
    fs::write(models.join(format!("stub-{language}-{tier}.txt")), script).unwrap();
    models
}

fn spawn_count(models_dir: &Path, language: &str, tier: &str) -> usize {
    let marker = models_dir.join(format!("stub-{language}-{tier}.txt.spawns"));
    fs::read_to_string(marker)
        .map(|content| content.lines().filter(|l| !l.trim().is_empty()).count())
        .unwrap_or(0)
}

fn tiny_image() -> CapturedImage {
    CapturedImage {
        width: 8,
        height: 8,
        bgra: vec![10; 8 * 8 * 4],
    }
}

fn fast_engine(program: PathBuf, models_dir: PathBuf, tier: ModelTier) -> PpOcrv6Engine {
    PpOcrv6Engine::with_timeouts(
        program,
        models_dir,
        tier,
        Duration::from_secs(5),
        Duration::from_millis(500),
    )
}

/// A unique scratch dir per test; the stub's spawn markers live inside it.
fn scratch_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir()
        .join("langception-ppocr-tests")
        .join(format!("{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

#[tokio::test]
async fn the_sidecar_spawns_lazily_and_is_reused_across_passes() {
    let dir = scratch_dir("reuse");
    let models = fake_models(&dir, "en", "small", &format!("OK\n{}", ok_reply("alpha")));

    let engine = fast_engine(stub_exe(), models.clone(), ModelTier::Small);
    let first = engine
        .recognize(&tiny_image(), SourceLanguage::En)
        .await
        .unwrap();
    let second = engine
        .recognize(&tiny_image(), SourceLanguage::En)
        .await
        .unwrap();

    assert_eq!(first.len(), 1);
    assert_eq!(first[0].text, "alpha");
    assert_eq!(second[0].text, "alpha");
    assert_eq!(
        spawn_count(&models, "en", "small"),
        1,
        "the second pass must reuse the live sidecar, not spawn another"
    );
}

#[tokio::test]
async fn a_tier_change_restarts_the_sidecar_on_the_matching_model() {
    let dir = scratch_dir("tier-switch");
    let models = fake_models(&dir, "en", "small", &format!("OK\n{}", ok_reply("small text")));
    fs::write(
        models.join("stub-en-medium.txt"),
        format!("OK\n{}", ok_reply("medium text")),
    )
    .unwrap();

    let small = fast_engine(stub_exe(), models.clone(), ModelTier::Small);
    let medium = fast_engine(stub_exe(), models.clone(), ModelTier::Medium);
    // Two engine instances model one setting being changed and the loop
    // rebuilt (the shell rebuilds the engine on a Settings switch) — what
    // matters is that each tier's launches carry that tier's contract, so
    // each selection gets its own sidecar and its own reply.
    let small_text = small
        .recognize(&tiny_image(), SourceLanguage::En)
        .await
        .unwrap()[0]
        .text
        .clone();
    let medium_text = medium
        .recognize(&tiny_image(), SourceLanguage::En)
        .await
        .unwrap()[0]
        .text
        .clone();

    assert_eq!(small_text, "small text");
    assert_eq!(medium_text, "medium text");
    assert_eq!(spawn_count(&models, "en", "small"), 1);
    assert_eq!(
        spawn_count(&models, "en", "medium"),
        1,
        "the medium launch must be a separate sidecar on the medium models"
    );
}

/// One engine instance whose tier stays fixed sees a language switch as
/// the other half of the restart key: the sidecar restarts on the new
/// language's models.
#[tokio::test]
async fn a_language_switch_restarts_the_sidecar_on_the_matching_model() {
    let dir = scratch_dir("language-switch");
    let models = fake_models(&dir, "en", "small", &format!("OK\n{}", ok_reply("alpha")));
    fs::write(
        models.join("stub-ko-small.txt"),
        format!("OK\n{}", ok_reply("annyeong")),
    )
    .unwrap();

    let engine = fast_engine(stub_exe(), models.clone(), ModelTier::Small);
    let en = engine
        .recognize(&tiny_image(), SourceLanguage::En)
        .await
        .unwrap();
    let ko = engine
        .recognize(&tiny_image(), SourceLanguage::Ko)
        .await
        .unwrap();

    assert_eq!(en[0].text, "alpha");
    assert_eq!(ko[0].text, "annyeong");
    assert_eq!(
        spawn_count(&models, "en", "small") + spawn_count(&models, "ko", "small"),
        2,
        "each model selection must get its own sidecar process"
    );
}

/// Waits until the stub process has recorded its exit, so a subsequent
/// recognize reliably finds the dead sidecar (no terminating-race). The
/// settle delay covers kernel teardown, which can trail the last write a
/// few milliseconds — more under parallel-test load.
fn wait_for_exit(models_dir: &Path, language: &str, tier: &str) {
    let marker = models_dir.join(format!("stub-{language}-{tier}.txt.exits"));
    for _ in 0..100 {
        if marker.is_file() {
            std::thread::sleep(Duration::from_millis(250));
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("the stub never recorded its exit");
}

#[tokio::test]
async fn a_crashed_sidecar_is_detected_and_the_next_call_recovers() {
    let dir = scratch_dir("crash-recovery");
    let models = fake_models(
        &dir,
        "en",
        "small",
        &format!("CRASH_FIRST_REPLY\n{}", ok_reply("alpha")),
    );

    let engine = fast_engine(stub_exe(), models.clone(), ModelTier::Small);
    let first = engine
        .recognize(&tiny_image(), SourceLanguage::En)
        .await
        .unwrap();
    // The crash completes before the next pass — in production the
    // fallback wrapper covers the (narrow) window where a just-died
    // sidecar is still visibly alive.
    wait_for_exit(&models, "en", "small");
    let second = engine
        .recognize(&tiny_image(), SourceLanguage::En)
        .await
        .unwrap();

    assert_eq!(first[0].text, "alpha", "the reply that arrived before the crash is delivered");
    assert_eq!(second[0].text, "alpha", "the next pass respawns the sidecar and works");
    assert_eq!(spawn_count(&models, "en", "small"), 2);
}

#[tokio::test]
async fn an_unexpected_sidecar_death_fires_the_restart_notice() {
    let dir = scratch_dir("restart-notice");
    let models = fake_models(
        &dir,
        "en",
        "small",
        &format!("CRASH_FIRST_REPLY\n{}", ok_reply("alpha")),
    );
    let notices: std::sync::Arc<std::sync::Mutex<Vec<String>>> = Default::default();

    let engine = {
        let notices = notices.clone();
        fast_engine(stub_exe(), models.clone(), ModelTier::Small).with_restart_notice(
            std::sync::Arc::new(move |message| notices.lock().unwrap().push(message)),
        )
    };
    engine
        .recognize(&tiny_image(), SourceLanguage::En)
        .await
        .unwrap();
    wait_for_exit(&models, "en", "small");
    engine
        .recognize(&tiny_image(), SourceLanguage::En)
        .await
        .unwrap();

    let notices = notices.lock().unwrap();
    assert_eq!(
        notices.len(),
        1,
        "one notice per incident: {notices:?}"
    );
    assert!(
        notices[0].contains("exited unexpectedly"),
        "the notice names the unexpected exit: {notices:?}"
    );
}

#[tokio::test]
async fn an_init_crash_surfaces_an_error_not_a_hang() {
    let dir = scratch_dir("init-crash");
    let models = fake_models(&dir, "en", "small", "CRASH_INIT\n");

    let engine = fast_engine(stub_exe(), models.clone(), ModelTier::Small);
    let error = engine
        .recognize(&tiny_image(), SourceLanguage::En)
        .await
        .unwrap_err();

    assert!(
        error.message.contains("exited during init"),
        "error names the failure: {}",
        error.message
    );
    assert!(
        error.message.contains("scripted init crash"),
        "the stderr tail is included for diagnosis: {}",
        error.message
    );
    assert_eq!(spawn_count(&models, "en", "small"), 1);
}

#[tokio::test]
async fn an_unresponsive_sidecar_hits_the_request_timeout() {
    let dir = scratch_dir("hang");
    let models = fake_models(&dir, "en", "small", "HANG_REPLY\n");

    let engine = PpOcrv6Engine::with_timeouts(
        stub_exe(),
        models,
        ModelTier::Small,
        Duration::from_secs(5),
        Duration::from_millis(300),
    );
    let error = engine
        .recognize(&tiny_image(), SourceLanguage::En)
        .await
        .unwrap_err();

    assert!(
        error.message.contains("did not reply"),
        "error names the timeout: {}",
        error.message
    );
}

// ---- The unsupported-combination rule (ticket 05) ----
//
// Tiny cannot read kana: a Japanese pass with tiny selected must launch
// the small model set and surface a one-line notice — once per
// substituted session, not per pass. The stub keys its scripts on the
// launch's (language, tier), so a differently-worded reply per script
// proves exactly which model set each pass launched.

fn substitution_engine(
    program: PathBuf,
    models_dir: PathBuf,
    notices: &std::sync::Arc<std::sync::Mutex<Vec<String>>>,
) -> PpOcrv6Engine {
    let notices = notices.clone();
    fast_engine(program, models_dir, ModelTier::Tiny).with_substitution_notice(
        // The seam (ticket 62) hands the callback the SELECTED tier — the
        // wording is composed on the app side, so the test pins the data,
        // and the app-side wording is pinned by its own seam test.
        std::sync::Arc::new(move |tier| {
            notices.lock().unwrap().push(tier.display_name().to_string())
        }),
    )
}

#[tokio::test]
async fn japanese_with_tiny_launches_the_small_model_and_notices_once() {
    let dir = scratch_dir("substitution");
    let models = fake_models(&dir, "ja", "tiny", &format!("OK\n{}", ok_reply("tiny-model text")));
    fs::write(
        models.join("stub-ja-small.txt"),
        format!("OK\n{}", ok_reply("small-model text")),
    )
    .unwrap();
    let notices = Default::default();

    let engine = substitution_engine(stub_exe(), models.clone(), &notices);
    let first = engine
        .recognize(&tiny_image(), SourceLanguage::Ja)
        .await
        .unwrap();
    let second = engine
        .recognize(&tiny_image(), SourceLanguage::Ja)
        .await
        .unwrap();

    assert_eq!(
        first[0].text, "small-model text",
        "the launch must carry -tier=small, not the requested tiny"
    );
    assert_eq!(second[0].text, "small-model text");
    assert_eq!(
        spawn_count(&models, "ja", "tiny"),
        0,
        "the kana-less tiny model must never be launched for Japanese"
    );
    assert_eq!(spawn_count(&models, "ja", "small"), 1, "one substituted session");
    let notices = notices.lock().unwrap();
    assert_eq!(
        notices.len(),
        1,
        "one notice per incident, not per pass: {notices:?}"
    );
    // The seam hands over the tier the user SELECTED — the substituted
    // small set is implied by the rule (effective_selection) and named by
    // the app's composed message.
    assert_eq!(
        notices[0], "PP-OCRv6-tiny",
        "the callback carries the selected tier: {notices:?}"
    );
}

#[tokio::test]
async fn leaving_and_reentering_the_pairing_notices_again() {
    let dir = scratch_dir("substitution-reenter");
    let models = fake_models(&dir, "ja", "tiny", &format!("OK\n{}", ok_reply("ja-tiny")));
    fs::write(models.join("stub-ja-small.txt"), format!("OK\n{}", ok_reply("ja-small"))).unwrap();
    fs::write(models.join("stub-en-tiny.txt"), format!("OK\n{}", ok_reply("en-tiny"))).unwrap();
    let notices = Default::default();

    let engine = substitution_engine(stub_exe(), models.clone(), &notices);
    assert_eq!(
        engine.recognize(&tiny_image(), SourceLanguage::Ja).await.unwrap()[0].text,
        "ja-small"
    );
    assert_eq!(
        engine.recognize(&tiny_image(), SourceLanguage::En).await.unwrap()[0].text,
        "en-tiny",
        "English is honored as selected (no substitution)"
    );
    assert_eq!(
        engine.recognize(&tiny_image(), SourceLanguage::Ja).await.unwrap()[0].text,
        "ja-small",
        "re-entering the pairing substitutes again"
    );

    assert_eq!(spawn_count(&models, "en", "tiny"), 1);
    assert_eq!(
        spawn_count(&models, "ja", "small"),
        2,
        "each re-entry is its own substituted session (a model-switch restart)"
    );
    let notices = notices.lock().unwrap();
    assert_eq!(
        notices.len(),
        2,
        "each transition into the pairing gets its own notice: {notices:?}"
    );
}

#[tokio::test]
async fn korean_and_the_other_pairings_are_honored_without_a_notice() {
    let dir = scratch_dir("no-substitution");
    let models = fake_models(&dir, "ko", "tiny", &format!("OK\n{}", ok_reply("ko-tiny")));
    fs::write(models.join("stub-en-tiny.txt"), format!("OK\n{}", ok_reply("en-tiny"))).unwrap();
    fs::write(
        models.join("stub-zh-tiny.txt"),
        format!("OK\n{}", ok_reply("zh-tiny")),
    )
    .unwrap();
    fs::write(
        models.join("stub-ko-medium.txt"),
        format!("OK\n{}", ok_reply("ko-medium")),
    )
    .unwrap();
    let notices = Default::default();

    let tiny = substitution_engine(stub_exe(), models.clone(), &notices);
    let medium = fast_engine(stub_exe(), models.clone(), ModelTier::Medium)
        .with_substitution_notice(std::sync::Arc::new(|_| {
            panic!("no pairing of medium substitutes");
        }));
    // Korean reads the tier's models at every tier (tier-independent by
    // definition, not a fallback) — tiny included.
    assert_eq!(tiny.recognize(&tiny_image(), SourceLanguage::Ko).await.unwrap()[0].text, "ko-tiny");
    assert_eq!(
        medium.recognize(&tiny_image(), SourceLanguage::Ko).await.unwrap()[0].text,
        "ko-medium"
    );
    // English and Chinese honor tiny (Latin + CJK ideographs are in its
    // vocabulary).
    assert_eq!(tiny.recognize(&tiny_image(), SourceLanguage::En).await.unwrap()[0].text, "en-tiny");
    assert_eq!(tiny.recognize(&tiny_image(), SourceLanguage::Zh).await.unwrap()[0].text, "zh-tiny");

    assert_eq!(spawn_count(&models, "ko", "tiny"), 1);
    assert_eq!(spawn_count(&models, "ko", "medium"), 1);
    assert_eq!(spawn_count(&models, "en", "tiny"), 1);
    assert_eq!(spawn_count(&models, "zh", "tiny"), 1);
    let notices = notices.lock().unwrap();
    assert!(
        notices.is_empty(),
        "honored pairings fire no notice: {notices:?}"
    );
}

// ---- The not-ready gate (deferred-model-packs ticket 02) ----
//
// A tier whose Model Pack is not on disk yet is a NORMAL state of the
// deferred-packs feature: the pass reports loading, no sidecar spawns,
// and — once the pack's files land — the NEXT pass on the same engine
// spawns into the active models directory (no app restart). The stub
// binary stands in for the sidecar; a manifest built over the same fake
// files ticket 01's module tests pin drives the real gate. CI needs no
// real models and no network.

use langception_core::model_packs::{file_sha256, ReadinessGate};

/// Writes the ten fake model files into `bundle` and the manifest
/// pinning them (the JSON shape ticket 05's packaging must emit), and
/// creates the empty active models directory beside it.
fn gate_fixture(root: &Path) -> (PathBuf, PathBuf) {
    let bundle = root.join("bundle");
    let active = root.join("active");
    fs::create_dir_all(&bundle).unwrap();
    fs::create_dir_all(&active).unwrap();
    let files: &[&str] = &[
        "pp-ocrv6_tiny_det.onnx",
        "pp-ocrv6_tiny_rec.onnx",
        "ppocrv6_tiny_dict.txt",
        "pp-ocrv6_small_det.onnx",
        "pp-ocrv6_small_rec.onnx",
        "ppocrv6_dict.txt",
        "pp-ocrv6_medium_det.onnx",
        "pp-ocrv6_medium_rec.onnx",
        "korean_pp-ocrv5_mobile_rec.onnx",
        "ppocrv5_korean_dict.txt",
    ];
    for (i, name) in files.iter().enumerate() {
        fs::write(bundle.join(name), format!("{name}-v{i}")).unwrap();
    }
    let pin = |name: &str| {
        format!(
            "{{\"name\": \"{name}\", \"sha256\": \"{}\", \"size\": {}}}",
            file_sha256(&bundle.join(name)).unwrap(),
            fs::metadata(bundle.join(name)).unwrap().len(),
        )
    };
    let set = |names: &[&str]| names.iter().map(|n| pin(n)).collect::<Vec<_>>().join(", ");
    let tiny = set(&["pp-ocrv6_tiny_det.onnx", "pp-ocrv6_tiny_rec.onnx", "ppocrv6_tiny_dict.txt"]);
    let small = set(&["pp-ocrv6_small_det.onnx", "pp-ocrv6_small_rec.onnx", "ppocrv6_dict.txt"]);
    let medium = set(&["pp-ocrv6_medium_det.onnx", "pp-ocrv6_medium_rec.onnx", "ppocrv6_dict.txt"]);
    let korean = set(&["korean_pp-ocrv5_mobile_rec.onnx", "ppocrv5_korean_dict.txt"]);
    let manifest = format!(
        r#"{{"manifest_version": 1,
            "bundled": {{"files": [{tiny}]}},
            "packs": {{
                "small": {{"version": "1", "urls": ["https://example.invalid/small.zip"], "files": [{small}]}},
                "medium": {{"version": "1", "urls": ["https://example.invalid/medium.zip"], "files": [{medium}]}},
                "korean": {{"version": "1", "urls": ["https://example.invalid/korean.zip"], "files": [{korean}]}}
            }}}}"#
    );
    fs::write(bundle.join("models-manifest.json"), manifest).unwrap();
    (bundle, active)
}

/// Copies one pinned file from the bundle into the active models
/// directory — the bytes a verified download would have landed there.
fn land_pack_file(bundle: &Path, active: &Path, name: &str) {
    fs::copy(bundle.join(name), active.join(name)).unwrap();
}

type GateNotices = std::sync::Arc<std::sync::Mutex<Vec<&'static str>>>;

/// The engine under test: launch target = the ACTIVE models directory,
/// gate = the real ReadinessGate over (bundle, active), loading notice =
/// recorded as "loading" (Some) / "ready" (None).
fn gated_engine(
    tier: ModelTier,
    bundle: &Path,
    active: &Path,
    notices: &GateNotices,
) -> PpOcrv6Engine {
    let notices = notices.clone();
    fast_engine(stub_exe(), active.to_path_buf(), tier)
        .with_readiness_gate(std::sync::Arc::new(ReadinessGate::new(bundle, active)))
        .with_not_ready_notice(std::sync::Arc::new(move |not_ready| {
            let mut notices = notices.lock().unwrap();
            match not_ready {
                Some(_) => notices.push("loading"),
                None => notices.push("ready"),
            }
        }))
}

#[tokio::test]
async fn a_selection_whose_pack_is_missing_reports_loading_and_never_spawns() {
    let dir = scratch_dir("gate-not-ready");
    let (bundle, active) = gate_fixture(&dir);
    let notices: GateNotices = Default::default();

    let engine = gated_engine(ModelTier::Small, &bundle, &active, &notices);
    let first = engine
        .recognize(&tiny_image(), SourceLanguage::En)
        .await
        .unwrap_err();
    let second = engine
        .recognize(&tiny_image(), SourceLanguage::En)
        .await
        .unwrap_err();

    assert!(
        first.not_ready && second.not_ready,
        "both passes report the not-ready state, not a plain failure"
    );
    assert!(
        first.message.contains("PP-OCRv6-small"),
        "the per-pass line names the selected tier: {}",
        first.message
    );
    assert!(
        first.message.contains("Small set") && first.message.contains("pp-ocrv6_small_det.onnx"),
        "the line names the missing pack and a missing file: {}",
        first.message
    );
    assert_eq!(
        spawn_count(&active, "en", "small"),
        0,
        "a not-ready pass must never attempt a spawn"
    );
    assert_eq!(
        notices.lock().unwrap().len(),
        1,
        "one loading notice per incident, not per pass: {:?}",
        notices.lock().unwrap()
    );
    // First engine use populated the active directory's bundled baseline
    // (assembly ran even though the pass was blocked).
    assert!(active.join("ppocrv6_tiny_dict.txt").is_file());
}

#[tokio::test]
async fn the_gate_transitions_not_ready_notice_ready_spawn_without_restart() {
    let dir = scratch_dir("gate-transitions");
    let (bundle, active) = gate_fixture(&dir);
    let notices: GateNotices = Default::default();

    let engine = gated_engine(ModelTier::Small, &bundle, &active, &notices);
    let blocked = engine
        .recognize(&tiny_image(), SourceLanguage::En)
        .await
        .unwrap_err();
    assert!(blocked.not_ready, "the pack is missing: loading, not failure");

    // The pack lands (ticket 03's downloader writes the same verified
    // bytes into the active directory) — nothing restarts, nothing
    // rebuilds the engine.
    land_pack_file(&bundle, &active, "pp-ocrv6_small_det.onnx");
    land_pack_file(&bundle, &active, "pp-ocrv6_small_rec.onnx");
    land_pack_file(&bundle, &active, "ppocrv6_dict.txt");
    fs::write(
        active.join("stub-en-small.txt"),
        format!("OK\n{}", ok_reply("active-dir text")),
    )
    .unwrap();

    let blocks = engine
        .recognize(&tiny_image(), SourceLanguage::En)
        .await
        .unwrap();
    assert_eq!(
        blocks[0].text, "active-dir text",
        "the next pass spawns into the ACTIVE models directory"
    );
    assert_eq!(spawn_count(&active, "en", "small"), 1);
    assert_eq!(
        notices.lock().unwrap().as_slice(),
        &["loading", "ready"],
        "the notice reports both transitions of the incident: {:?}",
        notices.lock().unwrap()
    );

    // The loading notice re-arms once ready: a fresh incident (a file
    // vanishing — disk cleanup, say) notices again.
    fs::remove_file(active.join("ppocrv6_dict.txt")).unwrap();
    let blocked_again = engine.recognize(&tiny_image(), SourceLanguage::En).await.unwrap_err();
    assert!(blocked_again.not_ready);
    assert_eq!(
        notices.lock().unwrap().as_slice(),
        &["loading", "ready", "loading"],
        "each not-ready incident notices once: {:?}",
        notices.lock().unwrap()
    );
}

#[tokio::test]
async fn ja_with_tiny_gates_the_small_set_and_the_substitution_notice_still_fires() {
    let dir = scratch_dir("gate-substitution");
    let (bundle, active) = gate_fixture(&dir);
    let loading: GateNotices = Default::default();
    let substitution: std::sync::Arc<std::sync::Mutex<Vec<String>>> = Default::default();

    // Effective selection ja+tiny = the Small set: the Small pack
    // gates the pass, the kana-less tiny model never launches, and no
    // substitution notice fires (nothing substituted ran).
    let substitution_for_notice = substitution.clone();
    let engine = gated_engine(ModelTier::Tiny, &bundle, &active, &loading)
        .with_substitution_notice(std::sync::Arc::new(move |tier| {
            substitution_for_notice.lock().unwrap().push(tier.display_name().to_string())
        }));
    let blocked = engine
        .recognize(&tiny_image(), SourceLanguage::Ja)
        .await
        .unwrap_err();
    assert!(blocked.not_ready);
    assert!(
        blocked.message.contains("Small set"),
        "the effective selection's pack is named: {}",
        blocked.message
    );
    assert_eq!(spawn_count(&active, "ja", "tiny"), 0);
    assert_eq!(spawn_count(&active, "ja", "small"), 0);
    assert!(substitution.lock().unwrap().is_empty());

    // The Small pack lands: the next pass launches the substituted
    // small tier's files (per the launch contract) and the substitution
    // notice fires — once, on the pass that actually ran it.
    land_pack_file(&bundle, &active, "pp-ocrv6_small_det.onnx");
    land_pack_file(&bundle, &active, "pp-ocrv6_small_rec.onnx");
    land_pack_file(&bundle, &active, "ppocrv6_dict.txt");
    fs::write(
        active.join("stub-ja-small.txt"),
        format!("OK\n{}", ok_reply("small-model text")),
    )
    .unwrap();
    let blocks = engine.recognize(&tiny_image(), SourceLanguage::Ja).await.unwrap();
    assert_eq!(blocks[0].text, "small-model text");
    assert_eq!(spawn_count(&active, "ja", "small"), 1);
    assert_eq!(
        substitution.lock().unwrap().as_slice(),
        &["PP-OCRv6-tiny"],
        "the notice carries the SELECTED tier, as before: {:?}",
        substitution.lock().unwrap()
    );
    assert_eq!(loading.lock().unwrap().as_slice(), &["loading", "ready"]);
}

#[tokio::test]
async fn a_stale_pack_file_is_not_ready_too() {
    let dir = scratch_dir("gate-stale");
    let (bundle, active) = gate_fixture(&dir);
    land_pack_file(&bundle, &active, "pp-ocrv6_small_det.onnx");
    land_pack_file(&bundle, &active, "pp-ocrv6_small_rec.onnx");
    land_pack_file(&bundle, &active, "ppocrv6_dict.txt");
    let notices: GateNotices = Default::default();
    let engine = gated_engine(ModelTier::Small, &bundle, &active, &notices);

    // A wrong-size file (a truncated download the manifest's size pin
    // catches) is stale: not-ready, never a spawn.
    fs::write(active.join("pp-ocrv6_small_det.onnx"), b"corrupted").unwrap();
    let stale = engine
        .recognize(&tiny_image(), SourceLanguage::En)
        .await
        .unwrap_err();
    assert!(stale.not_ready, "a stale file blocks like a missing one");
    assert_eq!(spawn_count(&active, "en", "small"), 0);

    // A same-size corrupted file fails the hash pin once the verdict is
    // recomputed — the gate's full-hash pass catches it. The det file
    // goes back to its pinned bytes so the rec corruption is isolated.
    let original_det = fs::read(bundle.join("pp-ocrv6_small_det.onnx")).unwrap();
    let mut same_size_rec = fs::read(bundle.join("pp-ocrv6_small_rec.onnx")).unwrap();
    same_size_rec[0] ^= 0xFF;
    fs::write(active.join("pp-ocrv6_small_rec.onnx"), &same_size_rec).unwrap();
    fs::write(active.join("pp-ocrv6_small_det.onnx"), &original_det).unwrap();
    let corrupted = engine
        .recognize(&tiny_image(), SourceLanguage::En)
        .await
        .unwrap_err();
    assert!(
        corrupted.not_ready,
        "same-size corruption still fails the hash pin: {}",
        corrupted.message
    );
    assert_eq!(spawn_count(&active, "en", "small"), 0);
}

/// The rotating re-verification (one pinned file re-hashed per pass on
/// the cache-hit path): same-size corruption of an ALREADY-READY pack
/// changes no size, so only the rotation catches it — within one
/// rotation window the gate flips to not-ready and the corrupted file
/// is named (spec story 7: hash before use).
#[tokio::test]
async fn a_same_size_corruption_of_a_ready_pack_blocks_within_one_rotation() {
    let dir = scratch_dir("gate-same-size");
    let (bundle, active) = gate_fixture(&dir);
    land_pack_file(&bundle, &active, "pp-ocrv6_small_det.onnx");
    land_pack_file(&bundle, &active, "pp-ocrv6_small_rec.onnx");
    land_pack_file(&bundle, &active, "ppocrv6_dict.txt");
    fs::write(
        active.join("stub-en-small.txt"),
        format!("OK\n{}", ok_reply("alpha")),
    )
    .unwrap();
    let notices: GateNotices = Default::default();
    let engine = gated_engine(ModelTier::Small, &bundle, &active, &notices);

    // Ready: one spawn, the live sidecar reused by every later pass.
    engine.recognize(&tiny_image(), SourceLanguage::En).await.unwrap();
    assert_eq!(spawn_count(&active, "en", "small"), 1);

    // Flip one byte in the small recognizer — same size, new hash.
    let mut bytes = fs::read(active.join("pp-ocrv6_small_rec.onnx")).unwrap();
    bytes[0] ^= 0xFF;
    fs::write(active.join("pp-ocrv6_small_rec.onnx"), &bytes).unwrap();

    // The rotation re-hashes one pinned file per pass; within one full
    // rotation (6 pinned files here) the verdict must flip.
    let mut blocked = None;
    for _ in 0..7 {
        match engine.recognize(&tiny_image(), SourceLanguage::En).await {
            Ok(_) => {}
            Err(e) => {
                blocked = Some(e);
                break;
            }
        }
    }
    let blocked = blocked.expect("same-size corruption must be caught within one rotation");
    assert!(blocked.not_ready);
    assert!(
        blocked.message.contains("pp-ocrv6_small_rec.onnx"),
        "the corrupted file is named: {}",
        blocked.message
    );
    assert_eq!(
        spawn_count(&active, "en", "small"),
        1,
        "the ready passes reused the live sidecar; the flip needs no respawn"
    );
    assert_eq!(notices.lock().unwrap().as_slice(), &["loading"]);

    // The blocked state holds on later passes — and the notice does not
    // re-fire per pass.
    let again = engine.recognize(&tiny_image(), SourceLanguage::En).await.unwrap_err();
    assert!(again.not_ready);
    assert_eq!(notices.lock().unwrap().as_slice(), &["loading"]);
}

#[tokio::test]
async fn tiny_selection_runs_off_the_assembled_baseline() {
    let dir = scratch_dir("gate-tiny");
    let (bundle, active) = gate_fixture(&dir);
    let notices: GateNotices = Default::default();

    // English + tiny needs no pack — the bundled baseline, copied into
    // the active directory on first engine use, is the whole requirement.
    let engine = gated_engine(ModelTier::Tiny, &bundle, &active, &notices);
    fs::write(
        active.join("stub-en-tiny.txt"),
        format!("OK\n{}", ok_reply("tiny text")),
    )
    .unwrap();
    let blocks = engine.recognize(&tiny_image(), SourceLanguage::En).await.unwrap();
    assert_eq!(blocks[0].text, "tiny text");
    assert_eq!(spawn_count(&active, "en", "tiny"), 1);
    assert!(
        active.join("pp-ocrv6_tiny_det.onnx").is_file(),
        "the assembly populated the active directory from the bundle"
    );
    assert!(
        notices.lock().unwrap().is_empty(),
        "a ready baseline never reports loading: {:?}",
        notices.lock().unwrap()
    );
}

#[tokio::test]
async fn a_broken_manifest_is_a_plain_failure_not_loading() {
    let dir = scratch_dir("gate-broken-manifest");
    let (bundle, active) = gate_fixture(&dir);
    fs::write(bundle.join("models-manifest.json"), "not json at all").unwrap();
    let notices: GateNotices = Default::default();

    let engine = gated_engine(ModelTier::Small, &bundle, &active, &notices);
    let error = engine
        .recognize(&tiny_image(), SourceLanguage::En)
        .await
        .unwrap_err();

    assert!(
        !error.not_ready,
        "a broken manifest is not the loading state — nothing a download fixes"
    );
    assert!(
        error.message.contains("models manifest"),
        "the error names the manifest problem: {}",
        error.message
    );
    assert_eq!(spawn_count(&active, "en", "small"), 0);
    assert!(
        notices.lock().unwrap().is_empty(),
        "no loading notice for a broken install: {:?}",
        notices.lock().unwrap()
    );
}
