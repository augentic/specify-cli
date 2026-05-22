//! Handlers for `specify source cache {lookup, write}` and the
//! `source resolve --explain` fingerprint-chain reader (RFC-27 §D8).

#![allow(
    clippy::needless_pass_by_value,
    reason = "Clap hands owned subcommand-arg structs to handlers in this module."
)]

use std::io::Write;
use std::path::{Path, PathBuf};

use jiff::Timestamp;
use serde::Serialize;
use specify_domain::adapter::{
    Adapter, Axis, CacheFingerprint, CacheIndexEntry, CacheLayout, CacheLookup, CacheMissReason,
    CacheMode, FingerprintSource, FingerprintToolVersion, LookupOutcome, ResolvedAdapter,
    SourceOperation, cache_lookup, cache_write, sha256_file,
};
use specify_domain::config::Layout;
use specify_domain::{adapter as adapter_mod, journal};
use specify_error::{Error, Result};

use crate::cli::Format;
use crate::commands::source::cli::CacheFingerprintArgs;
use crate::output;

/// Body emitted for `specify source cache lookup`.
#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct LookupBody {
    adapter: String,
    slice: String,
    source_key: String,
    operation: SourceOperation,
    fingerprint: String,
    cache_dir: PathBuf,
    status: LookupStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<CacheMissReason>,
}

#[derive(Serialize, Copy, Clone)]
#[serde(rename_all = "kebab-case")]
enum LookupStatus {
    Hit,
    Miss,
}

/// Body emitted for `specify source cache write`.
#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct WriteBody {
    adapter: String,
    slice: String,
    source_key: String,
    operation: SourceOperation,
    fingerprint: String,
    cache_dir: PathBuf,
    /// `true` when the adapter declared `cache: opt-out`; `false`
    /// when the body was persisted to `<fp>/<artifact>` and
    /// `<fp>/fingerprint.json`.
    opted_out: bool,
}

/// One row emitted for `specify source resolve --explain`.
#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct ExplainRow {
    #[serde(with = "specify_error::serde_rfc3339")]
    timestamp: Timestamp,
    fingerprint: String,
    slice: String,
    source_key: String,
    operation: SourceOperation,
}

/// Envelope returned by `--explain`.
#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
pub(super) struct ExplainBody {
    adapter: String,
    index_path: PathBuf,
    entries: Vec<ExplainRow>,
}

/// Dispatch for `specify source cache lookup`.
pub fn lookup(format: Format, args: CacheFingerprintArgs) -> Result<()> {
    let prepared = prepare(&args)?;
    let result = cache_lookup(
        prepared.layout,
        &prepared.fingerprint,
        prepared.cache_mode,
        &args.slice,
        &args.source_key,
        args.operation,
    )?;
    let body = render_lookup(&args, &result);
    let journal_layout = Layout::new(&args.project_dir);
    let event = journal_event_from_lookup(&args, &result, Timestamp::now());
    journal::append_batch(journal_layout, std::slice::from_ref(&event))?;
    output::emit(Box::new(std::io::stdout().lock()), format, &body, write_lookup_text)?;
    Ok(())
}

/// Dispatch for `specify source cache write`.
pub fn write(format: Format, args: CacheFingerprintArgs, payload: &Path) -> Result<()> {
    let prepared = prepare(&args)?;
    let bytes = std::fs::read(payload).map_err(|err| Error::Diag {
        code: "cache-payload-read-failed",
        detail: format!("failed to read cache payload {}: {err}", payload.display()),
    })?;
    let digest = prepared.fingerprint.digest();
    let entry = CacheIndexEntry {
        timestamp: Timestamp::now(),
        fingerprint: digest.clone(),
        slice: args.slice.clone(),
        source_key: args.source_key.clone(),
        adapter: args.adapter.clone(),
        operation: args.operation,
    };
    cache_write(
        prepared.layout,
        &prepared.fingerprint,
        &bytes,
        args.operation.artifact_name(),
        prepared.cache_mode,
        &entry,
    )?;
    let body = WriteBody {
        adapter: args.adapter.clone(),
        slice: args.slice.clone(),
        source_key: args.source_key.clone(),
        operation: args.operation,
        fingerprint: digest,
        cache_dir: prepared.layout.fingerprint_dir(&entry.fingerprint),
        opted_out: matches!(prepared.cache_mode, Some(CacheMode::OptOut)),
    };
    output::emit(Box::new(std::io::stdout().lock()), format, &body, write_write_text)?;
    Ok(())
}

/// Dispatch for `specify source resolve --explain`.
pub fn explain(format: Format, adapter: &str, project_dir: &Path) -> Result<()> {
    let layout = CacheLayout::new(project_dir, adapter);
    let entries = adapter_mod::cache_read_index(layout)?;
    let body = ExplainBody {
        adapter: adapter.to_string(),
        index_path: layout.index_path(),
        entries: entries
            .into_iter()
            .map(|e| ExplainRow {
                timestamp: e.timestamp,
                fingerprint: e.fingerprint,
                slice: e.slice,
                source_key: e.source_key,
                operation: e.operation,
            })
            .collect(),
    };
    output::emit(Box::new(std::io::stdout().lock()), format, &body, write_explain_text)?;
    Ok(())
}

/// Manifest + fingerprint material the lookup / write handlers
/// share. `layout` is the cache root for one adapter; `cache_mode`
/// is `Some(OptOut)` when the manifest declared opt-out.
struct Prepared<'a> {
    layout: CacheLayout<'a>,
    fingerprint: CacheFingerprint,
    cache_mode: Option<CacheMode>,
}

