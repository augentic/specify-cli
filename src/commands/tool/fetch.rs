//! `specify tool fetch` handler.

use jiff::Timestamp;
use specify_error::Result;
use specify_tool::cache::Status as CacheStatus;

use super::dto::{FetchBody, ToolFetchRow, cache_status_for, row_for, write_fetch_text};
use super::{build_inventory, emit_warnings_to_stderr, select};
use crate::cli::Format;
use crate::context::Ctx;

pub fn run(ctx: &Ctx, name: Option<&str>) -> Result<()> {
    let inventory = build_inventory(ctx)?;
    let selected = select(&inventory, name)?;
    let mut rows = Vec::with_capacity(selected.len());
    for scoped in selected {
        let before = cache_status_for(scoped)?;
        specify_tool::resolver::resolve(
            &scoped.scope,
            &scoped.tool,
            Timestamp::now(),
            &ctx.project_dir,
        )?;
        rows.push(ToolFetchRow {
            row: row_for(scoped)?,
            fetched: before != CacheStatus::Hit,
        });
    }

    let body = FetchBody {
        tools: rows,
        warnings: inventory.warnings,
    };
    ctx.write(&body, write_fetch_text)?;
    if matches!(ctx.format, Format::Text) {
        emit_warnings_to_stderr(&body.warnings);
    }
    Ok(())
}
