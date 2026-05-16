//! Handler for `specify change survey` — mechanical source scanner.
//!
//! Enumerates externally observable surfaces in legacy source trees via
//! the detector registry, then writes `surfaces.json` + `metadata.json`
//! per source-key atomically.

use std::collections::BTreeSet;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::Serialize;
use specify_domain::survey::{
    DetectorInput, DetectorRegistry, Language, MetadataDocument, SourcesFile, SurfacesDocument,
    merge_detector_outputs, validate_metadata, validate_surfaces,
};
use specify_error::Error;

use crate::context::Ctx;

// ── Public entry point ──────────────────────────────────────────────

/// Resolved invocation form: either single-source or batch.
pub enum Form {
    /// `<source-path> --source-key <key> --out <dir>`.
    Single {
        /// Path to the legacy source root.
        source_path: PathBuf,
        /// Kebab-case source key.
        source_key: String,
        /// Output directory.
        out: PathBuf,
    },
    /// `--sources <file> --out <dir>`.
    Batch {
        /// YAML file listing one row per source.
        sources_file: PathBuf,
        /// Output parent directory.
        out: PathBuf,
    },
}

/// Run the survey verb after clap has resolved the raw arguments.
pub fn run(ctx: &Ctx, form: Form) -> Result<(), Error> {
    let (rows, out, batch_mode) = match form {
        Form::Single {
            source_path,
            source_key,
            out,
        } => (vec![(source_key, source_path)], out, false),
        Form::Batch { sources_file, out } => {
            let file = SourcesFile::load(&sources_file)?;
            let rows: Vec<(String, PathBuf)> =
                file.sources.into_iter().map(|r| (r.key, r.path)).collect();
            (rows, out, true)
        }
    };

    let registry = DetectorRegistry::with_builtins();
    let outcomes = run_rows(&rows, &out, batch_mode, &registry)?;
    emit_summary(ctx, &outcomes)
}

// ── Per-row engine ──────────────────────────────────────────────────

/// Outcome of a single row's survey pass.
#[derive(Debug)]
struct RowOutcome {
    key: String,
    surface_count: usize,
    error: Option<RowError>,
}

#[derive(Debug)]
struct RowError {
    code: &'static str,
    detail: String,
}

fn run_rows(
    rows: &[(String, PathBuf)], out: &Path, batch_mode: bool, registry: &DetectorRegistry,
) -> Result<Vec<RowOutcome>, Error> {
    let mut outcomes: Vec<RowOutcome> = Vec::with_capacity(rows.len());

    for (key, source_path) in rows {
        match run_single_row(key, source_path, out, batch_mode, registry) {
            Ok(surface_count) => outcomes.push(RowOutcome {
                key: key.clone(),
                surface_count,
                error: None,
            }),
            Err(err) => {
                if batch_mode {
                    outcomes.push(RowOutcome {
                        key: key.clone(),
                        surface_count: 0,
                        error: Some(RowError {
                            code: extract_code(&err),
                            detail: err.to_string(),
                        }),
                    });
                } else {
                    return Err(err);
                }
            }
        }
    }

    let failures: Vec<&RowOutcome> = outcomes.iter().filter(|o| o.error.is_some()).collect();
    if !failures.is_empty() {
        let mut parts: Vec<String> = failures
            .iter()
            .map(|f| {
                let e = f.error.as_ref().expect("filtered for Some");
                format!("{}: {} ({})", f.key, e.detail, e.code)
            })
            .collect();
        parts.sort();
        return Err(Error::Diag {
            code: failures[0].error.as_ref().expect("filtered for Some").code,
            detail: parts.join("; "),
        });
    }

    Ok(outcomes)
}

