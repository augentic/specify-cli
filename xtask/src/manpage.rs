//! `xtask gen-man` — render `clap_mangen` roff man pages for the
//! `specify` binary and every (non-`help`) subcommand into a target
//! directory. The output is gitignored (`target/man/` by default);
//! release tooling can pick it up from there.

use std::path::Path;
use std::{fs, io};

/// Recursively render man pages for `cmd` and its subcommands into
/// `out_dir`. Returns the number of `.1` files written.
///
/// File naming follows the standard `man(1)` dash-joined convention
/// (`specify.1`, `specify-change.1`, `specify-change-plan-create.1`),
/// matching what `help2man` / Debian's `dh_installman` expect.
pub fn render(out_dir: &Path) -> io::Result<usize> {
    fs::create_dir_all(out_dir)?;
    let cmd = specify::command();
    let mut count = 0_usize;
    write_recursive(out_dir, &cmd, "specify", &mut count)?;
    Ok(count)
}

fn write_recursive(
    out_dir: &Path, cmd: &clap::Command, page_name: &str, count: &mut usize,
) -> io::Result<()> {
    let mut buffer = Vec::new();
    clap_mangen::Man::new(cmd.clone()).render(&mut buffer)?;
    let path = out_dir.join(format!("{page_name}.1"));
    fs::write(&path, &buffer)?;
    *count += 1;

    for sub in cmd.get_subcommands() {
        // Clap auto-injects a `help` subcommand on every parent; skip
        // it to avoid `specify-help.1`, `specify-change-help.1`, etc.
        // Hidden subcommands (e.g. internal escape hatches) are
        // likewise excluded from the public man-page surface.
        if sub.get_name() == "help" || sub.is_hide_set() {
            continue;
        }
        let sub_page = format!("{page_name}-{}", sub.get_name());
        write_recursive(out_dir, sub, &sub_page, count)?;
    }
    Ok(())
}
