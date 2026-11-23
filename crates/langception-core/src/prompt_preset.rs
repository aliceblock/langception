//! System Prompt Presets (CONTEXT.md) at the config level (ticket 42).
//!
//! A preset is a named pair of base instructions — exactly ticket 37's two
//! override texts, `[prompts] translation` and `[prompts] screen_reading`.
//! The built-ins' wordings live in [`crate::prompt`] (the versioned prompt
//! module); what the user saved lives in the config as
//! `[[prompt_presets]]` entries ([`PromptPreset`]).
//!
//! Presets sit on top of ticket 37's fields rather than forking them: the
//! two texts are ALWAYS what the LLM prompts are built from, and a preset
//! is a snapshot that gets loaded into them (select) or saved from them
//! (save as preset). `[prompts] preset` names the selection, and it holds
//! only while the texts are that preset's verbatim wording — anything else
//! is the custom selection, whatever the stored name says (a hand-edited
//! file, or a built-in reworded by an app update). See
//! [`AppConfig::active_prompt_preset`].

use crate::config::{
    AppConfig, ConfigError, PromptSettings, CUSTOM_PROMPT_PRESET as CUSTOM_SELECTION,
    DEFAULT_PROMPT_PRESET,
};
use crate::prompt::{BuiltinPreset, PromptTexts};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// A System Prompt Preset the user saved (Settings → Engines → "Save as
/// preset"): a name plus the two prompt texts it holds. Stored as a
/// `[[prompt_presets]]` table so multi-line texts stay readable. The name
/// is unique, and neither a built-in's ("general", "game", "novel",
/// "manga") nor "custom" ([`validate`]'s rules).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PromptPreset {
    pub name: String,
    /// Replaces the per-block translation prompt's base instruction; empty
    /// keeps the built-in default.
    pub translation: String,
    /// Replaces the fused screen-reading prompt's base instruction; empty
    /// keeps the built-in default. The JSON reply contract appends either
    /// way.
    pub screen_reading: String,
}

/// Which System Prompt Preset the stored prompt texts are — the selection
/// the Settings surface shows and the version tag reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActivePromptPreset {
    /// The built-in default: both texts are blank.
    General,
    /// A built-in (by its code) or a saved preset (by its name) whose
    /// verbatim wording the texts are.
    Named(String),
    /// Hand-written text that is no preset's verbatim wording.
    Custom,
}

impl ActivePromptPreset {
    /// The `[prompts] preset` spelling.
    pub fn code(&self) -> &str {
        match self {
            ActivePromptPreset::General => DEFAULT_PROMPT_PRESET,
            ActivePromptPreset::Named(name) => name,
            ActivePromptPreset::Custom => CUSTOM_SELECTION,
        }
    }
}

impl AppConfig {
    /// The System Prompt Preset in force — read off the TEXTS, because they
    /// are what the prompts are built from. Blank texts are General.
    /// Otherwise the stored selection's preset holds only while the texts
    /// are its verbatim wording (surrounding whitespace aside); anything
    /// else is the custom selection, whatever the stored name says.
    pub fn active_prompt_preset(&self) -> ActivePromptPreset {
        let texts = &self.prompts;
        if texts.translation.trim().is_empty() && texts.screen_reading.trim().is_empty() {
            return ActivePromptPreset::General;
        }
        match self.prompt_preset_texts(&texts.preset) {
            Some(preset) if texts_are_verbatim(
                &preset,
                &texts.translation,
                &texts.screen_reading,
            ) => ActivePromptPreset::Named(texts.preset.clone()),
            _ => ActivePromptPreset::Custom,
        }
    }

    /// Applies a selection made on the Settings surface (ticket 42): the
    /// two texts become the stored texts (trimmed — they are what the
    /// prompts are built from), and the stored name follows the same rule
    /// [`Self::active_prompt_preset`] reads — `name` when it is a known
    /// preset and the texts are its wording, "custom" for edited or
    /// hand-written text, "general" for blank texts. A caller that shows
    /// the user a preset's texts and lets them edit them can therefore send
    /// both back as displayed and never lose an edit. Unknown names are
    /// rejected and change nothing.
    pub fn select_prompt_preset(
        &mut self,
        name: &str,
        texts: PromptTexts,
    ) -> Result<ActivePromptPreset, String> {
        let name = name.trim();
        if name != CUSTOM_SELECTION && self.prompt_preset_texts(name).is_none() {
            return Err(format!("unknown prompt preset \"{name}\""));
        }
        let texts = trimmed_texts(texts);
        self.prompts.preset = name.to_string();
        self.prompts.translation = texts.translation;
        self.prompts.screen_reading = texts.screen_reading;
        let active = self.active_prompt_preset();
        self.prompts.preset = active.code().to_string();
        Ok(active)
    }

