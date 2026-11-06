//! Editors with no window, for batch work (see design.md, *Scripting*):
//!
//! - `opal-studio --headless [--python PATH] script.py [ARGS…]` runs a Python script against
//!   one. The script talks to it as to a running editor, through the `opal` package and a
//!   connection file of this run's own; the run ends with the script, with its exit code,
//!   once the files it asked for are written.
//! - `opal-studio --serve FILE` serves one on connection file `FILE` until its standard
//!   input ends: what the `opal` package starts for a script run with no editor running,
//!   which so ends with the script's process however that ends.

use crate::server::Server;
use crate::state::AppState;
use eframe::egui;
use std::ffi::OsString;
use std::path::PathBuf;

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

/// The command line after `--headless`: the Python to use (if `--python` or `OPAL_PYTHON`
/// names one; otherwise the preference's), the script, and its arguments.
#[derive(Debug)]
struct Run {
    python: Option<OsString>,
    script: OsString,
    args: Vec<OsString>,
}

fn parse(mut given: Vec<OsString>) -> Result<Run, String> {
    let mut python = std::env::var_os("OPAL_PYTHON");
    if given.first().is_some_and(|a| a == "--python") {
        given.remove(0);
        if given.is_empty() {
            return Err("--python needs the Python to run the script with.".to_owned());
        }
        python = Some(given.remove(0));
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

/// How long an editor with no window waits for a call before it does again what is done
/// between frames, when nothing that will be answered is under way.
const IDLE: std::time::Duration = std::time::Duration::from_millis(50);

/// Serves `server`'s clients with `st` and no window until `done` (asked after each round)
/// says to stop: each call carried out as soon as it comes (waited for, not looked for
/// again and again), and between them what the window does between frames, every
/// millisecond or so while work a call waits for is under way, every [`IDLE`] otherwise.
pub fn serve_windowless(
    server: &mut Server,
    st: &mut AppState,
    ctx: &egui::Context,
    mut done: impl FnMut(&Server) -> bool,
) {
    loop {
        server.serve(st, ctx, false);
        st.between_frames(false);
        if done(server) {
            return;
        }
        let soon = st.work_under_way();
        server.wait(if soon { std::time::Duration::from_millis(1) } else { IDLE });
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
    let mut st = windowless_state();
    let file = std::env::temp_dir().join(format!("opal-headless-{}.json", std::process::id()));
    let mut server = match Server::start(file.clone(), ctx.clone()) {
        Ok(server) => server,
        Err(e) => {
            eprintln!("The connection for the script could not be opened: {e}");
            return 1;
        }
    };
    let python = run.python.as_ref().map_or_else(|| st.prefs.text("scripting.python"), |p| p.to_string_lossy().into());
    let mut child = match crate::console::script_command(&python, &run.script, &run.args, &file).spawn() {
        Ok(child) => child,
        Err(e) => {
            eprintln!("{python} could not be run: {e}");
            return 1;
        }
    };
    // Until the script has ended and every client of this run has gone (a process the
    // script started may go on calling), each call carried out: a client's connection lasts
    // until its last call is answered.
    let mut ended = None;
    serve_windowless(&mut server, &mut st, &ctx, |server| {
        if ended.is_none() {
            ended = child.try_wait().ok().flatten();
        }
        ended.is_some() && server.clients() == 0
    });
    let status = ended.expect("served until it ended");
    finish(st, server);
    status.code().unwrap_or(1)
}

/// The state of an editor with no window: the user's preferences loaded (how exports are
/// written, the memory budgets), not saved back.
fn windowless_state() -> AppState {
    let mut st = AppState::new();
    st.windowless = true;
    crate::config::load(&mut st);
    st.apply_tile_budget();
    st
}

/// Ends an editor with no window: once the files asked for are written, it tells what a
/// dialog would have, and stops serving (its connection file goes).
fn finish(mut st: AppState, server: Server) {
    while st.writer.busy() {
        st.poll_writes(false);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    st.poll_writes(false);
    for m in st.messages.drain(..) {
        eprintln!("{}: {}", m.title, m.body);
    }
    drop(server);
}

/// What `--serve` prints on its standard output once it serves, for the process that
/// started it to wait for.
pub const SERVING: &str = "serving";

/// Serves an editor with no window, with the arguments after `--serve` (the connection
/// file), until standard input ends and every client has gone; the process's exit code.
pub fn serve(given: Vec<OsString>) -> i32 {
    let [file] = &given[..] else {
        eprintln!("usage: opal-studio --serve FILE (it serves until its standard input ends)");
        return 2;
    };
    let ctx = egui::Context::default();
    let mut st = windowless_state();
    let mut server = match Server::start(PathBuf::from(file), ctx.clone()) {
        Ok(server) => server,
        Err(e) => {
            eprintln!("The connection could not be opened: {e}");
            return 1;
        }
    };
    // Its standard input ends when the process that started it does (or lets it go).
    let ended = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let told = ended.clone();
    std::thread::spawn(move || {
        let mut buf = [0u8; 256];
        while std::io::Read::read(&mut std::io::stdin(), &mut buf).is_ok_and(|n| n > 0) {}
        told.store(true, std::sync::atomic::Ordering::Relaxed);
    });
    {
        use std::io::Write;
        let mut out = std::io::stdout();
        if writeln!(out, "{SERVING}").and_then(|()| out.flush()).is_err() {
            // No one to tell, nor to serve.
            return 1;
        }
    }
    serve_windowless(&mut server, &mut st, &ctx, |server| {
        ended.load(std::sync::atomic::Ordering::Relaxed) && server.clients() == 0
    });
    finish(st, server);
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_command_line_names_the_python_the_script_and_its_arguments() {
        let run = parse(vec!["--python".into(), "py3".into(), "go.py".into(), "a".into(), "--b".into()]).unwrap();
        assert_eq!(
            (run.python, run.script, run.args),
            (Some("py3".into()), "go.py".into(), vec!["a".into(), "--b".into()])
        );
        assert!(parse(vec![]).unwrap_err().contains("usage"));
        assert!(parse(vec!["--python".into()]).unwrap_err().contains("needs"));
        assert!(package_dir().is_some_and(|d| d.join("opal").join("__init__.py").is_file()));
    }
}