fn run_single_row(
    key: &str, source_path: &Path, out: &Path, batch_mode: bool, registry: &DetectorRegistry,
) -> Result<usize, Error> {
    if !source_path.exists() {
        return Err(Error::Argument {
            flag: "<source-path>",
            detail: format!("source path does not exist: {}", source_path.display()),
        });
    }

    if source_path.read_dir().is_err() {
        return Err(Error::Diag {
            code: "source-path-not-readable",
            detail: format!("source path is not readable: {}", source_path.display()),
        });
    }

    let row_dir = if batch_mode { out.join(key) } else { out.to_path_buf() };
    let surfaces_path = row_dir.join("surfaces.json");
    let metadata_path = row_dir.join("metadata.json");

    // Check for source-key mismatch before running detectors so a stale
    // file from a prior key is caught even when no detectors apply.
    guard_source_key_mismatch(&surfaces_path, key)?;

    let language_hint = detect_language(source_path);
    let language_str = language_hint.map_or_else(|| "unknown".to_string(), |l| l.to_string());

    let input = DetectorInput {
        source_root: source_path,
        language_hint,
    };

    let detector_results: Vec<(&'static str, Result<_, _>)> =
        registry.iter().map(|d| (d.name(), d.detect(&input))).collect();

    let surfaces = merge_detector_outputs(detector_results)?;

    // If the merged surface list is empty and no detector found anything
    // applicable, fail with `no-detectors`. This covers both the empty
    // registry case and the case where all detectors returned empty.
    if surfaces.is_empty() {
        return Err(Error::Diag {
            code: "no-detectors",
            detail: format!("no detector produced surfaces for source `{key}`"),
        });
    }

    let surfaces_doc = SurfacesDocument {
        version: 1,
        source_key: key.to_string(),
        language: language_str.clone(),
        surfaces,
    };
    validate_surfaces(&surfaces_doc)?;

    let metadata = compute_metadata(key, source_path, &language_str);
    validate_metadata(&metadata)?;

    let surface_count = surfaces_doc.surfaces.len();
    atomic_json_write(&surfaces_path, &surfaces_doc)?;
    atomic_json_write(&metadata_path, &metadata)?;

    Ok(surface_count)
}

// ── Language detection heuristic ────────────────────────────────────

fn detect_language(source_root: &Path) -> Option<Language> {
    if source_root.join("Cargo.toml").exists() {
        return Some(Language::Rust);
    }
    if source_root.join("go.mod").exists() {
        return Some(Language::Go);
    }
    if source_root.join("pyproject.toml").exists() {
        return Some(Language::Python);
    }
    if source_root.join("package.json").exists() {
        if source_root.join("tsconfig.json").exists() {
            return Some(Language::TypeScript);
        }
        return Some(Language::JavaScript);
    }
    None
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

fn compute_metadata(key: &str, source_root: &Path, language: &str) -> MetadataDocument {
    let mut loc: u64 = 0;
    let mut top_level: BTreeSet<String> = BTreeSet::new();

    if let Ok(entries) = std::fs::read_dir(source_root) {
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
    let Ok(entries) = std::fs::read_dir(dir) else {
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
    let Ok(content) = std::fs::read_to_string(path) else {
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

// ── Source-key mismatch guard ──────────────────────────────────────

fn guard_source_key_mismatch(surfaces_path: &Path, expected_key: &str) -> Result<(), Error> {
    if !surfaces_path.exists() {
        return Ok(());
    }
    let content = std::fs::read_to_string(surfaces_path).map_err(Error::Io)?;
    let value: serde_json::Value = serde_json::from_str(&content).map_err(|err| Error::Diag {
        code: "source-key-mismatch",
        detail: format!("existing surfaces.json is not valid JSON: {err}"),
    })?;
    if let Some(existing_key) = value.get("source-key").and_then(|v| v.as_str())
        && existing_key != expected_key
    {
        return Err(Error::Diag {
            code: "source-key-mismatch",
            detail: format!(
                "existing surfaces.json has source-key `{existing_key}`, \
                 refusing to overwrite with `{expected_key}`"
            ),
        });
    }
    Ok(())
}

// ── Atomic JSON writer ─────────────────────────────────────────────

fn atomic_json_write<T: Serialize>(path: &Path, value: &T) -> Result<(), Error> {
    let mut json = serde_json::to_string_pretty(value).map_err(|err| Error::Diag {
        code: "json-serialize-failed",
        detail: format!("failed to serialize survey JSON: {err}"),
    })?;
    json.push('\n');
    specify_domain::slice::atomic::bytes_write(path, json.as_bytes())
}

// ── Summary output ─────────────────────────────────────────────────

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct SurveyBody {
    rows: Vec<SurveyRowBody>,
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct SurveyRowBody {
    source_key: String,
    surfaces: usize,
    surfaces_path: PathBuf,
    metadata_path: PathBuf,
}

fn write_text(w: &mut dyn Write, body: &SurveyBody) -> std::io::Result<()> {
    for row in &body.rows {
        writeln!(
            w,
            "wrote {} ({} surface{})",
            row.surfaces_path.display(),
            row.surfaces,
            if row.surfaces == 1 { "" } else { "s" },
        )?;
    }
    Ok(())
}

fn emit_summary(ctx: &Ctx, outcomes: &[RowOutcome]) -> Result<(), Error> {
    let rows: Vec<SurveyRowBody> = outcomes
        .iter()
        .map(|o| SurveyRowBody {
            source_key: o.key.clone(),
            surfaces: o.surface_count,
            surfaces_path: PathBuf::from(format!("{}/surfaces.json", o.key)),
            metadata_path: PathBuf::from(format!("{}/metadata.json", o.key)),
        })
        .collect();
    let body = SurveyBody { rows };
    ctx.write(&body, write_text)?;
    Ok(())
}

// ── Error code extraction ──────────────────────────────────────────

fn extract_code(err: &Error) -> &'static str {
    match err {
        Error::Argument {
            flag: "<source-path>",
            ..
        } => "source-path-missing",
        Error::Argument {
            flag: "--sources",
            detail,
        } if detail.starts_with("sources file not found") => "sources-file-missing",
        Error::Argument {
            flag: "--sources", ..
        } => "sources-file-malformed",
        Error::Argument { .. } => "argument",
        Error::Diag { code, .. } => code,
        Error::Validation { .. } => "validation",
        _ => "io",
    }
}