    /// "Save as preset" (ticket 42): stores `texts` — the two prompts as
    /// displayed — as the saved preset `name`, and selects it. An existing
    /// saved preset is replaced only with `overwrite` (the caller's
    /// confirmation); a built-in's name is never taken. The config's own
    /// validation judges the name and the texts, so nothing can be saved
    /// that a hand-edited file could not contain.
    pub fn save_prompt_preset(
        &mut self,
        name: &str,
        texts: PromptTexts,
        overwrite: bool,
    ) -> Result<ActivePromptPreset, String> {
        let name = name.trim();
        let existing = self.saved_prompt_preset_index(name);
        if existing.is_some() && !overwrite {
            return Err(format!("a prompt preset named \"{name}\" already exists"));
        }
        let texts = trimmed_texts(texts);
        let mut next = self.clone();
        next.prompts.translation = texts.translation.clone();
        next.prompts.screen_reading = texts.screen_reading.clone();
        let preset = PromptPreset {
            name: name.to_string(),
            translation: texts.translation,
            screen_reading: texts.screen_reading,
        };
        match existing {
            Some(index) => next.prompt_presets[index] = preset,
            None => next.prompt_presets.push(preset),
        }
        next.prompts.preset = name.to_string();
        self.commit_prompt_presets(next)
    }

    /// Renames a saved preset; the selection follows it. The new name meets
    /// the same rules as a new preset's ([`Self::save_prompt_preset`]).
    pub fn rename_prompt_preset(
        &mut self,
        name: &str,
        new_name: &str,
    ) -> Result<ActivePromptPreset, String> {
        let new_name = new_name.trim();
        let mut next = self.clone();
        let index = next
            .saved_prompt_preset_index(name)
            .ok_or_else(|| not_a_saved_preset(name))?;
        if next.selected_preset_is(&next.prompt_presets[index].name) {
            next.prompts.preset = new_name.to_string();
        }
        next.prompt_presets[index].name = new_name.to_string();
        self.commit_prompt_presets(next)
    }

    /// Deletes a saved preset. Deleting the selected one returns the
    /// selection to General — its wording must not linger as anonymous
    /// custom text — unless the texts were edited away from the preset's
    /// wording, in which case they are the user's and stay (as the custom
    /// selection).
    pub fn delete_prompt_preset(&mut self, name: &str) -> Result<ActivePromptPreset, String> {
        let index = self
            .saved_prompt_preset_index(name)
            .ok_or_else(|| not_a_saved_preset(name))?;
        let preset = self.prompt_presets.remove(index);
        if self.selected_preset_is(&preset.name) {
            if texts_are_verbatim(
                &preset.texts(),
                &self.prompts.translation,
                &self.prompts.screen_reading,
            ) {
                self.prompts = PromptSettings::default();
            } else {
                self.prompts.preset = CUSTOM_SELECTION.to_string();
            }
        }
        Ok(self.active_prompt_preset())
    }

    /// Whether `[prompts] preset` names the saved preset `name` — matched
    /// case-insensitively, like every saved-name lookup.
    fn selected_preset_is(&self, name: &str) -> bool {
        name_key(&self.prompts.preset) == name_key(name)
    }

    /// Where the saved preset called `name` stands. Names are matched
    /// case-insensitively, the way validation keeps them unique.
    fn saved_prompt_preset_index(&self, name: &str) -> Option<usize> {
        let wanted = name_key(name);
        self.prompt_presets
            .iter()
            .position(|preset| name_key(&preset.name) == wanted)
    }

    /// Makes `next` the config once it passes the config's own validation.
    /// A rejection comes back as the bare reason — the caller's user never
    /// edited the file, so "config.toml is invalid" would mislead them.
    fn commit_prompt_presets(&mut self, next: AppConfig) -> Result<ActivePromptPreset, String> {
        match validate(&next) {
            Ok(()) => {
                *self = next;
                Ok(self.active_prompt_preset())
            }
            Err(ConfigError::Validation(message)) | Err(ConfigError::Parse(message)) => {
                Err(message)
            }
        }
    }

