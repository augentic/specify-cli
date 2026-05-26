use specify_authoring::check;
use specify_authoring::context::Context;
use specify_authoring::error::ToolingError;
use specify_authoring::exit::{Exit, exit_from_result};
use specify_authoring::finding::Finding;

use crate::authoring::output::{render_json, render_text};
use crate::shared::format::Format;

pub fn run(format: Format, framework_root: std::path::PathBuf) -> Exit {
    let result = (|| -> Result<(std::path::PathBuf, Vec<Finding>), ToolingError> {
        let ctx = Context::from_framework_root(framework_root)?;
        let framework_root = ctx.framework_root().to_path_buf();
        Ok((framework_root, check::run(&ctx)))
    })();

    match format {
        Format::Text => render_text(&result),
        Format::Json => render_json(&result),
    }

    match result {
        Ok((_, findings)) => exit_from_result(Ok(()), findings.len()),
        Err(error) => exit_from_result(Err(error), 0),
    }
}
