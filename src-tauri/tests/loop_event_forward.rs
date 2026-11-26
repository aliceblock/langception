//! The loop-event forward's regression test: one broadcast per event,
//! even while both consuming windows exist.
//!
//! History of the shape: the Results Window's history log is one entry per
//! received loop-event, and Tauri v2's `Emitter::emit` is an app-wide
//! broadcast whatever receiver it is called on (every listener hears every
//! emission). The old once-per-interested-window forward delivered each
//! Translated event twice as soon as the Debug Log window existed — the
//! user translated 3 times and the history page showed 6 entries, exactly
//! while the Debug Log window was open.
//!
//! Lives as an integration test on purpose (see
//! `shell::forward_test_seam`): the mock runtime's window machinery must
//! stay out of the unit-test binary, whose exe carries no app manifest.

use langception::forward_test_seam::{DEBUG_LABEL, RESULTS_LABEL, emit_loop_event, emit_pack_event, SETTINGS_LABEL};
use langception_core::model_packs::PackId;
use langception_core::orchestrator::{EventOrigin, LoopEvent, Stage, Via};
use langception_core::pack_downloader::PackEvent;
use langception_core::ports::TextBlock;
use std::sync::{Arc, Mutex};
use tauri::{Listener, WebviewUrl, WebviewWindowBuilder};

fn translated_event() -> LoopEvent {
    LoopEvent::Translated {
        key: "100,200".into(),
        block: TextBlock {
            x: 100,
            y: 200,
            width: 100,
            height: 20,
            text: "Hello".into(),
        },
        translation: "[TH] Hello".into(),
        via: Via::Engine,
        at_ms: 0,
        origin: EventOrigin::OneShot,
        style: None,
    }
}

fn error_event() -> LoopEvent {
    LoopEvent::Error {
        stage: Stage::Translate,
        message: "quota exhausted".into(),
        at_ms: 0,
    }
}

/// The mock app with the Results window alive and one `listen_any`
/// counter standing in for the windows' JS listeners: a broadcast reaches
/// it exactly once per emission, whichever window it is "sent from"
/// (Tauri's own manager tests pin that every listener hears every
/// emission).
fn forward_harness() -> (tauri::App<tauri::test::MockRuntime>, Arc<Mutex<usize>>) {
    let app = tauri::test::mock_app();
    WebviewWindowBuilder::new(&app, RESULTS_LABEL, WebviewUrl::default())
        .build()
        .unwrap();
    let deliveries = Arc::new(Mutex::new(0usize));
    let counter = Arc::clone(&deliveries);
    app.listen_any("loop-event", move |_| {
        *counter.lock().unwrap() += 1;
    });
    (app, deliveries)
}

/// The forward emits exactly ONE broadcast per Translated event even while
/// both consuming windows exist — the exact shape whose absence let the
/// history double.
#[test]
fn translated_events_forward_once_while_the_debug_log_window_is_open() {
    let (app, deliveries) = forward_harness();

    emit_loop_event(app.handle(), &translated_event());
    assert_eq!(*deliveries.lock().unwrap(), 1, "results-only: one broadcast");

    // The user opens the Debug Log window: a second interested window now
    // exists — this is where the history doubled.
    WebviewWindowBuilder::new(&app, DEBUG_LABEL, WebviewUrl::default())
        .build()
        .unwrap();
    emit_loop_event(app.handle(), &translated_event());
    assert_eq!(
        *deliveries.lock().unwrap(),
        2,
        "one broadcast per Translated event even with both windows open"
    );
}

/// Errors ride to both windows too; the same single-broadcast rule.
#[test]
fn error_events_forward_once_while_the_debug_log_window_is_open() {
    let (app, deliveries) = forward_harness();
    WebviewWindowBuilder::new(&app, DEBUG_LABEL, WebviewUrl::default())
        .build()
        .unwrap();

    emit_loop_event(app.handle(), &error_event());
    assert_eq!(
        *deliveries.lock().unwrap(),
        1,
        "one broadcast per Error event even with both windows open"
    );
}

/// The pack-event forward obeys the same emit discipline as the loop
/// events (deferred-model-packs ticket 04): ONE broadcast per event, and
/// only while a consuming window exists — the Engines tab is the only
/// download surface, so with the Settings window closed nothing emits at
/// all (the download continues regardless; the tab re-attaches to disk
/// truth on its next open).
#[test]
fn pack_events_forward_once_and_only_while_the_settings_window_exists() {
    let app = tauri::test::mock_app();
    let deliveries = Arc::new(Mutex::new(0usize));
    let counter = Arc::clone(&deliveries);
    app.listen_any("pack-event", move |_| {
        *counter.lock().unwrap() += 1;
    });
    let finished = PackEvent::Finished {
        pack: PackId::Small,
    };

    // No Settings window: nothing emits — the window-existence half.
    emit_pack_event(app.handle(), &finished);
    assert_eq!(
        *deliveries.lock().unwrap(),
        0,
        "no consuming window, no broadcast"
    );

    // The Settings window opens (the loop_event harness's one-listener
    // counter stands in for the page's JS listen): each event is ONE
    // broadcast, never one per receiver.
    WebviewWindowBuilder::new(&app, SETTINGS_LABEL, WebviewUrl::default())
        .build()
        .unwrap();
    emit_pack_event(app.handle(), &finished);
    emit_pack_event(app.handle(), &finished);
    assert_eq!(
        *deliveries.lock().unwrap(),
        2,
        "one broadcast per pack event while the window exists"
    );
}
