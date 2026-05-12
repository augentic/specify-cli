use std::io::Write;
use std::path::Path;
use std::process::ExitCode;

use serde::{Serialize, Serializer};
use specify_error::{Error, ValidationStatus, ValidationSummary};

use crate::cli::Format;

/// Output sink for [`emit`]. `Stdout` is the default success channel;
/// `Stderr` is reserved for failure envelopes and any diagnostic
/// rendering that should not interleave with the structured success
/// stream skills consume. Private to `src/output.rs`: handlers route
/// through `ctx.write(&Body, write_text)?;`; `Stream::Stderr` is
/// reached only by [`report`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stream {
    Stdout,
    Stderr,
}

/// Serialise `data` and write it to stdout in `format`, using
/// `render_text` for the text-format branch. The closure-based form
/// is the single success-path emission entry point — handlers either
/// reach for `ctx.write(&body, write_text)?;` or, on the rare
/// `Ctx`-less verbs, call this directly.
///
/// # Errors
///
/// Propagates the underlying serialization or I/O error from
/// [`emit`].
pub(crate) fn write<T: Serialize>(
    format: Format, data: &T, render_text: impl FnOnce(&mut dyn Write, &T) -> std::io::Result<()>,
) -> Result<(), Error> {
    emit(Stream::Stdout, format, data, render_text)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[must_use]
pub enum Exit {
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

impl Exit {
    pub const fn code(self) -> u8 {
        match self {
            // exit 0: handler-reported success.
            Self::Success => 0,
            // exit 1: catch-all failure (kebab discriminants: `io`,
            // `yaml`, `lifecycle`, `non-terminal-entries-present`,
            // `change-finalize-blocked`, plan/context/registry diag
            // codes, …anything `From<&Error>` doesn't map to a typed
            // slot).
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

impl From<Exit> for ExitCode {
    fn from(r: Exit) -> Self {
        Self::from(r.code())
    }
}

impl From<&Error> for Exit {
    fn from(err: &Error) -> Self {
        match err {
            Error::CliTooOld { .. } => Self::VersionTooOld,
            Error::Validation { .. } => Self::ValidationFailed,
            // Diag-routed siblings of the typed validation cluster.
            // Their typed variants collapsed to `Diag` but their exit
            // slot stays exit 2 — the kebab `code` is the wire contract
            // and skills branch on it.
            Error::Diag { code, .. }
                if matches!(
                    *code,
                    "plan-structural-errors"
                        | "compatibility-check-failed"
                        | "capability-check-failed"
                        | "slice-validation-failed"
                        | "tool-permission-denied"
                        | "tool-not-declared"
                ) =>
            {
                Self::ValidationFailed
            }
            Error::Argument { .. } => Self::ArgumentError,
            _ => Self::GenericFailure,
        }
    }
}

/// Render `err` as a failure envelope and return the matching exit
/// code. JSON serialises the body directly; Text writes
/// `error: {err}` plus any long-form hint for the variant. Both
/// formats route through [`emit`] against [`Stream::Stderr`] so
/// failure output never interleaves with the structured success
/// stream skills consume.
///
/// Single dispatcher entry point: handlers return
/// `Result<T, specify_error::Error>` and the run loop in
/// [`crate::commands`] hands the error here. The body shape is
/// always [`ErrorBody`]; for `Error::Validation`, the body's
/// `results` field carries the per-row failures.
pub(crate) fn report(format: Format, err: &Error) -> Exit {
    let code = Exit::from(err);
    let body = ErrorBody::from(err);
    let result = emit(Stream::Stderr, format, &body, write_error_text);
    if let Err(serialise_err) = result {
        eprintln!("error: {err}");
        eprintln!("error: {serialise_err}");
    }
    code
}

/// Return a locked stdout/stderr writer for `stream`. Boxed to keep
/// the JSON and text emitter signatures uniform across both sinks.
fn writer_for(stream: Stream) -> Box<dyn Write> {
    match stream {
        Stream::Stdout => Box::new(std::io::stdout().lock()),
        Stream::Stderr => Box::new(std::io::stderr().lock()),
    }
}

/// Emit `payload` to `stream` in the requested format. JSON
/// serialises the body directly via `serde_json::to_writer_pretty`;
/// Text locks the sink and delegates to `render_text`. The single
/// signature covers both success (`Stream::Stdout`) and failure
/// (`Stream::Stderr`) — there is one entry point for all structured
/// output.
fn emit<T: Serialize>(
    stream: Stream, format: Format, payload: &T,
    render_text: impl FnOnce(&mut dyn Write, &T) -> std::io::Result<()>,
) -> Result<(), Error> {
    match format {
        Format::Json => {
            let mut writer = writer_for(stream);
            serde_json::to_writer_pretty(&mut writer, payload).map_err(|err| Error::Diag {
                code: "json-serialize-failed",
                detail: format!("failed to serialize JSON response: {err}"),
            })?;
            writeln!(writer).map_err(Error::Io)
        }
        Format::Text => {
            let mut writer = writer_for(stream);
            render_text(&mut writer, payload).map_err(Error::Io)
        }
    }
}

/// Failure envelope used by [`report`] for every error variant. For
/// `Error::Validation`, `results` is `Some(rows)`; otherwise it is
/// `None` and serde elides the field from the JSON output via
/// `skip_serializing_if`.
///
/// Construct via `ErrorBody::from(&err)` — the variant is the only
/// shape on the wire.
#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) struct ErrorBody<'a> {
    pub(crate) error: String,
    pub(crate) message: String,
    pub(crate) exit_code: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) results: Option<Vec<ValidationRow<'a>>>,
    #[serde(skip)]
    hint_source: &'a Error,
}

impl<'a> From<&'a Error> for ErrorBody<'a> {
    fn from(err: &'a Error) -> Self {
        let results = match err {
            Error::Validation { results } => {
                Some(results.iter().map(ValidationRow::from).collect())
            }
            _ => None,
        };
        Self {
            error: err.variant_str().to_string(),
            message: err.to_string(),
            exit_code: Exit::from(err).code(),
            results,
            hint_source: err,
        }
    }
}

fn write_error_text(w: &mut dyn Write, body: &ErrorBody<'_>) -> std::io::Result<()> {
    writeln!(w, "error: {}", body.message)?;
    if let Some(hint) = body.hint_source.hint() {
        writeln!(w, "hint: {hint}")?;
    }
    Ok(())
}

/// JSON row in a validation envelope. Mirrors `ValidationSummary`
/// field-for-field so domain validators surface uniformly via
/// `ValidationRow::from(&summary)`. Callers that need a different row
/// shape (e.g. plan validate's `level/code/entry/message`) define
/// their own row type and embed it as `results: Vec<…>` directly on
/// their `*Body`.
#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) struct ValidationRow<'a> {
    pub(crate) status: ValidationStatus,
    pub(crate) rule_id: &'a str,
    pub(crate) rule: &'a str,
    pub(crate) detail: Option<&'a str>,
}

impl<'a> From<&'a ValidationSummary> for ValidationRow<'a> {
    fn from(summary: &'a ValidationSummary) -> Self {
        Self {
            status: summary.status,
            rule_id: &summary.rule_id,
            rule: &summary.rule,
            detail: summary.detail.as_deref(),
        }
    }
}

/// `#[serde(serialize_with)]` adapter for `*Body { path: PathBuf }`
/// fields. Always emits `Path::to_string_lossy` so the wire shape is a
/// pure function of the input path — no filesystem dependency, no
/// canonicalisation that varies with whether the file exists at
/// serialise time. Test fixtures and goldens stay reproducible.
pub(crate) fn serialize_path<S: Serializer>(p: &Path, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(&p.to_string_lossy())
}
