//! Detector registry — deferred extension point.
//!
//! v1 ships an empty registry; every legacy-code source flows through
//! the agent-driven [`mod@super::ingest`] pipeline. A future RFC may
//! register an in-binary detector for a specific (language, framework)
//! pair where regex-style enumeration is cheaper to maintain than the
//! brief; the artifact contract does not change in either direction.
//! See RFC-20 §"Future mechanical reversion".

use super::detector::Detector;

/// Registry of built-in detectors available to `specify change survey`.
///
/// Empty in v1; reserved for deferred extension points.
#[derive(Debug)]
pub struct DetectorRegistry {
    detectors: Vec<Box<dyn Detector + Send + Sync>>,
}

impl DetectorRegistry {
    /// Build the registry with all built-in detectors.
    ///
    /// Empty in v1; reserved for deferred extension points (RFC-20).
    #[must_use]
    pub fn with_builtins() -> Self {
        Self {
            detectors: Vec::new(),
        }
    }

    /// Iterate over registered detectors.
    pub fn iter(&self) -> impl Iterator<Item = &(dyn Detector + Send + Sync)> {
        self.detectors.iter().map(AsRef::as_ref)
    }
}

impl std::fmt::Debug for dyn Detector + Send + Sync {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Detector").field("name", &self.name()).finish()
    }
}
