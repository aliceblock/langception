//! Langception core: the translation-loop orchestrator, its ports, the
//! Glossary, the default Google Translate engine, and the config file model.
//! All of it is platform-independent and tested offline; Windows adapters
//! and the Tauri shell live in `src-tauri`.

pub mod config;
pub mod glossary;
pub mod google;
pub(crate) mod hash;
pub mod orchestrator;
pub mod ports;
pub mod profile;
