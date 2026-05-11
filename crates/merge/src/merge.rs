//! Pure in-memory delta merge — port of the Python `merge()` from
//! archived Python reference, `merge` lines 203–291.

use std::collections::{HashMap, HashSet};

use specify_error::Error;
use specify_spec::{REQ_HEADING, Requirement, has_delta_headers, parse_baseline, parse_delta};

/// Result of a successful [`merge`] call.
///
/// `output` is the merged baseline text (byte-for-byte parity with the
/// Python reference). `operations` records every change applied, in the
/// order `RENAMED → REMOVED → MODIFIED → ADDED` — the same order used
/// when mutating the underlying block list.
#[derive(Debug, Clone, PartialEq, Eq)]
#[must_use]
pub struct MergeResult {
    /// Merged baseline text.
    pub output: String,
    /// Ordered list of changes applied during the merge.
    pub operations: Vec<MergeOperation>,
}

/// One structured entry in [`MergeResult::operations`].
///
/// `CreatedBaseline` is the "no delta headers, baseline was empty" branch:
/// the delta text is kept verbatim as the new baseline and we just record
/// how many `### Requirement:` blocks it contains.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum MergeOperation {
    /// A requirement was renamed (ID preserved, heading changed).
    Renamed {
        /// Requirement ID.
        id: String,
        /// Previous name.
        old_name: String,
        /// New name.
        new_name: String,
    },
    /// A requirement was removed from the baseline.
    Removed {
        /// Requirement ID.
        id: String,
        /// Requirement name.
        name: String,
    },
    /// A requirement body was replaced.
    Modified {
        /// Requirement ID.
        id: String,
        /// Requirement name.
        name: String,
    },
    /// A new requirement was appended.
    Added {
        /// Requirement ID.
        id: String,
        /// Requirement name.
        name: String,
    },
    /// Baseline created from scratch (no delta headers present).
    CreatedBaseline {
        /// Number of `### Requirement:` blocks found in the verbatim text.
        requirement_count: usize,
    },
}

