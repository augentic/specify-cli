use std::io::Write;
use std::path::Path;
use std::process::ExitCode;

use serde::{Serialize, Serializer};
use specify_error::{Error, ValidationSummary};

use crate::cli::OutputFormat;

/// Output sink for [`emit`]. `Stdout` is the default success channel;
/// `Stderr` is reserved for failure envelopes and any diagnostic
/// rendering that should not interleave with the structured success
/// stream skills consume.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stream {
    Stdout,
    Stderr,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[must_use]
pub enum CliResult {
    Success,
    GenericFailure,
    ValidationFailed,
    VersionTooOld,
    /// Argument-shape failure: `clap` exits 2 for unknown flags / missing
    /// arguments; we mirror that for argument errors discovered after
    /// parsing (kebab-case checks, mutually exclusive payloads, etc.).
    ArgumentError,
    /// WASI tool exit-code passthrough. The only legitimate caller is
    /// `commands::tool::run`, which forwards the guest's exit code so
    /// `specify tool run` is a transparent shim over the underlying
    /// WASI binary. Other handlers MUST route through the typed
    /// variants above so the four-slot exit-code contract
    /// (0/1/2/3) stays auditable.
    Code(u8),
}

impl CliResult {
    pub const fn code(self) -> u8 {
        match self {
            // exit 0: handler-reported success.
            Self::Success => 0,
            // exit 1: catch-all failure (kebab discriminants: `io`,
            // `yaml`, `lifecycle`, plan/context/registry variants,
            // …anything `From<&Error>` doesn't map to a typed slot).
            Self::GenericFailure => 1,
            // exit 2: argument-shape error (kebab discriminant: `argument`).
            // exit 2: validation failure (kebab discriminant: `validation`,
            //         plus `tool-permission-denied`, `tool-not-declared`,
            //         `plan-structural-errors`).
            // Skills disambiguate the two by reading the kebab-case
            // `error` field of the JSON envelope; the numeric collapse
            // matches clap's own parser-error convention.
            Self::ArgumentError | Self::ValidationFailed => 2,
            // exit 3: CLI too old for the project's pinned floor
            // (kebab discriminant: `specify-version-too-old`).
            Self::VersionTooOld => 3,
            // exit N: opaque WASI-tool exit code, forwarded verbatim
            // by `commands::tool::run` (see `Code` doc-comment).
            Self::Code(code) => code,
        }
    }
}

impl From<CliResult> for ExitCode {
    fn from(r: CliResult) -> Self {
        Self::from(r.code())
    }
}

impl From<&Error> for CliResult {
    fn from(err: &Error) -> Self {
        match err {
            Error::CliTooOld { .. } => Self::VersionTooOld,
            Error::Validation { .. }
            | Error::ToolDenied(_)
            | Error::ToolNotDeclared { .. }
            | Error::PlanStructural => Self::ValidationFailed,
            Error::Argument { .. } => Self::ArgumentError,
            _ => Self::GenericFailure,
        }
    }
}

/// JSON envelope version emitted on every structured response (the
/// `schema-version` field of the wire shape, not a JSON-Schema spec).
/// Bumping it is a breaking change for skill authors.
pub const JSON_ENVELOPE_VERSION: u64 = 5;

/// Render `err` as a failure envelope and return the matching exit
/// code. JSON serialises the typed body directly; Text writes
/// `error: {err}` plus any long-form hint for the variant. Both
/// formats route through [`emit`] against [`Stream::Stderr`] so
/// failure output never interleaves with the structured success
/// stream skills consume.
///
/// Single dispatcher entry point: handlers return
/// `Result<T, specify_error::Error>` and the run loop in
/// [`crate::commands`] hands the error here. Internally the body is
/// the private [`ErrorBody`] enum; `Error::Validation` keeps its
/// [`ValidationErrorResponse`] row shape (R5 collapses the carve-out
/// into a single body type).
pub fn report_error(format: OutputFormat, err: &Error) -> CliResult {
    let code = CliResult::from(err);
    if let Err(serialise_err) = emit(Stream::Stderr, format, &ErrorBody::from(err)) {
        eprintln!("error: {err}");
        eprintln!("error: {serialise_err}");
    }
    code
}

