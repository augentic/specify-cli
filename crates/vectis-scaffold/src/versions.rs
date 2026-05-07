//! Version pin parsing for render-only Vectis scaffolds.
//!
//! RFC-16 narrows `vectis-scaffold` resolution to embedded defaults plus an
//! explicit complete TOML override. It deliberately does not inspect
//! project-local or user-local configuration.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::ScaffoldError;

/// The raw text of the embedded defaults compiled into `vectis-scaffold`.
const EMBEDDED_DEFAULTS: &str = include_str!("../embedded/versions.toml");

/// Top-level pinned version document.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[allow(missing_docs)]
pub struct Versions {
    pub crux: Crux,
    pub android: Android,
    #[serde(default)]
    pub ios: Ios,
    pub tooling: Tooling,
}

/// Crux + transitive Rust pins.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[allow(clippy::struct_field_names, missing_docs)]
pub struct Crux {
    pub crux_core: String,
    pub crux_http: String,
    pub crux_kv: String,
    pub crux_time: String,
    pub crux_platform: String,
    pub facet: String,
    pub facet_generate: String,
    pub serde: String,
    pub serde_json: String,
    pub uniffi: String,
    pub cargo_swift: String,
}

/// Android toolchain pins.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[allow(missing_docs)]
pub struct Android {
    pub compose_bom: String,
    pub koin: String,
    pub ktor: String,
    pub kotlin: String,
    pub agp: String,
    pub gradle: String,
    #[serde(default)]
    pub ndk: Option<String>,
}

/// iOS pins. Empty today, but part of the complete version-file shape.
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize, Serialize)]
#[allow(clippy::empty_structs_with_brackets)]
pub struct Ios {}

/// Tooling pins retained in the complete version-file shape.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[allow(missing_docs)]
pub struct Tooling {
    pub cargo_deny: String,
    pub cargo_vet: String,
    pub xcodegen: String,
}

impl Versions {
    /// Resolve version pins from an optional explicit file or embedded defaults.
    ///
    /// # Errors
    ///
    /// Returns [`ScaffoldError`] when the explicit file is missing or malformed,
    /// or if the embedded defaults ever stop parsing.
    pub fn resolve(version_file: Option<&Path>) -> Result<Self, ScaffoldError> {
        version_file.map_or_else(load_embedded, load_required)
    }

    /// Parse the embedded defaults.
    ///
    /// # Errors
    ///
    /// Returns [`ScaffoldError::Internal`] if the compiled-in TOML is malformed.
    pub fn embedded() -> Result<Self, ScaffoldError> {
        load_embedded()
    }
}

fn load_required(path: &Path) -> Result<Versions, ScaffoldError> {
    if !path.exists() {
        return Err(ScaffoldError::InvalidProject {
            message: format!("version file not found: {}", path.display()),
        });
    }
    let contents = std::fs::read_to_string(path)?;
    parse(&contents).map_err(|err| ScaffoldError::InvalidProject {
        message: format!("failed to parse {}: {err}", path.display()),
    })
}

fn load_embedded() -> Result<Versions, ScaffoldError> {
    parse(EMBEDDED_DEFAULTS).map_err(|err| ScaffoldError::Internal {
        message: format!("embedded versions.toml is malformed: {err}"),
    })
}

fn parse(contents: &str) -> Result<Versions, toml::de::Error> {
    toml::from_str(contents)
}
