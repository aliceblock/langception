//! The one versioned place for every LLM translation prompt (ticket 11).
//! Both LLM adapters — the OpenAI-compatible chat engine and Gemini — build
//! their system message through [`system_prompt`], so the prompt's wording,
//! its Glossary injection, and its Genderless instruction can never drift
//! between them. Shaped by [`TranslationGuidance`], which the orchestrator
//! computes per Text Block (relevant Glossary entries + the Genderless
//! flag + the user's optional custom base, ticket 37, and the System Prompt
//! Preset it is the wording of, ticket 42); prompt *construction* itself is
//! pure and tested offline here, and end to end through the pipeline seam
//! with a fake engine standing in for the HTTP LLM (`orchestrator_tests.rs`).
//!
//! Versioning: every wording change bumps [`PROMPT_VERSION`] and lands in
//! this module only — the version tag rides at the end of the prompt itself,
//! so a recorded request always shows which template produced it (a custom
//! base is tagged `custom`, ticket 37; a preset's wording is tagged with the
//! preset's name, ticket 42).

use crate::ports::{SourceLanguage, TranslationGuidance};

/// The prompt template's version, tagged at the end of every system prompt.
/// Bump on any wording change that could alter translation output.
pub const PROMPT_VERSION: &str = "3";

/// The fused vision prompt's version ([`fused_system_prompt`]), tagged the
/// same way. Independent of [`PROMPT_VERSION`]: the two templates change
/// for different reasons.
pub const FUSED_PROMPT_VERSION: &str = "3";

/// The built-in default's tuning — what to translate (unchanged since
/// ticket 10's OpenAI-compatible prompt). [`base_instruction`] appends the
/// output contract.
const DEFAULT_TRANSLATION_TUNING: &str = "You are a translation engine. Translate the user's text \
     from {source} to {target}.";

/// What every per-block translation prompt asks the model to reply with —
/// the contract models actually follow. The loop shows the engine's reply
/// verbatim, so the built-in default AND every built-in System Prompt
/// Preset's translation text (ticket 42) ends with this: a preset's tuning
/// can never drop the rules that keep the reply bare. The Arabic-numerals
/// rule (ticket 67) rides here too — Thai models habitually rewrite `2024`
/// as `๒๐๒๔`, and every built-in preset forbids it.
const TRANSLATION_OUTPUT_CONTRACT: &str = "Reply with ONLY the translation — no explanations, no \
     quotes around it, no notes. Preserve line breaks and surrounding whitespace. Keep digits as \
     Arabic numerals (0-9) — never convert them to Thai numerals (๐-๙). If the text is already \
     in {target}, repeat it unchanged.";

/// The base translation instruction for a tuning: the tuning, then the
/// output contract.
fn base_instruction(tuning: &str) -> String {
    format!("{tuning} {TRANSLATION_OUTPUT_CONTRACT}")
}

/// The fused vision call's base instruction (ticket 12): read every Text
/// Block in the attached screen capture and translate each one from
/// {source} to {target} as JSON. The reply *contract* — the exact shape
/// the loop parses mechanically — lives separately in
/// [`FUSED_REPLY_CONTRACT`] so a custom base (ticket 37) can never
/// suppress it; with the built-in base the two join into exactly the one
/// template that shipped before (pinned byte-for-byte by a test).
const FUSED_BASE_INSTRUCTION: &str = "You are a screen-reading translation engine. The attached \
     image is a capture of the user's screen showing text in {source}. Read every visible text \
     block and translate each one from {source} to {target}. Reply with ONLY a JSON array — no \
     explanations, no markdown fences, no text outside the array. One object per text block, \
     ordered top-to-bottom then left-to-right, exactly this shape:";

/// The fused reply contract: the pinned JSON array shape, the field
/// semantics, and the empty-array case — everything the mechanical parser
/// depends on. Always appended, whatever the base instruction says
/// (ticket 37): a custom prompt must not be able to break parsing. The
/// `{target}` placeholder substitutes like the base's.
const FUSED_REPLY_CONTRACT: &str = "\n\
     [{\"x\": 12, \"y\": 34, \"w\": 200, \"h\": 24, \"text\": \"the exact source text\", \"th\": \
     \"คำแปล\"}]\n\
     \"x\" and \"y\" are the block's top-left corner in image pixels; \"w\" and \"h\" are its \
     width and height in image pixels (all four integers). \"text\" is the source text exactly \
     as written on screen; \"th\" is the {target} translation, with digits as Arabic numerals \
     (0-9), never Thai numerals (๐-๙). Merge the lines of one paragraph \
     into a single block. Omit anything you cannot actually read. Reply with [] when the image \
     contains no readable text.";

/// The contract's own preamble, used only when the base is a custom
/// prompt (ticket 37): the built-in base's "exactly this shape:" intro
/// does not ride along, so the appended contract introduces itself.
const FUSED_CUSTOM_CONTRACT_PREAMBLE: &str = "The reply format below is fixed and applies \
     regardless of the instruction above — reply with ONLY this JSON array, one object per text \
     block:";

/// A System Prompt Preset's pair of base instructions (CONTEXT.md): exactly
/// ticket 37's two override texts, `{source}`/`{target}` placeholders
/// included. Empty fields mean the built-in default, as in the config.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PromptTexts {
    /// Replaces the per-block translation prompt's base instruction.
    pub translation: String,
    /// Replaces the fused screen-reading prompt's base instruction. The
    /// pinned JSON reply contract still appends outside of it.
    pub screen_reading: String,
}