/// Long-form recovery hints for tightened diagnostics. The
/// `#[error("…")]` body carries the kebab discriminant + immediate
/// cause; the renderer appends actionable follow-up so the
/// machine-readable JSON envelope stays compact while operators see
/// the full guidance on a TTY. Free function (not a method on
/// [`ErrorResponse`]) so it can be reused by any error renderer
/// without forcing the body type to own the variant identity.
fn write_text_hint(w: &mut dyn Write, err: &Error) -> std::io::Result<()> {
    match err {
        Error::InitNeedsCapability => {
            writeln!(
                w,
                "hint: `specify init <capability>` for a regular project, or `specify init --hub` for a platform hub."
            )?;
            writeln!(w, "see: docs/init.md")?;
        }
        Error::ContextUnfenced => {
            writeln!(w, "hint: rerun with --force to rewrite AGENTS.md.")?;
        }
        Error::ContextDrift => {
            writeln!(
                w,
                "hint: reconcile the edits or rerun with --force to replace the generated block."
            )?;
        }
        _ => {}
    }
    Ok(())
}

#[derive(Serialize)]
pub struct JsonEnvelope<T> {
    #[serde(rename = "schema-version")]
    schema_version: u64,
    #[serde(flatten)]
    payload: T,
}

impl<T: Serialize> JsonEnvelope<T> {
    const fn wrap(payload: T) -> Self {
        Self {
            schema_version: JSON_ENVELOPE_VERSION,
            payload,
        }
    }
}

/// Serialise `payload` inside the [`JsonEnvelope`] and write it to
/// `stream`. Private helper for [`emit`]'s JSON path; nothing else
/// should touch the wire envelope shape directly.
fn emit_json<T: Serialize>(stream: Stream, payload: &T) -> Result<(), Error> {
    let envelope = JsonEnvelope::wrap(payload);
    let map_serde = |err: serde_json::Error| Error::Diag {
        code: "json-serialize-failed",
        detail: format!("failed to serialize JSON response: {err}"),
    };
    match stream {
        Stream::Stdout => {
            let stdout = std::io::stdout();
            let mut handle = stdout.lock();
            serde_json::to_writer_pretty(&mut handle, &envelope).map_err(map_serde)?;
            writeln!(handle).map_err(Error::Io)?;
        }
        Stream::Stderr => {
            let stderr = std::io::stderr();
            let mut handle = stderr.lock();
            serde_json::to_writer_pretty(&mut handle, &envelope).map_err(map_serde)?;
            writeln!(handle).map_err(Error::Io)?;
        }
    }
    Ok(())
}

/// Format-agnostic command-result rendering. JSON is derived from
/// `serde::Serialize`; text rendering is delegated to `render_text`.
///
/// Implementors keep their text/JSON shapes side-by-side in one place,
/// and command dispatchers stop hand-rolling `match ctx.format`.
pub trait Render: Serialize {
    /// Write the human-readable text representation. Implementations
    /// should not append a trailing newline — `emit` adds one when the
    /// renderer leaves the underlying handle mid-line.
    fn render_text(&self, w: &mut dyn std::io::Write) -> std::io::Result<()>;
}

/// Emit `payload` to `stream` in the requested format. JSON wraps in
/// the standard envelope and writes to the chosen sink; Text locks the
/// sink and delegates to [`Render::render_text`]. The single signature
/// covers both success (`Stream::Stdout`) and failure
/// (`Stream::Stderr`) — there is one entry point for all structured
/// output. Failure envelopes go through [`report_error`], which
/// builds the typed body and routes it back through this function.
pub fn emit<R: Render>(stream: Stream, format: OutputFormat, payload: &R) -> Result<(), Error> {
    match format {
        OutputFormat::Json => emit_json(stream, payload),
        OutputFormat::Text => match stream {
            Stream::Stdout => {
                let stdout = std::io::stdout();
                let mut handle = stdout.lock();
                payload.render_text(&mut handle).map_err(Error::Io)
            }
            Stream::Stderr => {
                let stderr = std::io::stderr();
                let mut handle = stderr.lock();
                payload.render_text(&mut handle).map_err(Error::Io)
            }
        },
    }
}

/// Generic failure envelope used by [`report_error`] for every
/// variant outside `Error::Validation`. `hint_source` is the
/// originating error reference; it carries no JSON-visible state
/// (`#[serde(skip)]`) and exists solely so the [`Render`] impl can
/// dispatch to [`write_text_hint`] without re-deriving the variant.
#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct ErrorResponse<'a> {
    pub error: String,
    pub message: String,
    pub exit_code: u8,
    #[serde(skip)]
    hint_source: &'a Error,
}