/// Merge a delta spec into an optional baseline.
///
/// `baseline == None` (or `Some("")`, or `Some(whitespace-only)`) means
/// "new capability": the baseline is being created from scratch. In that
/// case:
///   * if the delta has **no** delta-section headers (per
///     [`specify_spec::has_delta_headers`]), the delta text is returned
///     verbatim and `operations` holds a single
///     [`MergeOperation::CreatedBaseline`] entry whose `requirement_count`
///     counts the `### Requirement:` blocks found in the delta body;
///   * otherwise the `## ADDED Requirements` section is flattened into a
///     fresh baseline.
///
/// `baseline = Some(non-empty)` applies `RENAMED → REMOVED → MODIFIED →
/// ADDED` in that order. Any delta entry whose id cannot be resolved (or,
/// for ADDED, whose id collides with a surviving baseline id) becomes an
/// `Err(Error::Diag { code: "merge-spec-conflicts", .. })` with all
/// failure messages joined by `"\n"`.
///
/// # Errors
///
/// Returns an error if the operation fails.
#[expect(clippy::too_many_lines, reason = "Single-shot merge driver: heading walk + delta classification + conflict aggregation in one pass.")]
pub fn merge(baseline: Option<&str>, delta: &str) -> Result<MergeResult, Error> {
    let baseline_text = baseline.unwrap_or("");
    let is_new = baseline_text.trim().is_empty();

    let delta_spec = parse_delta(delta);

    if is_new {
        // `specify_spec::has_delta_headers` uses a full-line match rather
        // than Python's substring `.lower() in delta_text.lower()`. See
        // the spec-crate unit test `has_delta_headers_requires_full_line_match`
        // for the pinning decision.
        if !has_delta_headers(delta) {
            let requirement_count = count_requirement_headings(delta);
            return Ok(MergeResult {
                output: delta.to_string(),
                operations: vec![MergeOperation::CreatedBaseline { requirement_count }],
            });
        }

        let mut operations: Vec<MergeOperation> = Vec::new();
        let mut result_blocks: Vec<String> = Vec::new();
        for block in &delta_spec.added {
            result_blocks.push(block.body.clone());
            operations.push(MergeOperation::Added {
                id: block.id.clone(),
                name: block.name.clone(),
            });
        }
        let output = if result_blocks.is_empty() {
            String::new()
        } else {
            let mut joined = result_blocks.join("\n\n");
            joined.push('\n');
            joined
        };
        return Ok(MergeResult { output, operations });
    }

    // --- Existing-baseline path ---------------------------------------------

    let parsed_baseline = parse_baseline(baseline_text);
    let mut blocks: Vec<Requirement> = parsed_baseline.requirements;
    let preamble = parsed_baseline.preamble;

    // Map id → index into `blocks`. Empty ids are excluded so stray
    // "missing-id" blocks never match against delta lookups.
    let mut blocks_by_id: HashMap<String, usize> = HashMap::new();
    for (i, block) in blocks.iter().enumerate() {
        if !block.id.is_empty() {
            blocks_by_id.insert(block.id.clone(), i);
        }
    }

    let mut errors: Vec<String> = Vec::new();
    let mut operations: Vec<MergeOperation> = Vec::new();

    // Step 1 — RENAMED.
    for entry in &delta_spec.renamed {
        let Some(&idx) = blocks_by_id.get(&entry.id) else {
            errors.push(format!("RENAMED: ID {} not found in baseline", entry.id));
            continue;
        };
        let old_block = blocks[idx].clone();
        let new_heading = format!("{} {}", REQ_HEADING, entry.new_name);
        // Python `str.replace(old, new, 1)` = first-occurrence replace.
        let new_body = replace_first(&old_block.body, &old_block.heading, &new_heading);
        operations.push(MergeOperation::Renamed {
            id: old_block.id.clone(),
            old_name: old_block.name.clone(),
            new_name: entry.new_name.clone(),
        });
        blocks[idx] = Requirement {
            heading: new_heading,
            name: entry.new_name.clone(),
            id: old_block.id,
            body: new_body,
            // `specify_spec::parse_scenarios` only looks at body text so
            // we could recompute; keep the old scenarios since rename
            // doesn't touch scenario text.
            scenarios: old_block.scenarios,
        };
    }

    // Step 2 — REMOVED (collect ids; deletion happens at the end so
    // MODIFIED/ADDED still see a stable index map).
    let mut ids_to_remove: HashSet<String> = HashSet::new();
    for block in &delta_spec.removed {
        if blocks_by_id.contains_key(&block.id) {
            ids_to_remove.insert(block.id.clone());
            operations.push(MergeOperation::Removed {
                id: block.id.clone(),
                name: block.name.clone(),
            });
        } else {
            errors.push(format!("REMOVED: ID {} not found in baseline", block.id));
        }
    }

    // Step 3 — MODIFIED.
    for mod_block in &delta_spec.modified {
        let Some(&idx) = blocks_by_id.get(&mod_block.id) else {
            errors.push(format!("MODIFIED: ID {} not found in baseline", mod_block.id));
            continue;
        };
        operations.push(MergeOperation::Modified {
            id: mod_block.id.clone(),
            name: mod_block.name.clone(),
        });
        blocks[idx] = mod_block.clone();
    }

    // Step 4 — ADDED.
    let mut existing_ids: HashSet<String> =
        blocks_by_id.keys().filter(|id| !ids_to_remove.contains(*id)).cloned().collect();
    for add_block in &delta_spec.added {
        if !add_block.id.is_empty() && existing_ids.contains(&add_block.id) {
            errors.push(format!("ADDED: ID {} already exists in baseline", add_block.id));
            continue;
        }
        operations.push(MergeOperation::Added {
            id: add_block.id.clone(),
            name: add_block.name.clone(),
        });
        blocks.push(add_block.clone());
        if !add_block.id.is_empty() {
            existing_ids.insert(add_block.id.clone());
        }
    }

    if !errors.is_empty() {
        return Err(Error::Diag {
            code: "merge-spec-conflicts",
            detail: errors.join("\n"),
        });
    }

    // Assemble result: preamble (if non-empty) + surviving blocks' stripped bodies.
    let mut parts: Vec<String> = Vec::new();
    if !preamble.trim().is_empty() {
        parts.push(rstrip(&preamble).to_string());
    }
    for block in &blocks {
        if ids_to_remove.contains(&block.id) && !block.id.is_empty() {
            continue;
        }
        parts.push(block.body.trim().to_string());
    }
    let mut output = parts.join("\n\n");
    output.push('\n');

    Ok(MergeResult { output, operations })
}

