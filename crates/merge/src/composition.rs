//! YAML delta merge for composition.yaml — screen-level operations
//! (added/modified/removed) applied to a baseline `screens` map.

use serde_json::Value;
use specify_error::Error;

/// Result of a successful composition merge.
#[derive(Debug, Clone)]
pub struct MergeResult {
    /// The merged baseline YAML string.
    pub output: String,
    /// Operations applied during the merge.
    pub operations: Vec<MergeOp>,
}

/// One screen-level operation applied during a composition merge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MergeOp {
    /// A new screen was added.
    Added {
        /// Screen slug.
        slug: String,
    },
    /// An existing screen was replaced.
    Modified {
        /// Screen slug.
        slug: String,
    },
    /// A screen was removed from the baseline.
    Removed {
        /// Screen slug.
        slug: String,
    },
    /// Baseline created from a full `screens` document.
    CreatedBaseline {
        /// Number of screens in the new baseline.
        screen_count: usize,
    },
}

/// Merge a composition delta into an optional baseline.
///
/// `baseline` is the existing `composition.yaml` with a `screens` map (or None for new).
/// `delta_text` is the per-change `composition.yaml` — may have `screens` (new baseline)
/// or `delta` (screen-level operations).
///
/// # Panics
///
/// Panics if the hardcoded fallback YAML literal fails to parse (should
/// never happen).
///
/// # Errors
///
/// Returns an error if the operation fails.
pub fn merge_composition(baseline: Option<&str>, delta_text: &str) -> Result<MergeResult, Error> {
    let delta_doc: Value = serde_saphyr::from_str(delta_text)
        .map_err(|e| Error::Merge(format!("failed to parse composition delta: {e}")))?;

    let has_screens = delta_doc.get("screens").is_some();
    let has_delta = delta_doc.get("delta").is_some();

    if has_screens && !has_delta {
        let screen_count =
            delta_doc.get("screens").and_then(|s| s.as_object()).map_or(0, serde_json::Map::len);
        return Ok(MergeResult {
            output: delta_text.to_string(),
            operations: vec![MergeOp::CreatedBaseline { screen_count }],
        });
    }

    if !has_delta {
        return Err(Error::Merge(
            "composition delta has neither `screens` nor `delta`".to_string(),
        ));
    }

    let delta = delta_doc
        .get("delta")
        .and_then(|d| d.as_object())
        .ok_or_else(|| Error::Merge("`delta` is not a mapping".to_string()))?;

    let baseline_text = baseline.unwrap_or("");
    let mut baseline_doc: Value = if baseline_text.trim().is_empty() {
        serde_saphyr::from_str("version: 1\nscreens: {}").unwrap()
    } else {
        serde_saphyr::from_str(baseline_text)
            .map_err(|e| Error::Merge(format!("failed to parse composition baseline: {e}")))?
    };

    let screens = baseline_doc
        .as_object_mut()
        .and_then(|m| m.get_mut("screens"))
        .and_then(|s| s.as_object_mut())
        .ok_or_else(|| Error::Merge("baseline has no `screens` mapping".to_string()))?;

    let mut operations: Vec<MergeOp> = Vec::new();
    let mut errors: Vec<String> = Vec::new();

    if let Some(removed) = delta.get("removed").and_then(|r| r.as_object()) {
        for (slug, _) in removed {
            screens.remove(slug.as_str());
            operations.push(MergeOp::Removed { slug: slug.clone() });
        }
    }

    if let Some(added) = delta.get("added").and_then(|a| a.as_object()) {
        for (slug, screen_entry) in added {
            if screens.contains_key(slug.as_str()) {
                errors.push(format!(
                    "screen `{slug}` already exists in baseline; use `modified` to update it"
                ));
                continue;
            }
            screens.insert(slug.clone(), screen_entry.clone());
            operations.push(MergeOp::Added { slug: slug.clone() });
        }
    }

    if let Some(modified) = delta.get("modified").and_then(|m| m.as_object()) {
        for (slug, screen_entry) in modified {
            if !screens.contains_key(slug.as_str()) {
                errors.push(format!(
                    "screen `{slug}` not found in baseline; use `added` for new screens"
                ));
                continue;
            }
            screens.insert(slug.clone(), screen_entry.clone());
            operations.push(MergeOp::Modified { slug: slug.clone() });
        }
    }

    if !errors.is_empty() {
        return Err(Error::Merge(errors.join("\n")));
    }

    let output = serde_saphyr::to_string(&baseline_doc)
        .map_err(|e| Error::Merge(format!("failed to serialize merged composition: {e}")))?;

    Ok(MergeResult { output, operations })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn screens_creates_baseline() {
        let delta =
            "version: 1\nscreens:\n  home:\n    title: Home\n  settings:\n    title: Settings\n";
        let result = merge_composition(None, delta).unwrap();
        assert_eq!(result.output, delta);
        assert_eq!(result.operations, vec![MergeOp::CreatedBaseline { screen_count: 2 }]);
    }

    #[test]
    fn delta_adds_screen() {
        let baseline = "version: 1\nscreens:\n  home:\n    title: Home\n";
        let delta = "delta:\n  added:\n    settings:\n      title: Settings\n";
        let result = merge_composition(Some(baseline), delta).unwrap();
        assert!(result.output.contains("settings"));
        assert!(result.output.contains("home"));
        assert_eq!(
            result.operations,
            vec![MergeOp::Added {
                slug: "settings".to_string()
            }]
        );
    }

    #[test]
    fn delta_modifies_screen() {
        let baseline = "version: 1\nscreens:\n  home:\n    title: Home\n";
        let delta = "delta:\n  modified:\n    home:\n      title: Home v2\n";
        let result = merge_composition(Some(baseline), delta).unwrap();
        assert!(result.output.contains("Home v2"));
        assert_eq!(
            result.operations,
            vec![MergeOp::Modified {
                slug: "home".to_string()
            }]
        );
    }

    #[test]
    fn delta_removes_screen() {
        let baseline =
            "version: 1\nscreens:\n  home:\n    title: Home\n  settings:\n    title: Settings\n";
        let delta = "delta:\n  removed:\n    settings:\n      reason: deprecated\n";
        let result = merge_composition(Some(baseline), delta).unwrap();
        assert!(!result.output.contains("settings"));
        assert!(result.output.contains("home"));
        assert_eq!(
            result.operations,
            vec![MergeOp::Removed {
                slug: "settings".to_string()
            }]
        );
    }

    #[test]
    fn duplicate_add_errors() {
        let baseline = "version: 1\nscreens:\n  home:\n    title: Home\n";
        let delta = "delta:\n  added:\n    home:\n      title: Another Home\n";
        let err = merge_composition(Some(baseline), delta).unwrap_err();
        match err {
            Error::Merge(msg) => assert!(msg.contains("already exists")),
            other => panic!("expected Error::Merge, got {other:?}"),
        }
    }

    #[test]
    fn missing_screen_errors() {
        let baseline = "version: 1\nscreens:\n  home:\n    title: Home\n";
        let delta = "delta:\n  modified:\n    ghost:\n      title: Ghost\n";
        let err = merge_composition(Some(baseline), delta).unwrap_err();
        match err {
            Error::Merge(msg) => assert!(msg.contains("not found")),
            other => panic!("expected Error::Merge, got {other:?}"),
        }
    }

    #[test]
    fn missing_screens_and_delta_errors() {
        let delta = "version: 1\nfoo: bar\n";
        let err = merge_composition(None, delta).unwrap_err();
        match err {
            Error::Merge(msg) => assert!(msg.contains("neither")),
            other => panic!("expected Error::Merge, got {other:?}"),
        }
    }

    #[test]
    fn delta_on_empty_baseline() {
        let delta = "delta:\n  added:\n    home:\n      title: Home\n";
        let result = merge_composition(None, delta).unwrap();
        assert!(result.output.contains("home"));
        assert_eq!(
            result.operations,
            vec![MergeOp::Added {
                slug: "home".to_string()
            }]
        );
    }
}
