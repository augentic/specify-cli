//! Merge + dedup helper for detector outputs. Consumes results from
//! all detectors, validates no duplicate surface ids across detectors,
//! and returns a sorted `Vec<Surface>` ready for `SurfacesDocument`.

use std::collections::HashMap;

use specify_error::Error;

use super::detector::{DetectorError, DetectorOutput};
use super::dto::Surface;

/// Merge detector outputs into a single sorted, deduplicated surface
/// list.
///
/// Each item in `outputs` pairs a detector name with its result.
///
/// # Errors
///
/// - `detector-failure`: a detector returned `Err`. The error payload
///   includes the detector name and reason.
/// - `detector-id-collision`: two detectors emitted the same surface
///   `id`. The error payload includes the colliding id and both
///   detector names.
pub fn merge_detector_outputs(
    outputs: impl IntoIterator<Item = (&'static str, Result<DetectorOutput, DetectorError>)>,
) -> Result<Vec<Surface>, Error> {
    let mut all_surfaces: Vec<(&'static str, Surface)> = Vec::new();

    for (detector_name, result) in outputs {
        let output = result.map_err(|err| Error::Diag {
            code: "detector-failure",
            detail: format!("detector '{detector_name}': {err}"),
        })?;
        for surface in output.surfaces {
            all_surfaces.push((detector_name, surface));
        }
    }

    let mut seen: HashMap<&str, &'static str> = HashMap::new();
    for (detector_name, surface) in &all_surfaces {
        if let Some(&first_detector) = seen.get(surface.id.as_str()) {
            if first_detector != *detector_name {
                return Err(Error::Diag {
                    code: "detector-id-collision",
                    detail: format!(
                        "surface id '{}' emitted by detectors: {first_detector}, {detector_name}",
                        surface.id
                    ),
                });
            }
        } else {
            seen.insert(&surface.id, detector_name);
        }
    }

    let mut surfaces: Vec<Surface> = all_surfaces.into_iter().map(|(_, s)| s).collect();
    surfaces.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(surfaces)
}
