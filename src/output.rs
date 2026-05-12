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
/// through `ctx.write(&Body)?;` (or [`write`] for the rare `Ctx`-less
/// verbs); `Stream::Stderr` is reached only by [`report`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stream {
    Stdout,
    Stderr,
}

/// Serialise `body` and write it to stdout in `format`. Use only in
/// `Ctx`-less handlers (`init`, `capability::resolve`,
/// `capability::check`); every other handler reaches for
/// `ctx.write(&Body)?;`.
///
/// # Errors
///
/// Propagates the underlying serialization or I/O error from
/// [`emit`].
pub(crate) fn write<R: Render>(format: Format, body: &R) -> Result<(), Error> {
    emit(Stream::Stdout, format, body)
}

/// Serialise `data` and write it to stdout in `format`, using
/// `render_text` for the text-format branch instead of requiring a
/// [`Render`] impl on the type. Lets one-off handlers ship their text
/// rendering as an inline closure beside the call site rather than as
/// a sibling `impl Render for *Body` block.
///
/// # Errors
///
/// Propagates the underlying serialization or I/O error from
/// [`emit_with`].
pub(crate) fn write_with<T: Serialize>(
    format: Format, data: &T, render_text: impl FnOnce(&mut dyn Write, &T) -> std::io::Result<()>,
) -> Result<(), Error> {
    emit_with(Stream::Stdout, format, data, render_text)
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

impl From<Exit> for ExitCode {
    fn from(r: Exit) -> Self {
        Self::from(r.code())
    }
}

impl From<&Error> for Exit {
    fn from(err: &Error) -> Self {
        match err {
            Error::CliTooOld { .. } => Self::VersionTooOld,
            Error::Validation { .. } | Error::ToolDenied(_) | Error::ToolNotDeclared { .. } => {
                Self::ValidationFailed
            }
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
                ) =>
            {
                Self::ValidationFailed
            }
            Error::Argument { .. } => Self::ArgumentError,
            _ => Self::GenericFailure,
        }
    }
}

/// Wire envelope version stamped onto every `--format json` body.
/// Bump on any incompatible shape change.
pub(crate) const ENVELOPE_VERSION: u64 = 6;

/// Render `err` as a failure envelope and return the matching exit
/// code. JSON serialises the typed body directly; Text writes
/// `error: {err}` plus any long-form hint for the variant. Both
/// formats route through [`emit`] against [`Stream::Stderr`] so
/// failure output never interleaves with the structured success
/// stream skills consume.
///
/// Single dispatcher entry point: handlers return
/// `Result<T, specify_error::Error>` and the run loop in
/// [`crate::commands`] hands the error here. The variant decides the
/// payload: `Error::Validation` builds a [`ValidationErrBody`]
/// (envelope keys + per-row `results`), every other variant builds an
/// [`ErrorBody`] (envelope keys only). Both wire shapes carry the
/// shared `error` / `message` / `exit-code` envelope fields.
pub(crate) fn report(format: Format, err: &Error) -> Exit {
    let code = Exit::from(err);
    let result = match err {
        Error::Validation { results } => {
            emit(Stream::Stderr, format, &ValidationErrBody::from((err, results.as_slice())))
        }
        _ => emit(Stream::Stderr, format, &ErrorBody::from(err)),
    };
    if let Err(serialise_err) = result {
        eprintln!("error: {err}");
        eprintln!("error: {serialise_err}");
    }
    code
}

#[derive(Serialize)]
pub(crate) struct Envelope<T> {
    #[serde(rename = "envelope-version")]
    envelope_version: u64,
    #[serde(flatten)]
    payload: T,
}

impl<T: Serialize> Envelope<T> {
    const fn new(payload: T) -> Self {
        Self {
            envelope_version: ENVELOPE_VERSION,
            payload,
        }
    }
}

/// Serialise `payload` inside the [`Envelope`] and write it to
/// `stream`. Private helper for [`emit`]'s JSON path; nothing else
/// should touch the wire envelope shape directly.
fn emit_json<T: Serialize>(stream: Stream, payload: &T) -> Result<(), Error> {
    let envelope = Envelope::new(payload);
    let mut writer = writer_for(stream);
    serde_json::to_writer_pretty(&mut writer, &envelope).map_err(|err| Error::Diag {
        code: "json-serialize-failed",
        detail: format!("failed to serialize JSON response: {err}"),
    })?;
    writeln!(writer).map_err(Error::Io)
}

/// Return a locked stdout/stderr writer for `stream`. Boxed to keep
/// the JSON and text emitter signatures uniform across both sinks.
fn writer_for(stream: Stream) -> Box<dyn Write> {
    match stream {
        Stream::Stdout => Box::new(std::io::stdout().lock()),
        Stream::Stderr => Box::new(std::io::stderr().lock()),
    }
}

