//! Handler for `specify change survey` — staged-candidate ingest.
//!
//! Validates a staged candidate `surfaces.json`, canonicalises it,
//! captures coarse source metadata, and writes the canonical sidecars
//! per source-key atomically. JSON-only; no LLM. The deterministic
//! pipeline lives in [`specify_domain::survey::ingest`]; this module
//! is the thin shell around it that loads inputs and writes outputs.

use std::io::Write;
use std::path::{Path, PathBuf};

use serde::Serialize;
use specify_domain::survey::{IngestInputs, SourcesFile, ingest};
use specify_error::Error;

use crate::context::Ctx;

// ── Public entry point ──────────────────────────────────────────────

struct Row {
    key: String,
    source: PathBuf,
    staged: PathBuf,
    out: PathBuf,
}

/// Run the survey verb after clap has resolved the raw arguments.
///
/// Accepts the raw clap fields directly and resolves them into either
/// single-source or batch shape; the shape mirrors the two mutually
/// exclusive argument groups declared on `ChangeAction::Survey`.
#[expect(
    clippy::too_many_arguments,
    reason = "mirrors the clap-declared ChangeAction::Survey fields one-to-one"
)]
pub fn run(
    ctx: &Ctx, source_path: Option<PathBuf>, source_key: Option<String>, surfaces: Option<PathBuf>,
    sources: Option<PathBuf>, staged: Option<PathBuf>, out: PathBuf, validate_only: bool,
) -> Result<(), Error> {
    let (rows, batch_mode) = plan_rows(source_path, source_key, surfaces, sources, staged, out)?;
    let outcomes = run_rows(&rows, validate_only, batch_mode)?;
    emit_summary(ctx, &outcomes, validate_only)
}

fn plan_rows(
    source_path: Option<PathBuf>, source_key: Option<String>, surfaces: Option<PathBuf>,
    sources: Option<PathBuf>, staged: Option<PathBuf>, out: PathBuf,
) -> Result<(Vec<Row>, bool), Error> {
    match (source_path, source_key, surfaces, sources, staged) {
        (Some(source_path), Some(source_key), Some(surfaces), None, None) => Ok((
            vec![Row {
                key: source_key,
                source: source_path,
                staged: surfaces,
                out,
            }],
            false,
        )),
        (None, None, None, Some(sources_file), Some(staged)) => {
            let file = SourcesFile::load(&sources_file)?;
            let rows: Vec<Row> = file
                .sources
                .into_iter()
                .map(|r| Row {
                    out: out.join(&r.key),
                    staged: staged.join(format!("{}.json", r.key)),
                    source: r.path,
                    key: r.key,
                })
                .collect();
            Ok((rows, true))
        }
        _ => Err(Error::Argument {
            flag: "<source-path> / --sources",
            detail: "provide either <source-path> --source-key <key> --surfaces <file> or \
                     --sources <file> --staged <dir>, not both or neither"
                .to_string(),
        }),
    }
}

// ── Per-row engine ──────────────────────────────────────────────────

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

fn run_rows(rows: &[Row], validate_only: bool, batch_mode: bool) -> Result<Vec<RowOutcome>, Error> {
    let mut outcomes: Vec<RowOutcome> = Vec::with_capacity(rows.len());

    for row in rows {
        match run_single_row(row, validate_only) {
            Ok(surface_count) => outcomes.push(RowOutcome {
                key: row.key.clone(),
                surface_count,
                error: None,
            }),
            Err(err) => {
                if batch_mode {
                    outcomes.push(RowOutcome {
                        key: row.key.clone(),
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

fn run_single_row(row: &Row, validate_only: bool) -> Result<usize, Error> {
    let surfaces_path = row.out.join("surfaces.json");
    let metadata_path = row.out.join("metadata.json");

    guard_existing_source_key(&surfaces_path, &row.key)?;

    let outcome = ingest(&IngestInputs {
        source_key: &row.key,
        source_path: &row.source,
        staged_path: &row.staged,
        validate_only,
    })?;

    let surface_count = outcome.surfaces.surfaces.len();
    if !validate_only {
        atomic_json_write(&surfaces_path, &outcome.surfaces)?;
        let metadata =
            outcome.metadata.as_ref().expect("ingest returns Some(metadata) when !validate_only");
        atomic_json_write(&metadata_path, metadata)?;
    }

    Ok(surface_count)
}

// ── Source-key mismatch guard against existing canonical file ──────

fn guard_existing_source_key(surfaces_path: &Path, expected_key: &str) -> Result<(), Error> {
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
    validate_only: bool,
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
        if body.validate_only {
            writeln!(
                w,
                "validated {} ({} surface{})",
                row.source_key,
                row.surfaces,
                if row.surfaces == 1 { "" } else { "s" },
            )?;
        } else {
            writeln!(
                w,
                "wrote {} ({} surface{})",
                row.surfaces_path.display(),
                row.surfaces,
                if row.surfaces == 1 { "" } else { "s" },
            )?;
        }
    }
    Ok(())
}

fn emit_summary(ctx: &Ctx, outcomes: &[RowOutcome], validate_only: bool) -> Result<(), Error> {
    let rows: Vec<SurveyRowBody> = outcomes
        .iter()
        .map(|o| SurveyRowBody {
            source_key: o.key.clone(),
            surfaces: o.surface_count,
            surfaces_path: PathBuf::from(format!("{}/surfaces.json", o.key)),
            metadata_path: PathBuf::from(format!("{}/metadata.json", o.key)),
        })
        .collect();
    let body = SurveyBody { validate_only, rows };
    ctx.write(&body, write_text)?;
    Ok(())
}

// ── Error code extraction ──────────────────────────────────────────

const fn extract_code(err: &Error) -> &'static str {
    match err {
        Error::Diag { code, .. } => code,
        Error::Validation { .. } => "validation",
        _ => "io",
    }
}
