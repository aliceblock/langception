//! The PaddleOCR sidecar state machine (ticket 31), driven against the
//! scripted `paddle_stub` binary: lazy spawn + reuse across passes,
//! Source-Language switch = restart with the matching model, crash
//! detection with recovery on the next call, and the request timeout.
//! (Reply parsing has its own fixture-based unit tests in `paddle.rs`.)

use langception_core::paddle::PaddleOcrEngine;
use langception_core::ports::{CapturedImage, OcrEngine, SourceLanguage};
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

/// Writes a stub script (directive line + reply lines) as the named model
/// config inside a fresh fake models dir.
fn fake_models(root: &Path, config: &str, script: &str) -> PathBuf {
    let models = root.join("models");
    fs::create_dir_all(&models).unwrap();
    fs::write(models.join(config), script).unwrap();
    models
}

fn spawn_count(models_dir: &Path, config: &str) -> usize {
    let marker = models_dir.join(format!("{config}.spawns"));
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

fn fast_engine(program: PathBuf, models_dir: PathBuf) -> PaddleOcrEngine {
    PaddleOcrEngine::with_timeouts(
        program,
        models_dir,
        Duration::from_secs(5),
        Duration::from_millis(500),
    )
}

/// A unique scratch dir per test; the stub's spawn markers live inside it.
fn scratch_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir()
        .join("langception-paddle-tests")
        .join(format!("{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

#[tokio::test]
async fn the_sidecar_spawns_lazily_and_is_reused_across_passes() {
    let dir = scratch_dir("reuse");
    let models = fake_models(&dir, "config_en.txt", &format!("OK\n{}", ok_reply("alpha")));

    let engine = fast_engine(stub_exe(), models.clone());
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
        spawn_count(&models, "config_en.txt"),
        1,
        "the second pass must reuse the live sidecar, not spawn another"
    );
}

#[tokio::test]
async fn a_language_switch_restarts_the_sidecar_on_the_matching_model() {
    let dir = scratch_dir("language-switch");
    let models = fake_models(
        &dir,
        "config_en.txt",
        &format!("OK\n{}", ok_reply("alpha")),
    );
    fs::write(
        models.join("config_japan.txt"),
        format!("OK\n{}", ok_reply("konnichiwa")),
    )
    .unwrap();

    let engine = fast_engine(stub_exe(), models.clone());
    let en = engine
        .recognize(&tiny_image(), SourceLanguage::En)
        .await
        .unwrap();
    let ja = engine
        .recognize(&tiny_image(), SourceLanguage::Ja)
        .await
        .unwrap();

    assert_eq!(en[0].text, "alpha");
    assert_eq!(ja[0].text, "konnichiwa");
    assert_eq!(
        spawn_count(&models, "config_en.txt") + spawn_count(&models, "config_japan.txt"),
        2,
        "each model selection must get its own sidecar process"
    );
}

/// Waits until the stub process has recorded its exit, so a subsequent
/// recognize reliably finds the dead sidecar (no terminating-race). The
/// settle delay covers kernel teardown, which can trail the last write a
/// few milliseconds — more under parallel-test load.
fn wait_for_exit(models_dir: &Path, config: &str) {
    let marker = models_dir.join(format!("{config}.exits"));
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
        "config_en.txt",
        &format!("CRASH_FIRST_REPLY\n{}", ok_reply("alpha")),
    );

    let engine = fast_engine(stub_exe(), models.clone());
    let first = engine
        .recognize(&tiny_image(), SourceLanguage::En)
        .await
        .unwrap();
    // The crash completes before the next pass — in production the
    // fallback wrapper covers the (narrow) window where a just-died
    // sidecar is still visibly alive.
    wait_for_exit(&models, "config_en.txt");
    let second = engine
        .recognize(&tiny_image(), SourceLanguage::En)
        .await
        .unwrap();

    assert_eq!(first[0].text, "alpha", "the reply that arrived before the crash is delivered");
    assert_eq!(second[0].text, "alpha", "the next pass respawns the sidecar and works");
    assert_eq!(spawn_count(&models, "config_en.txt"), 2);
}

#[tokio::test]
async fn an_unexpected_sidecar_death_fires_the_restart_notice() {
    let dir = scratch_dir("restart-notice");
    let models = fake_models(
        &dir,
        "config_en.txt",
        &format!("CRASH_FIRST_REPLY\n{}", ok_reply("alpha")),
    );
    let notices: std::sync::Arc<std::sync::Mutex<Vec<String>>> = Default::default();

    let engine = {
        let notices = notices.clone();
        fast_engine(stub_exe(), models.clone()).with_restart_notice(std::sync::Arc::new(
            move |message| notices.lock().unwrap().push(message),
        ))
    };
    engine.recognize(&tiny_image(), SourceLanguage::En).await.unwrap();
    wait_for_exit(&models, "config_en.txt");
    engine.recognize(&tiny_image(), SourceLanguage::En).await.unwrap();

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
    let models = fake_models(&dir, "config_en.txt", "CRASH_INIT\n");

    let engine = fast_engine(stub_exe(), models.clone());
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
    assert_eq!(spawn_count(&models, "config_en.txt"), 1);
}

#[tokio::test]
async fn an_unresponsive_sidecar_hits_the_request_timeout() {
    let dir = scratch_dir("hang");
    let models = fake_models(&dir, "config_en.txt", "HANG_REPLY\n");

    let engine = PaddleOcrEngine::with_timeouts(
        stub_exe(),
        models,
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
