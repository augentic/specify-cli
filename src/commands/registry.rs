//! `specify registry *` dispatcher. Per-subcommand handlers live in
//! sibling modules; shared response DTOs live in `registry/dto.rs`.

mod add;
pub mod cli;
mod dto;
mod remove;
mod show;
mod validate;

use specify_error::Result;

use crate::cli::RegistryAction;
use crate::context::Ctx;

pub fn run(ctx: &Ctx, action: RegistryAction) -> Result<()> {
    match action {
        RegistryAction::Show => show::run(ctx),
        RegistryAction::Validate => validate::run(ctx),
        RegistryAction::Add {
            name,
            url,
            capability,
            description,
        } => add::run(ctx, name, url, capability, description),
        RegistryAction::Remove { name } => remove::run(ctx, name),
    }
}
