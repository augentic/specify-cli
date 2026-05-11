//! `specify registry show` handler.

use specify_error::Result;
use specify_registry::Registry;

use super::dto::ShowBody;
use crate::context::Ctx;

pub(super) fn run(ctx: &Ctx) -> Result<()> {
    let path = Registry::path(&ctx.project_dir);
    let registry = Registry::load(&ctx.project_dir)?;
    ctx.out().write(&ShowBody { registry, path })?;
    Ok(())
}
