//! The one versioned place for every LLM translation prompt (ticket 11).
//! Both LLM adapters — the OpenAI-compatible chat engine and Gemini — build
//! their system message through [`system_prompt`], so the prompt's wording,
//! its Glossary injection, and its Genderless instruction can never drift
//! between them. Shaped by [`TranslationGuidance`], which the orchestrator
//! computes per Text Block (relevant Glossary entries + the Genderless
//! flag); prompt *construction* itself is pure and tested offline here, and
//! end to end through the pipeline seam with a fake engine standing in for
//! the HTTP LLM (`orchestrator_tests.rs`).
//!
//! Versioning: every wording change bumps [`PROMPT_VERSION`] and lands in
//! this module only — the version tag rides at the end of the prompt itself,
//! so a recorded request always shows which template produced it.

use crate::ports::{SourceLanguage, TranslationGuidance};

/// The prompt template's version, tagged at the end of every system prompt.
/// Bump on any wording change that could alter translation output.
pub const PROMPT_VERSION: &str = "1";

/// The base translation instruction (unchanged since ticket 10's
/// OpenAI-compatible prompt — the contract models actually follow).
const BASE_INSTRUCTION: &str = "You are a translation engine. Translate the user's text from \
     {source} to {target}. Reply with ONLY the translation — no explanations, no quotes around \
     it, no notes. Preserve line breaks and surrounding whitespace. If the text is already in \
     {target}, repeat it unchanged.";

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

/// Builds the LLM system prompt: the base translation instruction, then —
/// only when present — the Glossary section (the entries relevant to the
/// block being translated, ticket 11) and the Genderless instruction
/// (ticket 08 at prompt level), then the version tag.
pub fn system_prompt(source: SourceLanguage, target: &str, guidance: &TranslationGuidance) -> String {
    let mut prompt = BASE_INSTRUCTION
        .replace("{source}", source_name(source))
        .replace("{target}", &target_name(target));

    if !guidance.glossary.is_empty() {
        prompt.push_str("\n\nGlossary — fixed translations. When one of these source terms \
                        appears in the text, translate it with exactly the given Thai wherever \
                        it occurs, including mid-sentence:");
        for (term, thai) in &guidance.glossary {
            prompt.push_str(&format!("\n- {term} → {thai}"));
        }
    }

    if guidance.genderless {
        prompt.push_str("\n\n");
        prompt.push_str(GENDERLESS_INSTRUCTION);
    }

    prompt.push_str(&format!("\n\n(translation-prompt v{PROMPT_VERSION})"));
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
}
