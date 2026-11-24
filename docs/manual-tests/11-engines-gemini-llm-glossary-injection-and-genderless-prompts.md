# Manual smoke checklist — Ticket 11 (Engines: Gemini + LLM glossary injection + genderless prompts)

Gemini request/response parsing, error classification, request/URL shapes,
prompt construction (glossary injection + genderless instruction, OpenAI
adapter included), the guidance seam, and the config model are covered by
offline tests (`cargo test`, `gemini` / `prompt` / `openai` / `glossary` /
`config` modules + the orchestrator seam tests). This checklist covers what
only a real session with a live endpoint can show: the Gemini round trip,
prompt-injected output quality, rate-limit surfacing, and the settings
round-trip. Run once on a real machine.

## Setup

1. A Google AI Studio API key: [aistudio.google.com/apikey](https://aistudio.google.com/apikey)
   (free tier; the Generative Language API must be enabled for the key's
   project — it is by default for AI Studio keys).
2. A model id served under `v1beta`, e.g. `gemini-4-hydrogen`.
3. `cargo run` from the repo root.

## Checklist

### Engines tab — Gemini card

- [ ] **Card fields prefill**: reopening the tab fills the key (masked),
      model, and temperature from config.toml.
- [ ] **Key link**: "Create a free key…" opens AI Studio's key page.
- [ ] **Missing key rejected**: default = Gemini with an empty key, Save →
      visible toast naming the key; config unchanged.
- [ ] **Missing model rejected**: key pasted, model cleared, Save → toast
      naming the model; config unchanged.
- [ ] **Save + test**: key + `gemini-4-hydrogen`, default = Gemini, Save →
      config.toml gains `engine = "gemini"` under `[engines.gemini]`;
      "Test default engine" → note shows `works — "Hello" → สวัสดี…`.
- [ ] **Invalid key classifies**: corrupt one character of the key, Save,
      Test → the toast names the key ("Gemini rejected the API key (HTTP
      400)…") and carries Google's own message.
- [ ] **Bad model classifies**: model = `gemini-99`, Test → the toast says
      Gemini has no such model (HTTP 404) and names Settings → Engines.
- [ ] **Rate limit surfaces visibly**: with the free tier's per-minute
      limit exhausted (rapid-fire Test clicks work), the error names the
      quota/rate limit (HTTP 429) — and during a running loop it appears in
      the Results Window banner AND a tray notification, without stacking
      toasts.

### Prompt injection through a real LLM (Gemini or the OpenAI-compatible card)

- [ ] **In-sentence glossary term**: add a Glossary entry `Vertin →
      เวอร์ติน` (Settings → Glossary), select an LLM engine, draw a zone
      over text like `Vertin picks up the case` → the translation renders
      the name as เวอร์ติน mid-sentence, per the Glossary.
- [ ] **Irrelevant entries do not leak**: with an unrelated extra entry
      (`Regulus → เรกุลุส`) also configured, blocks without it still
      translate normally (behavior unchanged).
- [ ] **Whole-block match still short-circuits**: a block whose ENTIRE text
      is `Vertin` shows เวอร์ติน instantly (Via: Glossary in the Results
      Window) — the engine is never called.
- [ ] **Genderless instruction**: turn Genderless Mode on (Settings →
      General), translate a line of dialogue through an LLM engine → the
      output carries no ครับ/ค่ะ particles and no ผม/ฉัน (personally
      verifiable against the source; the rule-based scrub also runs, so
      stray particles are still stripped).

### Per-Profile + fallback

- [ ] **Profile override**: with the global default on Google, create a
      Profile whose engine is Gemini → the profiled app translates through
      Gemini (Results Window shows "Gemini", ☁ badge); other apps keep
      Google.
- [ ] **Wizard passes through**: re-run the wizard (About tab) → the Azure
      step still works and saved Gemini fields round-trip untouched.