fn prepare(args: &CacheFingerprintArgs) -> Result<Prepared<'_>> {
    let resolved = Adapter::resolve(Axis::Source, &args.adapter, &args.project_dir)?;
    let brief_path = brief_for(&resolved, args.operation)?;
    let brief_sha256 = sha256_file(&brief_path)?;

    let source = source_from(args)?;
    let adapter_join = format!("{}@{}", resolved.manifest.name, resolved.manifest.version);
    let tool_versions: Vec<FingerprintToolVersion> = resolved
        .manifest
        .tools
        .iter()
        .map(|t| FingerprintToolVersion {
            name: t.name.clone(),
            version: t.version.clone(),
        })
        .collect();
    let candidate = match args.operation {
        SourceOperation::Extract => args.candidate.clone(),
        SourceOperation::Enumerate => None,
    };

    let fingerprint =
        CacheFingerprint::new(source, adapter_join, brief_sha256, tool_versions, candidate);
    Ok(Prepared {
        layout: CacheLayout::new(&args.project_dir, &args.adapter),
        fingerprint,
        cache_mode: resolved.manifest.cache,
    })
}

fn brief_for(resolved: &ResolvedAdapter, operation: SourceOperation) -> Result<PathBuf> {
    let key = operation.to_string();
    resolved.brief_path(&key).ok_or_else(|| Error::Diag {
        code: "cache-adapter-brief-missing",
        detail: format!(
            "adapter `{}` declares no brief for operation `{key}`",
            resolved.manifest.name
        ),
    })
}

fn source_from(args: &CacheFingerprintArgs) -> Result<FingerprintSource> {
    if let Some(path) = &args.source_path {
        return FingerprintSource::from_path(path);
    }
    if let Some(value) = &args.source_value {
        return Ok(FingerprintSource::from_value(value.as_bytes()));
    }
    Err(Error::Argument {
        flag: "--source-path",
        detail: "either --source-path or --source-value is required".to_string(),
    })
}

fn render_lookup(args: &CacheFingerprintArgs, result: &CacheLookup) -> LookupBody {
    let (status, reason) = match result.outcome {
        LookupOutcome::Hit { .. } => (LookupStatus::Hit, None),
        LookupOutcome::Miss { reason } => (LookupStatus::Miss, Some(reason)),
    };
    LookupBody {
        adapter: args.adapter.clone(),
        slice: args.slice.clone(),
        source_key: args.source_key.clone(),
        operation: args.operation,
        fingerprint: result.digest.clone(),
        cache_dir: result.cache_dir.clone(),
        status,
        reason,
    }
}

fn journal_event_from_lookup(
    args: &CacheFingerprintArgs, result: &CacheLookup, now: Timestamp,
) -> journal::Event {
    let kind = match result.outcome {
        LookupOutcome::Hit { .. } => journal::EventKind::SliceExtractCacheHit {
            slice_name: args.slice.clone(),
            source_key: args.source_key.clone(),
            adapter: args.adapter.clone(),
            fingerprint: result.digest.clone(),
        },
        LookupOutcome::Miss { reason } => journal::EventKind::SliceExtractCacheMiss {
            slice_name: args.slice.clone(),
            source_key: args.source_key.clone(),
            adapter: args.adapter.clone(),
            fingerprint: result.digest.clone(),
            reason,
        },
    };
    journal::Event::new(now, kind)
}

fn write_lookup_text(w: &mut dyn Write, body: &LookupBody) -> std::io::Result<()> {
    writeln!(w, "adapter: {}", body.adapter)?;
    writeln!(w, "slice: {}", body.slice)?;
    writeln!(w, "source-key: {}", body.source_key)?;
    writeln!(w, "operation: {}", body.operation)?;
    writeln!(w, "fingerprint: {}", body.fingerprint)?;
    writeln!(w, "cache-dir: {}", body.cache_dir.display())?;
    let status = match body.status {
        LookupStatus::Hit => "hit",
        LookupStatus::Miss => "miss",
    };
    writeln!(w, "status: {status}")?;
    if let Some(reason) = body.reason {
        writeln!(w, "reason: {reason}")?;
    }
    Ok(())
}

fn write_write_text(w: &mut dyn Write, body: &WriteBody) -> std::io::Result<()> {
    writeln!(w, "adapter: {}", body.adapter)?;
    writeln!(w, "slice: {}", body.slice)?;
    writeln!(w, "source-key: {}", body.source_key)?;
    writeln!(w, "operation: {}", body.operation)?;
    writeln!(w, "fingerprint: {}", body.fingerprint)?;
    writeln!(w, "cache-dir: {}", body.cache_dir.display())?;
    writeln!(w, "opted-out: {}", body.opted_out)?;
    Ok(())
}

fn write_explain_text(w: &mut dyn Write, body: &ExplainBody) -> std::io::Result<()> {
    writeln!(w, "adapter: {}", body.adapter)?;
    writeln!(w, "index: {}", body.index_path.display())?;
    if body.entries.is_empty() {
        writeln!(w, "  (no cache writes recorded yet)")?;
        return Ok(());
    }
    for entry in &body.entries {
        writeln!(
            w,
            "  {ts} {op} {slice}/{key} {fp}",
            ts = entry.timestamp,
            op = entry.operation,
            slice = entry.slice,
            key = entry.source_key,
            fp = entry.fingerprint
        )?;
    }
    Ok(())
}
