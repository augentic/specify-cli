//! Version pin parsing for render-only Vectis scaffolds.
//!
//! Resolution is narrowed to embedded defaults plus an explicit complete
//! TOML override; project-local and user-local configuration are
//! deliberately not inspected.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::scaffold::ScaffoldError;

/// The raw text of the embedded defaults compiled into the scaffold renderer.
const EMBEDDED_DEFAULTS: &str = include_str!("../../embedded/versions.toml");

/// Top-level pinned version document.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct Versions {
    /// Crux + transitive Rust pins.
    pub crux: Crux,
    /// Android toolchain pins.
    pub android: Android,
    /// iOS pins.
    #[serde(default)]
    pub ios: Ios,
    /// Tooling pins.
    pub tooling: Tooling,
}

/// Crux + transitive Rust pins.
///
/// The Rust field names drop the `crux_` prefix that the TOML keys carry;
/// `#[serde(rename = ...)]` preserves version-file parity.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct Crux {
    /// `crux_core` version.
    #[serde(rename = "crux_core")]
    pub core: String,
    /// `crux_http` version.
    #[serde(rename = "crux_http")]
    pub http: String,
    /// `crux_kv` version.
    #[serde(rename = "crux_kv")]
    pub kv: String,
    /// `crux_time` version.
    #[serde(rename = "crux_time")]
    pub time: String,
    /// `crux_platform` version.
    #[serde(rename = "crux_platform")]
    pub platform: String,
    /// `facet` version.
    pub facet: String,
    /// `facet_generate` version.
    pub facet_generate: String,
    /// `serde` version.
    pub serde: String,
    /// `serde_json` version.
    pub serde_json: String,
    /// `uniffi` version.
    pub uniffi: String,
    /// `cargo-swift` version.
    pub cargo_swift: String,
}

/// Android toolchain pins.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
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
#[expect(
    clippy::empty_structs_with_brackets,
    reason = "Brackets keep the struct shape symmetric with siblings; a future iOS pin lands here."
)]
pub struct Ios {}

/// Tooling pins retained in the complete version-file shape.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
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
