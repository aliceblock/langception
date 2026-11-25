fn main() {
    tauri_build::build();
    // Tauri's Windows resource (the app manifest: Common-Controls v6,
    // DPI awareness, the icon) rides `rustc-link-arg-bins` — the app exe
    // only. A lib test exe that links any menu/tray object file imports
    // TaskDialogIndirect, a comctl32 v6 export: without the manifest the
    // loader binds comctl32 v5 and the test process dies before main with
    // STATUS_ENTRYPOINT_NOT_FOUND (exactly the trap the loop-event
    // forward's regression test hit). Test exes get the same resource —
    // the file tauri_build just compiled into OUT_DIR.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let out_dir = std::env::var("OUT_DIR").expect("cargo sets OUT_DIR");
        let resource = std::path::Path::new(&out_dir).join("resource.lib");
        if resource.exists() {
            // The `-tests` scope only: integration test exes that drive the
            // mock runtime's window machinery link muda's Common-Controls
            // v6 menu code (TaskDialogIndirect — a comctl32 v6 export), and
            // without the manifest the loader binds comctl32 v5 and dies
            // before main with STATUS_ENTRYPOINT_NOT_FOUND. The unscoped
            // form here is NOT an option — it would link the resource into
            // the app exe a second time (CVT1100 duplicate VERSION), which
            // is why the lib unit tests keep the mock runtime's windows out
            // of reach and the forward test lives in tests/.
            println!("cargo:rustc-link-arg-tests={}", resource.display());
        } else {
            println!(
                "cargo:warning=tauri_build left no OUT_DIR/resource.lib — test exes miss the Common-Controls v6 manifest"
            );
        }
    }
}
