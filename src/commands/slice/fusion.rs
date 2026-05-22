//! `specify slice fusion show` — RFC-27 §D4 inspection verb.
//!
//! Reads the slice's `fusion.yaml`, schema-validates it via
//! [`FusionIndex::load`], and re-emits the parsed shape. The JSON
//! format is a verbatim re-serialisation of the validated struct
//! (byte-stable across repeated runs); the text format renders one
//! requirement per section with the inline `value` payload truncated
//! for terminal display so the operator can audit synthesis decisions
//! without opening `evidence/*.yaml`.
//!
//! Writes are out of scope for 2.6 — the agent-side
//! `/spec:refine` skill body lands in Change 3.2 and uses
//! [`specify_domain::slice::fusion::FusionIndex::write_atomic`]
//! directly. The CLI verb here owns inspection only.

use std::io::Write;

use specify_domain::slice::fusion::{self, FusionIndex};
use specify_error::{Error, Result};

use crate::context::Ctx;

/// Truncation budget for inline `value` payloads in the text
/// renderer. The schema caps `value` at 16 `KiB`; terminal output
/// trims to a single readable line plus a trailing `…` marker so the
/// audit print stays scannable. JSON output is unaffected.
const TEXT_VALUE_TRUNCATE_COLUMNS: usize = 200;

pub(super) fn show(ctx: &Ctx, name: &str) -> Result<()> {
    let slice_dir = ctx.slices_dir().join(name);
    let fusion_path = fusion::fusion_path(&slice_dir);
    if !fusion_path.is_file() {
        return Err(Error::Diag {
            code: "slice-fusion-not-found",
            detail: format!(
                "no fusion.yaml at {}; run `/spec:refine` to write the reconciliation index",
                fusion_path.display()
            ),
        });
    }
    // Load already routes schema failure through `Error::Validation`
    // (exit 2). Filesystem read failure routes through
    // `Error::Filesystem` (exit 1). Both shapes are the documented
    // behaviour for this verb.
    let index = FusionIndex::load(&fusion_path)?;
    ctx.write(&index, write_show_text)?;
    Ok(())
}

fn write_show_text(w: &mut dyn Write, index: &FusionIndex) -> std::io::Result<()> {
    writeln!(w, "slice: {}", index.slice)?;
    writeln!(w, "generator: {}", index.generator)?;
    writeln!(w, "generated-at: {}", index.generated_at)?;
    writeln!(w, "requirements: {}", index.requirements.len())?;
    for req in &index.requirements {
        writeln!(w)?;
        writeln!(w, "{}", req.id)?;
        writeln!(w, "  status: {}", req.status.as_str())?;
        writeln!(w, "  resolution: {}", req.resolution)?;
        if !req.sources.is_empty() {
            writeln!(w, "  sources: [{}]", req.sources.join(", "))?;
        }
        if let Some(trace) = &req.resolution_trace {
            write!(w, "  resolution-trace: step={}", trace.step)?;
            if let Some(winner) = &trace.winner {
                write!(w, " winner={winner}")?;
            }
            if let Some(map) = &trace.r#override {
                write!(w, " override={map}")?;
            }
            writeln!(w)?;
        }
        if req.contributing_claims.is_empty() {
            writeln!(w, "  contributing-claims: (none)")?;
            continue;
        }
        writeln!(w, "  contributing-claims:")?;
        for claim in &req.contributing_claims {
            let marker = match claim.winner {
                Some(true) => "winner: true ",
                Some(false) => "winner: false",
                None => "             ",
            };
            writeln!(
                w,
                "    - [{marker}] {source} :: {claim_id} (kind: {kind})",
                source = claim.source,
                claim_id = claim.claim_id,
                kind = claim.kind,
            )?;
            if let Some(value) = &claim.value {
                writeln!(w, "        value: {}", truncate_value_for_display(value))?;
            }
            if let Some(path) = &claim.path {
                writeln!(w, "        path:  {path}")?;
            }
        }
    }
    Ok(())
}

/// Replace embedded newlines, collapse internal whitespace, and clip
/// the result to [`TEXT_VALUE_TRUNCATE_COLUMNS`] characters with a
/// trailing `…` marker. The schema-side writer already truncates
/// multi-line claim bodies to the first non-empty line (RFC-27
/// §Reconciliation index `value` rule), but this defensive trim
/// keeps the inspection verb honest against hand-written
/// `fusion.yaml` files that might slip past the truncation rule.
fn truncate_value_for_display(value: &str) -> String {
    let mut flattened = String::with_capacity(value.len());
    let mut prev_space = false;
    for ch in value.chars() {
        if ch.is_whitespace() {
            if !prev_space && !flattened.is_empty() {
                flattened.push(' ');
            }
            prev_space = true;
        } else {
            flattened.push(ch);
            prev_space = false;
        }
    }
    let trimmed = flattened.trim_end().to_string();
    let char_count = trimmed.chars().count();
    if char_count <= TEXT_VALUE_TRUNCATE_COLUMNS {
        return trimmed;
    }
    // Cut on a character boundary; `take` is char-aware.
    let cut: String = trimmed.chars().take(TEXT_VALUE_TRUNCATE_COLUMNS).collect();
    let cut = cut.trim_end().to_string();
    format!("{cut}…")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_value_for_display_passes_short_values_through() {
        assert_eq!(truncate_value_for_display("hello"), "hello");
    }

    #[test]
    fn truncate_value_for_display_flattens_newlines_to_single_space() {
        assert_eq!(truncate_value_for_display("a\n\nb\n\tc"), "a b c");
    }

    #[test]
    fn truncate_value_for_display_clips_at_budget_and_appends_ellipsis() {
        let s = "x".repeat(TEXT_VALUE_TRUNCATE_COLUMNS + 50);
        let out = truncate_value_for_display(&s);
        assert!(out.ends_with('…'), "must end with ellipsis, got: {out}");
        assert!(
            out.chars().count() <= TEXT_VALUE_TRUNCATE_COLUMNS + 1,
            "truncated to budget + ellipsis, got {} chars",
            out.chars().count()
        );
    }
}
