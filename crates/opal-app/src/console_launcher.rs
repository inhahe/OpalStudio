//! `opal-studio.com`: Opal Studio from a command prompt (see design.md, *Scripting*).
//!
//! The editor is a windowed program, which a prompt (cmd, PowerShell) does not wait for, so
//! `opal-studio --headless script.py` would come back at once, before the script had run,
//! with no exit code to test. This small console program, which a prompt finds before the
//! `.exe` beside it (`.COM` comes first in `PATHEXT`), runs the editor with the same
//! arguments, on the same console: for `--headless` and `--mcp` it waits for it and ends
//! with its exit code; for the window, it starts it and ends at once.

use std::process::Command;

fn main() {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let Ok(launcher) = std::env::current_exe() else { std::process::exit(1) };
    let editor = launcher.with_file_name("opal-studio.exe");
    let waits = args.first().is_some_and(|a| a == "--headless" || a == "--mcp");
    let mut command = Command::new(&editor);
    command.args(&args);
    let code = if waits {
        match command.status() {
            Ok(status) => status.code().unwrap_or(1),
            Err(e) => {
                eprintln!("{} could not be started: {e}", editor.display());
                1
            }
        }
    } else {
        match command.spawn() {
            Ok(_) => 0,
            Err(e) => {
                eprintln!("{} could not be started: {e}", editor.display());
                1
            }
        }
    };
    std::process::exit(code)
}
