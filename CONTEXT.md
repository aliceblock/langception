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

**Adjust Mode**:
A hotkey-toggled state in which Capture Zones and overlays become draggable/resizable instead of click-through.
_Avoid_: edit mode, config mode

### Translation loop

**Auto-translate**:
The continuous mode: capture, detect change, OCR, translate. Text that has not changed is never re-translated.
_Avoid_: live translate, realtime mode

**OCR Engine**:
A pluggable component that turns a captured image region into text blocks with positions.
_Avoid_: reader, recognizer

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
