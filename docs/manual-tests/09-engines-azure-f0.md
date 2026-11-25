# Manual smoke checklist — Ticket 09 (Engines: Azure F0)

Request/response parsing, error classification, request shapes, and the
config model (engine choice, keys, validation) are covered by offline tests
(`cargo test`, `azure` / `config` modules). This checklist covers what only
a real Windows session with a real key can show: live API calls, error
surfacing in the Results Window + tray notifications, wizard upgrade,
per-Profile override, and the config-file round-trip. Run once on a real
machine.

## Setup

1. Create the free key: **Azure**: portal.azure.com → Create resource →
   Translator, pricing tier "Free F0", region "Global" → Keys and Endpoint
   → copy KEY 1.
2. `cargo run` from the repo root.

## Checklist

### Engines tab

- [ ] **Default-engine select** lists the offered engines (Google first);
      reopening the tab prefills it from config.toml.
- [ ] **Create-key link**: "Create a free key…" opens the Azure portal's
      Translator creation blade in a browser. The Settings window itself
      does not navigate.
- [ ] **Google Test** (default, no key needed): "Test default engine" →
      note shows `works — "Hello" → สวัสดี…`.
- [ ] **Azure save + test**: paste the Azure key (+ region if the resource
      is regional), set default = Azure, Save → config.toml gains
      `engine = "azure"` and the key under `[engines.azure]`; Test → Thai
      appears. Keys are saved masked (password fields) but round-trip.
- [ ] **Azure without a key is rejected**: clear the key, keep default =
      Azure, Save → visible toast ("Azure needs an API key…"); config
      unchanged.
- [ ] **Wrong key classifies visibly**: corrupt the Azure key (one char),
      Save, Test → the toast names the key ("Azure rejected the API key
      (HTTP 401)…").

### Loop behavior + error surfacing

- [ ] **Live loop through Azure**: draw a zone over some English text,
      Start → translations arrive with the ☁ badge showing "Azure AI
      Translator" (tray tooltip + Results Window chip).
- [ ] **Quota/key error → Results Window**: start the loop with a broken
      key and a stable zone → the error banner shows
      `[translate] Azure rejected the API key…`, the status pill shows
      Error.
- [ ] **Quota/key error → tray notification**: same broken-key run → a
      Windows toast appears within moments; the SAME error does not re-toast
      more than about once a minute even though the loop retries (no toast
      spam). A DIFFERENT error message notifies immediately.
- [ ] **Recovery is visible**: fix the key in Settings → Engines (Save
      restarts the loop) → translations resume, banner clears on the next
      success.
- [ ] **Profile override**: create a Profile for a running app with engine
      = Gemini while the global default is Azure → with that app in the
      foreground the ☁ badge names Gemini; background apps keep Azure.

### Wizard

- [ ] **Azure upgrade step** (fresh config, wizard visible): step 2 shows
      Google as "active by default" plus a "Recommended upgrade: Azure AI
      Translator" card with a key field, region field, and the create-key
      link.
- [ ] **Use Azure F0**: paste a real key → "Use Azure F0" → note + toast
      confirm; config.toml has `engine = "azure"` and the key. Finish the
      wizard → Engines tab shows Azure as default with the key prefilled.
- [ ] **Wizard with empty key**: "Use Azure F0" with no key → visible
      rejection, nothing saved.
- [ ] **Skip still works**: "Skip setup" → zero-config Google engine active.

### Config file round-trip

- [ ] **Removed-engine migration (ADR-0007)**: stop translation, set
      `engine = "deepl"` by hand in config.toml (with a
      `rate_limit_fallback = "deepl"` and a Profile `engine = "deepl"` for
      good measure), start → the app loads on Google with a one-time
      banner + app-log notice naming each change, and config.toml was
      rewritten without any "deepl" left. A second start shows no notice.
- [ ] **Unknown engine rejected loudly**: set `engine = "babelfish"` by
      hand → after stop/start, the config error appears in the Results
      Window ("engine \"babelfish\" is unknown — engines: google, azure,
      openai, lmstudio, gemini"); restore a valid value.
- [ ] **Ticket-08 configs still parse**: a config.toml without the
      `engine` key / `[engines]` tables loads with the Google default
      (older config files must not break).