    /// The base instructions of System Prompt Preset called `name`: a
    /// built-in's by its code (General's are empty — it IS the default), or
    /// a saved preset's by its name. `None` for "custom" and for unknown
    /// names.
    pub fn prompt_preset_texts(&self, name: &str) -> Option<PromptTexts> {
        if let Some(builtin) = BuiltinPreset::from_code(name) {
            return Some(builtin.texts());
        }
        let wanted = name_key(name);
        self.prompt_presets
            .iter()
            .find(|preset| name_key(&preset.name) == wanted)
            .map(PromptPreset::texts)
    }
}

impl PromptPreset {
    /// The preset's two texts as the prompt-module's pair.
    fn texts(&self) -> PromptTexts {
        PromptTexts {
            translation: self.translation.clone(),
            screen_reading: self.screen_reading.clone(),
        }
    }
}

/// Saved-preset names match case-insensitively — the way validation keeps
/// them unique, and the way every other user-named thing in this config
/// (zones, profiles, Glossary terms) matches.
fn name_key(name: &str) -> String {
    name.trim().to_lowercase()
}

/// Whether the two prompt texts are `preset`'s verbatim wording
/// (surrounding whitespace aside — stored texts are trimmed on save).
fn texts_are_verbatim(preset: &PromptTexts, translation: &str, screen_reading: &str) -> bool {
    preset.translation.trim() == translation.trim()
        && preset.screen_reading.trim() == screen_reading.trim()
}

/// The texts as they are stored: trimmed — they are what the prompts are
/// built from, and the trim is what "verbatim" compares.
fn trimmed_texts(texts: PromptTexts) -> PromptTexts {
    PromptTexts {
        translation: texts.translation.trim().to_string(),
        screen_reading: texts.screen_reading.trim().to_string(),
    }
}

/// A name no saved preset may take: a built-in's (whatever the case — a
/// saved "Game" would shadow the built-in in every list), or the custom
/// selection's own code.
fn is_reserved_name(name: &str) -> bool {
    let lowered = name_key(name);
    lowered == CUSTOM_SELECTION || BuiltinPreset::from_code(&lowered).is_some()
}

/// Rename/delete's rejection: only saved presets can be changed.
fn not_a_saved_preset(name: &str) -> String {
    format!(
        "prompt preset \"{name}\" is not a saved preset — built-ins cannot be renamed or deleted"
    )
}

