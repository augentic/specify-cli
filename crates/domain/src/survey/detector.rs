//! Detector contract: trait, input/output shapes, error type, and the
//! `Language` hint enum. See RFC-20 §"Detector Contract".

use std::fmt;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::dto::Surface;

/// Primary programming language of a legacy source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    /// `TypeScript` source.
    TypeScript,
    /// `JavaScript` source.
    JavaScript,
    /// Rust source.
    Rust,
    /// Python source.
    Python,
    /// Go source.
    Go,
}

impl fmt::Display for Language {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TypeScript => f.write_str("typescript"),
            Self::JavaScript => f.write_str("javascript"),
            Self::Rust => f.write_str("rust"),
            Self::Python => f.write_str("python"),
            Self::Go => f.write_str("go"),
        }
    }
}

/// Input passed to every detector during a survey run.
#[derive(Debug)]
pub struct DetectorInput<'a> {
    /// Root directory of the legacy source tree.
    pub source_root: &'a Path,
    /// Operator-supplied language hint, when available.
    pub language_hint: Option<Language>,
}

/// Output returned by a detector on success.
#[derive(Debug, Clone)]
pub struct DetectorOutput {
    /// Surfaces discovered by this detector. Empty when the detector's
    /// framework signatures are absent from the source.
    pub surfaces: Vec<Surface>,
}

/// Errors a detector may return. The merge helper translates these into
/// `specify_error::Error` keyed for `detector-failure`.
#[derive(Debug)]
pub enum DetectorError {
    /// The detector found framework signatures but could not produce a
    /// well-formed `Surface` from them.
    Malformed {
        /// Human-readable explanation.
        reason: String,
    },
    /// An I/O error prevented the detector from completing its scan.
    Io {
        /// Human-readable explanation.
        reason: String,
    },
}

impl fmt::Display for DetectorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Malformed { reason } => write!(f, "malformed output: {reason}"),
            Self::Io { reason } => write!(f, "I/O error: {reason}"),
        }
    }
}

impl std::error::Error for DetectorError {}

/// A unit of mechanical, framework-specific surface enumeration.
///
/// Implementations are registered at binary build time via
/// [`super::registry::DetectorRegistry`]. Each detector self-reports
/// applicability internally: when its framework signatures are absent
/// the detector returns `Ok(DetectorOutput { surfaces: vec![] })`.
pub trait Detector: Send + Sync {
    /// Stable identifier used in error payloads (e.g. `"express"`,
    /// `"nestjs"`, `"bullmq"`).
    fn name(&self) -> &'static str;

    /// Run the detector against the source tree described by `input`.
    ///
    /// Implementations MUST return
    /// `Ok(DetectorOutput { surfaces: vec![] })` when their framework
    /// signatures are absent. They MUST NOT panic; panics are caught at
    /// the merge boundary and reported as `detector-failure`.
    ///
    /// # Errors
    ///
    /// Returns [`DetectorError`] when the source tree contains
    /// framework signatures but the detector cannot produce valid
    /// surfaces from them.
    fn detect(&self, input: &DetectorInput<'_>) -> Result<DetectorOutput, DetectorError>;
}
