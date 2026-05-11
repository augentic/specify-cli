//! `specify tool list` handler.

use specify_error::Result;

use super::dto::{ListBody, rows_for};
use super::{build_inventory, emit_warnings_to_stderr};
use crate::cli::Format;
use crate::context::Ctx;

pub(crate) fn run(ctx: &Ctx) -> Result<()> {
    let inventory = build_inventory(ctx)?;
    let rows = rows_for(&inventory.tools)?;
    let body = ListBody {
        tools: rows,
        warnings: inventory.warnings,
    };
    ctx.out().write(&body)?;
    if matches!(ctx.format, Format::Text) {
        emit_warnings_to_stderr(&body.warnings);
    }
    Ok(())
}
