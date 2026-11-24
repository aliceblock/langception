//! Langception's OCR sidecar (ticket 02 of the pp-ocrv6-sidecar feature):
//! a binary we own, built on the oar-ocr pipeline crate, speaking the
//! bundled engine's JSON-lines pipe protocol — one
//! `{"image_base64": "<base64 PNG>"}` task line in, one reply line out —
//! so the app's existing sidecar machinery (spawn, handshake, timeouts,
//! crash recovery) works against it unchanged (ADR-0005).
//!
//! Models are fixed at launch by Source Language + Model Tier (see
//! `model_selection`); a language or tier change is a sidecar restart, the
//! same rule the bundled engine follows today. The launch contract —
//! arguments and models-dir layout — is documented in the ticket and
//! parsed in `args`.
//!
//! ```text
//! pp-ocrv6-sidecar.exe -models_path=<dir> -language=<en|ja|zh|ko> -tier=<tiny|small|medium>
//! ```
//!
//! After its models load, the sidecar prints `OCR init completed` on
//! stdout (the recorded engine's readiness marker — the client scans for
//! it), serves tasks until stdin closes, then exits 0. Malformed tasks and
//! pipeline failures are error replies, not exits; diagnostics go to
//! stderr, which the client drains.

mod args;
mod model_selection;
mod pipeline;
mod protocol;

use std::io::{BufRead, Write};

fn main() {
    let launch = match args::parse_args(std::env::args().skip(1)) {
        Ok(launch) => launch,
        Err(e) => {
            eprintln!("pp-ocrv6-sidecar: {e}");
            eprintln!(
                "usage: pp-ocrv6-sidecar -models_path=<dir> -language=<en|ja|zh|ko> \
                 -tier=<tiny|small|medium>"
            );
            std::process::exit(2);
        }
    };

    let selection = model_selection::selection_for(launch.language, launch.tier);
    // Which models this process serves — the first thing to read when a
    // user reports one engine tier misbehaving.
    eprintln!(
        "pp-ocrv6-sidecar: language={} tier={} (det {} rec {} dict {}) models_dir {}",
        launch.language.as_str(),
        launch.tier.as_str(),
        selection.detection,
        selection.recognition,
        selection.dictionary,
        launch.models_path.display()
    );

    let pipeline = match pipeline::build_pipeline(
        &launch.models_path,
        &selection,
        model_selection::thresholds_for(launch.tier),
    ) {
        Ok(pipeline) => pipeline,
        Err(e) => {
            eprintln!("pp-ocrv6-sidecar: {e}");
            std::process::exit(1);
        }
    };
    println!("OCR init completed");

    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    let mut line = String::new();
    loop {
        line.clear();
        match stdin.lock().read_line(&mut line) {
            // EOF: the parent closed the pipe — leave cleanly.
            Ok(0) => break,
            Ok(_) => {}
            Err(e) => {
                eprintln!("pp-ocrv6-sidecar: failed to read the task: {e}");
                std::process::exit(1);
            }
        }
        let task = line.trim();
        if task.is_empty() {
            continue;
        }
        // A failed task is an error reply, not an exit: the client
        // surfaces the message for that pass and keeps using the sidecar
        // (the bundled engine behaves the same on bad input).
        let reply = match protocol::parse_task(task) {
            Err(message) => protocol::error_reply(protocol::TASK_ERROR, &message),
            Ok(image) => match pipeline::run_once(&pipeline, &image) {
                Ok(regions) if regions.is_empty() => protocol::no_text_reply(),
                Ok(regions) => protocol::success_reply(&regions),
                Err(message) => protocol::error_reply(protocol::PIPELINE_ERROR, &message),
            },
        };
        if let Err(e) = writeln!(stdout, "{reply}").and_then(|_| stdout.flush()) {
            // A broken stdout pipe means the parent is gone; keep the
            // diagnostic and leave.
            eprintln!("pp-ocrv6-sidecar: failed to write the reply: {e}");
            std::process::exit(1);
        }
    }
}
