//! Detector registry populated at binary build time. See RFC-20
//! §"Detector Contract" — mirrors the resolver layering in
//! `crates/tool/src/resolver/`.

use super::detector::Detector;

/// Registry of built-in detectors available to `specify change survey`.
///
/// Populated at binary build time via [`Self::with_builtins`]. Change D
/// wires concrete detectors; until then the registry is empty.
#[derive(Debug)]
pub struct DetectorRegistry {
    detectors: Vec<Box<dyn Detector + Send + Sync>>,
}

impl DetectorRegistry {
    /// Build the registry with all built-in detectors.
    #[must_use]
    pub fn with_builtins() -> Self {
        Self {
            detectors: vec![
                Box::new(super::detectors::BullMqDetector),
                Box::new(super::detectors::ExpressDetector),
                Box::new(super::detectors::NestJsDetector),
            ],
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
