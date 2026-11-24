//! A scripted stand-in for the bundled OCR sidecar (pp-ocrv6-sidecar),
//! speaking its protocol (init banner → `OCR init completed.`, one-line
//! tasks, one-line replies, `exit` to quit) so the sidecar state machine
//! in `sidecar.rs` is integration-tested offline. Only std — no async
//! runtime needed.
//!
//! Scripting: the stub reads its script from the engine's launch
//! arguments — `-models_path=<dir>` with `-language=<l>` and `-tier=<t>`
//! select `<dir>/stub-<l>-<t>.txt`; different model selections read
//! different scripts, which is how tests prove a restart. The file's
//! line 1 is a directive, the rest (joined) is the reply printed for
//! every task. Directives:
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

/// The script file this launch reads: the `-models_path` + `-language` +
/// `-tier` launch arguments select `<models>/stub-<language>-<tier>.txt`.
/// None when the known shape is absent (the script then defaults to `OK`,
/// no markers written).
fn script_path() -> Option<String> {
    let args: Vec<String> = std::env::args().collect();
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

/// Appends one `x` line to `<path>` — the marker files
/// (`<script>.spawns` / `<script>.exits`) are line counters tests read
/// back to assert reuse, respawn, and real process death.
fn append_marker(path: &str) {
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(file, "x");
    }
}

/// Records the exit in `<script>.exits` so tests can wait for the
/// process to be REALLY gone (the exit marker is written before the
/// process exits, closing the still-terminating race window).
fn record_exit(script: &Option<String>) {
    if let Some(script) = script {
        append_marker(&format!("{script}.exits"));
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
        append_marker(&format!("{path}.spawns"));
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
