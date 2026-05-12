//! `xtask gen-completions` — render shell completion scripts for the
//! `specify` binary. Output is gitignored at `target/completions/`
//! by default.

use std::fs;
use std::path::Path;

use clap_complete::Shell;

/// Render completion scripts for every [`clap_complete::Shell`] value
/// into `out_dir/<shell>/specify.<ext>`. Returns the number of files
/// written.
pub fn render(out_dir: &Path) -> std::io::Result<usize> {
    let mut cmd = specify::command();
    let bin_name = cmd.get_name().to_string();
    let mut count = 0_usize;
    for shell in [Shell::Bash, Shell::Elvish, Shell::Fish, Shell::PowerShell, Shell::Zsh] {
        let shell_dir = out_dir.join(shell.to_string());
        fs::create_dir_all(&shell_dir)?;
        let mut buffer = Vec::new();
        clap_complete::generate(shell, &mut cmd, &bin_name, &mut buffer);
        let file_name = completion_file_name(&bin_name, shell);
        fs::write(shell_dir.join(file_name), &buffer)?;
        count += 1;
    }
    Ok(count)
}

/// Per-shell completion file name. Mirrors the conventional name each
/// shell auto-loads from its completion search path.
fn completion_file_name(bin_name: &str, shell: Shell) -> String {
    match shell {
        Shell::Bash => format!("{bin_name}.bash"),
        Shell::Elvish => format!("{bin_name}.elv"),
        Shell::Fish => format!("{bin_name}.fish"),
        Shell::PowerShell => format!("_{bin_name}.ps1"),
        Shell::Zsh => format!("_{bin_name}"),
        _ => format!("{bin_name}.{shell}"),
    }
}
