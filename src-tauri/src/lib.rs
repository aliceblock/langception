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

pub fn run() {
    shell::run();
}