/// The built-in System Prompt Presets (ticket 42, CONTEXT.md), in selector
/// order. They ship with the app and cannot be deleted or edited in place:
/// selecting one copies its [`texts`](Self::texts) into ticket 37's two
/// override fields, and editing the copy leaves the preset (the selection
/// becomes custom). Their wordings live here like every other prompt — a
/// change to one bumps the same prompt version.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuiltinPreset {
    /// The app's default: no override texts, the built-in templates.
    General,
    /// Game UI and dialogue: terse strings read as natural Thai UI text.
    Game,
    /// Narrative prose: literary, flowing Thai.
    Novel,
    /// Manga pages: dialogue and sound effects.
    Manga,
}

impl BuiltinPreset {
    /// Every built-in, in selector order.
    pub const ALL: [Self; 4] = [Self::General, Self::Game, Self::Novel, Self::Manga];

    /// The config-file / settings-surface spelling.
    pub fn code(self) -> &'static str {
        match self {
            BuiltinPreset::General => "general",
            BuiltinPreset::Game => "game",
            BuiltinPreset::Novel => "novel",
            BuiltinPreset::Manga => "manga",
        }
    }

    /// Parses [`Self::code`] output; `None` for anything else (a saved
    /// preset's name, "custom", a typo).
    pub fn from_code(code: &str) -> Option<Self> {
        BuiltinPreset::ALL
            .into_iter()
            .find(|preset| preset.code() == code)
    }

    /// The selector label.
    pub fn name(self) -> &'static str {
        match self {
            BuiltinPreset::General => "General",
            BuiltinPreset::Game => "Game",
            BuiltinPreset::Novel => "Novel",
            BuiltinPreset::Manga => "Manga",
        }
    }

    /// The preset's base instructions. General has none — it IS the built-in
    /// default — so both its fields are empty, like an unconfigured config.
    pub fn texts(self) -> PromptTexts {
        let (translation, screen_reading) = match self {
            BuiltinPreset::General => return PromptTexts::default(),
            BuiltinPreset::Game => (GAME_TUNING, GAME_SCREEN_READING),
            BuiltinPreset::Novel => (NOVEL_TUNING, NOVEL_SCREEN_READING),
            BuiltinPreset::Manga => (MANGA_TUNING, MANGA_SCREEN_READING),
        };
        PromptTexts {
            translation: base_instruction(translation),
            screen_reading: screen_reading.to_string(),
        }
    }
}

/// The built-in default's two base instructions — exactly what a blank
/// pair of override fields (the General preset, ticket 68) sends: the
/// default tuning with the output contract, and the fused read-and-
/// translate base (the reply contract excluded — it appends regardless,
/// the same shape every preset's screen-reading text has). The Settings
/// surface shows this wording in the prompt fields' placeholders, since
/// General's stored texts are empty by design.
pub fn default_texts() -> PromptTexts {
    PromptTexts {
        translation: base_instruction(DEFAULT_TRANSLATION_TUNING),
        screen_reading: FUSED_BASE_INSTRUCTION.to_string(),
    }
}

/// Game — translation tuning: game UI and dialogue. Terse strings must read
/// as natural Thai UI text (concise, an imperative stays imperative, a
/// single-word label stays a single word), proper nouns stay consistent,
/// in-world terms stay stable, symbols and placeholders survive.
const GAME_TUNING: &str = "You are a translation engine for a video game. Translate the user's \
     text from {source} to {target}. The text is game content — menu items, button labels, item \
     and skill names, tooltips, system messages, or character dialogue — so read terse strings as \
     natural Thai UI text: stay concise, keep an imperative an imperative, and keep a single-word \
     label a single word. Render proper nouns and in-world terms (places, factions, items, \
     skills) the way they are usually written in Thai, and the same way throughout the text. \
     Leave symbols and placeholders (★, ♥, ×2, %s, {0}) exactly as written.";

const GAME_SCREEN_READING: &str = "You are a screen-reading translation engine for a video game. \
     The attached image is a capture of the user's screen showing {source} game content — menu \
     items, button labels, item and skill names, tooltips, system messages, or character \
     dialogue. Read every visible text block and translate each one from {source} to {target}. \
     Read terse strings as natural Thai UI text: stay concise, keep an imperative an imperative, \
     and keep a single-word label a single word. Render proper nouns and in-world terms (places, \
     factions, items, skills) the way they are usually written in Thai. Leave symbols and \
     placeholders (★, ♥, ×2, %s, {0}) exactly as written.";

/// Novel — translation tuning: narrative prose. Literary, flowing Thai;
/// paragraph breaks preserved; the register stays consistent across
/// consecutive text.
const NOVEL_TUNING: &str = "You are a literary translation engine. Translate the user's text from \
     {source} to {target}. The text is narrative prose, so write literary, flowing Thai that \
     reads as if the story had been written in Thai: preserve paragraph breaks, keep the register \
     (person and tense) consistent across consecutive passages, and keep character and place \
     names consistent.";

const NOVEL_SCREEN_READING: &str = "You are a screen-reading literary translation engine. The \
     attached image is a capture of the user's screen showing {source} narrative prose. Read \
     every visible text block and translate each one from {source} to {target} as literary, \
     flowing Thai that reads as if the story had been written in Thai: preserve paragraph breaks, \
     keep the register (person and tense) consistent across consecutive passages, and keep \
     character and place names consistent.";

/// Manga — translation tuning: dialogue- and SFX-heavy pages. Conversational
/// Thai with per-speaker voice consistency; short punchy lines stay punchy;
/// sound effects follow common Thai manga conventions.
const MANGA_TUNING: &str = "You are a manga translation engine. Translate the user's text from \
     {source} to {target}. The text comes from manga or comic pages — speech bubbles, narration \
     boxes, and sound effects — so write conversational Thai that fits a speaker talking: keep \
     each speaker's voice and level of politeness consistent, keep short punchy lines short and \
     punchy, and render sound effects the way Thai manga usually does.";