impl Render for ErrorResponse<'_> {
    fn render_text(&self, w: &mut dyn Write) -> std::io::Result<()> {
        writeln!(w, "error: {}", self.message)?;
        write_text_hint(w, self.hint_source)
    }
}

/// JSON row in a validation envelope. Mirrors `ValidationSummary`
/// field-for-field so domain validators surface uniformly via
/// `ValidationRow::from_summary`. Callers that need a different row
/// shape (e.g. plan validate's `level/code/entry/message`) define
/// their own row type and reuse [`Validation`] for the envelope.
#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct ValidationRow<'a> {
    pub status: String,
    pub rule_id: &'a str,
    pub rule: &'a str,
    pub detail: Option<&'a str>,
}

impl<'a> ValidationRow<'a> {
    #[must_use]
    pub fn from_summary(summary: &'a ValidationSummary) -> Self {
        Self {
            status: summary.status.to_string(),
            rule_id: &summary.rule_id,
            rule: &summary.rule,
            detail: summary.detail.as_deref(),
        }
    }
}

/// Shared validation results envelope. Serialises as `{"results": [...]}`
/// and renders text by delegating per-row formatting to each row's
/// [`Render`] impl. Callers wrap it in a typed `*Body` and extend it
/// with metadata fields via `#[serde(flatten)]`:
///
/// - `commands::codex::validate` adds `ok`, `rule-count`, `error-count`.
/// - `commands::change::plan::lifecycle::validate` adds `plan`, `passed`.
#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct Validation<R> {
    pub results: Vec<R>,
}

impl<R: Serialize + Render> Render for Validation<R> {
    fn render_text(&self, w: &mut dyn std::io::Write) -> std::io::Result<()> {
        for row in &self.results {
            row.render_text(w)?;
        }
        Ok(())
    }
}

/// Validation-specific failure envelope used by [`report_error`]
/// when the variant is `Error::Validation`. The JSON shape flattens
/// `Validation<ValidationRow>` into the envelope so skills see
/// `error`/`message`/`exit-code` alongside the row-level results.
/// R5 will refactor this back into the [`ErrorResponse`] family.
#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct ValidationErrorResponse<'a> {
    error: &'static str,
    message: String,
    exit_code: u8,
    #[serde(flatten)]
    validation: Validation<ValidationRow<'a>>,
    #[serde(skip)]
    hint_source: &'a Error,
}

impl Render for ValidationErrorResponse<'_> {
    fn render_text(&self, w: &mut dyn Write) -> std::io::Result<()> {
        writeln!(w, "error: {}", self.message)?;
        write_text_hint(w, self.hint_source)
    }
}

/// Private body type that picks the right wire shape per error
/// variant: `Error::Validation` routes through
/// [`ValidationErrorResponse`], every other variant through
/// [`ErrorResponse`]. The enum gives [`report_error`] one
/// `impl Render` payload to hand to [`emit`] while keeping the two
/// wire shapes intact for skill consumers. R5 collapses the
/// variants into a single body.
enum ErrorBody<'a> {
    Generic(ErrorResponse<'a>),
    Validation(ValidationErrorResponse<'a>),
}

impl<'a> From<&'a Error> for ErrorBody<'a> {
    fn from(err: &'a Error) -> Self {
        let exit_code = CliResult::from(err).code();
        match err {
            Error::Validation { results, .. } => Self::Validation(ValidationErrorResponse {
                error: "validation",
                message: err.to_string(),
                exit_code,
                validation: Validation {
                    results: results.iter().map(ValidationRow::from_summary).collect(),
                },
                hint_source: err,
            }),
            _ => Self::Generic(ErrorResponse {
                error: err.variant_str().to_string(),
                message: err.to_string(),
                exit_code,
                hint_source: err,
            }),
        }
    }
}

impl Serialize for ErrorBody<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Generic(body) => body.serialize(serializer),
            Self::Validation(body) => body.serialize(serializer),
        }
    }
}

impl Render for ErrorBody<'_> {
    fn render_text(&self, w: &mut dyn Write) -> std::io::Result<()> {
        match self {
            Self::Generic(body) => body.render_text(w),
            Self::Validation(body) => body.render_text(w),
        }
    }
}

/// Render `path` as a UTF-8 string, preferring the `canonicalize`
/// result when the entry exists and falling back to `to_string_lossy`
/// otherwise.
pub fn path_string(path: &Path) -> String {
    std::fs::canonicalize(path)
        .ok()
        .map_or_else(|| path.to_string_lossy().into_owned(), |p| p.to_string_lossy().into_owned())
}
