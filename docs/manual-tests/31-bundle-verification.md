# Manual tests — Ticket 06 (pp-ocrv6-sidecar): the bundle and the release artifacts

The old PaddleOCR-json engine — its exe, runtime DLLs, and v3 per-language
model sets — left the product entirely with this ticket: no flag, no
"classic" mode. What ships is the PP-OCRv6 sidecar exe beside the ten
fetched model files (three tiers + the Korean PP-OCRv5 recognizer). The
engine surface itself (four options, tier/language restarts, substitution
notice, dev-layout fallback) is checklist `32-ppocrv6-engine-options.md`;
this checklist exercises the SHIPPED artifacts — installer, portable zip,
installed-layout fallback — and the odds and ends that outlived ticket 31's
original doc. Most items need a real desktop and a `cargo tauri build`.

## Setup

1. The bundle in place at `src-tauri/resources/pp-ocrv6/`:
   `powershell -File packaging/fetch-pp-ocrv6.ps1` (SHA-256-verified ten
   files) plus the sidecar exe beside them
   (`cargo build -p pp-ocrv6-sidecar --release`, then copy
   `target/release/pp-ocrv6-sidecar.exe` in — the release workflow's step).
2. `cargo tauri build` (Tauri CLI: `cargo install tauri-cli`). The NSIS
   installer lands at
   `target/release/bundle/nsis/Langception_<version>_x64-setup.exe`.

## Checklist

- [ ] **No old-engine artifacts anywhere**: the installer's staged
      `resources/` contains only `pp-ocrv6/`; after installing, the app
      directory has no `resources/paddleocr/`, no `PaddleOCR-json.exe`,
      and none of the old runtime DLLs (paddle_inference, mklml, mkldnn,
      opencv_world). A repo-wide search finds no reference to the deleted
      engine implementation (`paddle.rs` is gone; only the legacy config
      VALUE "paddle" remains, mapping to the small tier).
- [ ] **Installer carries the whole new bundle**: after installing,
      `resources/pp-ocrv6/` beside the exe holds all TEN contract files
      (tiny/small/medium det+rec+dicts, the Korean v5 recognizer and its
      dictionary) plus `pp-ocrv6-sidecar.exe` — and a bundled-engine
      selection reads text on a machine with NO Windows OCR language
      packs (the point of carrying the models).
- [ ] **Installer size vs the previous release**: record the new
      installer's size next to the last published release's — none
      exists yet, so the first post-switch release BECOMES the baseline.
      (The measured context from the switch itself: the raw payload
      SHRANK 121 MB, 311 → 190 MB, but the old v3 float32 models
      compressed ~3× under LZMA while the new ONNX set barely compresses,
      so the compressed installer is somewhat LARGER — see the ticket-06
      notes. The quality gain from ticket 01's harness bought that.)
- [ ] **Portable zip contains the new engine, all three tiers, and the
      Korean model**: assemble as the release workflow does (bare exe +
      README.txt + LICENSE.txt + `resources/pp-ocrv6/`), open the zip —
      the folder holds the ten files + sidecar exe, and nothing named
      paddle. On a clean machine (WebView2 present) the extracted exe
      starts and a bundled-engine selection reads text.
- [ ] **Broken-bundle fallback in the INSTALLED layout**: rename
      `resources/pp-ocrv6/` away from an installed (or portable) app,
      start translation with a bundled tier selected — every pass still
      reads text (Windows OCR fallback) and a warning says the engine
      could not run and why. Restore the folder; the next start reads
      through the sidecar again.
- [ ] **Clean exit, no orphan**: quit via tray → `pp-ocrv6-sidecar.exe`
      disappears from the process list (also check after closing the app
      from the taskbar while a pass is in flight).
- [ ] **Privacy surfaces**: with a bundled PP-OCRv6 tier running, the
      tray tooltip and Results Window show the ☁ badge only for cloud
      TRANSLATE engines — never for the OCR engine (it is fully offline).
