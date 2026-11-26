# Manual tests — Ticket 06 (pp-ocrv6-sidecar): the bundle and the release artifacts

The old PaddleOCR-json engine — its exe, runtime DLLs, and v3 per-language
model sets — left the product entirely with this ticket: no flag, no
"classic" mode. Since the deferred-model-packs feature (ADR-0008, its
ticket 05) the shipped bundle is TINY-ONLY: the sidecar exe beside the
three Tiny model files and the generated `models-manifest.json` —
Small, Medium, and the Korean recognizer ship as Model Pack archives
attached to the GitHub Release and download on first selection. The
engine surface itself (four options, tier/language restarts, substitution
notice, dev-layout fallback) is checklist `32-ppocrv6-engine-options.md`;
this checklist exercises the SHIPPED artifacts — installer, portable zip,
pack archives, installed-layout fallback — and the odds and ends that
outlived ticket 31's original doc. Most items need a real desktop and a
`cargo tauri build`. (The deferred-model-packs end-to-end pass — full
download states, cancel/retry/delete flows, sizes recorded against the
baseline — is that feature's own ticket 06; this doc pins the artifact
truths.)

## Setup

1. The tiny-only bundle in place at `src-tauri/resources/pp-ocrv6/`:
   `powershell -File packaging/fetch-pp-ocrv6.ps1` — fetches and
   SHA-256-verifies all ten model files into a work dir, stages ONLY
   Tiny's three plus the generated `models-manifest.json` into the
   bundle, and builds the three Model Pack archives into `packaging/`
   (roundtrip-verified). Plus the sidecar exe beside them
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
- [ ] **Installer carries the tiny-only bundle**: after installing,
      `resources/pp-ocrv6/` beside the exe holds exactly the three Tiny
      files (det + rec + dict), `models-manifest.json`,
      `pp-ocrv6-sidecar.exe`, and the license file — and NOTHING named
      small, medium, or korean. A PP-OCRv6 tiny selection reads English
      and Chinese on a machine with NO Windows OCR language packs and no
      network (the offline baseline the bundle exists for).
- [ ] **The manifest ships inside the bundle and its pins verify**: the
      installed `models-manifest.json` names the three bundled files
      under `bundled` (the app hash-checks them like a download —
      see the corrupted-bundle arm below) and per pack an ordered URL
      list whose FIRST entry is this release's own asset
      `.../releases/download/v<version>/pp-ocrv6-<pack>.zip`.
- [ ] **The release carries the three pack archives**: the tag's GitHub
      Release lists `pp-ocrv6-small.zip`, `pp-ocrv6-medium.zip`,
      `pp-ocrv6-korean.zip` beside the installer and portable zip.
      Each archive unpacks to exactly its manifest file list, flat (the
      build script roundtrip-verifies this; this arm is the release
      artifact being the same file).
- [ ] **Pack flow on a clean install**: with Windows OCR unusable (no
      language packs), select PP-OCRv6 small — the tier is honored
      immediately, the Engines tab shows the pack as not downloaded and
      the download starts as a consequence of the selection; when it
      finishes, passes read text through the sidecar. The app fetched
      the release asset named by the manifest (offline machine: the
      download fails with a Retry instead of silently falling back to
      Windows OCR).
- [ ] **Corrupted bundled file is caught, not silently used**: in the
      INSTALLED layout, flip one byte inside a bundled Tiny file in
      `resources/pp-ocrv6/` (same size — e.g. overwrite one byte in
      `ppocrv6_tiny_dict.txt`), start translation — the sidecar never
      runs on the corrupt bytes: every pass reads through the Windows
      OCR fallback with a loud warning saying the bundled engine failed
      verification and could not run (a mutated bundle is Broken —
      nothing a download fixes — so unlike a still-downloading pack the
      fallback DOES cover the pass). Restore the file; the next start
      reads through the sidecar again.
- [ ] **Installer size vs the previous release**: record the new
      installer's size next to the last published release's — the
      deferred-model-packs feature's ticket 06 turns this into a
      recorded comparison against the v0.2.1 baseline (the bundle
      shrank by the Small/Medium/Korean models, ~170 MB of ONNX).
- [ ] **Portable zip contains the tiny-only engine and nothing more**:
      assemble as the release workflow does (bare exe + README.txt +
      LICENSE.txt + `resources/pp-ocrv6/`), open the zip — the folder
      holds the three Tiny files + the manifest + sidecar exe, and
      nothing named small, medium, korean, or paddle. On a clean machine
      (WebView2 present) the extracted exe starts and a tiny selection
      reads text.
- [ ] **Portable first pack download creates `models/` beside the
      exe**: run the portable app, select PP-OCRv6 small, let the
      download finish — a `models` folder now exists next to
      `Langception.exe` (the zip's README.txt explains it), and moving
      the whole folder elsewhere carries the downloaded pack with it.
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
