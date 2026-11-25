# Langception

A desktop screen-capture translation tool: it reads text visible on screen (via OCR) and shows its Thai translation over or beside the original. Windows-first; macOS/Linux later. Primary content: games and app UI text.

## Language

### Capture

**Capture Target**:
What the app watches: the whole screen, one app's window, or a user-drawn region. Exactly one is active at a time.
_Avoid_: capture area, source, monitor region

**Capture Zone**:
A user-drawn rectangular screen region captured and translated repeatedly. Belongs either to the global context or to a Profile.
_Avoid_: region, box, area

**One-shot Capture**:
A hotkey-triggered capture of a screen area that runs a single OCR+translate pass into the Results Window without creating a Capture Zone.
_Avoid_: snip, quick capture

**Excluded Block**:
A Text Block position the user marked as not-to-be-translated, consulted in full-screen mode.
_Avoid_: blacklist, ignore list

**Profile**:
A per-app configuration: which app it targets, the source language, engines, and any Capture Zones. A Profile is active while its app is the foreground window.
_Avoid_: app config, preset, per-app setting

**Source Language**:
The language Langception expects to read: English, Japanese, Chinese, or Korean. Fixed per Profile.
_Avoid_: from-language, input language

### Display

**In-place Overlay**:
Translated text drawn directly over the original text's position on a transparent, click-through window.
_Avoid_: on-screen translation, replace mode

**Results Window**:
An always-on-top window listing detected text alongside its translation, for modes where the Overlay is off or unreadable.
_Avoid_: separate window, side panel

**Auto Colors**:
A Text Style setting that samples each Text Block's text and pill colors from the captured pixels, so translations dress like the scene they cover. The manual colors stay as the fallback; font size, outline, and pill opacity keep applying.
_Avoid_: auto style, adaptive colors, color picking

**Placement**:
A Text Style setting that positions translated text Over its Text Block (default), Below it, or Beside it.
_Avoid_: offset, position mode, anchor

**Background Cover Source**:
A Text Style setting that stretches the pill over the whole Text Block instead of hugging the translated text. Takes precedence over Placement when both are on.
_Avoid_: full background, block background, cover source

**Text Alignment**:
A Text Style setting that aligns translated text inside a Background Cover Source pill along both axes: horizontally — Left (default), Center, or Right — and vertically — Top, Center (default), or Bottom.
_Avoid_: justify, text position, vertical justify

**Adjust Mode**:
A hotkey-toggled state in which Capture Zones and overlays become draggable/resizable instead of click-through.
_Avoid_: edit mode, config mode

### Translation loop

**Auto-translate**:
The continuous mode: capture, detect change, OCR, translate. Text that has not changed is never re-translated.
_Avoid_: live translate, realtime mode

**Pass**:
One step of the translation loop for a Capture Zone or a One-shot Capture: capture, then either OCR plus per-block translation (classic Reading Mode) or the fused read-and-translate call. A pass that detects no change ends at capture and never translates. A pass is complete when every stage of its Reading Mode ran.
_Avoid_: process, run, iteration, cycle

**OCR Engine**:
A pluggable component that turns a captured image region into text blocks with positions.
_Avoid_: reader, recognizer

**Model Tier**:
The size grade of the bundled OCR Engine — Tiny (fastest, weakest on small text), Small (the default balance), or Medium (most accurate, slowest). Chosen as part of the OCR Engine selection itself — the engine dropdown lists the tiers as separate options — not as a separate setting.
_Avoid_: model size, model version, quality level

**Translate Engine**:
A pluggable component that translates text from a Source Language to Thai.
_Avoid_: translator, backend

**Reading Mode**:
How on-screen text is read: classic (an OCR Engine feeding per-block translation) or fused (one vision-LLM call reads every Text Block and translates it, bypassing both stages).
_Avoid_: pipeline mode, OCR mode, vision mode

**Text Block**:
One contiguous run of text found by an OCR Engine, with its bounding box on screen. The unit of tracking, display, and re-translation.
_Avoid_: line, segment, chunk

**Glossary**:
A user-defined mapping from a source-language term to a fixed Thai translation that takes precedence over Translate Engine output.
_Avoid_: dictionary, term base, custom translation

**Genderless Mode**:
A translation output setting that removes gendered Thai politeness particles and pronouns, so translated speech carries no speaker gender.
_Avoid_: neutral mode, gender-free mode

**System Prompt Preset**:
A named pair of base instructions (per-block translation and screen-reading) that LLM prompts are built from, selectable globally or per Profile. Built-ins: General, Game, Novel, Manga.
_Avoid_: prompt template, style preset, persona

**Translation Cache**:
A store of earlier Translate Engine outputs, reused whenever the same text is translated again for the same language pair and engine. It applies in classic Reading Mode only, sits below the Glossary (a Glossary match always wins), survives app restarts, and can be cleared in Settings.
_Avoid_: translation memory, history, response cache

**Translation Context**:
A translation-loop setting that appends the most recent successful translations of the same Capture Zone to the prompt, keeping consecutive text consistent in tone and naming. 0 (default) disables it.
_Avoid_: history, memory, cache

### App

**Capture Visibility**:
The setting controlling whether Langception's own windows (Settings, Results Window, zone picker, Debug Log window) appear in captures made by other tools — screenshots, recordings, remote sessions. Off by default; the In-place Overlay is never visible to capture.
_Avoid_: privacy mode, screenshot mode, capture exclusion

**Update Check**:
A Settings button and an optional on-launch fetch that compare the running version against the maintainer's published app list, reporting a newer version with a download link. Informational only — never downloads or installs.
_Avoid_: auto-update, updater, version monitor

**Debug Log Window**:
A window streaming the translation loop's per-pass stage timings in real time, so a slow stage (capture, OCR, translate, fused) can be identified at a glance, plus a read trace of what OCR saw and why each Text Block did or didn't translate. Diagnostic only — it never changes loop behavior.
_Avoid_: perf panel, profiler, timing view, live log
