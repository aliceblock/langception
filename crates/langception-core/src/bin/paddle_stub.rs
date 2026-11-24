//! A scripted stand-in for both bundled OCR sidecars (PaddleOCR-json and
//! pp-ocrv6-sidecar), speaking the same protocol (init banner → `OCR init
//! completed.`, one-line tasks, one-line replies, `exit` to quit) so the
//! sidecar state machine in `sidecar.rs` is integration-tested offline
//! through both engines. Only std — no async runtime needed.
//!
//! Scripting: the stub locates its script from the engine's launch
//! arguments — either `-config_path=<file>` (the old engine) or
//! `-models_path=<dir>` with `-language=<l>` and `-tier=<t>`, for which
//! it reads `<dir>/stub-<l>-<t>.txt` (the new engine; different model
//! selections read different scripts, which is how tests prove a
//! restart). The file's line 1 is a directive, the rest (joined) is the
//! reply printed for every task. Directives:
//! - `OK` — normal operation (also the fallback when the file is missing)
//! - `CRASH_INIT` — exit(1) before announcing readiness
//! - `CRASH_FIRST_REPLY` — answer the first task, then exit
//! - `HANG_REPLY` — never answer (for the request-timeout path)
//!
//! Each spawn also appends one `x` line to `<script>.spawns`, giving
//! tests a spawn counter for reuse/respawn assertions.

use std::fs::{self, OpenOptions};
use std::io::{self, BufRead, Write};
use std::process;
use std::thread;
use std::time::Duration;

const DEFAULT_REPLY: &str = "{\"code\":100,\"data\":[{\"box\":[[0,0],[10,0],[10,10],[0,10]],\"score\":1.0,\"text\":\"stub text\"}]}";

/// The script file this launch reads: `-config_path=` wins; otherwise the
/// new engine's `-models_path` + `-language` + `-tier` select
/// `<models>/stub-<language>-<tier>.txt`. None when no known shape is
/// present (the script then defaults to `OK`, no markers written).
fn script_path() -> Option<String> {
    let args: Vec<String> = std::env::args().collect();
    if let Some(config) = args.iter().find_map(|arg| {
        arg.strip_prefix("-config_path=").map(str::to_string)
    }) {
        return Some(config);
    }
    let models = args
        .iter()
        .find_map(|arg| arg.strip_prefix("-models_path=").map(str::to_string))?;
    let language = args
        .iter()
        .find_map(|arg| arg.strip_prefix("-language=").map(str::to_string))?;
    let tier = args
        .iter()
        .find_map(|arg| arg.strip_prefix("-tier=").map(str::to_string))?;
    Some(format!("{models}/stub-{language}-{tier}.txt"))
}

/// Appends one line to `<script>.exits` so tests can wait for the
/// process to be REALLY gone (the exit marker is written before the
/// process exits, closing the still-terminating race window).
fn record_exit(script: &Option<String>) {
    if let Some(script) = script {
        let exits = format!("{script}.exits");
        if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(&exits) {
            let _ = writeln!(file, "x");
        }
    }
}

fn main() {
    let script_file = script_path();
    let script = script_file
        .as_deref()
        .and_then(|path| fs::read_to_string(path).ok())
        .unwrap_or_default();
    let mut lines = script.lines();
    let directive = lines.next().unwrap_or("OK").trim().to_string();
    let reply: String = lines
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    let reply = if reply.is_empty() {
        DEFAULT_REPLY.to_string()
    } else {
        reply
    };

    if let Some(path) = &script_file {
        let spawns = format!("{path}.spawns");
        if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(&spawns) {
            let _ = writeln!(file, "x");
        }
    }

    if directive == "CRASH_INIT" {
        eprintln!("stub: scripted init crash");
        record_exit(&script_file);
        process::exit(1);
    }
    println!("OCR init completed.");

    let stdin = io::stdin();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        let line = line.trim();
        if line == "exit" {
            break;
        }
        if line.is_empty() {
            continue;
        }
        match directive.as_str() {
            "HANG_REPLY" => loop {
                thread::sleep(Duration::from_secs(3600));
            },
            "CRASH_FIRST_REPLY" => {
                println!("{reply}");
                let _ = io::stdout().flush();
                record_exit(&script_file);
                process::exit(0);
            }
            _ => println!("{reply}"),
        }
    }
    record_exit(&script_file);
}
