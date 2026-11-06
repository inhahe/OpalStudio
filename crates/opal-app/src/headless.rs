//! `opal-studio --headless [--python PATH] script.py [ARGS…]`: runs a Python script against
//! an editor with no window, for batch work (see design.md, *Scripting*). The script talks
//! to it as to a running editor, through the `opal` package and a connection file of this
//! run's own; the run ends with the script, with its exit code, once the files it asked for
//! are written.

use crate::server::Server;
use crate::state::AppState;
use eframe::egui;
use std::ffi::OsString;
use std::path::PathBuf;
use std::process::Command;

/// Where the `opal` package is: beside the executable (as the folder it is built into holds
/// it), or in the source tree it was built from.
pub fn package_dir() -> Option<PathBuf> {
    let beside = std::env::current_exe().ok()?.parent()?.join("python");
    if beside.join("opal").is_dir() {
        return Some(beside);
    }
    let source = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../python");
    source.join("opal").is_dir().then_some(source)
}

/// The command line after `--headless`: the Python to use, the script, and its arguments.
#[derive(Debug)]
struct Run {
    python: OsString,
    script: OsString,
    args: Vec<OsString>,
}

fn parse(mut given: Vec<OsString>) -> Result<Run, String> {
    let mut python = std::env::var_os("OPAL_PYTHON").unwrap_or_else(|| "python".into());
    if given.first().is_some_and(|a| a == "--python") {
        given.remove(0);
        if given.is_empty() {
            return Err("--python needs the Python to run the script with.".to_owned());
        }
        python = given.remove(0);
    }
    if given.is_empty() {
        return Err("usage: opal-studio --headless [--python PATH] script.py [arguments…]".to_owned());
    }
    let script = given.remove(0);
    Ok(Run { python, script, args: given })
}

/// Lets this process write to the terminal it was started from (a release build has no
/// console of its own), and so the script, which shares it.
fn use_parent_console() {
    #[cfg(windows)]
    // SAFETY: AttachConsole has no preconditions; it fails harmlessly when there is no
    // parent console, or one is attached already.
    unsafe {
        let _ = windows::Win32::System::Console::AttachConsole(windows::Win32::System::Console::ATTACH_PARENT_PROCESS);
    }
}

/// Runs headless with the arguments after `--headless`; the process's exit code.
pub fn run(given: Vec<OsString>) -> i32 {
    use_parent_console();
    let run = match parse(given) {
        Ok(run) => run,
        Err(why) => {
            eprintln!("{why}");
            return 2;
        }
    };
    let ctx = egui::Context::default();
    let mut st = AppState::new();
    // The user's preferences (how exports are written, the memory budgets), not saved back.
    crate::config::load(&mut st);
    st.apply_tile_budget();
    let file = std::env::temp_dir().join(format!("opal-headless-{}.json", std::process::id()));
    let mut server = match Server::start(file.clone(), ctx.clone()) {
        Ok(server) => server,
        Err(e) => {
            eprintln!("The connection for the script could not be opened: {e}");
            return 1;
        }
    };
    let mut command = Command::new(&run.python);
    command.arg(&run.script).args(&run.args).env("OPAL_STUDIO_API", &file);
    if let Some(package) = package_dir() {
        let mut path = std::env::split_paths(&std::env::var_os("PYTHONPATH").unwrap_or_default()).collect::<Vec<_>>();
        path.insert(0, package);
        if let Ok(joined) = std::env::join_paths(path) {
            command.env("PYTHONPATH", joined);
        }
    }
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(e) => {
            eprintln!("{} could not be run: {e}", run.python.to_string_lossy());
            return 1;
        }
    };
    let status = loop {
        server.serve(&mut st, &ctx, false);
        st.poll_writes(false);
        st.poll_copies();
        if let Ok(Some(status)) = child.try_wait() {
            break status;
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    };
    // The files asked for, written before this ends.
    while st.writer.busy() {
        st.poll_writes(false);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    st.poll_writes(false);
    // What would have been told in a dialog.
    for m in st.messages.drain(..) {
        eprintln!("{}: {}", m.title, m.body);
    }
    drop(server);
    status.code().unwrap_or(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_command_line_names_the_python_the_script_and_its_arguments() {
        let run = parse(vec!["--python".into(), "py3".into(), "go.py".into(), "a".into(), "--b".into()]).unwrap();
        assert_eq!((run.python, run.script, run.args), ("py3".into(), "go.py".into(), vec!["a".into(), "--b".into()]));
        assert!(parse(vec![]).unwrap_err().contains("usage"));
        assert!(parse(vec!["--python".into()]).unwrap_err().contains("needs"));
        assert!(package_dir().is_some_and(|d| d.join("opal").join("__init__.py").is_file()));
    }
}
