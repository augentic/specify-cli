//! `specify tool run` handler — transparent shim over the underlying
//! WASI binary that returns the guest's exit byte verbatim via
//! [`crate::runtime::output::Exit::Code`].

use jiff::Timestamp;
use specify_error::Result;
use specify_tool::host::{RunContext, WasiRunner};
use specify_tool::manifest::ToolScope;

use super::{build_inventory, emit_warnings_to_stderr, find};
use crate::runtime::context::Ctx;

pub fn run(ctx: &Ctx, name: &str, args: Vec<String>) -> Result<u8> {
    let inventory = build_inventory(ctx)?;
    emit_warnings_to_stderr(&inventory.warnings);
    let scoped = find(&inventory, name)?;
    let resolved = specify_tool::resolver::resolve(
        &scoped.scope,
        &scoped.tool,
        Timestamp::now(),
        &ctx.project_dir,
    )?;
    let mut run_ctx = RunContext::new(&ctx.project_dir, args);
    if let ToolScope::Plugin { capability_dir, .. } = &scoped.scope {
        run_ctx = run_ctx.with_capability_dir(capability_dir);
    }
    let runner = WasiRunner::new()?;
    let exit = runner.run(&resolved, &run_ctx)?;
    Ok(exit.clamp(0, i32::from(u8::MAX)).try_into().unwrap_or_default())
}
