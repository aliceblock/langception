# Manual tests — Ticket 49: Debug log export (share what the window saw)

There is no automated seam for this ticket: the export's logic lives in
the debug window's vanilla JS (no test runner exists for the UI) and the
Rust side is a bare `fs::write`. This checklist exercises everything the
acceptance criteria pin, against a live session.

## Setup

1. Build and run the dev app: `cargo run -p langception` (or your usual
   `tauri dev` flow).
2. Open **Debug Log Window…** from the tray.
3. Draw a Capture Zone over a window with changing text, start
   translation (an LLM engine makes the rows and traces interesting).
4. Let it run a bit: some translated passes, an idle stretch (unchanged
   collapse), a One-shot (Alt+Shift+Q), and — to get an error row — briefly
   break the engine key.

## Checklist

- [ ] **Hidden while empty**: on a freshly opened window (before any
      pass) there is no Export… button — just "waiting for the first
      pass…". It appears with the first event.
- [ ] **Save dialog → file**: clicking **Export…** opens the native save
      dialog, default name `langception-debug-YYYYMMDD-HHMMSS.txt` with a
      Text filter; the file is written to the picked path and the header
      flashes a transient **Saved ✓**.
- [ ] **Header contents**: the file starts with app version (matching
      About), exported-at, window-opened-at (matching when you opened the
      window), the pass count (matching the header bar), and the covered
      event time range.
- [ ] **Summary matches the header bar**: the `per-stage summary` lines
      carry the same avg/min/max ×n numbers as the window's chips, and
      the same stage is marked `← slowest`.
- [ ] **Every visible row is in the file**: full rows with all their
      stage ms (`◂` on the pass's slowest stage), collapsed unchanged
      runs as one `unchanged ×N · capture ~Xms` block with count and
      capture ms matching the window, fast-path rows carrying
      **OCR skipped — frame unchanged**, One-shot rows badged `One-shot`.
- [ ] **Read traces verbatim**: a pass's `reads (N):` lines show the same
      words (curly quotes) and plain-words reasons as the window's
      expanded lines — waiting with elapsed/total, gone with miss counts,
      suppressed naming the text; continuation reads aligned under the
      first.
- [ ] **Errors inline**: the error line (`--- <time> · error · <stage> ·
      <message>`) sits in stream order, between the passes it
      interrupted.
- [ ] **Paste fidelity**: open the file in a plain text editor and paste
      it into a chat — readable chronological text, one block per pass,
      no markup.
- [ ] **Snapshot-stable re-export**: export twice in a row without new
      events between; diff the two files — identical except the
      `exported at` line (the body is a pure function of the history; no
      DOM dependence).
- [ ] **Repeated exports**: export again after more events — the dialog
      reopens with a fresh default name, the new file contains the new
      events, "Saved ✓" flashes each time.
- [ ] **Failure surfaces**: export to a path that cannot be written
      (e.g. a read-only file's folder) — the header shows the failure
      message in red instead of "Saved ✓", and the window keeps working.
- [ ] **Trimmed start is self-describing**: with 5000+ entries in the
      history (a long idle run of unchanged ticks helps), the covered
      range in the header describes only what the file actually holds.
- [ ] **Loop untouched**: while the window is open and exporting, the
      loop behaves exactly as before — same translations at the same
      times in the Results Window/overlays; nothing is written anywhere
      automatically (only `app-*.log` content as before, no new files
      until you click Export…).
- [ ] **Closed window is inert**: with the debug window closed, the loop,
      overlays, and Results Window are unaffected and no export files
      appear anywhere.

## Cleanup

- Stop translation, quit the app, delete the exported `.txt` files,
  restore any changed `config.toml` (engine key).
