# Manual tests — Ticket 70: Debug Log window rebuilt to the redesign mock

The window is vanilla JS against the loop-event stream; the repo has no
JS test harness, so the redesign is verified in the browser (a stubbed
`__TAURI__` harness lives in `.scratch/qa70/`) and against a live
session. This checklist pins what screenshots can't: the live behaviors
behind the new look.

## Setup

1. Build and run the dev app (`cargo tauri dev` or your usual flow).
2. Open **Debug Log Window…** from the tray.
3. Point a Capture Zone at changing dialogue text with an LLM engine so
   passes come out translated, waiting, suppressed, and occasionally
   failing; let an idle stretch accumulate for the unchanged-run card;
   fire a One-shot (Alt+Shift+Q).

## Checklist

- [ ] **Header**: the LC tile + "Langception / Debug Log" crumb; the
      pass-counter pill's dot pulses and its count matches every pass
      since open (unchanged ticks included). Export… and Clear are
      hidden until the first event renders.
- [ ] **KPI cards**: one card per stage that ran + total (classic
      reading: capture/ocr/translate/total; fused: capture/fused/total),
      each with ×count, `Nms avg`, and min/max. The highest-average
      stage's card is amber with the BOTTLENECK badge; it moves when a
      different stage becomes the slowest.
- [ ] **Filter pills**: All/Translated/Waiting/Suppressed with counts; a
      translated pass appears under Translated only, a suppressed-only
      pass under Suppressed, waiting under Waiting; unchanged runs and
      errors live under All. Counts agree with the cards.
- [ ] **Search**: typing filters cards by text read, reason words, zone
      name, translation text ("storm" finds the Storm pass); the ✕ clears;
      a no-match view shows the dashed empty card and **Reset Filters**
      restores everything.
- [ ] **Full pipeline checkbox**: unchecked on open (like every view
      state here, it is not persisted); checking it keeps only complete
      passes — a classic pass with its capture/ocr/translate pills all
      present, a fused pass with capture/fused — and hides unchanged-run
      cards, error cards, and passes whose translate (or fused) never
      fired (every read waiting/suppressed); pill counts and the
      "Showing N" legend follow the same gate; it ANDs with the pills
      and the search box; **Reset Filters** unchecks it, and Clear
      resets it like reopening the window; an idle stretch while it is
      on still collapses into ONE run entry — unchecking renders it as
      one ×N card, not a pile of ×1 cards.
- [ ] **Cards**: the pass's slowest stage pill carries the amber border +
      ◀; blocks turn emerald when > 0; the total badge goes amber on
      passes ≥ 500ms; the colored latency bar proportions roughly match
      the pills (capture sky / ocr indigo / translate amber / fused
      violet).
- [ ] **Read trace**: each read's words (curly-quoted) with its badge —
      emerald check for translated routes, amber pulsing for waiting
      (elapsed/total ticking), zinc ban-pill for suppressed, dim mono for
      unchanged — and, under a translated read, the emerald quote block
      with the translation the Results window showed at the same moment.
- [ ] **Diagnostics**: clicking a card header expands Pipeline Stage
      Diagnostics (per-stage ms + %, Overhead); the chevron flips;
      clicking again collapses; the expanded flag survives filtering.
- [ ] **Idle collapse**: an idle screen is ONE compact
      "unchanged ×N · capture ~Xms" card that bumps its count in place.
- [ ] **Auto-scroll**: emerald while on — new cards scroll into view;
      toggling off greys it and stops the following; re-enabling snaps to
      the newest card and toasts.
- [ ] **Export modal**: Export… opens the mock's modal with a JSON
      preview; the toggle switches to Plain Text; Copy to Clipboard
      flashes a success toast (or the sticky failure toast if blocked);
      Download File opens the native save dialog with the format's
      extension and writes the file that is previewed.
- [ ] **Export contents**: the JSON parses (summary + entries, each
      read's disposition/reason/shown); the Plain Text keeps the
      ticket-49 shape with a `↳ "…"` line per shown translation; the
      per-stage summary numbers match the KPI cards.
- [ ] **Clear**: one click empties the stream, zeroes the pill and KPI
      cards, hides Export…/Clear, shows the waiting card, resets any
      active filter/search (like reopening the window), and toasts
      "Cleared ✓"; the loop keeps flowing into the fresh window.
- [ ] **Errors**: a failed translation renders a rose error card in
      stream order, and the pass next to it keeps its full row.
- [ ] **Language switch**: saving Thai in Settings flips this window's
      static labels immediately (crumb, buttons, pills, legend, search
      placeholder, KPI cards); cards already rendered keep their
      language; new cards render in Thai with Thai reasons and the Thai
      quote block intact.
- [ ] **Narrow window**: at the 520px minimum nothing overflows the
      frame — the header rows wrap, the filter pills scroll.
- [ ] **Reduced motion**: with Windows' "Show animations" off, the pulse
      and slide-in are effectively still (theme.css neutralizer).
