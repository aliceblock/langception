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
download states, cancel/retry/delete flows — is that feature's half of
`32-ppocrv6-engine-options.md`; the sizes this doc once deferred to that
pass are recorded at the bottom, measured when that feature's ticket 06
landed.)

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
      INSTALLED layout on a FRESH install (no
      `%LOCALAPPDATA%\langception\models` yet — delete it if the app
      already ran), flip one byte inside a bundled Tiny file in
      `resources/pp-ocrv6/` (same size — e.g. overwrite one byte in
      `ppocrv6_tiny_dict.txt`), start translation — the sidecar never
      runs on the corrupt bytes: every pass reads through the Windows
      OCR fallback with a loud warning saying the bundled engine failed
      verification and could not run (a mutated bundle is Broken —
      nothing a download fixes — so unlike a still-downloading pack the
      fallback DOES cover the pass). Restore the file; the next start
      reads through the sidecar again.
- [ ] **Installed first pack download lands in the local app-data
      directory**: run the INSTALLED app, select PP-OCRv6 small, let the
      download finish — the pack files land in
      `%LOCALAPPDATA%\langception\models` (not beside the exe — installs
      keep packs like any app data, so they survive app updates; the
      NSIS uninstaller leaves them in place). The bundled baseline is
      copied into the same directory on the first gated engine start.
- [ ] **Installer size vs the previous release**: the bundle-shrink
      comparison is recorded below (measured when deferred-model-packs
      ticket 06 landed); at each later release cut, re-check the
      installer stays tiny-only sized (tens of MB, not ~143 MB).
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

## Recorded sizes vs the v0.2.1 baseline

Measured 2026-11-26 (deferred-model-packs ticket 06) from a local
`cargo tauri build` of this branch, the portable zip assembled exactly as
`release.yml` does it, and the pack archives from the fetch script's
staging — all from the same verified bytes. The version string was still
0.2.1 (the release bump happens at cut time); the v0.2.1 baseline numbers
are the published release's asset sizes.

| Artifact | This feature's build | v0.2.1 baseline | Delta |
| --- | --- | --- | --- |
| `Langception_*_x64-setup.exe` (NSIS installer) | 18,152,807 B (17.3 MiB) | 149,960,243 B (143.0 MiB) | −87.9% |
| `Langception_*_x64-portable.zip` | 23,261,704 B (22.2 MiB) | 158,289,620 B (150.9 MiB) | −85.3% |
| `pp-ocrv6-small.zip` (download size) | 26,094,836 B (24.9 MiB) | — | new asset |
| `pp-ocrv6-medium.zip` (download size) | 100,552,327 B (95.9 MiB) | — | new asset |
| `pp-ocrv6-korean.zip` (download size) | 8,327,624 B (7.9 MiB) | — | new asset |

The installer and portable zip no longer carry the Small/Medium/Korean
ONNX (~183 MB raw, ~131 MB compressed inside the old NSIS payload);
instead the user downloads only the packs a selection needs. The worst
case — every pack fetched — moves the same bytes, just not up front
(ADR-0008's trade). The download sizes above are what the README quotes;
ADR-0008's ~31 / ~139 / ~13.5 MB are the raw set sizes before Deflate —
both measures are true, each doc says which one it means.

Also verified on this build: the portable zip holds exactly nine entries
(exe, README.txt, LICENSE.txt, `resources/pp-ocrv6/` with the three Tiny
files + manifest + sidecar exe + license) and nothing named small,
medium, korean, or paddle; the workspace test suite (695 passed / 0
failed) and clippy are green on a side target dir; the `pp_ocrv6_smoke`
headless check runs
the REAL sidecar through the real Tiny bundle (spawn → handshake →
capture → parsed blocks, pass reuse, and the Japanese pass's skip note on
a pack-free dir).