fn count_requirement_headings(text: &str) -> usize {
    text.lines().filter(|line| line.trim_start().starts_with(REQ_HEADING)).count()
}

/// Python's `str.replace(old, new, 1)`: replace only the first occurrence.
/// If `needle` is empty we mirror Python by returning `haystack` unchanged.
fn replace_first(haystack: &str, needle: &str, replacement: &str) -> String {
    if needle.is_empty() {
        return haystack.to_string();
    }
    haystack.find(needle).map_or_else(
        || haystack.to_string(),
        |idx| {
            let mut out = String::with_capacity(haystack.len() + replacement.len());
            out.push_str(&haystack[..idx]);
            out.push_str(replacement);
            out.push_str(&haystack[idx + needle.len()..]);
            out
        },
    )
}

fn rstrip(s: &str) -> &str {
    s.trim_end_matches([' ', '\t', '\n', '\r', '\x0b', '\x0c'])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn greenfield_no_headers_verbatim() {
        let delta = "# Greenfield spec\n\nJust prose.\n";
        let result = merge(None, delta).expect("merge ok");
        assert_eq!(result.output, delta);
        assert_eq!(
            result.operations,
            vec![MergeOperation::CreatedBaseline { requirement_count: 0 }]
        );
    }

    #[test]
    fn greenfield_counts_blocks() {
        let delta =
            "# Spec\n\n### Requirement: A\n\nID: REQ-001\n\n### Requirement: B\n\nID: REQ-002\n";
        let result = merge(None, delta).expect("merge ok");
        assert_eq!(result.output, delta);
        assert!(matches!(
            result.operations.as_slice(),
            [MergeOperation::CreatedBaseline { requirement_count: 2 }]
        ));
    }

    #[test]
    fn modified_unknown_id_errors() {
        let baseline = "# Base\n\n### Requirement: Alpha\n\nID: REQ-001\n\n#### Scenario: ok\n\n- ok\n\n### Requirement: Beta\n\nID: REQ-002\n\n#### Scenario: ok\n\n- ok\n";
        let delta = "# delta\n\n## MODIFIED Requirements\n\n### Requirement: Ghost\n\nID: REQ-999\n\n#### Scenario: none\n\n- nothing\n";
        let err = merge(Some(baseline), delta).expect_err("expected merge failure");
        match err {
            Error::Diag { code, detail } => {
                assert_eq!(code, "merge-spec-conflicts");
                assert!(
                    detail.contains("MODIFIED: ID REQ-999 not found in baseline"),
                    "unexpected merge error: {detail}"
                );
            }
            other => panic!("expected merge-spec-conflicts diag, got {other:?}"),
        }
    }

    #[test]
    fn added_id_collision_errors() {
        let baseline = "### Requirement: A\n\nID: REQ-001\n\n#### Scenario: ok\n\n- ok\n";
        let delta = "## ADDED Requirements\n\n### Requirement: Another A\n\nID: REQ-001\n\n#### Scenario: ok\n\n- ok\n";
        let err = merge(Some(baseline), delta).expect_err("expected merge failure");
        match err {
            Error::Diag { code, detail } => {
                assert_eq!(code, "merge-spec-conflicts");
                assert!(
                    detail.contains("ADDED: ID REQ-001 already exists in baseline"),
                    "unexpected error: {detail}"
                );
            }
            other => panic!("expected merge-spec-conflicts diag, got {other:?}"),
        }
    }

    #[test]
    fn rename_records_op() {
        let baseline =
            "# B\n\n### Requirement: Old name\n\nID: REQ-001\n\n#### Scenario: ok\n\n- ok\n";
        let delta = "## RENAMED Requirements\n\nID: REQ-001\nTO: Shiny new name\n";
        let result = merge(Some(baseline), delta).expect("merge ok");
        assert!(result.output.contains("### Requirement: Shiny new name"));
        assert!(!result.output.contains("Old name"));
        assert_eq!(
            result.operations,
            vec![MergeOperation::Renamed {
                id: "REQ-001".to_string(),
                old_name: "Old name".to_string(),
                new_name: "Shiny new name".to_string(),
            }]
        );
    }

    #[test]
    fn replace_first_once() {
        assert_eq!(replace_first("abab", "ab", "XY"), "XYab");
        assert_eq!(replace_first("abc", "z", "Q"), "abc");
        assert_eq!(replace_first("abc", "", "Q"), "abc");
    }
}
