//! `specify tool show` handler.

use specify_error::Result;

use super::dto::{ShowBody, show_row_for, write_show_text};
use super::{build_inventory, emit_warnings_to_stderr, find};
use crate::cli::Format;
use crate::context::Ctx;

pub(crate) fn run(ctx: &Ctx, name: &str) -> Result<()> {
    let inventory = build_inventory(ctx)?;
    let scoped = find(&inventory, name)?;
    let row = show_row_for(scoped)?;
    let body = ShowBody {
        tool: row,
        warnings: inventory.warnings,
    };
    ctx.write(&body, write_show_text)?;
    if matches!(ctx.format, Format::Text) {
        emit_warnings_to_stderr(&body.warnings);
    }
    Ok(())
}
