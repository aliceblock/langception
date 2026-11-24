//! The PP-OCRv6 sidecar state machine (pp-ocrv6-sidecar feature ticket
//! 04), driven against the same scripted `paddle_stub` binary the old
//! engine's tests use — the new engine launches it with its own contract
//! (`-models_path -language -tier`, the stub keys its scripts on the
//! language/tier pair). Covered here: lazy spawn + reuse across passes,
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
    PathBuf::from(env!("CARGO_BIN_EXE_paddle_stub"))
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
