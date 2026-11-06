//! A script run as `python script.py` with no editor running: the `opal` package starts one
//! with no window (`opal-studio --serve`), which ends with the script, its files written.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// A folder of the test's own, removed when it goes (whether the test passed or not).
struct Scratch(PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Runs `code` in Python with the package, no editor to find, and this build's program to
/// start; its exit status and what it printed. Fails if it has not ended within a minute.
fn run(python: &str, code: &str, scratch: &Path) -> (Option<i32>, String, String) {
    let package = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../python");
    let mut child = Command::new(python)
        .arg("-c")
        .arg(code)
        .env("PYTHONPATH", &package)
        .env("OPAL_STUDIO_API", scratch.join("no-editor.json"))
        .env("OPAL_STUDIO_EXE", env!("CARGO_BIN_EXE_opal-studio"))
        .current_dir(scratch)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("ran");
    let started = Instant::now();
    while child.try_wait().unwrap().is_none() {
        if started.elapsed() > Duration::from_secs(60) {
            let _ = child.kill();
            panic!("the script did not end (waiting for the editor it started?)");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let out = child.wait_with_output().unwrap();
    (out.status.code(), String::from_utf8_lossy(&out.stdout).into(), String::from_utf8_lossy(&out.stderr).into())
}

/// Whether process `pid` has ended, waiting a few seconds for it to.
fn ended(pid: u32) -> bool {
    for _ in 0..100 {
        let list = Command::new("tasklist").args(["/FI", &format!("PID eq {pid}"), "/NH"]).output().unwrap();
        if !String::from_utf8_lossy(&list.stdout).contains(&pid.to_string()) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    false
}

#[test]
fn a_script_with_no_editor_running_starts_one_that_ends_with_it() {
    let python = std::env::var("OPAL_PYTHON").unwrap_or_else(|_| "python".to_owned());
    if !Command::new(&python).arg("--version").output().is_ok_and(|o| o.status.success()) {
        eprintln!("No Python found (set OPAL_PYTHON): a script with no editor was not tried.");
        return;
    }
    let scratch = Scratch(std::env::temp_dir().join(format!("opal-serve-test-{}", std::process::id())));
    std::fs::create_dir_all(&scratch.0).unwrap();
    // Disconnected: the editor has ended, its files written, when that returns.
    let (code, out, err) = run(
        &python,
        "import opal\n\
         with opal.connect() as editor:\n\
         \x20   pid = editor._process.pid\n\
         \x20   editor.new(width='1in', height=10, resolution=40)\n\
         \x20   editor.export(path='one.png')\n\
         print(pid, editor._process)\n",
        &scratch.0,
    );
    assert_eq!(code, Some(0), "{err}");
    let mut said = out.split_whitespace();
    let pid: u32 = said.next().unwrap().parse().unwrap();
    assert_eq!(said.next(), Some("None"), "the process was kept after disconnecting");
    let header = std::fs::read(scratch.0.join("one.png")).unwrap_or_else(|_| panic!("not written: {err}"));
    assert_eq!(u32::from_be_bytes(header[16..20].try_into().unwrap()), 40);
    assert!(ended(pid), "the editor went on after the script disconnected");
    // Never disconnected, the script ending at once: the editor ends all the same.
    let (code, out, err) = run(
        &python,
        "import opal, os\n\
         editor = opal.connect()\n\
         editor.new(width=8, height=8)\n\
         print(editor._process.pid, os.getpid(), flush=True)\n\
         os._exit(0)\n",
        &scratch.0,
    );
    assert_eq!(code, Some(0), "{err}");
    let pids: Vec<u32> = out.split_whitespace().map(|p| p.parse().unwrap()).collect();
    assert!(ended(pids[0]), "the editor went on after the script had ended");
    // Told to use only a running editor: none is, so it says so.
    let (code, _, err) = run(&python, "import opal\nopal.connect(windowless=False)\n", &scratch.0);
    assert_ne!(code, Some(0));
    assert!(err.contains("not running"), "{err}");
    // No connection file of the editor it started is left behind (named after the script).
    let left: Vec<_> = std::fs::read_dir(std::env::temp_dir())
        .unwrap()
        .filter_map(Result::ok)
        .filter(|e| e.file_name().to_string_lossy().starts_with(&format!("opal-serve-{}-", pids[1])))
        .collect();
    assert!(left.is_empty(), "{left:?}");
}
