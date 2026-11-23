//! A scripted stand-in for the PaddleOCR-json sidecar, speaking the same
//! protocol (init banner → `OCR init completed.`, one-line tasks, one-line
//! replies, `exit` to quit) so the sidecar state machine in `paddle.rs` is
//! integration-tested offline. Only std — no async runtime needed.
//!
//! Scripting: the engine passes `-config_path=<file>` at launch; the stub
//! reads that file as its script — line 1 is a directive, the rest (joined)
//! is the reply printed for every task. Directives:
//! - `OK` — normal operation (also the fallback when the file is missing)
//! - `CRASH_INIT` — exit(1) before announcing readiness
//! - `CRASH_FIRST_REPLY` — answer the first task, then exit
//! - `HANG_REPLY` — never answer (for the request-timeout path)
//!
//! Each spawn also appends one `x` line to `<config_path>.spawns`, giving
//! tests a spawn counter for reuse/respawn assertions.

use std::fs::{self, OpenOptions};
use std::io::{self, BufRead, Write};
use std::process;
use std::thread;
use std::time::Duration;

const DEFAULT_REPLY: &str = "{\"code\":100,\"data\":[{\"box\":[[0,0],[10,0],[10,10],[0,10]],\"score\":1.0,\"text\":\"stub text\"}]}";

/// Appends one line to `<config_path>.exits` so tests can wait for the
/// process to be REALLY gone (the exit marker is written before the
/// process exits, closing the still-terminating race window).
fn record_exit(config_path: &Option<String>) {
    if let Some(config_path) = config_path {
        let exits = format!("{config_path}.exits");
        if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(&exits) {
            let _ = writeln!(file, "x");
        }
    }
}

fn main() {
    let config_path = std::env::args().find_map(|arg| {
        arg.strip_prefix("-config_path=").map(str::to_string)
    });
    let script = config_path
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

    if let Some(config_path) = &config_path {
        let spawns = format!("{config_path}.spawns");
        if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(&spawns) {
            let _ = writeln!(file, "x");
        }
    }

    if directive == "CRASH_INIT" {
        eprintln!("stub: scripted init crash");
        record_exit(&config_path);
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
                record_exit(&config_path);
                process::exit(0);
            }
            _ => println!("{reply}"),
        }
    }
    record_exit(&config_path);
}
