// Adapter types are public so the opt-in manual smoke test can drive them
// directly (tests/manual_smoke.rs).
pub mod capture;
#[cfg(windows)]
pub mod ocr;
mod shell;

pub fn run() {
    shell::run();
}
