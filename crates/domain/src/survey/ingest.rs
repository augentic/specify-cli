//! Per-row ingest pipeline for `specify change survey`.
//!
//! Replaces the retired "run detectors → write" loop with the agent
//! producer model from RFC-20: ingest a staged candidate
//! `surfaces.json`, validate against schema + invariants, verify paths
//! resolve under the source root on disk, canonicalize, and capture
//! coarse metadata. The CLI handler wraps this with file I/O and
//! atomic writes; everything that is deterministic (and therefore
//! integration-testable) lives here.
//!
//! The exit-discriminant set is the public surface; each branch maps
//! to one [`Error::Diag`] code:
//!
//! - `staged-input-missing`, `staged-input-malformed`
//! - `surfaces-validation-failed`, `surfaces-id-collision`,
//!   `surfaces-touches-out-of-tree`
//! - `source-path-missing`, `source-path-not-readable`,
//!   `source-key-mismatch`

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use specify_error::Error;

use super::dto::{MetadataDocument, SurfacesDocument};
use super::validate::{RULE_TOUCHES_OUT_OF_TREE, strip_line_suffix, validate_surfaces};

const SURFACES_SCHEMA: &str = include_str!("../../../../schemas/surfaces.schema.json");

/// Inputs to a single ingest pass.
#[derive(Debug)]
pub struct IngestInputs<'a> {
    /// Kebab-case identifier the operator is asking the verb to ingest.
    pub source_key: &'a str,
    /// Legacy source-root path on disk.
    pub source_path: &'a Path,
    /// Path to the staged candidate `surfaces.json`.
    pub staged_path: &'a Path,
    /// Skip metadata capture when `true` — useful for the skill's
    /// repair loop. Validation, schema check, on-disk path-under-root
    /// verification, and canonicalisation still run.
    pub validate_only: bool,
}

/// Output of a successful ingest pass.
#[derive(Debug)]
pub struct IngestOutcome {
    /// Canonicalised `surfaces.json` ready for atomic write.
    pub surfaces: SurfacesDocument,
    /// Coarse metadata captured from the source root. `None` when the
    /// caller passed `validate_only: true`.
    pub metadata: Option<MetadataDocument>,
}

/// Drive a single staged candidate through schema validation, semantic
/// invariants, on-disk path verification, canonicalisation, and
/// optional metadata capture. See module docs for the discriminant set.
///
/// # Errors
///
/// Returns `Error::Diag` keyed for the discriminant set documented at
/// the module level.
pub fn ingest(inputs: &IngestInputs<'_>) -> Result<IngestOutcome, Error> {
    let raw = read_staged(inputs.staged_path)?;
    let instance = parse_staged(&raw)?;
    schema_validate(&instance)?;

    let mut doc: SurfacesDocument =
        serde_json::from_value(instance).map_err(|err| Error::Diag {
            code: "surfaces-validation-failed",
            detail: format!("staged candidate failed to deserialize: {err}"),
        })?;

    canonicalise(&mut doc);
    map_validate(validate_surfaces(&doc))?;

    if doc.source_key != inputs.source_key {
        return Err(Error::Diag {
            code: "source-key-mismatch",
            detail: format!(
                "staged candidate declares source-key `{}`, expected `{}`",
                doc.source_key, inputs.source_key
            ),
        });
    }

    let canonical_root = canonicalise_source(inputs.source_path)?;
    verify_paths_on_disk(&doc, &canonical_root)?;

    let metadata = if inputs.validate_only {
        None
    } else {
        Some(compute_metadata(inputs.source_key, inputs.source_path, &doc.language))
    };

    Ok(IngestOutcome {
        surfaces: doc,
        metadata,
    })
}

// ── Staged input loading ────────────────────────────────────────────

fn read_staged(path: &Path) -> Result<String, Error> {
    match fs::read_to_string(path) {
        Ok(s) => Ok(s),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Err(Error::Diag {
            code: "staged-input-missing",
            detail: format!("staged candidate not found: {}", path.display()),
        }),
        Err(err) => Err(Error::Diag {
            code: "staged-input-malformed",
            detail: format!("staged candidate not readable: {}: {err}", path.display()),
        }),
    }
}

fn parse_staged(raw: &str) -> Result<serde_json::Value, Error> {
    serde_json::from_str(raw).map_err(|err| Error::Diag {
        code: "staged-input-malformed",
        detail: format!("staged candidate is not valid JSON: {err}"),
    })
}

fn schema_validate(instance: &serde_json::Value) -> Result<(), Error> {
    let schema: serde_json::Value =
        serde_json::from_str(SURFACES_SCHEMA).expect("baked-in surfaces.schema.json is valid JSON");
    let validator =
        jsonschema::validator_for(&schema).expect("baked-in surfaces.schema.json must compile");

    let first =
        validator.iter_errors(instance).next().map(|e| format!("{}: {e}", e.instance_path()));
    if let Some(detail) = first {
        return Err(Error::Diag {
            code: "surfaces-validation-failed",
            detail: format!("schema mismatch — {detail}"),
        });
    }
    Ok(())
}

// ── validate_surfaces → Diag mapping ────────────────────────────────