const MANGA_SCREEN_READING: &str = "You are a screen-reading manga translation engine. The \
     attached image is a capture of the user's screen showing {source} manga or comic text — \
     speech bubbles, narration boxes, and sound effects. Read every visible text block and \
     translate each one from {source} to {target} in conversational Thai that fits a speaker \
     talking: keep each speaker's voice and level of politeness consistent, keep short punchy \
     lines short and punchy, and render sound effects the way Thai manga usually does.";

/// The Genderless Mode instruction (ticket 08's rule at prompt level):
/// particles drop, first person goes neutral — ฉัน, per ticket 66 (it was
/// เรา, which reads as "we"). Worded as a directive — models follow "do
/// not use X" better than descriptions.
const GENDERLESS_INSTRUCTION: &str = "Genderless Mode: write the translation so it carries no \
     speaker gender. Do not use gendered Thai politeness particles (ครับ, ค่ะ, คะ, จ้า, จ้ะ, \
     นะครับ, นะคะ) — omit them. Do not use gendered first-person pronouns (ผม, หนู) — use \
     the neutral ฉัน instead.";

/// The Translation Context section's instruction (ticket 44): what the pairs
/// below it are and what the model should do with them — continuity of tone,
/// register, and naming with what this same area already showed.
const RECENT_TRANSLATIONS_INSTRUCTION: &str = "Recent translations of this area, oldest first \
     — keep the tone, register, and naming consistent with them:";

/// Language names for the prompt — models follow names better than ISO
/// codes, and "th" alone reads ambiguous.
fn source_name(source: SourceLanguage) -> &'static str {
    match source {
        SourceLanguage::En => "English",
        SourceLanguage::Ja => "Japanese",
        SourceLanguage::Zh => "Chinese",
        SourceLanguage::Ko => "Korean",
    }
}

fn target_name(target: &str) -> String {
    match target {
        "th" => "Thai".to_string(),
        other => other.to_string(),
    }
}

/// Appends the Glossary section — shared by both templates so the wording
/// (and the entries' rendering) can never drift between them.
fn push_glossary_section(prompt: &mut String, guidance: &TranslationGuidance) {
    if !guidance.glossary.is_empty() {
        prompt.push_str("\n\nGlossary — fixed translations. When one of these source terms \
                         appears in the text, translate it with exactly the given Thai wherever \
                         it occurs, including mid-sentence:");
        for (term, thai) in &guidance.glossary {
            prompt.push_str(&format!("\n- {term} → {thai}"));
        }
    }
}

/// Appends the Translation Context section (ticket 44) — shared by both
/// templates like the Glossary's, so the continuity wording can never
/// drift. Rendered only when the caller sent pairs: with the feature off
/// (the default) nothing appends and the prompt is byte-identical to the
/// pre-44 template apart from the version tag. Pairs are already in
/// oldest-first order (the orchestrator's rolling buffer order).
fn push_recent_translations_section(prompt: &mut String, guidance: &TranslationGuidance) {
    if !guidance.recent_translations.is_empty() {
        prompt.push_str("\n\n");
        prompt.push_str(RECENT_TRANSLATIONS_INSTRUCTION);
        for (source, thai) in &guidance.recent_translations {
            prompt.push_str(&format!("\n- {source} → {thai}"));
        }
    }
}

/// The user's custom base instruction (ticket 37), normalized: leading and
/// trailing whitespace is dropped, and a whitespace-only value counts as
/// empty (`None` = the built-in template). One rule for every consumer, so
/// a stray paste can never silently change what every translation is asked.
fn custom_base(guidance: &TranslationGuidance) -> Option<&str> {
    let text = guidance.custom_base.as_deref()?.trim();
    (!text.is_empty()).then_some(text)
}

/// How much of a preset's name the version tag carries: the tag rides in
/// every request, and a saved preset's name is the user's free text.
const PRESET_TAG_LABEL_MAX_CHARS: usize = 40;

/// A preset name as the version tag spells it: one line, without the
/// parentheses that would end the tag early, at most
/// [`PRESET_TAG_LABEL_MAX_CHARS`] characters. A name with nothing left
/// still reads as a label.
fn preset_tag_label(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .filter(|c| {
            // Whitespace controls (\n, \t) stay for the collapse below;
            // every other control character drops.
            !matches!(c, '(' | ')') && (!c.is_control() || c.is_whitespace())
        })
        .collect();
    let collapsed = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    let cut: String = collapsed.chars().take(PRESET_TAG_LABEL_MAX_CHARS).collect();
    if cut.trim().is_empty() {
        "unnamed".to_string()
    } else {
        cut.trim().to_string()
    }
}

/// Appends the version tag — marked `custom` when a user base is in
/// effect, so a recorded request always shows what produced it (ticket
/// 37); when that base is a System Prompt Preset's verbatim wording the
/// tag names the preset instead (ticket 42). Shared by both builders so
/// the tag's shape can never drift.
fn push_version_tag(
    prompt: &mut String,
    guidance: &TranslationGuidance,
    version: &str,
    kind: &str,
) {
    let marker = match (custom_base(guidance).is_some(), guidance.preset.as_deref()) {
        (false, _) => String::new(),
        (true, Some(preset)) => format!(" preset:{}", preset_tag_label(preset)),
        (true, None) => " custom".to_string(),
    };
    prompt.push_str(&format!("\n\n({kind} v{version}{marker})"));
}

