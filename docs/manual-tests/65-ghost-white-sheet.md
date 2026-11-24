# 65: Ghost white sheet — hidden Settings/Results window composes as a
# blank white sheet when another window opens

The bug: hide Settings (or Results), then open the Debug Log window — a
blank white sheet appears at the hidden window's exact position and
size. It is invisible to window enumeration (`IsWindowVisible` is false
for every window there) but visible to the eye: it is the hidden
window's DWM surface being composed, made blank by the new webview's
surface allocation. When the hidden window is also always-on-top
(`Keep Settings window on top`), the sheet floats above everything and
is glaring; with the flag off it composes below the foreground windows
and is easy to miss. Opening the hidden window again paints over it and
the sheet disappears.

Fixed by DWM-cloaking the frameless windows (Settings, Results) around
hide/show — a cloaked window is removed from the composition tree, so a
hidden Langception window can never be composed.

## Verify the fix (reporter's recipe, ~2 minutes)

1. Settings → General → tick **Keep Settings window on top** → Save
   Changes → close Settings (✕ hides it).
2. Tray → **Debug Log Window…**.
3. PASS: the Debug Log opens normally AND nothing else appears anywhere
   — in particular no white sheet at the spot where Settings was.
   (Before the fix this spot turned pure white, stuck, until Settings
   was reopened.)
4. Open Settings again — it must render normally (dark theme, all tabs).
5. Untick **Keep Settings window on top** → Save → close Settings →
   reopen the Debug Log → same PASS (the sheet used to appear buried
   under other windows in this state too — the flag was never the cure,
   only the z-order).
6. Results-window variant: hide the Results Window (✕), open the Debug
   Log — no white sheet at the Results' old spot. Reopen Results —
   renders normally.
7. Regression harness (agent-runnable):
   `.scratch/diag-white-window/sotp-repro.ps1` (UIA-drives the recipe,
   samples the screen at the hidden window's rect — RED = ratio 1.00,
   GREEN = 0.00) and `live-probe2.ps1` (both-monitors white-window
   scanner). These need a live dev app; no unit test seam exists for DWM
   composition behavior.

## Note: two different "white windows"

- THIS ghost (blank sheet at a hidden window's rect, appears when
  another window opens) — fixed here.
- The stale-dev-build error sheet (WebView2's "can't reach this page /
  127.0.0.1 refused to connect") — happens only when a `cargo tauri dev`
  build runs without its dev server; not an app bug. See the addendum in
  `61-ui-restyle.md`. If a white window ever shows the Edge error page,
  it is the dev-environment case, not this bug.
