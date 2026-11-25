// Adapter types are public so the opt-in manual smoke test can drive them
// directly (tests/manual_smoke.rs).
pub mod capture;
pub mod fonts;
pub mod foreground;
pub mod logging;
#[cfg(windows)]
pub mod ocr;
#[cfg(windows)]
pub mod ocr_detect;
#[cfg(windows)]
pub mod ocr_fallback;
mod overlay;
pub mod settings;
mod shell;

// Re-exported for the integration tests (tests/): the loop-event forward
// regression test constructs core event payloads through the same public
// seam the pipeline uses.
pub use langception_core;
// `shell` itself stays private; only its documented test seam is public.
#[doc(hidden)]
pub use shell::forward_test_seam;

pub fn run() {
    shell::run();
}
