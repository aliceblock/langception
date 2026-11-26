# Manual tests — Ticket 04 (pp-ocrv6-sidecar): the four OCR engine options

The new engine's lifecycle (lazy spawn, reuse, restart on model change,
crash recovery, timeouts) is covered offline by `cargo test` against the
scripted stub (`ppocr_sidecar_tests`); this checklist exercises the REAL
sidecar, the REAL pack downloads, and the packaging — everything that
needs a live desktop and the fetched bundle (the installer/portable
artifact checks live in `31-bundle-verification.md`).

Since deferred-model-packs (ADR-0008) the Engines tab is also the Model
Pack download surface: tiny ships with the app, while small, medium, and
the Korean recognizer set download on first selection — with visible
progress, Cancel/Retry, hash verification, and a delete-unused button.
Those flows are this checklist's second half (deferred-model-packs
ticket 06's end-to-end pass): download states, cancel/retry, the
substitution and Korean pulls, cleanup, and the storage pointers.

## Setup

1. The bundle (git-ignored) must be in place at
   `src-tauri/resources/pp-ocrv6/`:
   `powershell -File packaging/fetch-pp-ocrv6.ps1` fetches and
   SHA-256-verifies all ten model files (into a work dir under
   `packaging/stage/`), stages the tiny-only bundle — the three Tiny
   files plus the generated `models-manifest.json` — into the bundle
   dir, and builds the three Model Pack archives into `packaging/`.
   The sidecar exe comes from
   `cargo build -p pp-ocrv6-sidecar` (the workspace target dir is one of
   the app's resolution candidates in dev).
   Because the bundle carries the manifest, a `cargo run` dev app speaks
   packs: the active models directory is
   `%LOCALAPPDATA%\langception\models` (a dev exe has no portable
   marker) — delete that directory first for a clean-machine pass.
2. Optional headless check (live desktop needed):
   `cargo test -p langception --test manual_smoke pp_ocrv6_smoke -- --ignored --nocapture`
   proves spawn → handshake → capture → parsed blocks through the REAL
   engine on the REAL Tiny bundle (passes: en fresh, en reuse; the
   ja-restart pass self-skips with a note until the directory holds the
   Small set — point `LANGCEPTION_PPOCRV6_DIR` at an active models
   directory with the Small pack to run it).
3. Where downloads come from on a pre-release checkout: the shipped
   manifest's primary URL names this release's own asset
   (`.../releases/download/v<version>/pp-ocrv6-<pack>.zip`), so a
   version that is not cut yet 404s, and the pinned upstream fallbacks
   serve raw model files, not pack archives — every source fails. To
   exercise the SUCCESS path before cutting the release, serve the built
   archive locally and put that URL first in the bundle's
   `models-manifest.json` (git-ignored — regenerate with the fetch
   script afterwards): e.g. `python -m http.server 8765 --directory
   packaging`, then set the pack under test's first URL to
   `http://127.0.0.1:8765/pp-ocrv6-small.zip`. On a cut release the
   primary URL is real and no override is needed.
4. Fresh config or your usual `config.toml`, `cargo run`. Display some
   text on screen (any window).

## Checklist

- [ ] **Dropdown shows the four options, each with its pack state**:
      Settings → Engines → OCR engine lists Windows OCR (selected by
      default), PP-OCRv6 tiny, PP-OCRv6 small, PP-OCRv6 medium — each
      label names its tier, and tiny's label says it ships with the app.
      Small/medium carry their pack state as a label suffix: "not
      downloaded" on a clean machine, "downloading N%" while a download
      runs, "download failed" after a failure, "ready" once the pack
      verifies on disk, "update pending" when a downloaded file no
      longer matches the manifest.
- [ ] **The pack panel shows one row per required pack**: picking a
      tier under a Source Language shows rows for exactly the packs the
      EFFECTIVE selection needs — en/zh+tiny shows none (just the
      "Ships with the app" note); ja+tiny shows the Small set (the
      requirement follows the substitution); ko+tiny shows the Korean
      recognizer set (tiny's bundled detector is the detection file);
      ko+small shows Small + Korean. Row buttons follow the state:
      Download (missing/update pending), progress bar + Cancel
      (downloading — the fill animates when the source declares no
      total), Retry with the failure message on the row tooltip
      (failed), quiet (ready). The panel hides only for a Windows-OCR
      user with nothing on disk; a Windows user WITH packs still sees
      it (Delete unused packs must reach them).
- [ ] **Legacy "paddle" config maps to small — packs included**:
      hand-edit `config.toml`'s `[ocr] engine = "paddle"`, start
      translation — the log line says `ocr=pp-ocrv6-small`; on a clean
      machine the Small pack downloads first (same flow as the first-use
      arm below), then passes read. Open Settings → Engines — the
      dropdown displays PP-OCRv6 small; saving writes
      `engine = "pp-ocrv6-small"`.
- [ ] **Default untouched (story 17)**: with Windows OCR selected,
      everything behaves exactly as before this feature — no sidecar
      process, no pack rows, no download, the launch log says
      `ocr=windows`.
- [ ] **First selection downloads the pack (stories 3/4)**: on a clean
      active dir, select PP-OCRv6 small — the tier is honored
      immediately (the selection saves before the pack exists), and the
      download starts as a consequence of the selection: the option
      label flips to "downloading N%", the row shows the progress bar +
      Cancel, and the app log gets a "pack download started" line (one
      per source attempt, each with its declared total). When it lands:
      "pack download finished" in the log, the option reads "ready", and
      the next pass reads text through the sidecar
      (`ocr=pp-ocrv6-small`). Nothing downloads for tiers nobody
      selected.
- [ ] **Passes during a download report loading, never Windows OCR
      (story 18)**: while that pack downloads, run a pass — the
      recovery-notice surfaces (status warnings + app log) say
      "PP-OCRv6 small is loading — its model pack (Small set) is not
      ready yet. Passes wait for the download instead of using Windows
      OCR; reading starts automatically once it lands." (Thai UI: the
      Thai wording.) Reading starts by itself when the download
      finishes — no restart, no engine switch, no fallback.
- [ ] **Cancel mid-download (story 6)**: hit Cancel on a downloading
      row (medium's ~96 MB gives time; or throttle the local server) —
      the option returns to "not downloaded", the row loses its progress
      bar, and nothing partial is left on disk: the models dir gains no
      file, and the download's `.langception-`-prefixed temp — which
      lives BESIDE the models dir, never inside it — is gone. A cancel
      during a stalled source or a mirrored attempt lands instantly.
      Selecting the tier again downloads from scratch (no byte-resume,
      per ADR-0008).
- [ ] **Closing Settings mid-download cancels nothing (story 20)**:
      start a download, close Settings (app stays in the tray), wait,
      reopen the Engines tab — the tab re-queried disk + downloader
      truth on activation: the download still runs (or finished), the
      row shows the true percent — never a replayed or fabricated state.
- [ ] **Failed download + Retry (story 5)**: point the manifest's URLs
      at unreachable sources (or drop the network) and select a tier —
      the option shows "download failed", the row tooltip carries the
      downloader's message (the last source's verdict), and passes keep
      reporting the loading notice — the loop never falls back to
      Windows OCR on its own. Retry restarts from zero; once a source
      serves the verified archive, the pack lands and passes resume.
- [ ] **Update pending heals by re-download (story 22)**: flip one byte
      (same size) in a downloaded pack file inside the models dir, then
      press that pack row's Download (or switch tiers away and back) —
      the option reads "update pending" until the re-download replaces
      the stale file, then "ready" again. A corrupted file of a pack in
      CURRENT use is caught before use within the gate's rotation window
      (one pinned file re-hashed per pass): passes report loading again
      instead of feeding the engine bad bytes.
- [ ] **Delete unused packs (stories 14/15)**: with several packs on
      disk (say Small + Medium + Korean) and a selection needing fewer
      (en+tiny), press Delete unused packs — the confirm names the packs
      ("Delete the packs the current selection does not use (…)?
      Selecting them again re-downloads from scratch."), then the note
      says "Deleted N file(s)."; the models dir holds only the bundled
      baseline. A pack with a download in flight is NOT deleted (Cancel
      makes it deletable). With everything on disk in use, the button
      answers "No unused packs — everything on disk is in use or ships
      with the app."
- [ ] **Tier change restarts the sidecar**: with the loop running,
      switch small → medium (its pack downloads first if missing — wait
      for "ready") and save — the next capture reloads models (slower
      first pass) and the log names the restart; Task Manager shows one
      `pp-ocrv6-sidecar.exe` at a time, replaced on the switch.
- [ ] **Language switch restarts too**: cycle the Source Language
      en → ja → ko — each restarts the sidecar on that language's models
      and reads its text (Korean reads through the official Korean
      PP-OCRv5 model at every tier — same behavior whatever the tier).
- [ ] **ja+tiny pulls the Small set, then surfaces the substitution
      notice (story 10)**: select PP-OCRv6 tiny and set the Source
      Language to ja — the Small pack downloads (the row list names it:
      the requirement follows the effective selection); once ready, the
      app log gets one `ocr` line saying tiny cannot read Japanese (no
      kana) and the small model set runs instead; kana text reads
      correctly. One line per substituted session: switching to en and
      back to ja produces a second line (each transition is its own
      incident), and so does saving any setting that rebuilds the loop.
      A failing pass stays silent here — the fallback warning covers
      broken bundles. en/zh with tiny fire no line and no download.
- [ ] **The Korean set downloads once, shared across tiers (stories
      11/12)**: with ko as the Source Language, select tiny — only the
      Korean pack downloads; switch tiny → small — the Small pack joins
      it and the Korean recognizer set is NOT re-downloaded (its row
      already reads ready); switch small → medium — same, only Medium's
      pack arrives. Korean keeps working identically at every tier (the
      sidecar launch line names the same Korean recognizer each time)
      and fires no notice.
- [ ] **Storage follows the build shape (stories 16/17)**: a dev or
      installed build lands packs in `%LOCALAPPDATA%\langception\models`;
      the portable build lands them in a `models` folder beside the exe,
      and moving that folder carries the downloaded packs with it — the
      artifact-level checks are `31-bundle-verification.md`'s storage
      arms.
- [ ] **No console flash**: the sidecar runs hidden — no terminal window
      appears with the app.
- [ ] **Crash → surfaced, then recovery**: kill `pp-ocrv6-sidecar.exe` in
      Task Manager. No hang, no app crash, and the death is visible: the
      app log gets an `ocr` line ("exited unexpectedly — restarting")
      and the next pass respawns it. One line per incident, not one per
      pass.
- [ ] **Missing bundle → fallback**: rename `src-tauri/resources/pp-ocrv6/`
      away and start translation with PP-OCRv6 selected — every pass
      still reads text (Windows OCR fallback) and a warning says
      PP-OCRv6 could not run and why. The distinction to feel: a missing
      BUNDLE is a failure the fallback covers loudly; a missing PACK is
      a loading state the loop waits through, never a fallback.
- [ ] **Debug Log Window (stories 12/26)**: per-pass OCR timing still
      shows; the engine/tier is visible from the selection (the launch
      line) with no new field. Pack downloads are answerable from the
      log with the Engines tab closed: started/finished/failed/canceled
      lines under the `packs` channel (progress percent stays UI-only).
- [ ] **Wizard untouched**: the first-run wizard still flows through the
      Windows-OCR language-pack check; it never mentions tiers.
- [ ] **Config roundtrip**: switch tiers in Settings → Engines, save,
      reopen — the choice persisted in `[ocr] engine`; an `[ocr]` section
      missing entirely (pre-ticket-31 config) still loads as windows.
