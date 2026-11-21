//! Glossary: a user-defined mapping from a source-language term to a fixed
//! Thai translation that takes precedence over Translate Engine output.
//!
//! Matching rule (spec): trimmed, case-insensitive, **whole-block exact
//! match** short-circuits the engine entirely. Sub-sentence replacement is
//! explicitly out of scope (cross-script replacement is unreliable).

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Glossary(#[serde(default)] pub BTreeMap<String, String>);

impl Glossary {
    /// Whole-block lookup: `block_text` is trimmed and compared
    /// case-insensitively against every term (also trimmed).
    pub fn lookup(&self, block_text: &str) -> Option<String> {
        let text = block_text.trim();
        if text.is_empty() {
            return None;
        }
        self.0
            .iter()
            .filter(|(term, _)| !term.trim().is_empty())
            .find(|(term, _)| term.trim().eq_ignore_ascii_case(text))
            .map(|(_, thai)| thai.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn glossary(entries: &[(&str, &str)]) -> Glossary {
        Glossary(
            entries
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        )
    }

    #[test]
    fn exact_match_returns_thai() {
        let g = glossary(&[("Vertin", "เวอร์ติน")]);
        assert_eq!(g.lookup("Vertin"), Some("เวอร์ติน".to_string()));
    }

    #[test]
    fn match_is_case_insensitive() {
        let g = glossary(&[("Vertin", "เวอร์ติน")]);
        assert_eq!(g.lookup("VERTIN"), Some("เวอร์ติน".to_string()));
        assert_eq!(g.lookup("vertin"), Some("เวอร์ติน".to_string()));
    }

    #[test]
    fn match_trims_surrounding_whitespace() {
        let g = glossary(&[("Vertin", "เวอร์ติน")]);
        assert_eq!(g.lookup("  Vertin  "), Some("เวอร์ติน".to_string()));
    }

    #[test]
    fn term_with_surrounding_whitespace_still_matches() {
        let g = glossary(&[("  Vertin  ", "เวอร์ติน")]);
        assert_eq!(g.lookup("Vertin"), Some("เวอร์ติน".to_string()));
    }

    #[test]
    fn partial_match_does_not_short_circuit() {
        let g = glossary(&[("Vertin", "เวอร์ติน")]);
        // Whole-block exact match only — a sentence containing the term is
        // left to the engine.
        assert_eq!(g.lookup("Vertin appears here"), None);
        assert_eq!(g.lookup("Vert"), None);
    }

    #[test]
    fn whitespace_only_block_never_matches() {
        let g = glossary(&[("Vertin", "เวอร์ติน")]);
        assert_eq!(g.lookup("   "), None);
        assert_eq!(g.lookup(""), None);
    }

    #[test]
    fn non_latin_term_matches_exactly() {
        let g = glossary(&[("勇者", "ผู้กล้า")]);
        assert_eq!(g.lookup("勇者"), Some("ผู้กล้า".to_string()));
        assert_eq!(g.lookup(" 勇者 "), Some("ผู้กล้า".to_string()));
    }

    #[test]
    fn serializes_as_plain_toml_table() {
        let g = glossary(&[("Vertin", "เวอร์ติน")]);
        let toml = toml::to_string(&g).unwrap();
        assert!(toml.contains("Vertin = \"เวอร์ติน\""), "toml was: {toml}");
    }
}
