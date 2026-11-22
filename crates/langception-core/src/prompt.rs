//! The one versioned place for every LLM translation prompt (ticket 11).
//! Both LLM adapters — the OpenAI-compatible chat engine and Gemini — build
//! their system message through [`system_prompt`], so the prompt's wording,
//! its Glossary injection, and its Genderless instruction can never drift
//! between them. Shaped by [`TranslationGuidance`], which the orchestrator
//! computes per Text Block (relevant Glossary entries + the Genderless
//! flag + the user's optional custom base, ticket 37); prompt
//! *construction* itself is pure and tested offline here, and
//! end to end through the pipeline seam with a fake engine standing in for
//! the HTTP LLM (`orchestrator_tests.rs`).
//!
//! Versioning: every wording change bumps [`PROMPT_VERSION`] and lands in
//! this module only — the version tag rides at the end of the prompt itself,
//! so a recorded request always shows which template produced it (a custom
//! base is tagged `custom`, ticket 37).

use crate::ports::{SourceLanguage, TranslationGuidance};

/// The prompt template's version, tagged at the end of every system prompt.
/// Bump on any wording change that could alter translation output.
pub const PROMPT_VERSION: &str = "1";

/// The fused vision prompt's version ([`fused_system_prompt`]), tagged the
/// same way. Independent of [`PROMPT_VERSION`]: the two templates change
/// for different reasons.
pub const FUSED_PROMPT_VERSION: &str = "1";

/// The base translation instruction (unchanged since ticket 10's
/// OpenAI-compatible prompt — the contract models actually follow).
const BASE_INSTRUCTION: &str = "You are a translation engine. Translate the user's text from \
     {source} to {target}. Reply with ONLY the translation — no explanations, no quotes around \
     it, no notes. Preserve line breaks and surrounding whitespace. If the text is already in \
     {target}, repeat it unchanged.";

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
     as written on screen; \"th\" is the {target} translation. Merge the lines of one paragraph \
     into a single block. Omit anything you cannot actually read. Reply with [] when the image \
     contains no readable text.";

/// The contract's own preamble, used only when the base is a custom
/// prompt (ticket 37): the built-in base's "exactly this shape:" intro
/// does not ride along, so the appended contract introduces itself.
const FUSED_CUSTOM_CONTRACT_PREAMBLE: &str = "The reply format below is fixed and applies \
     regardless of the instruction above — reply with ONLY this JSON array, one object per text \
     block:";

/// The Genderless Mode instruction (ticket 08's rule at prompt level):
/// particles drop, first person goes neutral. Worded as a directive — models
/// follow "do not use X" better than descriptions.
const GENDERLESS_INSTRUCTION: &str = "Genderless Mode: write the translation so it carries no \
     speaker gender. Do not use gendered Thai politeness particles (ครับ, ค่ะ, คะ, จ้า, จ้ะ, \
     นะครับ, นะคะ) — omit them. Do not use gendered first-person pronouns (ผม, ฉัน, หนู) — use \
     the neutral เรา instead.";

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

/// The user's custom base instruction (ticket 37), normalized: leading and
/// trailing whitespace is dropped, and a whitespace-only value counts as
/// empty (`None` = the built-in template). One rule for every consumer, so
/// a stray paste can never silently change what every translation is asked.
fn custom_base(guidance: &TranslationGuidance) -> Option<&str> {
    let text = guidance.custom_base.as_deref()?.trim();
    (!text.is_empty()).then_some(text)
}

/// Appends the version tag — marked `custom` when a user base is in
/// effect, so a recorded request always shows what produced it (ticket
/// 37). Shared by both builders so the tag's shape can never drift.
fn push_version_tag(prompt: &mut String, custom: bool, version: &str, kind: &str) {
    let marker = if custom { " custom" } else { "" };
    prompt.push_str(&format!("\n\n({kind} v{version}{marker})"));
}

/// Builds the LLM system prompt: the base translation instruction (the
/// user's custom one when configured, ticket 37), then — only when
/// present — the Glossary section (the entries relevant to the block being
/// translated, ticket 11) and the Genderless instruction (ticket 08 at
/// prompt level), then the version tag (marked `custom` when a custom base
/// is in effect, so a recorded request shows what produced it).
pub fn system_prompt(
    source: SourceLanguage,
    target: &str,
    guidance: &TranslationGuidance,
) -> String {
    let custom = custom_base(guidance);
    let mut prompt = match custom {
        Some(text) => text.to_string(),
        None => BASE_INSTRUCTION.to_string(),
    }
    .replace("{source}", source_name(source))
    .replace("{target}", &target_name(target));

    push_glossary_section(&mut prompt, guidance);

    if guidance.genderless {
        prompt.push_str("\n\n");
        prompt.push_str(GENDERLESS_INSTRUCTION);
    }

    push_version_tag(&mut prompt, custom.is_some(), PROMPT_VERSION, "translation-prompt");
    prompt
}

/// Builds the fused vision call's system prompt (ticket 12): the
/// read-and-translate base instruction followed by the pinned JSON reply
/// contract — split constants that join into exactly the one template that
/// shipped before, and that stay joined under a custom base (ticket 37),
/// because the loop parses the reply mechanically. Then the same Glossary
/// section and Genderless instruction the per-block translation prompt
/// uses — the whole Glossary rides in a fused call because the model, not
/// an OCR engine, is what reads the text (the orchestrator passes every
/// entry) — then the fused version tag (marked `custom` under a custom
/// base).
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

    push_version_tag(&mut prompt, custom.is_some(), FUSED_PROMPT_VERSION, "fused-prompt");
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
            prompt.contains("(translation-prompt v1)"),
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
        assert!(prompt.contains("เรา"), "the neutral pronoun must be named, was: {prompt}");
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
        assert!(prompt.contains("(fused-prompt v1)"), "was: {prompt}");
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
        // The pre-ticket-37 bytes, pinned: an absent override, an empty
        // one, and a whitespace-only one all produce exactly the built-in
        // prompt — nothing about the default wording may shift.
        let expected = "You are a translation engine. Translate the user's text from \
English to Thai. Reply with ONLY the translation — no explanations, no quotes \
around it, no notes. Preserve line breaks and surrounding whitespace. If the \
text is already in Thai, repeat it unchanged.\n\n(translation-prompt v1)";
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
        // (ticket 37) must not move a single byte of what shipped before.
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
exactly as written on screen; \"th\" is the Thai translation. Merge the lines of \
one paragraph into a single block. Omit anything you cannot actually read. \
Reply with [] when the image contains no readable text.\n\n(fused-prompt v1)";
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
            prompt.contains("(translation-prompt v1 custom)"),
            "was: {prompt}"
        );
        assert!(
            !prompt.contains("(translation-prompt v1)"),
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
        assert!(prompt.contains("(fused-prompt v1 custom)"), "was: {prompt}");
        assert!(
            !prompt.contains("(fused-prompt v1)"),
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
}