fn map_validate(result: Result<(), Error>) -> Result<(), Error> {
    let err = match result {
        Ok(()) => return Ok(()),
        Err(err) => err,
    };
    let Error::Validation { results } = &err else {
        return Err(err);
    };
    let Some(first) = results.first() else {
        return Err(err);
    };
    let rule_id = first.rule_id.clone();
    let detail = first.detail.clone().unwrap_or_else(|| first.rule.clone());
    let code = match rule_id.as_str() {
        "surface-id-duplicate" => "surfaces-id-collision",
        RULE_TOUCHES_OUT_OF_TREE => RULE_TOUCHES_OUT_OF_TREE,
        _ => "surfaces-validation-failed",
    };
    Err(Error::Diag {
        code,
        detail: format!("{rule_id}: {detail}"),
    })
}

// ── Canonicalisation (sort) ─────────────────────────────────────────

fn canonicalise(doc: &mut SurfacesDocument) {
    doc.surfaces.sort_by(|a, b| a.id.cmp(&b.id));
    for s in &mut doc.surfaces {
        s.touches.sort();
        s.declared_at.sort();
    }
}

// ── Source root resolution + on-disk path-under-root ────────────────

fn canonicalise_source(source_path: &Path) -> Result<PathBuf, Error> {
    match fs::canonicalize(source_path) {
        Ok(p) => Ok(p),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Err(Error::Diag {
            code: "source-path-missing",
            detail: format!("source path does not exist: {}", source_path.display()),
        }),
        Err(err) => Err(Error::Diag {
            code: "source-path-not-readable",
            detail: format!("source path is not readable: {}: {err}", source_path.display()),
        }),
    }
}

fn verify_paths_on_disk(doc: &SurfacesDocument, canonical_root: &Path) -> Result<(), Error> {
    for (i, s) in doc.surfaces.iter().enumerate() {
        for (j, p) in s.touches.iter().enumerate() {
            check_under_root(canonical_root, p, &format!("surfaces[{i}].touches[{j}]"), false)?;
        }
        for (j, p) in s.declared_at.iter().enumerate() {
            check_under_root(canonical_root, p, &format!("surfaces[{i}].declared-at[{j}]"), true)?;
        }
    }
    Ok(())
}

fn check_under_root(
    root: &Path, entry: &str, field_path: &str, allow_line_suffix: bool,
) -> Result<(), Error> {
    let bare = if allow_line_suffix { strip_line_suffix(entry) } else { entry };
    let joined = root.join(bare);
    let canonical = fs::canonicalize(&joined).map_err(|err| Error::Diag {
        code: RULE_TOUCHES_OUT_OF_TREE,
        detail: format!("{field_path}: {entry} (not found under source root: {err})"),
    })?;
    if !canonical.starts_with(root) {
        return Err(Error::Diag {
            code: RULE_TOUCHES_OUT_OF_TREE,
            detail: format!("{field_path}: {entry} (resolves outside source root)"),
        });
    }
    Ok(())
}

// ── Coarse metadata ────────────────────────────────────────────────

/// Directories excluded from LOC counting, module listing, and
/// general traversal.
const SKIP_DIRS: &[&str] =
    &["node_modules", "vendor", "target", "dist", "build", ".git", "__pycache__", ".specify"];

const SKIP_TEST_DIRS: &[&str] = &["tests", "__tests__", "test", "__test__"];

fn is_test_filename(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.contains(".test.")
        || lower.contains(".spec.")
        || lower.contains("_test.")
        || lower.contains("_spec.")
}

fn is_comment_line(trimmed: &str) -> bool {
    trimmed.starts_with("//")
        || trimmed.starts_with('#')
        || trimmed.starts_with("/*")
        || trimmed.starts_with('*')
}

/// Compute coarse [`MetadataDocument`] facts (LOC, module count,
/// top-level modules) for a legacy source root.
#[must_use]
pub fn compute_metadata(key: &str, source_root: &Path, language: &str) -> MetadataDocument {
    let mut loc: u64 = 0;
    let mut top_level: BTreeSet<String> = BTreeSet::new();

    if let Ok(entries) = fs::read_dir(source_root) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if SKIP_DIRS.contains(&name.as_str()) || SKIP_TEST_DIRS.contains(&name.as_str()) {
                continue;
            }
            let Ok(ft) = entry.file_type() else {
                continue;
            };
            if ft.is_dir() {
                top_level.insert(name.clone());
                loc += count_loc_recursive(&entry.path());
            } else if ft.is_file() && !is_test_filename(&name) {
                top_level.insert(name);
                loc += count_loc_file(&entry.path());
            }
        }
    }

    let top_level_modules: Vec<String> = top_level.into_iter().collect();

    MetadataDocument {
        version: 1,
        source_key: key.to_string(),
        language: language.to_string(),
        loc,
        module_count: top_level_modules.len() as u64,
        top_level_modules,
    }
}

fn count_loc_recursive(dir: &Path) -> u64 {
    let mut total: u64 = 0;
    let Ok(entries) = fs::read_dir(dir) else {
        return 0;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if SKIP_DIRS.contains(&name.as_str()) || SKIP_TEST_DIRS.contains(&name.as_str()) {
            continue;
        }
        let Ok(ft) = entry.file_type() else {
            continue;
        };
        if ft.is_dir() {
            total += count_loc_recursive(&entry.path());
        } else if ft.is_file() && !is_test_filename(&name) {
            total += count_loc_file(&entry.path());
        }
    }
    total
}

fn count_loc_file(path: &Path) -> u64 {
    let Ok(content) = fs::read_to_string(path) else {
        return 0;
    };
    content
        .lines()
        .filter(|line| {
            let trimmed = line.trim();
            !trimmed.is_empty() && !is_comment_line(trimmed)
        })
        .count() as u64
}
