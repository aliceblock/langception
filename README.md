# Langception

[![CI](https://github.com/aliceblock/langception/actions/workflows/ci.yml/badge.svg)](https://github.com/aliceblock/langception/actions/workflows/ci.yml)

Langception reads the text on your screen and shows it in Thai, in place over
the original or in a separate always-on-top window. Point it at a game or an
app written in **English, Japanese, Chinese, or Korean** and it keeps the
translation current as the text changes. Windows first (10/11, 64-bit); the
translation core is platform-independent so macOS/Linux ports can come later.

Built for Thai players and users: machine translation that defaults to zero
configuration, that never assumes the speaker's gender, and where character
names can be pinned so `Vertin` always renders as `เวอร์ติน`.

## Features

- **Capture Targets** — draw Capture Zones over just the text you care about
  (Alt+Shift+Z), translate an entire monitor (full-screen mode, with per-position
  Excluded Blocks), or scope everything to one app with per-app **Profiles**
  that activate when their app comes to the foreground.
- **Two display surfaces** — the In-place Overlay (translated text drawn over
  the original on a transparent, click-through window, one per monitor) and
  the Results Window (always-on-top list with click-to-copy and an error
  banner).
- **Auto-translate loop** — captures continuously, hashes each Text Block
  (dHash), and re-translates only what actually changed; a stability window
  debounces animations. Pause with Alt+Shift+P.
- **Zero-config default engine** — works on first launch with no accounts and
  no keys (unofficial Google endpoint; see the engine table for the trade-off).
- **Upgradeable engines** — Azure AI Translator, DeepL, Gemini API, any
  OpenAI-compatible endpoint, and LM Studio for fully local, private
  translation.
- **Two reading modes** — classic (Windows OCR feeds per-block translation)
  or **fused** (one vision-LLM call reads every Text Block and translates it).
- **Glossary** — pin exact Thai translations for terms; whole-block matches
  short-circuit the engine entirely, in-sentence terms are injected into LLM
  prompts. JSON import/export.
- **Genderless Mode** — strips gendered Thai politeness particles and
  pronouns (ครับ/ค่ะ, ผม/ฉัน) from output, via prompt instruction for LLM
  engines and a rule-based scrubber for the rest. Global toggle, per-Profile
  override.
- **Adjust Mode** (Alt+Shift+A) — zones and overlays become draggable/resizable
  (click-through off) so you can fine-tune positions live; full-screen blocks
  can be marked as Excluded Blocks by clicking them.
- **One-shot Capture** (Alt+Shift+Q) — translate an arbitrary screen area once,
  no zone setup.
- **Text style** — font, size, color, outline, and a semi-transparent
  background pill, with auto-shrink so long Thai never covers the screen.
  Noto Sans Thai ships with the app, so Thai always renders.
- **Tray-first shell** — start/stop, modes, one-shot, and Settings live in the
  tray menu; global hotkeys are rebindable (Settings → Hotkeys).
- **Visible privacy state** — whenever the active engine is sending screen
  text to a cloud, the Results Window and the tray tooltip say so.

## Requirements

- Windows 10 or 11, 64-bit.
- The **WebView2 Runtime** — preinstalled on Windows 11 and on up-to-date
  Windows 10. The installer sets it up automatically if it is missing; the
  portable build needs it present (see below).
- An **OCR language pack** for your source language — the default OCR
  engine (Windows OCR) needs one (see next section). If you cannot install
  packs, the bundled PP-OCRv6 engine tiers work without any.
- Internet access for every engine except LM Studio.

## Install

Grab both artifacts from the
[Releases page](../../releases) — every release ships:

| Artifact | What it is |
| --- | --- |
| `Langception_<version>_x64-setup.exe` | NSIS installer. Installs for the current user (no admin), adds Start-menu shortcuts, and **downloads the WebView2 Runtime automatically if it is missing**. |
| `Langception_<version>_x64-portable.zip` | Portable zip: one `Langception.exe`, no installer, no registry entries. WebView2 must already be present (Windows 11 has it; otherwise install the [Evergreen Runtime](https://developer.microsoft.com/microsoft-edge/webview2/) once). A `README.txt` inside the zip repeats this. |

The binaries are not code-signed, so Windows SmartScreen may show
"Windows protected your PC" on first run — choose **More info → Run
anyway**. This is expected for unsigned open-source apps.

## First run

On the very first launch the **setup wizard** opens automatically (it is a
page inside the Settings window; you can skip it at any time — Langception
works with zero configuration). It:

1. **Checks your OCR language packs** — the default OCR engine is
   Windows OCR, which needs a pack for your source language. The wizard
   shows exactly which of the four source languages are readable, with a
   button that jumps to Windows' language settings if something is missing
   (and points at the bundled PP-OCRv6 tiers if you cannot install packs).
2. Walks through **choosing an engine** — the default needs nothing; keyed
   engines have links to their signup pages.
3. Shows the **hotkeys** so the app is usable without touching a window.

The wizard can be re-run any time from **Settings → About**. After that:

1. Start translation (tray menu → *Start Translation*, or Alt+Shift+T).
2. Draw a zone over some text (Alt+Shift+Z), or switch to full-screen mode in
   the Results Window or tray menu.
3. Translations appear — in place over the original, and in the Results
   Window.

Everything the app keeps lives in one config file:
`%APPDATA%\langception\config.toml` (tray menu → *Open Config Folder*).
API keys, glossaries, and profiles are stored only there, on your machine.

## OCR language packs

The default OCR engine is **Windows OCR** (Windows.Media.Ocr) — lighter and
faster than bundling models, but it needs a **language pack** for your
source language — English, Japanese, Chinese, or Korean. Without the right
pack you get no text at all, so this is the first thing to check:

1. In Langception: **Settings → Engines** lists the installed packs and which
   source languages are covered.
2. To install a missing pack, use the wizard's jump button (or open Windows
   **Settings → Time & Language → Language & region** yourself), click
   **Add a language**, pick the language, and install it. Windows installs
   text recognition as part of the language's features.
3. Re-check Settings → Engines (the list is probed fresh on each visit); if
   the new pack still doesn't show, sign out of Windows and back in, or
   restart, so the language feature registers system-wide.

> For Chinese, both Simplified and Traditional recognizers are supported;
> when a Profile or config asks for generic `zh`, the Simplified pack wins.

**No pack, or prefer offline reads?** Every release also bundles the
**PP-OCRv6** engine in three tiers: select **PP-OCRv6 tiny**, **PP-OCRv6
small**, or **PP-OCRv6 medium** in **Settings → Engines → OCR engine** and
reads work with no Windows language packs at all, fully offline — tiny is
the fastest, medium the most accurate. They are heavier per pass than
Windows OCR (the bundled models total ~190 MB), and if the engine cannot
run a pass it automatically falls back to Windows OCR. One quirk: tiny's
vocabulary has no kana, so a Japanese source runs the small model set
instead (the app says so in its log); Korean reads through a dedicated
Korean model at every tier.

## Translation engines

Choose the global default in **Settings → Engines** (per-Profile overrides
exist too). Cloud engines show a privacy badge in the Results Window and the
tray tooltip while they are in flight.

| Engine | Key needed | Notes |
| --- | --- | --- |
| **Google** *(default)* | none | Unofficial free endpoint (`translate.googleapis.com`); zero-config but unofficial — it can slow down or break without notice, and it sends screen text to Google. |
| **Azure AI Translator** | key (+ optional region) | The recommended upgrade: free F0 tier, official and stable. Create a Translator resource in the [Azure portal](https://portal.azure.com) (Settings has this link) and paste the key; leave the region empty for a global resource. |
| **DeepL** | key | [DeepL API](https://www.deepl.com/en/pro-api) — free tier available. Paste the key; free keys (ending `:fx`) are routed to DeepL's free host automatically. |
| **Gemini** | key | Google AI Studio models. Get a key at [aistudio.google.com](https://aistudio.google.com/apikey), set a model id — `gemini-4-hydrogen` or `gemini-3.8-flash` are good picks. |
| **OpenAI-compatible** | key | Any `/chat/completions` server: OpenAI (`https://api.openai.com/v1`), OpenRouter, or anything speaking the same API. Set base URL, key, and model. |
| **LM Studio** | none (local) | Fully offline and private — see the next section. |

Engine errors are never silent: they surface in the Results Window and as a
tray notification (throttled so a dead engine doesn't spam you every capture
tick). **Settings → Engines → Test** sends one real request to verify a
configuration.

### LM Studio (local translation)

[LM Studio](https://lmstudio.ai/) lets you run open models on your own
machine, so nothing ever leaves it:

1. Install LM Studio, download a model (`gemma-4-e4b` and
   `qwen3-4b-instruct` are good picks), and start its server:
   **Developer tab → Start server** (default port 1234). Leave LM Studio
   running while you translate.
2. In Langception: **Settings → Engines → LM Studio**. The default base URL
   (`http://localhost:1234/v1`) matches LM Studio's server. Press **Check**
   to list the loaded models and pick one from the dropdown.
3. Choose **LM Studio** as the engine. No API key is needed and the privacy
   badge stays off — translation is entirely on-machine.

Tips: pick a model that translates well into Thai; small instruct models are
fast but rough. If you enable **fused reading** (below), the model must
accept images (a vision model), since the screen capture is attached to the
prompt.

### Reading modes (classic vs fused)

- **Classic** (default): Windows OCR reads Text Blocks, the chosen engine
  translates each block. Works with every engine and is fast.
- **Fused** (Settings → Engines → Reading): one vision-LLM call reads *and*
  translates the whole frame — the OCR stage is bypassed and layout context
  helps with game UI text. Requires an LLM engine (Gemini, OpenAI-compatible,
  or LM Studio) with a vision-capable model.

## Usage

| Hotkey | Action |
| --- | --- |
| `Alt+Shift+T` | Start / stop translation |
| `Alt+Shift+Z` | Draw a new Capture Zone (drag on the dimmed screen) |
| `Alt+Shift+P` | Pause / resume Auto-translate |
| `Alt+Shift+A` | Adjust Mode (drag/resize zones; click full-screen blocks to exclude them) |
| `Alt+Shift+Q` | One-shot Capture (translate an area once) |

All hotkeys are rebindable in **Settings → Hotkeys** and apply live. The
tray menu exposes the same actions; left-clicking the tray icon focuses the
Results Window.

> The defaults use `Alt+Shift+{Key}` so they don't collide with games' and
> apps' common `Alt+{Key}` menu shortcuts. If you remapped Windows'
> input-language switch to Alt+Shift, note that the bare two-key chord
> still swaps your layout — the three-key hotkeys above are distinct
> chords and never trigger the swap. A config saved by an older version
> keeps its old hotkeys; nothing is rewritten on upgrade.

- **Capture Targets**: Capture Zones (global or per-Profile), or full-screen
  mode on a selected monitor. Exactly one target is active at a time;
  Profiles take over whenever their app is the foreground window and fall
  back to the global target otherwise.
- **Excluded Blocks** (full-screen mode): click a block in Adjust Mode to
  stop translating that position — useful for menus, HUDs, and logs that
  never change.
- **Profiles** (Settings → Profiles): per-app source language, engine,
  Genderless override, glossary layer, and window-relative zones that follow
  the app's window around.
- **Glossary** (Settings → Glossary): map a source term to a fixed Thai
  translation; Profile glossaries layer over the global one. Import/export
  as JSON.
- **Genderless Mode** (Settings → General): removes gendered particles and
  first-person pronouns from Thai output (ครับ/ค่ะ/ผม/ฉัน → neutral forms).
- **Text Style** (Settings → Text Style): font, size, color, outline,
  background pill, all applied to the overlay live.
- **UI Language** (Settings → General): the language of Langception's own
  windows and tray — English or ไทย (Thai). Saving applies it to every
  open window immediately (log entries already shown keep their original
  language). (Source/target language names in the selects are
  native names, not localized.)

## Privacy

- Screen text leaves your machine **only** while a cloud engine (Google,
  Azure, DeepL, Gemini, OpenAI-compatible) is translating; the Results
  Window and tray tooltip show a badge whenever that is happening.
- LM Studio translation never sends anything off-machine.
- Even with **Show Langception in screen captures** on (Settings →
  General), the translation loop masks Langception's own windows out of
  every frame it processes — the app never reads, translates, or uploads
  its own output, and the In-place Overlay is never visible to captures.
- The optional update check is fetch-only: it sends nothing beyond the
  request itself and never downloads or installs anything.
- Config, keys, and glossaries stay in `%APPDATA%\langception\` on your
  machine. Langception has no telemetry.

## Anti-cheat and overlay caution

Langception draws always-on-top, click-through overlay windows, registers
global hotkeys, and reads the screen with GDI capture. It does **not**
inject into or attach to any game process — it is a plain external Windows
app — but anti-cheat systems (EasyAntiCheat, BattlEye, Vanguard, and
friends) are deliberately paranoid about overlays, global hooks, and screen
automation, and some flag even external tools like this one.

**Use good judgment:**

- Offline / single-player games: safe in practice.
- Online games with kernel-level or strict anti-cheat: Langception may be
  detected as suspicious **even though it does nothing to the game**. A ban
  risk is yours to take — read the game's rules, and prefer the Results
  Window (rather than the overlay) or a second monitor where possible.
- Any anti-cheat that forbids screen-reading tools outright: don't use
  Langception there.

## Building from source

Prerequisites: [Rust](https://rustup.rs/) (stable) and
[Node.js](https://nodejs.org/) (for the Tauri CLI; the frontend itself is
static files in `ui/` and needs no build step).

```sh
npm install -g @tauri-apps/cli   # or: cargo install tauri-cli --locked
cargo test --workspace           # offline unit tests (570+)
cargo tauri dev                  # run the app
cargo tauri build                # NSIS installer in target/release/bundle/nsis/
```

`cargo test` never touches the network (fakes + golden recorded fixtures);
there is an opt-in manual capture smoke:
`cargo test -p langception --test manual_smoke -- --ignored --nocapture`.

Cutting a release (tag → CI → published installer + zip) is described in
[docs/releasing.md](docs/releasing.md).

## License

[Apache-2.0](LICENSE). The bundled [Noto Sans Thai](ui/fonts/) font is
© The Noto Project Authors, under the
[SIL Open Font License 1.1](ui/fonts/LICENSE.txt).
