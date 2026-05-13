//! YAML delta merge for composition.yaml — screen-level operations
//! (added/modified/removed) applied to a baseline `screens` map.

use serde_json::Value;
use specify_error::Error;

use crate::merge::merge::{MergeOperation, MergeResult};

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
pub fn merge(baseline: Option<&str>, delta_text: &str) -> Result<MergeResult, Error> {
    let delta_doc: Value = serde_saphyr::from_str(delta_text).map_err(|e| Error::Diag {
        code: "composition-delta-malformed",
        detail: format!("failed to parse composition delta: {e}"),
    })?;

    let has_screens = delta_doc.get("screens").is_some();
    let has_delta = delta_doc.get("delta").is_some();

    if has_screens && !has_delta {
        let screen_count =
            delta_doc.get("screens").and_then(|s| s.as_object()).map_or(0, serde_json::Map::len);
        return Ok(MergeResult {
            output: delta_text.to_string(),
            operations: vec![MergeOperation::CreatedBaseline { requirement_count: screen_count }],
        });
    }

    if !has_delta {
        return Err(Error::Diag {
            code: "composition-delta-empty",
            detail: "composition delta has neither `screens` nor `delta`".to_string(),
        });
    }

    let delta = delta_doc.get("delta").and_then(|d| d.as_object()).ok_or_else(|| Error::Diag {
        code: "composition-delta-not-mapping",
        detail: "`delta` is not a mapping".to_string(),
    })?;

    let baseline_text = baseline.unwrap_or("");
    let mut baseline_doc: Value = if baseline_text.trim().is_empty() {
        serde_saphyr::from_str("version: 1\nscreens: {}").unwrap()
    } else {
        serde_saphyr::from_str(baseline_text).map_err(|e| Error::Diag {
            code: "composition-baseline-malformed",
            detail: format!("failed to parse composition baseline: {e}"),
        })?
    };

    let screens = baseline_doc
        .as_object_mut()
        .and_then(|m| m.get_mut("screens"))
        .and_then(|s| s.as_object_mut())
        .ok_or_else(|| Error::Diag {
            code: "composition-baseline-no-screens",
            detail: "baseline has no `screens` mapping".to_string(),
        })?;

    let mut operations: Vec<MergeOperation> = Vec::new();
    let mut errors: Vec<String> = Vec::new();

    if let Some(removed) = delta.get("removed").and_then(|r| r.as_object()) {
        for (slug, _) in removed {
            screens.remove(slug.as_str());
            operations.push(MergeOperation::Removed { id: slug.clone(), name: slug.clone() });
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
            operations.push(MergeOperation::Added { id: slug.clone(), name: slug.clone() });
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
            operations.push(MergeOperation::Modified { id: slug.clone(), name: slug.clone() });
        }
    }

    if !errors.is_empty() {
        return Err(Error::Diag {
            code: "composition-screen-conflict",
            detail: errors.join("\n"),
        });
    }

    let output = serde_saphyr::to_string(&baseline_doc).map_err(|e| Error::Diag {
        code: "composition-serialize-failed",
        detail: format!("failed to serialize merged composition: {e}"),
    })?;

    Ok(MergeResult { output, operations })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn screens_creates_baseline() {
        let delta =
            "version: 1\nscreens:\n  home:\n    title: Home\n  settings:\n    title: Settings\n";
        let result = merge(None, delta).unwrap();
        assert_eq!(result.output, delta);
        assert_eq!(result.operations, vec![MergeOperation::CreatedBaseline { requirement_count: 2 }]);
    }

    #[test]
    fn delta_adds_screen() {
        let baseline = "version: 1\nscreens:\n  home:\n    title: Home\n";
        let delta = "delta:\n  added:\n    settings:\n      title: Settings\n";
        let result = merge(Some(baseline), delta).unwrap();
        assert!(result.output.contains("settings"));
        assert!(result.output.contains("home"));
        assert_eq!(
            result.operations,
            vec![MergeOperation::Added {
                id: "settings".to_string(),
                name: "settings".to_string(),
            }]
        );
    }

    #[test]
    fn delta_modifies_screen() {
        let baseline = "version: 1\nscreens:\n  home:\n    title: Home\n";
        let delta = "delta:\n  modified:\n    home:\n      title: Home v2\n";
        let result = merge(Some(baseline), delta).unwrap();
        assert!(result.output.contains("Home v2"));
        assert_eq!(
            result.operations,
            vec![MergeOperation::Modified {
                id: "home".to_string(),
                name: "home".to_string(),
            }]
        );
    }

    #[test]
    fn delta_removes_screen() {
        let baseline =
            "version: 1\nscreens:\n  home:\n    title: Home\n  settings:\n    title: Settings\n";
        let delta = "delta:\n  removed:\n    settings:\n      reason: deprecated\n";
        let result = merge(Some(baseline), delta).unwrap();
        assert!(!result.output.contains("settings"));
        assert!(result.output.contains("home"));
        assert_eq!(
            result.operations,
            vec![MergeOperation::Removed {
                id: "settings".to_string(),
                name: "settings".to_string(),
            }]
        );
    }

    #[test]
    fn duplicate_add_errors() {
        let baseline = "version: 1\nscreens:\n  home:\n    title: Home\n";
        let delta = "delta:\n  added:\n    home:\n      title: Another Home\n";
        let err = merge(Some(baseline), delta).unwrap_err();
        match err {
            Error::Diag { code, detail } => {
                assert_eq!(code, "composition-screen-conflict");
                assert!(detail.contains("already exists"));
            }
            other => panic!("expected composition-screen-conflict diag, got {other:?}"),
        }
    }

    #[test]
    fn missing_screen_errors() {
        let baseline = "version: 1\nscreens:\n  home:\n    title: Home\n";
        let delta = "delta:\n  modified:\n    ghost:\n      title: Ghost\n";
        let err = merge(Some(baseline), delta).unwrap_err();
        match err {
            Error::Diag { code, detail } => {
                assert_eq!(code, "composition-screen-conflict");
                assert!(detail.contains("not found"));
            }
            other => panic!("expected composition-screen-conflict diag, got {other:?}"),
        }
    }

    #[test]
    fn missing_screens_and_delta_errors() {
        let delta = "version: 1\nfoo: bar\n";
        let err = merge(None, delta).unwrap_err();
        match err {
            Error::Diag { code, detail } => {
                assert_eq!(code, "composition-delta-empty");
                assert!(detail.contains("neither"));
            }
            other => panic!("expected composition-delta-empty diag, got {other:?}"),
        }
    }

    #[test]
    fn delta_on_empty_baseline() {
        let delta = "delta:\n  added:\n    home:\n      title: Home\n";
        let result = merge(None, delta).unwrap();
        assert!(result.output.contains("home"));
        assert_eq!(
            result.operations,
            vec![MergeOperation::Added {
                id: "home".to_string(),
                name: "home".to_string(),
            }]
        );
    }
}
