use std::io::Write;

use serde::Serialize;
use specify_domain::change::{PlanLockReleased, Stamp};
use specify_error::Result;

use crate::cli::Format;
use crate::context::Ctx;
use crate::output::Render;

pub(super) fn acquire(ctx: &Ctx, pid: Option<u32>) -> Result<()> {
    let our_pid = pid.unwrap_or_else(std::process::id);
    let acquired = Stamp::acquire(&ctx.project_dir, our_pid)?;
    ctx.write(&AcquireBody {
        held: true,
        pid: acquired.pid,
        already_held: acquired.already_held,
        reclaimed_stale_pid: acquired.reclaimed_stale_pid,
    })?;
    Ok(())
}

pub(super) fn release(ctx: &Ctx, pid: Option<u32>) -> Result<()> {
    let our_pid = pid.unwrap_or_else(std::process::id);
    let outcome = Stamp::release(&ctx.project_dir, our_pid)?;
    let body = match &outcome {
        PlanLockReleased::Removed { pid } => ReleaseBody {
            result: "removed",
            pid: Some(*pid),
            our_pid: None,
        },
        PlanLockReleased::WasAbsent => ReleaseBody {
            result: "was-absent",
            pid: None,
            our_pid: None,
        },
        PlanLockReleased::HeldByOther { pid } => ReleaseBody {
            result: "held-by-other",
            pid: *pid,
            our_pid: Some(our_pid),
        },
    };
    ctx.write(&body)?;
    if matches!(ctx.format, Format::Text) {
        match outcome {
            PlanLockReleased::HeldByOther { pid: Some(other) } => {
                eprintln!(
                    "warning: plan lock is held by pid {other}, not {our_pid}; not removing."
                );
            }
            PlanLockReleased::HeldByOther { pid: None } => {
                eprintln!(
                    "warning: plan lock contents are malformed; refusing to clobber (run the L2.G self-heal path)."
                );
            }
            PlanLockReleased::Removed { .. } | PlanLockReleased::WasAbsent => {}
        }
    }
    Ok(())
}

pub(super) fn status(ctx: &Ctx) -> Result<()> {
    let state = Stamp::status(&ctx.project_dir)?;
    ctx.write(&StatusBody {
        held: state.held,
        pid: state.pid,
        stale: state.stale,
    })?;
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct AcquireBody {
    held: bool,
    pid: u32,
    already_held: bool,
    reclaimed_stale_pid: Option<u32>,
}

impl Render for AcquireBody {
    fn render_text(&self, w: &mut dyn Write) -> std::io::Result<()> {
        if self.already_held {
            writeln!(w, "Lock already held by pid {}; re-stamped.", self.pid)?;
        } else {
            writeln!(w, "Acquired plan lock for pid {}.", self.pid)?;
        }
        if let Some(stale) = self.reclaimed_stale_pid {
            writeln!(w, "  (reclaimed stale stamp from pid {stale})")?;
        }
        Ok(())
    }
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct ReleaseBody {
    result: &'static str,
    pid: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    our_pid: Option<u32>,
}

impl Render for ReleaseBody {
    fn render_text(&self, w: &mut dyn Write) -> std::io::Result<()> {
        match self.result {
            "removed" => {
                if let Some(pid) = self.pid {
                    writeln!(w, "Released plan lock held by pid {pid}.")?;
                }
            }
            "was-absent" => writeln!(w, "No plan lock to release.")?,
            _ => {}
        }
        Ok(())
    }
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct StatusBody {
    held: bool,
    pid: Option<u32>,
    stale: Option<bool>,
}

impl Render for StatusBody {
    fn render_text(&self, w: &mut dyn Write) -> std::io::Result<()> {
        match self.pid {
            Some(pid) => {
                if self.stale.unwrap_or(false) {
                    writeln!(w, "stale (pid {pid} no longer alive)")
                } else {
                    writeln!(w, "held by pid {pid}")
                }
            }
            None => match self.stale {
                Some(true) => writeln!(w, "stale (malformed lockfile contents)"),
                _ => writeln!(w, "no lock"),
            },
        }
    }
}
