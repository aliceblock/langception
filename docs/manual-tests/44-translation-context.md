# Manual tests — Ticket 44: Translation Context (last X translations assist the current one)

The mechanics are covered offline by `cargo test`: the prompt section
(oldest-first pairs + the consistency instruction, byte-identical prompts at
0), the per-zone rolling buffer (records only shown emissions, evicts
oldest-first, never crosses zones), the live window, and the config
roundtrip/cap. This checklist exercises what only a live desktop shows: the
Settings flow, real-model behavior, and the loop staying warm across a
change.

## Setup

1. An LLM engine configured (Gemini / OpenAI-compatible / LM Studio), the
   loop running (Alt+Shift+T) over a Capture Zone covering a game's dialogue box.
2. Fresh `context_translations = 0` in `config.toml` (the default template
   writes it; a pre-ticket-44 config without the key loads as 0).

## Checklist

- [ ] **Default is off**: fresh config shows `Translation context = 0` in
      Settings → General; translations behave exactly as before (spot-check
      that consecutive lines translate independently, no visible change).
      The config file keeps `context_translations = 0`.
- [ ] **Setting + live apply**: with the loop running, set the field to 3
      and Save — the loop does NOT restart (no re-translation of what is on
      screen, no flicker), and the very next new line of dialogue already
      benefits. The app log's next loop start (e.g. after a pause toggle)
      shows `, context=3` in the launch line.
- [ ] **Consistency in practice**: feed the zone several consecutive lines
      of the same character's dialogue — pronoun choice, politeness
      particles, and name renderings stay consistent line to line (compare
      against the same lines translated with the field at 0; Gemini/
      OpenAI-compatible needed to *see* the difference).
- [ ] **Oldest-first, capped**: with a small window (2) and four lines
      translated, the fourth line's prompt (verify via a local LM Studio
      server logging requests, or a recorded OpenAI-compatible endpoint)
      carries exactly the last two `- source → translation` pairs.
- [ ] **Failures don't pollute**: with the engine erroring (e.g. invalid
      key), let a few blocks fail, then fix the key — the next successful
      translation's context contains only successful pairs (nothing shows a
      stale/failed line).
- [ ] **Per-zone isolation**: two zones over different dialogue boxes —
      lines translated in zone A never appear as context in zone B's
      prompts (and vice versa).
- [ ] **Non-LLM engines unaffected**: switch the default engine to Google/
      Azure/DeepL with context set to 3 — behavior identical to context 0
      (those engines never see prompts; the setting is inert for them).
- [ ] **Fused mode**: Reading mode "Vision model" + context 3 — the fused
      call's system prompt carries the same "Recent translations" section
      for its zone, and continuity works there too.
- [ ] **Cap enforced**: type 99 into the field and Save — the field clamps
      the value to 20 and saves 20 (the clamp happens page-side). The
      command-side cap is still enforced for the paths that bypass the
      page: hand-editing `context_translations = 99` in config.toml breaks
      the config with a visible error naming the 0–20 range, per the app's
      hand-editing contract.
- [ ] **Back to 0**: set the field back to 0 and Save — the very next
      translation has no context (and nothing stale resurfaces if the
      field is raised again later; the window starts filling fresh).