/// Format-agnostic command-result rendering. JSON is derived from
/// `serde::Serialize`; text rendering is delegated to `render_text`.
///
/// Implementors keep their text/JSON shapes side-by-side in one place,
/// and command dispatchers stop hand-rolling `match ctx.format`.
pub(crate) trait Render: Serialize {
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
/// output. Private to this module: success-path handlers route
/// through `ctx.write(&body)` (or [`write`] for the rare `Ctx`-less
/// verbs); failure envelopes go through [`report`], which builds the
/// typed body and routes it back through this function.
fn emit<R: Render>(stream: Stream, format: Format, payload: &R) -> Result<(), Error> {
    emit_with(stream, format, payload, |w, p| p.render_text(w))
}

/// Closure-based peer to [`emit`]: JSON wraps `payload` in the
/// envelope (identical to [`emit`]) and Text delegates to
/// `render_text`. Single emission core so the two surfaces share a
/// stream/sink/error pipeline.
fn emit_with<T: Serialize>(
    stream: Stream, format: Format, payload: &T,
    render_text: impl FnOnce(&mut dyn Write, &T) -> std::io::Result<()>,
) -> Result<(), Error> {
    match format {
        Format::Json => emit_json(stream, payload),
        Format::Text => {
            let mut writer = writer_for(stream);
            render_text(&mut writer, payload).map_err(Error::Io)
        }
    }
}

/// Generic failure envelope used by [`report`] for every
/// variant outside `Error::Validation`. `hint_source` is the
/// originating error reference; it carries no JSON-visible state
/// (`#[serde(skip)]`) and exists solely so the [`Render`] impl can
/// dispatch to [`write_text_hint`] without re-deriving the variant.
///
/// Construct via `ErrorBody::from(&err)` — never inline at a call
/// site (the `error-envelope-inlined` xtask predicate fails any
/// hand-rolled construction outside `src/output.rs`).
#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) struct ErrorBody<'a> {
    pub(crate) error: String,
    pub(crate) message: String,
    pub(crate) exit_code: u8,
    #[serde(skip)]
    hint_source: &'a Error,
}

impl<'a> From<&'a Error> for ErrorBody<'a> {
    fn from(err: &'a Error) -> Self {
        Self {
            error: err.variant_str().to_string(),
            message: err.to_string(),
            exit_code: Exit::from(err).code(),
            hint_source: err,
        }
    }
}

impl Render for ErrorBody<'_> {
    fn render_text(&self, w: &mut dyn Write) -> std::io::Result<()> {
        writeln!(w, "error: {}", self.message)?;
        if let Some(hint) = self.hint_source.hint() {
            writeln!(w, "hint: {hint}")?;
        }
        Ok(())
    }
}

/// JSON row in a validation envelope. Mirrors `ValidationSummary`
/// field-for-field so domain validators surface uniformly via
/// `ValidationRow::from(&summary)`. Callers that need a different row
/// shape (e.g. plan validate's `level/code/entry/message`) define
/// their own row type and reuse [`Validation`] for the envelope.
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

/// Shared validation results envelope. Serialises as `{"results": [...]}`
/// and renders text by delegating per-row formatting to each row's
/// [`Render`] impl. Callers wrap it in a typed `*Body` and extend it
/// with metadata fields via `#[serde(flatten)]`:
///
/// - `commands::codex::validate` adds `ok`, `rule-count`, `error-count`.
/// - `commands::change::plan::lifecycle::validate` adds `plan`, `passed`.
#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) struct Validation<R> {
    pub(crate) results: Vec<R>,
}

impl<R: Serialize + Render> Render for Validation<R> {
    fn render_text(&self, w: &mut dyn std::io::Write) -> std::io::Result<()> {
        for row in &self.results {
            row.render_text(w)?;
        }
        Ok(())
    }
}

/// Validation-specific failure envelope used by [`report`]
/// when the variant is `Error::Validation`. Peer to [`ErrorBody`]:
/// shares the same envelope keys (`error`/`message`/`exit-code`) and
/// flattens [`Validation<ValidationRow>`] for the per-row `results`
/// list skills consume.
///
/// Construct via `ValidationErrBody::from((&err, results))` — never
/// inline at a call site (the `error-envelope-inlined` xtask
/// predicate fails any hand-rolled construction outside
/// `src/output.rs`).
#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) struct ValidationErrBody<'a> {
    pub(crate) error: &'static str,
    pub(crate) message: String,
    pub(crate) exit_code: u8,
    #[serde(flatten)]
    pub(crate) validation: Validation<ValidationRow<'a>>,
    #[serde(skip)]
    hint_source: &'a Error,
}

impl<'a> From<(&'a Error, &'a [ValidationSummary])> for ValidationErrBody<'a> {
    fn from((err, results): (&'a Error, &'a [ValidationSummary])) -> Self {
        Self {
            error: "validation",
            message: err.to_string(),
            exit_code: Exit::from(err).code(),
            validation: Validation {
                results: results.iter().map(ValidationRow::from).collect(),
            },
            hint_source: err,
        }
    }
}

impl Render for ValidationErrBody<'_> {
    fn render_text(&self, w: &mut dyn Write) -> std::io::Result<()> {
        writeln!(w, "error: {}", self.message)?;
        if let Some(hint) = self.hint_source.hint() {
            writeln!(w, "hint: {hint}")?;
        }
        Ok(())
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
