# Overlay windows are capture-invisible (WDA + plain SRCCOPY)

The In-place Overlay (ticket 03) is a transparent, always-on-top window drawn over exactly the screen regions the translation loop watches. If the capture included it, the loop would OCR its own Thai output and re-translate the result every tick — translate-garbage loops, or render→clear→render oscillation. The feature could never stabilize.

Two mechanisms combine (both verified live on Windows 11):

1. **Each overlay window carries `SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE)`** (Windows 10 2004+): it stays fully visible on screen but capture pipelines skip it.
2. **The capture adapter uses plain `SRCCOPY`, deliberately without `CAPTUREBLT`.** With `CAPTUREBLT` set and a `WDA_EXCLUDEFROMCAPTURE` window on screen, the blit is poisoned — it succeeds but returns an image the OCR reads as nothing. Plain `SRCCOPY` captures normal windows fine and honors the exclusion.

Rejected alternatives, both verified live: `WS_EX_LAYERED` + `SetLayeredWindowAttributes` (a WebView2 window is DirectComposition-composited, so its content bypasses the GDI layered redirection surface and still appears in captures), and removing `CAPTUREBLT` alone (the DComp overlay is not a GDI layered window, so it is still captured).

A subtlety that shaped the loop design: Windows OCR intermittently returns **empty results on a perfectly good capture** (cold engines, mid-run glitches). The orchestrator therefore only reports a block as removed ([`LoopEvent::BlocksRemoved`]) after two consecutive passes miss it — a one-tick OCR glitch must never clear a visible translation.

## Consequences

- The loop always OCRs the underlying app's original text; dedup stays stable and the overlay renders once per text change without flicker.
- Langception's overlays are also invisible to other capture tools (OBS, screenshots, DWM thumbnails). Accepted: users capture the *content*, not our translation layer. Manual verification of overlay rendering must use the human eye or the browser-mock harness, not screenshots.
- If `SetWindowDisplayAffinity` fails (Windows older than 2004), the overlay window is not created and the failure surfaces as a visible warning — translation continues into the Results Window rather than running an unstable feedback loop.
- Other apps' layered windows ARE captured again (plain `SRCCOPY` excludes only true GDI-layered windows; `CAPTUREBLT` is off, but ordinary DWM composition is unaffected) — slice-01 capture fidelity is otherwise unchanged.
- One-shot Capture works the same way: the zone picker closes before the pass runs, and the overlay itself never pollutes its own results.
