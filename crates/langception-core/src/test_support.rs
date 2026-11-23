//! Test-only fixture builders shared by unit tests across modules
//! (compiled only under `cfg(test)`; never part of the shipped crate).

use crate::ports::TranslationGuidance;

/// A [`TranslationGuidance`] from `(term, thai)` pairs plus the genderless
/// flag — the guidance shape every LLM-adapter test builds.
pub(crate) fn guided(entries: &[(&str, &str)], genderless: bool) -> TranslationGuidance {
    TranslationGuidance {
        glossary: entries
            .iter()
            .map(|(term, thai)| (term.to_string(), thai.to_string()))
            .collect(),
        genderless,
        custom_base: None,
        preset: None,
    }
}