/// Builds the LLM system prompt: the base translation instruction (the
/// user's custom one when configured, ticket 37), then — only when
/// present — the Glossary section (the entries relevant to the block being
/// translated, ticket 11), the Genderless instruction (ticket 08 at
/// prompt level), and the Translation Context section (the zone's recent
/// translations, ticket 44), then the version tag (marked `custom` when a
/// custom base is in effect, so a recorded request shows what produced it).
pub fn system_prompt(
    source: SourceLanguage,
    target: &str,
    guidance: &TranslationGuidance,
) -> String {
    let custom = custom_base(guidance);
    let mut prompt = match custom {
        Some(text) => text.to_string(),
        None => base_instruction(DEFAULT_TRANSLATION_TUNING),
    }
    .replace("{source}", source_name(source))
    .replace("{target}", &target_name(target));

    push_glossary_section(&mut prompt, guidance);

    if guidance.genderless {
        prompt.push_str("\n\n");
        prompt.push_str(GENDERLESS_INSTRUCTION);
    }

    push_recent_translations_section(&mut prompt, guidance);

    push_version_tag(&mut prompt, guidance, PROMPT_VERSION, "translation-prompt");
    prompt
}

/// Builds the fused vision call's system prompt (ticket 12): the
/// read-and-translate base instruction followed by the pinned JSON reply
/// contract — split constants that join into exactly the one template that
/// shipped before, and that stay joined under a custom base (ticket 37),
/// because the loop parses the reply mechanically. Then the same Glossary
/// section, Genderless instruction, and Translation Context section (the
/// target zone's recent translations, ticket 44) the per-block translation
/// prompt uses — the whole Glossary rides in a fused call because the
/// model, not an OCR engine, is what reads the text (the orchestrator
/// passes every entry) — then the fused version tag (marked `custom` under
/// a custom base).
pub fn fused_system_prompt(
    source: SourceLanguage,
    target: &str,
    guidance: &TranslationGuidance,
) -> String {
    let custom = custom_base(guidance);
    let mut prompt = match custom {
        Some(text) => format!("{text}\n\n{FUSED_CUSTOM_CONTRACT_PREAMBLE}{FUSED_REPLY_CONTRACT}"),
        None => format!("{FUSED_BASE_INSTRUCTION}{FUSED_REPLY_CONTRACT}"),
    }
    .replace("{source}", source_name(source))
    .replace("{target}", &target_name(target));

    push_glossary_section(&mut prompt, guidance);

    if guidance.genderless {
        prompt.push_str("\n\n");
        prompt.push_str(GENDERLESS_INSTRUCTION);
    }

    push_recent_translations_section(&mut prompt, guidance);

    push_version_tag(&mut prompt, guidance, FUSED_PROMPT_VERSION, "fused-prompt");
    prompt
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prompt_with(entries: &[(&str, &str)], genderless: bool) -> String {
        system_prompt(
            SourceLanguage::En,
            "th",
            &TranslationGuidance {
                glossary: entries
                    .iter()
                    .map(|(t, th)| (t.to_string(), th.to_string()))
                    .collect(),
                genderless,
                custom_base: None,
                preset: None,
                recent_translations: Vec::new(),
            },
        )
    }

    #[test]
    fn base_prompt_names_both_languages() {
        let prompt = system_prompt(SourceLanguage::Ja, "th", &TranslationGuidance::plain());
        assert!(prompt.contains("Japanese"), "was: {prompt}");
        assert!(prompt.contains("Thai"), "was: {prompt}");
        assert!(prompt.contains("translation engine"), "was: {prompt}");
    }

    #[test]
    fn chinese_source_maps_to_chinese_in_the_prompt() {
        let prompt = system_prompt(SourceLanguage::Zh, "th", &TranslationGuidance::plain());
        assert!(prompt.contains("Chinese"), "zh must say Chinese, not the code");
    }

    #[test]
    fn every_prompt_carries_the_version_tag() {
        let prompt = system_prompt(SourceLanguage::En, "th", &TranslationGuidance::plain());
        assert!(
            prompt.contains("(translation-prompt v3)"),
            "the version tag must ride in the prompt, was: {prompt}"
        );
    }

    #[test]
    fn glossary_entries_are_injected_as_fixed_translations() {
        let prompt = prompt_with(&[("Vertin", "เวอร์ติน")], false);
        assert!(prompt.contains("Glossary"), "was: {prompt}");
        assert!(prompt.contains("- Vertin → เวอร์ติน"), "was: {prompt}");
        assert!(
            prompt.contains("mid-sentence"),
            "the instruction must cover in-sentence occurrence, was: {prompt}"
        );
    }

    #[test]
    fn every_glossary_entry_gets_its_own_line() {
        let prompt = prompt_with(&[("Vertin", "เวอร์ติน"), ("勇者", "ผู้กล้า")], false);
        assert!(prompt.contains("- Vertin → เวอร์ติน"), "was: {prompt}");
        assert!(prompt.contains("- 勇者 → ผู้กล้า"), "was: {prompt}");
    }

    #[test]
    fn empty_glossary_injects_no_section() {
        let prompt = system_prompt(SourceLanguage::En, "th", &TranslationGuidance::plain());
        assert!(!prompt.contains("Glossary"), "was: {prompt}");
    }

    #[test]
    fn genderless_mode_adds_the_instruction() {
        let prompt = prompt_with(&[], true);
        assert!(prompt.contains("Genderless Mode"), "was: {prompt}");
        assert!(prompt.contains("ครับ"), "particles must be named, was: {prompt}");
        assert!(prompt.contains("ฉัน"), "the neutral pronoun must be named, was: {prompt}");
    }

    #[test]
    fn genderless_off_omits_the_instruction() {
        let prompt = prompt_with(&[], false);
        assert!(!prompt.contains("Genderless Mode"), "was: {prompt}");
    }

    #[test]
    fn glossary_and_genderless_compose() {
        // The Reverse: 1999 combination — pinned names AND no speaker gender.
        let prompt = prompt_with(&[("Vertin", "เวอร์ติน")], true);
        assert!(prompt.contains("- Vertin → เวอร์ติน"), "was: {prompt}");
        assert!(prompt.contains("Genderless Mode"), "was: {prompt}");
    }

    // ---- Fused vision prompt (ticket 12) ----

    #[test]
    fn fused_prompt_carries_the_json_contract_and_version_tag() {
        let prompt = fused_system_prompt(SourceLanguage::Ja, "th", &TranslationGuidance::plain());
        assert!(prompt.contains("JSON array"), "was: {prompt}");
        assert!(prompt.contains("no markdown fences"), "was: {prompt}");
        assert!(prompt.contains("\"th\""), "was: {prompt}");
        assert!(prompt.contains("(fused-prompt v3)"), "was: {prompt}");
        // The per-block translation template is untouched by the fused one.
        assert!(!prompt.contains("(translation-prompt"), "was: {prompt}");
    }

    #[test]
    fn fused_prompt_names_both_languages() {
        let prompt = fused_system_prompt(SourceLanguage::Ko, "th", &TranslationGuidance::plain());
        assert!(prompt.contains("Korean"), "was: {prompt}");
        assert!(prompt.contains("Thai"), "was: {prompt}");
    }

    #[test]
    fn fused_prompt_injects_the_same_glossary_and_genderless_wording() {
        // Ticket 12's acceptance: glossary injection and the Genderless
        // instruction apply to the fused call — the shared section builders,
        // so the entries render exactly as in the translation prompt.
        let prompt = fused_system_prompt(
            SourceLanguage::En,
            "th",
            &TranslationGuidance {
                glossary: vec![("Vertin".into(), "เวอร์ติน".into())],
                genderless: true,
                custom_base: None,
                preset: None,
                recent_translations: Vec::new(),
            },
        );
        assert!(prompt.contains("- Vertin → เวอร์ติน"), "was: {prompt}");
        assert!(prompt.contains("mid-sentence"), "was: {prompt}");
        assert!(prompt.contains("Genderless Mode"), "was: {prompt}");
        assert!(prompt.contains("ครับ"), "was: {prompt}");
    }

    #[test]
    fn fused_prompt_omits_empty_glossary_and_off_genderless() {
        let prompt = fused_system_prompt(SourceLanguage::En, "th", &TranslationGuidance::plain());
        assert!(!prompt.contains("Glossary"), "was: {prompt}");
        assert!(!prompt.contains("Genderless Mode"), "was: {prompt}");
    }

    // ---- Custom base instruction (ticket 37) ----

    fn with_custom_base(custom: &str) -> TranslationGuidance {
        TranslationGuidance {
            custom_base: Some(custom.into()),
            ..TranslationGuidance::plain()
        }
    }

    #[test]
    fn empty_and_whitespace_overrides_are_byte_identical_to_the_default_prompt() {
        // The built-in default, pinned byte-for-byte: an absent override, an
        // empty one, and a whitespace-only one all produce exactly this —
        // nothing about the default wording may shift. (Ticket 44 bumped
        // the version tag v1 → v2 by the versioning rule; ticket 67 added
        // the Arabic-numerals sentence and bumped v2 → v3, and with
        // Translation Context off no section appends.)
        let expected = "You are a translation engine. Translate the user's text from \
English to Thai. Reply with ONLY the translation — no explanations, no quotes \
around it, no notes. Preserve line breaks and surrounding whitespace. Keep \
digits as Arabic numerals (0-9) — never convert them to Thai numerals (๐-๙). \
If the text is already in Thai, repeat it unchanged.\n\n(translation-prompt v3)";
        assert_eq!(
            system_prompt(SourceLanguage::En, "th", &TranslationGuidance::plain()),
            expected
        );
        for blank in ["", "   ", " \n\t "] {
            let prompt = system_prompt(SourceLanguage::En, "th", &with_custom_base(blank));
            assert_eq!(prompt, expected, "blank override {blank:?} must be the default");
        }
    }

    #[test]
    fn the_default_fused_prompt_is_byte_identical_to_the_built_in_template() {
        // Splitting the reply contract out of the base instruction
        // (ticket 37) must not move a single byte of what shipped before;
        // ticket 44's only change is the bumped version tag, ticket 67's
        // only change is the Arabic-numerals sentence (v2 → v3).
        let expected = "You are a screen-reading translation engine. The attached image is a \
capture of the user's screen showing text in Japanese. Read every visible text \
block and translate each one from Japanese to Thai. Reply with ONLY a JSON \
array — no explanations, no markdown fences, no text outside the array. One \
object per text block, ordered top-to-bottom then left-to-right, exactly this \
shape:\n\
[{\"x\": 12, \"y\": 34, \"w\": 200, \"h\": 24, \"text\": \"the exact source text\", \"th\": \
\"คำแปล\"}]\n\
\"x\" and \"y\" are the block's top-left corner in image pixels; \"w\" and \"h\" are its \
width and height in image pixels (all four integers). \"text\" is the source text \
exactly as written on screen; \"th\" is the Thai translation, with digits as \
Arabic numerals (0-9), never Thai numerals (๐-๙). Merge the lines of \
one paragraph into a single block. Omit anything you cannot actually read. \
Reply with [] when the image contains no readable text.\n\n(fused-prompt v3)";
        assert_eq!(
            fused_system_prompt(SourceLanguage::Ja, "th", &TranslationGuidance::plain()),
            expected
        );
        let prompt = fused_system_prompt(SourceLanguage::Ja, "th", &with_custom_base("  \n"));
        assert_eq!(prompt, expected, "a whitespace-only override must be the default");
    }

    #[test]
    fn a_custom_translation_prompt_replaces_the_base_instruction_only() {
        let prompt = system_prompt(
            SourceLanguage::Ja,
            "th",
            &with_custom_base(
                "Always translate {source} game menus into {target}. Keep item names short.",
            ),
        );
        // Placeholders substitute exactly like the default template's.
        assert!(
            prompt.contains("Always translate Japanese game menus into Thai."),
            "was: {prompt}"
        );
        assert!(
            !prompt.contains("translation engine"),
            "the built-in base must be gone, was: {prompt}"
        );
        // The version tag still rides, marked custom so a recorded request
        // shows what produced it.
        assert!(
            prompt.contains("(translation-prompt v3 custom)"),
            "was: {prompt}"
        );
        assert!(
            !prompt.contains("(translation-prompt v3)"),
            "the plain tag must not leak into custom prompts, was: {prompt}"
        );
    }

    #[test]
    fn a_custom_translation_prompt_can_repeat_placeholders() {
        // Every occurrence substitutes, not just the first.
        let prompt = system_prompt(
            SourceLanguage::Zh,
            "th",
            &with_custom_base("{source} → {target}, from {source} into {target}."),
        );
        assert!(prompt.contains("Chinese → Thai, from Chinese into Thai."), "was: {prompt}");
        assert!(!prompt.contains("{source}"), "was: {prompt}");
        assert!(!prompt.contains("{target}"), "was: {prompt}");
    }

    #[test]
    fn glossary_and_genderless_still_attach_to_a_custom_translation_prompt() {
        let mut guidance = with_custom_base("Translate like a pirate.");
        guidance.glossary = vec![("Vertin".into(), "เวอร์ติน".into())];
        guidance.genderless = true;
        let prompt = system_prompt(SourceLanguage::En, "th", &guidance);
        assert!(prompt.contains("Translate like a pirate."), "was: {prompt}");
        assert!(prompt.contains("- Vertin → เวอร์ติน"), "was: {prompt}");
        assert!(prompt.contains("Genderless Mode"), "was: {prompt}");
    }

    #[test]
    fn a_custom_fused_prompt_still_receives_the_reply_contract() {
        // The loop parses the reply mechanically; a custom base must not be
        // able to break it — the pinned contract is re-appended outside the
        // base, with its own preamble.
        let prompt = fused_system_prompt(
            SourceLanguage::En,
            "th",
            &with_custom_base("Read the screen like {source} subtitles for a {target} viewer."),
        );
        assert!(
            prompt.contains("Read the screen like English subtitles for a Thai viewer."),
            "was: {prompt}"
        );
        assert!(
            !prompt.contains("screen-reading translation engine"),
            "the built-in base must be gone, was: {prompt}"
        );
        assert!(prompt.contains("JSON array"), "was: {prompt}");
        assert!(prompt.contains("\"th\""), "was: {prompt}");
        assert!(prompt.contains("no readable text"), "was: {prompt}");
        assert!(prompt.contains("(fused-prompt v3 custom)"), "was: {prompt}");
        assert!(
            !prompt.contains("(fused-prompt v3)"),
            "the plain tag must not leak into custom prompts, was: {prompt}"
        );
    }

    #[test]
    fn a_custom_fused_prompt_keeps_glossary_and_genderless_too() {
        let mut guidance = with_custom_base("Describe every block tersely.");
        guidance.glossary = vec![("勇者".into(), "ผู้กล้า".into())];
        guidance.genderless = true;
        let prompt = fused_system_prompt(SourceLanguage::Ja, "th", &guidance);
        assert!(prompt.contains("- 勇者 → ผู้กล้า"), "was: {prompt}");
        assert!(prompt.contains("Genderless Mode"), "was: {prompt}");
    }

    // ---- System Prompt Presets (ticket 42) ----

    #[test]
    fn a_preset_base_is_tagged_with_the_preset_name() {
        // A recorded request shows which preset produced it: the tag names
        // the preset rather than saying "custom".
        let mut guidance = with_custom_base("Translate like a pirate.");
        guidance.preset = Some("game".into());
        let prompt = system_prompt(SourceLanguage::En, "th", &guidance);
        assert!(
            prompt.contains("(translation-prompt v3 preset:game)"),
            "was: {prompt}"
        );
        assert!(
            !prompt.contains("(translation-prompt v3 custom)"),
            "a preset is not an anonymous custom prompt, was: {prompt}"
        );
    }

    #[test]
    fn a_preset_name_cannot_break_or_pad_the_version_tag() {
        // A saved preset's name is the user's free text; the tag must stay
        // one well-formed line no matter what the name contains. The
        // parentheses go (they would end the tag early), whitespace
        // collapses to single spaces, control characters drop, and a name
        // with nothing left still reads as a label.
        for (name, label) in [
            (
                "My  (pirate)\n\r\nspeak \t preset",
                "My pirate speak preset",
            ),
            ("\0()\u{7}", "unnamed"),
        ] {
            let mut guidance = with_custom_base("Translate like a pirate.");
            guidance.preset = Some(name.into());
            let prompt = system_prompt(SourceLanguage::En, "th", &guidance);
            assert!(
                prompt.ends_with(&format!("\n\n(translation-prompt v3 preset:{label})")),
                "name {name:?}, was: {prompt}"
            );
        }
    }

    #[test]
    fn a_long_preset_name_is_cut_short_in_the_tag() {
        let mut guidance = with_custom_base("Translate like a pirate.");
        guidance.preset = Some("x".repeat(200));
        let prompt = system_prompt(SourceLanguage::En, "th", &guidance);
        let tag = prompt.rfind('(').map(|at| &prompt[at..]).unwrap_or("");
        assert_eq!(
            tag,
            format!("(translation-prompt v3 preset:{})", "x".repeat(40))
        );
    }

    #[test]
    fn a_preset_that_leaves_one_prompt_at_its_default_tags_that_prompt_plainly() {
        // A preset may override only the screen-reading text. Its
        // translation prompt is then exactly the built-in default, and the
        // tag must not claim a preset produced it.
        let guidance = TranslationGuidance {
            custom_base: Some("  \n".into()),
            preset: Some("novel".into()),
            ..TranslationGuidance::plain()
        };
        assert_eq!(
            system_prompt(SourceLanguage::En, "th", &guidance),
            system_prompt(SourceLanguage::En, "th", &TranslationGuidance::plain())
        );
    }

    // ---- Built-in System Prompt Presets (ticket 42) ----

    #[test]
    fn the_four_built_ins_are_general_game_novel_and_manga() {
        let codes: Vec<&str> = BuiltinPreset::ALL
            .iter()
            .map(|preset| preset.code())
            .collect();
        assert_eq!(codes, ["general", "game", "novel", "manga"]);
        for preset in BuiltinPreset::ALL {
            assert_eq!(BuiltinPreset::from_code(preset.code()), Some(preset));
        }
        assert_eq!(BuiltinPreset::from_code("pirate"), None);
    }

    #[test]
    fn general_is_the_built_in_default_and_the_tuned_built_ins_carry_both_texts() {
        // General has no override texts — it IS the built-in default.
        assert_eq!(BuiltinPreset::General.texts(), PromptTexts::default());
        for preset in BuiltinPreset::ALL {
            let texts = preset.texts();
            for (kind, text) in [
                ("translation", &texts.translation),
                ("screen-reading", &texts.screen_reading),
            ] {
                if preset == BuiltinPreset::General {
                    assert!(text.is_empty());
                    continue;
                }
                assert!(!text.is_empty(), "{} has no {kind} text", preset.code());
                assert!(
                    text.contains("{source}") && text.contains("{target}"),
                    "{} {kind} text must name both languages by placeholder: {text}",
                    preset.code()
                );
            }
        }
    }

    #[test]
    fn each_tuned_built_in_wording_matches_its_intent() {
        // The ticket's intents, pinned by the concepts each wording must
        // carry — in BOTH of its texts.
        let intents: [(BuiltinPreset, &[&str]); 3] = [
            (
                BuiltinPreset::Game,
                &[
                    "game",
                    "concise",
                    "imperative",
                    "single-word label",
                    "proper nouns",
                    "symbols",
                ],
            ),
            (
                BuiltinPreset::Novel,
                &["literary", "flowing Thai", "paragraph breaks", "register"],
            ),
            (
                BuiltinPreset::Manga,
                &[
                    "conversational Thai",
                    "speaker's voice",
                    "punchy",
                    "sound effects",
                ],
            ),
        ];
        for (preset, concepts) in intents {
            let texts = preset.texts();
            for (kind, text) in [
                ("translation", &texts.translation),
                ("screen-reading", &texts.screen_reading),
            ] {
                for concept in concepts {
                    assert!(
                        text.contains(concept),
                        "{} {kind} text must cover \"{concept}\": {text}",
                        preset.code()
                    );
                }
            }
        }
    }

    #[test]
    fn every_built_in_translation_text_keeps_the_output_contract() {
        // The loop shows the engine's reply verbatim: a preset's tuning may
        // not drop the rules that keep that reply bare.
        for preset in BuiltinPreset::ALL {
            let texts = preset.texts();
            if texts.translation.is_empty() {
                continue;
            }
            assert!(
                texts.translation.ends_with(TRANSLATION_OUTPUT_CONTRACT),
                "{} translation text must end with the output contract: {}",
                preset.code(),
                texts.translation
            );
        }
    }

    #[test]
    fn general_renders_the_byte_identical_default_prompts() {
        // The default state out of the box: selecting General (with or
        // without its name on the guidance) is exactly what ticket 37's
        // default renders — pinned to the literal bytes by the two tests
        // above.
        let general = BuiltinPreset::General.texts();
        for preset in [None, Some("general")] {
            let guidance = TranslationGuidance {
                custom_base: Some(general.translation.clone()),
                preset: preset.map(str::to_string),
                ..TranslationGuidance::plain()
            };
            assert_eq!(
                system_prompt(SourceLanguage::En, "th", &guidance),
                system_prompt(SourceLanguage::En, "th", &TranslationGuidance::plain())
            );
            let fused = TranslationGuidance {
                custom_base: Some(general.screen_reading.clone()),
                preset: preset.map(str::to_string),
                ..TranslationGuidance::plain()
            };
            assert_eq!(
                fused_system_prompt(SourceLanguage::Ja, "th", &fused),
                fused_system_prompt(SourceLanguage::Ja, "th", &TranslationGuidance::plain())
            );
        }
    }

    #[test]
    fn a_built_in_preset_switches_both_prompt_wordings_with_every_appendix_attached() {
        let texts = BuiltinPreset::Game.texts();
        let guidance = TranslationGuidance {
            glossary: vec![("Vertin".into(), "เวอร์ติน".into())],
            genderless: true,
            custom_base: Some(texts.translation.clone()),
            preset: Some(BuiltinPreset::Game.code().into()),
            recent_translations: Vec::new(),
        };
        let prompt = system_prompt(SourceLanguage::Ja, "th", &guidance);
        // The Game wording, with the placeholders substituted...
        assert!(
            prompt.starts_with(
                "You are a translation engine for a video game. Translate the user's text from \
                 Japanese to Thai."
            ),
            "was: {prompt}"
        );
        assert!(
            prompt.contains("single-word label"),
            "the Game tuning must replace the default's, was: {prompt}"
        );
        // ...not the default's...
        assert!(
            !prompt.contains("You are a translation engine. Translate the user's text"),
            "the built-in default base must be gone, was: {prompt}"
        );
        // ...and every automatic appendix still attached.
        assert!(prompt.contains("- Vertin → เวอร์ติน"), "was: {prompt}");
        assert!(prompt.contains("Genderless Mode"), "was: {prompt}");
        assert!(
            prompt.ends_with("(translation-prompt v3 preset:game)"),
            "was: {prompt}"
        );

        let fused = fused_system_prompt(
            SourceLanguage::Ja,
            "th",
            &TranslationGuidance {
                custom_base: Some(texts.screen_reading.clone()),
                ..guidance.clone()
            },
        );
        assert!(fused.contains("single-word label"), "was: {fused}");
        assert!(
            !fused.contains("screen-reading translation engine. The attached"),
            "the built-in fused base must be gone, was: {fused}"
        );
        assert!(
            fused.contains(r#""th""#),
            "the reply contract must attach, was: {fused}"
        );
        assert!(fused.contains("no readable text"), "was: {fused}");
        assert!(fused.contains("- Vertin → เวอร์ติน"), "was: {fused}");
        assert!(fused.contains("Genderless Mode"), "was: {fused}");
        assert!(
            fused.ends_with("(fused-prompt v3 preset:game)"),
            "was: {fused}"
        );
    }

    // ---- Translation Context section (ticket 44) ----

    fn with_recent(recent: &[(&str, &str)]) -> TranslationGuidance {
        TranslationGuidance {
            recent_translations: recent
                .iter()
                .map(|(s, th)| (s.to_string(), th.to_string()))
                .collect(),
            ..TranslationGuidance::plain()
        }
    }

    #[test]
    fn the_recent_translations_section_lists_pairs_oldest_first_with_the_instruction() {
        let prompt = system_prompt(
            SourceLanguage::En,
            "th",
            &with_recent(&[("Hello", "สวัสดี"), ("Goodbye", "ลาก่อน")]),
        );
        assert!(
            prompt.contains("keep the tone, register, and naming consistent"),
            "the continuity instruction must ride: {prompt}"
        );
        // Oldest → newest, in buffer order, one pair per line.
        let hello_at = prompt.find("- Hello → สวัสดี").expect("first pair line");
        let goodbye_at = prompt.find("- Goodbye → ลาก่อน").expect("second pair line");
        assert!(
            hello_at < goodbye_at,
            "pairs must render oldest first, was: {prompt}"
        );
        // The section sits before the version tag.
        let tag_at = prompt.find("(translation-prompt v3)").unwrap();
        assert!(hello_at < tag_at, "the section precedes the tag: {prompt}");
    }

    #[test]
    fn an_empty_recent_list_renders_no_section() {
        // The default (feature off): not even the section's wording may
        // appear — the prompt is exactly the pre-44 default pinned above.
        for guidance in [TranslationGuidance::plain(), with_recent(&[])] {
            let prompt = system_prompt(SourceLanguage::En, "th", &guidance);
            assert!(
                !prompt.contains("Recent translations"),
                "no section may render while the feature is off: {prompt}"
            );
            assert_eq!(
                prompt,
                system_prompt(SourceLanguage::En, "th", &TranslationGuidance::plain()),
                "an empty list must equal the plain default"
            );
        }
    }

    #[test]
    fn the_fused_prompt_renders_the_same_recent_translations_section() {
        let prompt = fused_system_prompt(
            SourceLanguage::Ja,
            "th",
            &with_recent(&[("勇者", "ผู้กล้า")]),
        );
        assert!(
            prompt.contains("- 勇者 → ผู้กล้า"),
            "the shared builder must render the pairs: {prompt}"
        );
        assert!(
            prompt.contains("keep the tone, register, and naming consistent"),
            "was: {prompt}"
        );
        assert!(
            prompt.contains("(fused-prompt v3)"),
            "was: {prompt}"
        );

        // And with the feature off, nothing renders there either.
        let plain = fused_system_prompt(SourceLanguage::Ja, "th", &TranslationGuidance::plain());
        assert!(!plain.contains("Recent translations"), "was: {plain}");
    }

    #[test]
    fn recent_translations_compose_with_glossary_genderless_and_custom_base() {
        let mut guidance = with_recent(&[("Hello", "สวัสดี")]);
        guidance.glossary = vec![("Vertin".into(), "เวอร์ติน".into())];
        guidance.genderless = true;
        guidance.custom_base = Some("Translate like a pirate.".into());
        let prompt = system_prompt(SourceLanguage::En, "th", &guidance);
        assert!(prompt.contains("Translate like a pirate."), "was: {prompt}");
        assert!(prompt.contains("- Vertin → เวอร์ติน"), "was: {prompt}");
        assert!(prompt.contains("Genderless Mode"), "was: {prompt}");
        assert!(prompt.contains("- Hello → สวัสดี"), "was: {prompt}");
        assert!(
            prompt.contains("(translation-prompt v3 custom)"),
            "was: {prompt}"
        );
    }
}
