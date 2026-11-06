//! `opal-studio --headless script.py`: a script run against an editor with no window, the
//! files it asks for written before the run ends with the script's exit code; directly, and
//! through the console launcher a command prompt finds (`opal-studio.com`), which waits.

use std::path::PathBuf;
use std::process::Command;

/// A folder of the test's own, removed when it goes (whether the test passed or not).
struct Scratch(PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn a_script_runs_headless_and_its_files_are_written_before_it_ends() {
    let python = std::env::var("OPAL_PYTHON").unwrap_or_else(|_| "python".to_owned());
    if !Command::new(&python).arg("--version").output().is_ok_and(|o| o.status.success()) {
        eprintln!("No Python found (set OPAL_PYTHON): headless mode was not tried.");
        return;
    }
    let scratch = Scratch(std::env::temp_dir().join(format!("opal-headless-test-{}", std::process::id())));
    std::fs::create_dir_all(&scratch.0).unwrap();
    let script = scratch.0.join("make.py");
    std::fs::write(
        &script,
        "import sys, opal\n\
         with opal.connect() as editor:\n\
         \x20   editor.new(width=64, height=32)\n\
         \x20   editor.new_layer(name='Ink')\n\
         \x20   editor.export(path=sys.argv[1])\n\
         sys.exit(3)\n",
    )
    .unwrap();
    for (program, png) in [
        (env!("CARGO_BIN_EXE_opal-studio"), scratch.0.join("out.png")),
        (env!("CARGO_BIN_EXE_opal-studio-console"), scratch.0.join("through-the-launcher.png")),
    ] {
        let out = Command::new(program)
            .arg("--headless")
            .arg("--python")
            .arg(&python)
            .arg(&script)
            .arg(&png)
            .output()
            .expect("ran");
        let said = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(3), "the script's exit code, from {program}: {said}");
        let header = std::fs::read(&png).unwrap_or_else(|_| panic!("not written once {program} ended: {said}"));
        let side = |at: usize| u32::from_be_bytes(header[at..at + 4].try_into().unwrap());
        assert_eq!((side(16), side(20)), (64, 32));
        // Without a script: how to call it.
        let usage = Command::new(program).arg("--headless").output().unwrap();
        assert_eq!(usage.status.code(), Some(2));
        assert!(String::from_utf8_lossy(&usage.stderr).contains("usage"), "from {program}");
    }
    // A worker the script started, connected and still calling when the script ends: the
    // run waits for it.
    std::fs::write(
        scratch.0.join("worker.py"),
        "import sys, time, opal\n\
         editor = opal.connect(script=None)\n\
         editor.new(width=8, height=8)\n\
         print('connected', flush=True)\n\
         time.sleep(0.5)\n\
         editor.export(path=sys.argv[1])\n",
    )
    .unwrap();
    let starts = scratch.0.join("starts.py");
    std::fs::write(
        &starts,
        "import pathlib, subprocess, sys\n\
         worker = pathlib.Path(__file__).with_name('worker.py')\n\
         w = subprocess.Popen([sys.executable, str(worker), sys.argv[1]], stdout=subprocess.PIPE, text=True)\n\
         assert w.stdout.readline().strip() == 'connected'\n",
    )
    .unwrap();
    let png = scratch.0.join("by-the-worker.png");
    let out = Command::new(env!("CARGO_BIN_EXE_opal-studio"))
        .args(["--headless", "--python", &python])
        .arg(&starts)
        .arg(&png)
        .output()
        .expect("ran");
    let said = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "{said}");
    assert!(png.is_file(), "the worker's export was not written: {said}");
}
