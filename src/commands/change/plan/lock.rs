use serde::Serialize;
use specify_change::{Acquired, PlanLockReleased, PlanLockState, Stamp};
use specify_error::Error;

use crate::cli::OutputFormat;
use crate::context::CommandContext;
use crate::output::{CliResult, emit_response};

pub fn acquire(ctx: &CommandContext, pid: Option<u32>) -> Result<CliResult, Error> {
    let our_pid = pid.unwrap_or_else(std::process::id);
    let acquired = Stamp::acquire(&ctx.project_dir, our_pid)?;
    emit_acquired(ctx.format, &acquired)
}

fn emit_acquired(format: OutputFormat, acquired: &Acquired) -> Result<CliResult, Error> {
    #[derive(Serialize)]
    #[serde(rename_all = "kebab-case")]
    struct AcquiredBody {
        held: bool,
        pid: u32,
        already_held: bool,
        reclaimed_stale_pid: Option<u32>,
    }
    match format {
        OutputFormat::Json => emit_response(AcquiredBody {
            held: true,
            pid: acquired.pid,
            already_held: acquired.already_held,
            reclaimed_stale_pid: acquired.reclaimed_stale_pid,
        })?,
        OutputFormat::Text => {
            if acquired.already_held {
                println!("Lock already held by pid {}; re-stamped.", acquired.pid);
            } else {
                println!("Acquired plan lock for pid {}.", acquired.pid);
            }
            if let Some(stale) = acquired.reclaimed_stale_pid {
                println!("  (reclaimed stale stamp from pid {stale})");
            }
        }
    }
    Ok(CliResult::Success)
}

pub fn release(ctx: &CommandContext, pid: Option<u32>) -> Result<CliResult, Error> {
    let our_pid = pid.unwrap_or_else(std::process::id);
    let outcome = Stamp::release(&ctx.project_dir, our_pid)?;
    emit_released(ctx.format, our_pid, &outcome)
}

fn emit_released(
    format: OutputFormat, our_pid: u32, outcome: &PlanLockReleased,
) -> Result<CliResult, Error> {
    match format {
        OutputFormat::Json => {
            #[derive(Serialize)]
            #[serde(rename_all = "kebab-case")]
            struct ReleasedBody {
                result: &'static str,
                pid: Option<u32>,
                #[serde(skip_serializing_if = "Option::is_none")]
                our_pid: Option<u32>,
            }
            let payload = match outcome {
                PlanLockReleased::Removed { pid } => ReleasedBody {
                    result: "removed",
                    pid: Some(*pid),
                    our_pid: None,
                },
                PlanLockReleased::WasAbsent => ReleasedBody {
                    result: "was-absent",
                    pid: None,
                    our_pid: None,
                },
                PlanLockReleased::HeldByOther { pid } => ReleasedBody {
                    result: "held-by-other",
                    pid: *pid,
                    our_pid: Some(our_pid),
                },
            };
            emit_response(payload)?;
        }
        OutputFormat::Text => match outcome {
            PlanLockReleased::Removed { pid } => {
                println!("Released plan lock held by pid {pid}.");
            }
            PlanLockReleased::WasAbsent => {
                println!("No plan lock to release.");
            }
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
        },
    }
    Ok(CliResult::Success)
}

pub fn status(ctx: &CommandContext) -> Result<CliResult, Error> {
    let state = Stamp::status(&ctx.project_dir)?;
    emit_state(ctx.format, &state)
}

fn emit_state(format: OutputFormat, state: &PlanLockState) -> Result<CliResult, Error> {
    #[derive(Serialize)]
    #[serde(rename_all = "kebab-case")]
    struct StateBody {
        held: bool,
        pid: Option<u32>,
        stale: Option<bool>,
    }
    match format {
        OutputFormat::Json => emit_response(StateBody {
            held: state.held,
            pid: state.pid,
            stale: state.stale,
        })?,
        OutputFormat::Text => match state.pid {
            Some(pid) => {
                let is_stale = state.stale.unwrap_or(false);
                if is_stale {
                    println!("stale (pid {pid} no longer alive)");
                } else {
                    println!("held by pid {pid}");
                }
            }
            None => match state.stale {
                Some(true) => println!("stale (malformed lockfile contents)"),
                _ => println!("no lock"),
            },
        },
    }
    Ok(CliResult::Success)
}
