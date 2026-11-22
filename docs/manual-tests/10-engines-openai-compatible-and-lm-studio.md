# Manual smoke checklist — Ticket 10 (Engines: OpenAI-compatible + LM Studio)

Request/response parsing, error classification, request shapes, model-list
parsing, the local-URL privacy rule, and the config model (engine choice,
endpoint settings, validation) are covered by offline tests (`cargo test`,
`openai` / `config` modules). This checklist covers what only a real
Windows session with real endpoints can show: live cloud calls, LM Studio
discovery, error surfacing in the Results Window + tray notifications, the
global default vs per-Profile choice, and the config-file round-trip. Run
once on a real machine.

## Setup

1. **Cloud option** (pick one): an OpenAI API key, or an OpenRouter key
   (openrouter.ai → Keys). Model name you intend to use, e.g.
   `gpt-4o-mini` (OpenAI) or `google/gemini-flash-1.5` (OpenRouter).
2. **Local option**: LM Studio installed with at least one model
   downloaded. Start its server (Developer tab → Start server; default
   `http://localhost:1234/v1`) and load a model.
3. `cargo run` from the repo root.

## Checklist

### Engines tab — OpenAI-compatible

- [ ] **Card fields prefill**: reopening the tab fills base URL, key
      (masked), model, and temperature from config.toml.
- [ ] **Save + test (cloud)**: base URL `https://api.openai.com/v1` (or
      `https://openrouter.ai/api/v1`), paste the key, type the model,
      default = "OpenAI-compatible", Save → config.toml gains
      `engine = "openai"` under `[engines.openai]`; "Test default engine"
      → note shows `works — "Hello" → สวัสดี…`.
- [ ] **Missing model rejected**: clear the model field, keep default =
      OpenAI-compatible, Save → visible toast naming the model; config
      unchanged.
- [ ] **Auth error classifies**: corrupt the key (one char), Save, Test →
      the toast names the key ("The endpoint rejected the API key (HTTP
      401)…") and carries the provider's own message.
- [ ] **Bad base URL classifies as 404**: set the base URL without
      `/v1` (e.g. `https://api.openai.com`), Save, Test → the error names
      "model name or base URL is wrong (base URL must include /v1)".

### Engines tab — LM Studio

- [ ] **Server off → inline failure**: with the LM Studio server stopped,
      press "Check server" → the note under the card shows the connection
      error naming `localhost:1234` ("connection refused" / "server
      unreachable"), the model dropdown keeps only the placeholder.
- [ ] **Server on → dropdown fills**: start the server, load a model,
      press "Check server" → note shows "Server reachable — 1 model
      available" (or more) and the dropdown lists every served model id.
- [ ] **No model loaded → clear message**: unload all models, check again
      → "Server reachable, but it serves no models — load one in LM
      Studio…".
- [ ] **Save + loop**: pick a model from the dropdown, default = LM
      Studio, Save → config.toml gains `engine = "lmstudio"` and the model
      under `[engines.lmstudio]`; draw a zone over some English text,
      Start → Thai translations arrive through the local model.
- [ ] **Local engine shows no ☁ badge**: while the LM Studio loop runs,
      the tray tooltip and Results Window chip show "LM Studio" with NO
      cloud badge (screen text stays local).
- [ ] **Saved model survives an offline restart**: stop the LM Studio
      server, reopen the Settings Engines tab → the saved model still
      appears in the dropdown marked "(saved, not currently served)".

### Error surfacing in the loop (one cloud run is enough)

- [ ] **Malformed response surfaces**: point the OpenAI-compatible base
      URL at any server that returns non-chat JSON (e.g. an LM Studio URL
      while the default is "OpenAI-compatible") and start the loop → the
      Results Window banner shows the "unexpected shape / not valid JSON"
      message and a tray notification fires; the loop keeps retrying.
- [ ] **Recovery is visible**: fix the base URL (Save restarts the loop)
      → translations resume and the banner clears on the next success.

### Global default vs per-Profile

- [ ] **Profile override**: with the global default on Google, create a
      Profile (Settings → Profiles) whose engine is LM Studio → while that
      app is foregrounded, translations flow through LM Studio (Results
      Window shows "LM Studio", no cloud badge); other apps keep Google.
- [ ] **Profile inherits the LLM default**: set the global default to
      OpenAI-compatible, set the Profile's engine back to "Inherit" → the
      profiled app translates through the OpenAI-compatible endpoint (☁
      badge returns).

### Wizard still works

- [ ] **Azure upgrade intact**: re-run the wizard (About tab) → the Azure
      step still prefills and "Use Azure F0" still switches the default
      without disturbing saved OpenAI/LM Studio fields (they round-trip).