/// Shape checks for the System Prompt Presets, with `[prompts]`-prefixed
/// messages like the rest of [`AppConfig::validate`]: the selection must
/// name a known preset, and every saved preset needs a non-empty name that
/// is neither reserved nor a duplicate, plus at least one text (a preset
/// with none would just be General again).
pub(crate) fn validate(config: &AppConfig) -> Result<(), ConfigError> {
    let mut names = HashSet::new();
    for preset in &config.prompt_presets {
        let name = preset.name.trim();
        if name.is_empty() {
            return Err(ConfigError::Validation(
                "every prompt preset needs a non-empty name".into(),
            ));
        }
        if is_reserved_name(name) {
            return Err(ConfigError::Validation(format!(
                "prompt preset \"{name}\" is a reserved name — saved presets cannot be called \
                 general, game, novel, manga, or custom"
            )));
        }
        if !names.insert(name.to_lowercase()) {
            return Err(ConfigError::Validation(format!(
                "two prompt presets are named \"{name}\" — preset names must be unique"
            )));
        }
        if preset.translation.trim().is_empty() && preset.screen_reading.trim().is_empty() {
            return Err(ConfigError::Validation(format!(
                "prompt preset \"{name}\" needs a translation or a screen-reading text"
            )));
        }
    }
    let selection = &config.prompts.preset;
    if selection != CUSTOM_SELECTION && config.prompt_preset_texts(selection).is_none() {
        let mut accepted: Vec<&str> = BuiltinPreset::ALL
            .iter()
            .map(|preset| preset.code())
            .collect();
        accepted.push(CUSTOM_SELECTION);
        accepted.extend(
            config
                .prompt_presets
                .iter()
                .map(|preset| preset.name.as_str()),
        );
        return Err(ConfigError::Validation(format!(
            "[prompts] preset \"{selection}\" is unknown — presets: {}",
            accepted.join(", ")
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::CUSTOM_PROMPT_PRESET;

    fn config() -> AppConfig {
        AppConfig::from_toml_str(AppConfig::DEFAULT_TEMPLATE).unwrap()
    }

    fn saved(name: &str, translation: &str, screen_reading: &str) -> PromptPreset {
        PromptPreset {
            name: name.into(),
            translation: translation.into(),
            screen_reading: screen_reading.into(),
        }
    }

    #[test]
    fn blank_texts_are_general_whatever_the_stored_name() {
        let mut fresh = config();
        assert_eq!(fresh.active_prompt_preset(), ActivePromptPreset::General);

        // "Custom" with nothing typed is still the built-in default.
        fresh.prompts.preset = CUSTOM_PROMPT_PRESET.into();
        fresh.prompts.translation = " \n ".into();
        assert_eq!(fresh.active_prompt_preset(), ActivePromptPreset::General);
    }

    #[test]
    fn a_named_selection_holds_only_while_the_texts_are_its_verbatim_wording() {
        // A built-in, selected with its texts in place.
        let game = BuiltinPreset::Game.texts();
        let mut on_game = config();
        on_game.prompts.preset = "game".into();
        on_game.prompts.translation = game.translation.clone();
        on_game.prompts.screen_reading = game.screen_reading.clone();
        assert_eq!(
            on_game.active_prompt_preset(),
            ActivePromptPreset::Named("game".into())
        );

        // A saved preset, likewise.
        let mut on_saved = config();
        on_saved.prompt_presets = vec![saved("My JRPG", "Be brief.", "")];
        on_saved.prompts.preset = "My JRPG".into();
        on_saved.prompts.translation = "Be brief.".into();
        assert_eq!(
            on_saved.active_prompt_preset(),
            ActivePromptPreset::Named("My JRPG".into())
        );

        // An edited text leaves the preset, whatever the stored name says.
        let mut edited = on_game.clone();
        edited.prompts.screen_reading.push_str(" Answer tersely.");
        assert_eq!(edited.active_prompt_preset(), ActivePromptPreset::Custom);

        // Surrounding whitespace is not an edit.
        let mut padded = on_game.clone();
        padded.prompts.translation = format!(" \n{}\n", padded.prompts.translation);
        assert_eq!(
            padded.active_prompt_preset(),
            ActivePromptPreset::Named("game".into())
        );
    }

    fn texts(translation: &str, screen_reading: &str) -> PromptTexts {
        PromptTexts {
            translation: translation.into(),
            screen_reading: screen_reading.into(),
        }
    }

    #[test]
    fn selecting_a_preset_stores_its_wording_under_its_name() {
        let game = BuiltinPreset::Game.texts();
        let mut config = config();
        config.prompts.translation = "Be brief.".into();

        let active = config.select_prompt_preset("game", game.clone()).unwrap();

        assert_eq!(active, ActivePromptPreset::Named("game".into()));
        // What a restart reads back: the name and both of its texts.
        let reloaded = AppConfig::from_toml_str(&config.to_toml_string().unwrap()).unwrap();
        assert_eq!(reloaded.prompts.preset, "game");
        assert_eq!(reloaded.prompts.translation, game.translation);
        assert_eq!(reloaded.prompts.screen_reading, game.screen_reading);
        assert_eq!(
            reloaded.active_prompt_preset(),
            ActivePromptPreset::Named("game".into())
        );
    }

    #[test]
    fn selecting_general_clears_both_texts() {
        let mut config = config();
        config.prompts.translation = "Be brief.".into();
        config.prompts.screen_reading = "Read everything.".into();

        let active = config
            .select_prompt_preset("general", texts("", ""))
            .unwrap();

        assert_eq!(active, ActivePromptPreset::General);
        assert_eq!(config.prompts, PromptSettings::default());
    }

    #[test]
    fn edited_wording_is_kept_and_stored_as_the_custom_selection() {
        // The user picks Game, tweaks one text, saves: the tweak is what
        // gets sent, and the selection says so.
        let game = BuiltinPreset::Game.texts();
        let mut config = config();
        let edited = texts(
            &format!("{} Be polite.", game.translation),
            &game.screen_reading,
        );

        let active = config.select_prompt_preset("game", edited.clone()).unwrap();

        assert_eq!(active, ActivePromptPreset::Custom);
        assert_eq!(config.prompts.preset, CUSTOM_PROMPT_PRESET);
        assert_eq!(config.prompts.translation, edited.translation);
        assert_eq!(config.prompts.screen_reading, edited.screen_reading);
    }

    #[test]
    fn the_custom_selection_keeps_the_typed_texts_trimmed() {
        let mut config = config();

        let active = config
            .select_prompt_preset(CUSTOM_PROMPT_PRESET, texts(" Be brief.\n", "  "))
            .unwrap();

        assert_eq!(active, ActivePromptPreset::Custom);
        assert_eq!(config.prompts.preset, CUSTOM_PROMPT_PRESET);
        assert_eq!(config.prompts.translation, "Be brief.");
        assert_eq!(config.prompts.screen_reading, "");
    }

    #[test]
    fn an_unknown_preset_cannot_be_selected() {
        let mut config = config();
        let before = config.clone();

        let err = config
            .select_prompt_preset("babelfish", texts("Be brief.", ""))
            .unwrap_err();

        assert!(
            err.contains("unknown prompt preset \"babelfish\""),
            "was: {err}"
        );
        assert_eq!(config, before, "a rejected selection must change nothing");
    }

    fn reloaded(config: &AppConfig) -> AppConfig {
        AppConfig::from_toml_str(&config.to_toml_string().unwrap()).unwrap()
    }

    #[test]
    fn save_as_preset_stores_the_texts_under_a_new_name_and_selects_it() {
        let mut config = config();
        let typed = texts(" Be brief.\n", "Read everything.");

        let active = config
            .save_prompt_preset("  My JRPG  ", typed, false)
            .unwrap();

        assert_eq!(active, ActivePromptPreset::Named("My JRPG".into()));
        assert_eq!(
            config.prompt_presets,
            vec![saved("My JRPG", "Be brief.", "Read everything.")]
        );
        // The typed texts are what is in force, and a restart reads the
        // selection back identically.
        let after_restart = reloaded(&config);
        assert_eq!(after_restart, config);
        assert_eq!(
            after_restart.active_prompt_preset(),
            ActivePromptPreset::Named("My JRPG".into())
        );
    }

    #[test]
    fn saving_over_an_existing_preset_needs_the_overwrite_flag() {
        let mut config = config();
        config
            .save_prompt_preset("My JRPG", texts("Be brief.", ""), false)
            .unwrap();
        let before = config.clone();

        let err = config
            .save_prompt_preset("my jrpg", texts("Be polite.", ""), false)
            .unwrap_err();

        assert!(
            err.contains("a prompt preset named \"my jrpg\" already exists"),
            "was: {err}"
        );
        assert_eq!(
            config, before,
            "an unconfirmed overwrite must change nothing"
        );

        let active = config
            .save_prompt_preset("my jrpg", texts("Be polite.", ""), true)
            .unwrap();

        assert_eq!(config.prompt_presets.len(), 1);
        assert_eq!(config.prompt_presets[0].translation, "Be polite.");
        assert_eq!(config.prompts.translation, "Be polite.");
        assert_eq!(active, ActivePromptPreset::Named("my jrpg".into()));
    }

    #[test]
    fn built_ins_cannot_be_overwritten_by_save_as() {
        for name in ["general", "Game", "novel", "MANGA", "custom"] {
            let mut config = config();
            config.prompts.translation = "Be brief.".into();
            let before = config.clone();

            let err = config
                .save_prompt_preset(name, texts("Be polite.", "Read."), true)
                .unwrap_err();

            assert!(err.contains("reserved name"), "{name}: was: {err}");
            assert_eq!(
                config, before,
                "{name}: a built-in's place must stay untouched"
            );
        }
    }

    #[test]
    fn save_as_rejects_an_unusable_name_or_nothing_to_save() {
        let mut config = config();
        config
            .save_prompt_preset("My JRPG", texts("Be brief.", ""), false)
            .unwrap();

        for (name, typed, expected) in [
            ("   ", texts("Be polite.", ""), "non-empty name"),
            ("My novel", texts("  ", " \n"), "needs a translation"),
        ] {
            let err = config.save_prompt_preset(name, typed, true).unwrap_err();
            assert!(err.contains(expected), "{name:?}: was: {err}");
        }
        assert_eq!(
            config.prompt_presets.len(),
            1,
            "a rejected save adds nothing"
        );
    }

    #[test]
    fn a_saved_preset_can_be_renamed_and_the_selection_follows() {
        let mut config = config();
        config
            .save_prompt_preset("My JRPG", texts("Be brief.", ""), false)
            .unwrap();

        config
            .rename_prompt_preset("My JRPG", "  JRPG dialogue ")
            .unwrap();

        assert_eq!(
            config.prompt_presets,
            vec![saved("JRPG dialogue", "Be brief.", "")]
        );
        assert_eq!(
            reloaded(&config).active_prompt_preset(),
            ActivePromptPreset::Named("JRPG dialogue".into())
        );
    }

    #[test]
    fn renaming_onto_a_taken_or_reserved_name_is_rejected_and_built_ins_stay() {
        let mut config = config();
        config
            .save_prompt_preset("My JRPG", texts("Be brief.", ""), false)
            .unwrap();
        config
            .save_prompt_preset("My novel", texts("Be literary.", ""), false)
            .unwrap();
        let before = config.clone();

        for (from, to, expected) in [
            ("My JRPG", "my NOVEL", "must be unique"),
            ("My JRPG", "manga", "reserved name"),
            ("My JRPG", "", "non-empty name"),
            ("novel", "Prose", "not a saved preset"),
        ] {
            let err = config.rename_prompt_preset(from, to).unwrap_err();
            assert!(err.contains(expected), "{from} → {to:?}: was: {err}");
        }
        assert_eq!(config, before, "a rejected rename must change nothing");
    }

    #[test]
    fn deleting_the_selected_preset_returns_to_general_with_cleared_texts() {
        let mut config = config();
        config
            .save_prompt_preset("My JRPG", texts("Be brief.", "Read everything."), false)
            .unwrap();

        config.delete_prompt_preset("My JRPG").unwrap();

        assert!(config.prompt_presets.is_empty());
        assert_eq!(config.prompts, PromptSettings::default());
        assert_eq!(config.active_prompt_preset(), ActivePromptPreset::General);
    }

    #[test]
    fn deleting_an_unselected_preset_leaves_the_selection_alone() {
        let mut config = config();
        config
            .save_prompt_preset("My JRPG", texts("Be brief.", ""), false)
            .unwrap();
        config
            .save_prompt_preset("My novel", texts("Be literary.", ""), false)
            .unwrap();

        config.delete_prompt_preset("My JRPG").unwrap();

        assert_eq!(
            config.prompt_presets,
            vec![saved("My novel", "Be literary.", "")]
        );
        assert_eq!(config.prompts.translation, "Be literary.");
        assert_eq!(
            config.active_prompt_preset(),
            ActivePromptPreset::Named("My novel".into())
        );
    }

    #[test]
    fn deleting_a_preset_whose_wording_was_edited_keeps_the_edited_text() {
        // A hand-edited file can still name a preset whose texts were then
        // changed: that text is the user's, not the preset's — deleting the
        // preset must not take it away.
        let mut config = config();
        config
            .save_prompt_preset("My JRPG", texts("Be brief.", ""), false)
            .unwrap();
        config.prompts.translation = "Be brief, and polite.".into();

        config.delete_prompt_preset("My JRPG").unwrap();

        assert_eq!(config.prompts.translation, "Be brief, and polite.");
        assert_eq!(config.active_prompt_preset(), ActivePromptPreset::Custom);
    }

    #[test]
    fn built_ins_cannot_be_renamed_or_deleted() {
        let mut config = config();
        for name in ["general", "game", "novel", "manga", "custom", "babelfish"] {
            let err = config.delete_prompt_preset(name).unwrap_err();
            assert!(err.contains("not a saved preset"), "{name}: was: {err}");
            let err = config.rename_prompt_preset(name, "Whatever").unwrap_err();
            assert!(err.contains("not a saved preset"), "{name}: was: {err}");
        }
        assert!(config.prompt_presets.is_empty());
    }

    #[test]
    fn preset_names_match_case_insensitively_everywhere() {
        // Saved names are unique case-insensitively, so every lookup must
        // match the same way — selecting, a hand-edited selection, and a
        // delete through a differently-cased name of the SELECTED preset.
        let mut config = config();
        config
            .save_prompt_preset("My JRPG", texts("Be brief.", ""), false)
            .unwrap();

        let active = config
            .select_prompt_preset("my jrpg", texts("Be brief.", ""))
            .unwrap();
        assert_eq!(
            active,
            ActivePromptPreset::Named("my jrpg".into()),
            "a case-variant spelling of a saved preset selects it"
        );
        assert_eq!(
            reloaded(&config).active_prompt_preset(),
            ActivePromptPreset::Named("my jrpg".into()),
            "the case-variant selection must survive a restart"
        );

        config.delete_prompt_preset("MY JRPG").unwrap();
        assert_eq!(
            config.prompts,
            PromptSettings::default(),
            "deleting the selected preset through another casing still resets to General"
        );
    }
}
