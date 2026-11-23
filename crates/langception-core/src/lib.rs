//! Langception core: the translation-loop orchestrator, its ports, the
//! Glossary, the Genderless scrubber, the default Google Translate engine,
//! and the config file model. All of it is platform-independent and tested
//! offline; Windows adapters and the Tauri shell live in `src-tauri`.

pub mod azure;
pub mod config;
pub mod deepl;
pub mod fused;
pub mod gemini;
pub mod genderless;
pub mod glossary;
pub mod google;
pub(crate) mod hash;
pub mod merge;
pub mod openai;
pub mod orchestrator;
pub mod paddle;
pub mod ports;
pub mod profile;
pub mod sample;
pub mod prompt;
pub mod prompt_preset;
#[cfg(test)]
pub(crate) mod test_support;
