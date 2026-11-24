# Manual smoke checklist — Ticket 12 (Vision-LLM fused engine: OCR + translate in one call)

The fused reply contract, request shapes (Gemini `inlineData`, OpenAI
`image_url`), PNG encoding, prompt construction (whole-glossary injection +
genderless instruction), and the config model are covered by offline tests
(`cargo test`, `fused` / `prompt` / `config` modules + the orchestrator seam
tests, which drive a REAL adapter against a localhost HTTP server). This
checklist covers what only a live session can show: the actual vision round
trips, block positioning on screen, latency/cost behavior, and the settings
round-trip. Run once on a real machine.

## Setup

1. A vision-capable engine configured (Settings → Engines):
   - Gemini: an AI Studio key + a vision model id (e.g.
     `gemini-4-hydrogen`), or
   - LM Studio: its server running with a **vision** model loaded (e.g. a
     Qwen2-VL GGUF) + "Check server" picking it from the dropdown.
2. Text on screen: ideally stylized/graphics-style text that Windows OCR
   misreads or misses (a game logo, an art-styled menu) plus one plain
   text line as a control.
3. `cargo run` from the repo root.

## Checklist

### Engines tab — reading mode

- [ ] **Select + warning**: "Screen reading" defaults to "Windows OCR +
      engine translation"; switching to "Vision model — read & translate in
      one call" reveals the latency/cost warning; saving persists
      `reading = "fused"` in config.toml; reopening the tab prefills it.
- [ ] **Non-LLM engine rejected at start**: default engine = Google (or
      Azure/DeepL) with reading = fused, Start → a visible error naming the
      fix ("Fused reading needs an engine that can read images…"); no loop.
- [ ] **Missing key/model rejected**: reading = fused with the chosen LLM
      engine's key or model emptied, Start → the usual settings-surface
      error (same messages as the plain engine).

### Fused loop behavior (Gemini or LM Studio vision model)

- [ ] **Blocks translate in one pass**: draw a Capture Zone over the
      stylized text, Start → the In-place Overlay shows Thai pills over the
      original blocks AND the Results Window lists block text + translation
      (positions match the on-screen layout).
- [ ] **dHash gating**: with nothing changing in the zone, the endpoint
      sees no new requests (check LM Studio's request log / API console);
      translations stay on screen unchanged.
- [ ] **Changed frame only**: change one block's text in the captured area
      → exactly one new fused call; every other block's pill does not
      flicker or re-translate.
- [ ] **Glossary rides in the fused call**: with a Glossary entry
      (`Vertin → เวอร์ติน`) set, a block containing that name renders the
      pinned Thai; a block whose ENTIRE text is the term shows Via:
      "Glossary" in the Results Window (the pin overrides the model).
- [ ] **Genderless Mode applies**: with the toggle on, dialogue-like text
      comes back without ครับ/ค่ะ particles and without ผม/ฉัน.
- [ ] **Errors surface**: stop the LM Studio server (or corrupt the Gemini
      key) with the loop running → the Results Window banner shows a
      `[fused]` error and a tray notification appears (throttled); restart
      the server / fix the key and the loop recovers on the next pass
      without restarting the app.
- [ ] **One-shot capture**: Alt+Shift+Q over an area → one fused pass into the
      Results Window (One-shot badge), OCR never involved.
- [ ] **Status line names it**: while running, the Results Window config
      line shows "· fused vision reading".
- [ ] **Privacy badge**: with a cloud fused engine (Gemini / remote
      OpenAI-compatible) the tray tooltip and Results Window show the ☁
      badge naming the engine; with LM Studio (localhost) neither does.
- [ ] **Latency honesty**: measure a few passes — a fused pass is visibly
      slower than the OCR path (seconds, not milliseconds); the cadence
      still paces the loop and the app stays responsive.

### Per-Profile interplay

- [ ] **Profile engine drives the fused call**: global default = Google +
      reading = fused, with a Profile whose engine = LM Studio → the
      profiled app translates through the local vision model (no ☁ badge
      while it is foreground), other apps fail with the needs-an-LLM error
      (Google cannot fuse).
