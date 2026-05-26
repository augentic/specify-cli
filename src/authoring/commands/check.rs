use specify_authoring::check;
use specify_authoring::context::Context;
use specify_authoring::error::ToolingError;
use specify_authoring::exit::{Exit, exit_from_result};
use specify_authoring::finding::Finding;

use crate::authoring::output::{check_body, write_check_text};
use crate::output::{self, Format};

pub fn run(format: Format, framework_root: std::path::PathBuf) -> Exit {
    let result = (|| -> Result<(std::path::PathBuf, Vec<Finding>), ToolingError> {
        let ctx = Context::from_framework_root(framework_root)?;
        let framework_root = ctx.framework_root().to_path_buf();
        Ok((framework_root, check::run(&ctx)))
    })();

    let body = check_body(&result);
    if let Err(error) =
        output::emit(Box::new(std::io::stdout().lock()), format, &body, |w, body| {
            write_check_text(w, body, &result)
        })
    {
        eprintln!("error: {error}");
    }

    match result {
        Ok((_, findings)) => exit_from_result(Ok(()), findings.len()),
        Err(error) => exit_from_result(Err(error), 0),
    }
}
