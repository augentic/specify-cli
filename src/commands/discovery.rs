//! Dispatcher for `specify discovery *`. The verb owns inspection of
//! `<project_dir>/discovery.md` (RFC-27 §D6); alias writes belong to
//! `specify plan amend --add-alias` / `--remove-alias`.

pub mod cli;
mod show;

use cli::DiscoveryAction;
use specify_error::Result;

use crate::context::Ctx;

pub fn run(ctx: &Ctx, action: &DiscoveryAction) -> Result<()> {
    match action {
        DiscoveryAction::Show { aliases } => show::show(ctx, *aliases),
    }
}
