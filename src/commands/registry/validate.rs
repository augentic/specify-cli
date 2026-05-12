//! `specify registry validate` handler.

use specify_domain::config::ProjectConfig;
use specify_domain::registry::Registry;
use specify_error::Result;

use super::dto::{ValidateBody, write_validate_text};
use crate::context::Ctx;

pub(super) fn run(ctx: &Ctx) -> Result<()> {
    let path = Registry::path(&ctx.project_dir);
    // Hub repos opt into the stricter shape via `project.yaml:hub:
    // true`. Tolerate a missing/unparseable project.yaml here —
    // `specify registry validate` is allowed to run before `specify
    // init`, in which case there is no hub flag to honour and the base
    // shape check is the right behaviour.
    let hub_mode = ProjectConfig::load(&ctx.project_dir).is_ok_and(|cfg| cfg.hub);
    let registry = Registry::load(&ctx.project_dir)?;
    if hub_mode && let Some(reg) = registry.as_ref() {
        reg.validate_shape_hub()?;
    }
    ctx.write(
        &ValidateBody {
            registry,
            path,
            hub_mode,
        },
        write_validate_text,
    )?;
    Ok(())
}
