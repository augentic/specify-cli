//! `specrun tool schema` handler — convenience wrapper that delegates
//! to the tool's `schema <name>` subcommand and passes through the
//! guest's exit code.

use specify_error::Result;

use super::run;
use crate::runtime::context::Ctx;

pub fn schema(ctx: &Ctx, tool_name: &str, schema_name: &str) -> Result<u8> {
    run(ctx, tool_name, vec!["schema".to_string(), schema_name.to_string()])
}
