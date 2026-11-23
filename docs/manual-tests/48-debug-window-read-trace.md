# Manual tests — Ticket 48: Debug window read trace (what OCR read, and why)

Every disposition is pinned offline by `cargo test` on the exact loop
scenario that produces it (translated engine/glossary/fused/cached,
suppressed, waiting with elapsed/total, unchanged, excluded, empty, gone
with miss counts), the fast path is pinned marked "OCR skipped", and the
serde spellings are pinned against the UI's switch. This checklist
exercises what only a live desktop shows: the trace's rendering in the
window — counts pill, expansion, plain-words reasons — plus that trace
building changed nothing about the loop.

## Setup

1. Build and run the dev app: `cargo run -p langception` (or your usual
   `tauri dev` flow).
2. Open **Debug Log Window…** from the tray.
3. Draw a Capture Zone over a window with changing text, start
   translation (an LLM engine makes the translated rows obvious).

## Checklist

- [ ] **Counts pill**: a pass that read blocks shows a compact pill —
      e.g. `3 reads: 3 translated` — grouped by disposition (every
      translated route reports as "translated"); passes that read nothing
      (unchanged collapse) show none.
- [ ] **Expandable lines**: clicking the pill expands the per-block lines
      — the words OCR read in quotes plus the reason in plain words — and
      clicking again collapses them. Other rows keep streaming while a
      row is expanded.
- [ ] **Waiting countdown**: set a stability window (Settings → general
      stability, e.g. 2s) and change watched text — the detecting pass
      traces `waiting 0/2000ms — 2000ms to go`; a pass that re-reads
      mid-window counts up; the matured pass flips the entry to
      `translated · engine`.
- [ ] **Suppressed made visible**: let text jitter on screen (a video
      subtitle or animation under the zone) after it translated once —
      the trace shows `suppressed — identical text already shown` with
      the words, the only place these reads appear anywhere.
- [ ] **Unchanged named**: after a translation shows, keep the same
      pixels and force a re-read (e.g. nudge another part of the zone's
      content so the frame checksum changes) — the block traces
      `unchanged — pixels match what is shown`.
- [ ] **Fast-path rows marked**: with a stability window set, let a
      change mature on a static screen — the row is marked
      **OCR skipped — frame unchanged**, carries no `ocr` chip, and its
      pill lists the matured fire as `translated · cached read (no OCR
      this pass)`.
- [ ] **Excluded named**: exclude a block (ticket 04 flow) that the zone
      still reads — its line says `excluded — marked
      not-to-be-translated`; clearing the exclusion traces it translated
      again.
- [ ] **Gone with miss counts**: make watched text disappear (close the
      source window under the zone) — two passes trace `gone from the
      read · miss 1` then `miss 2`, beside the BlocksRemoved-driven
      overlay clear.
- [ ] **One-shot rows trace**: Alt+Q a One-shot Capture — its row's pill
      lists the blocks with `translated · engine` (or `translated ·
      fused read` in fused mode) and never waiting/unchanged/gone
      entries.
- [ ] **Errors still carry the story**: break the engine key — the red
      error row appears as before, and the pass's trace stays empty (no
      fake "translated" line for a failed read).
- [ ] **Loop untouched**: with the trace in use (expanding rows during
      live translation), the loop behaves exactly as before — same
      translations at the same times in the Results Window/overlays, and
      closing the window mid-run changes nothing.

## Cleanup

- Stop translation, quit the app, restore any changed `config.toml`
  (stability window, engine key).
