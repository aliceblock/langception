# Manual tests — Ticket 47: Debug log window (live per-stage timing)

The mechanics are covered offline by `cargo test`: the timing events carry
pinned fake-clock durations (capture + ocr + translate summing to the
total), unchanged ticks emit `blocks_translated: 0`, fused passes report
the fused stage (no ocr/translate), a matured fast-path fire charges its
translate time to its own pass, and one-shot passes emit timing rows like
zone passes. This checklist exercises what only a live desktop shows: the
window itself — its rows, collapse behavior, summary, and the guarantee
that opening/closing it never touches the loop.

## Setup

1. Build and run the dev app: `cargo run -p langception` (or your usual
   `tauri dev` flow).
2. Draw a Capture Zone over a window with changing text (e.g. a terminal
   running a script that prints lines periodically), start translation.

## Checklist

- [ ] **Tray entry**: the tray menu shows **Debug Log Window…** (between
      Settings and Open Config Folder). Clicking it opens a normal window
      titled "Langception — Debug Log"; clicking it again while open
      just focuses it (no second window).
- [ ] **Rows stream live**: with the loop running, timing rows appear as
      passes happen — one per zone per tick, newest at the bottom, with a
      clock time, the zone's name, and per-stage `capture / ocr /
      translate` ms plus the total pushed to the row's right.
- [ ] **Slow stage at a glance**: in each row the largest stage duration
      is highlighted (yellow, `◂` marker). On a pass where translation
      dominates, `translate` is highlighted; after a pause on a static
      screen, `capture` is.
- [ ] **Unchanged runs collapse**: let the screen go static for ~10
      ticks — the window shows ONE compact row ("unchanged ×N · capture
      ~Xms", count growing), not one row per tick. Touch the watched text
      (a normal timing row appears) — the run is broken and a new run
      starts after.
- [ ] **Rolling summary**: the header shows the pass count and one chip
      per stage that ran (`avg · min · max ms ×n`), the slowest by average
      highlighted. Capture's ×n counts unchanged ticks; ocr/translate's ×n
      counts only read passes.
- [ ] **Errors alongside**: trigger a translate error (e.g. break the
      engine config's key, or point at an unreachable endpoint) — a red
      `error · translate · …` row appears in the stream between timing
      rows, and the tray toast still fires as before.
- [ ] **One-shot rows**: with the window open, run a One-shot Capture
      (Alt+Q) — its pass produces a row badged **One-shot** like a zone
      row (works whether the loop is running or stopped).
- [ ] **Fused mode**: switch `reading = "fused"` (Settings → Engines) and
      restart — rows show `capture · fused · total`, no ocr/translate
      chips, and unchanged ticks still collapse.
- [ ] **Open/close never disturbs the loop**: repeatedly open and close
      the window while translation runs — the loop keeps translating
      (Results Window keeps streaming, overlays keep updating), nothing
      warns. While the window is CLOSED, the app log stays free of new
      timing lines (no persistence; failures still log as before ticket
      27).
- [ ] **History starts on open**: close the window, let several passes
      run, reopen — the summary starts from zero (only passes since the
      reopen appear; the past is not replayed).
- [ ] **Auto-scroll + manual scroll**: with a busy zone the view sticks
      to the newest row; scrolling up to inspect stops the yanking, and
      scrolling back to the bottom resumes following.
- [ ] **Results Window unchanged**: everything above leaves the Results
      Window behaving exactly as before — no timing rows appear there.
- [ ] **Own window is never a capture source**: point a zone at the
      Debug Log window itself — its rows are not OCR'd/translated
      (ADR-0003 exclusion); a capture-overlapping warning would say so if
      the exclusion failed.
- [ ] **app-*.log unchanged**: after a full session with the window open,
      the log contains only the lines it always had (loop start/stop,
      stage failures) — no per-pass timing writes.

## Cleanup

- Stop translation, quit the app, restore any changed `config.toml`
  (engine key, reading mode).
