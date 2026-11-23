# Capture Visibility: own windows can face the camera, the loop is protected by masking

ADR-0003 excluded every Langception window from screen capture (`WDA_EXCLUDEFROMCAPTURE`) so the translation loop can never read its own output. Its accepted side effect — the app invisible to screenshots, recordings, and remote-desktop sessions — later became a real problem: users cannot screenshot their own UI or see it over remote desktop. We made that side effect optional with a single setting, **Capture Visibility** ("Show Langception in screen captures", off by default):

- **Off (default)**: exactly ADR-0003's behavior — every window excluded from capture.
- **On**: the exclusion flag is lifted from the Settings window, Results Window, and zone picker. They then appear in every capture pipeline — screenshots, OBS, DWM thumbnails, and remote sessions — because the one flag governs them all. Loop safety moves to masking by construction: the loop masks the Settings window's and Results Window's current screen rectangles out of every captured frame before OCR, no matter where Capture Zones are drawn. The zone picker is the exception — its window blankets the whole monitor, so masking it would blind the loop; while it is open (and only when visibility is on) the loop pauses and resumes on close. One-shot already closes the picker before its pass.
- The In-place Overlay stays excluded unconditionally; there is no toggle for it.

## Considered options

- **Warn when a Capture Zone overlaps a visible window**: rejected — makes loop safety the user's diligence instead of a guarantee.
- **Mask and also warn**: rejected — noise without information once masking already guarantees safety.

## Consequences

- While a window is visible, the loop sees a hole where it sits: text *behind* the Results Window no longer composes through to the loop as it did under exclusion. Accepted; it is the price of the window really being on screen.
- The default stays off until masking has soaked in live use; flipping the default later is a one-line change.
- ADR-0003's other rules stand unchanged: the In-place Overlay is always excluded, and capture stays plain `SRCCOPY` without `CAPTUREBLT` — masking is a post-capture rectangle fill, so the raster-op reasoning is untouched.
