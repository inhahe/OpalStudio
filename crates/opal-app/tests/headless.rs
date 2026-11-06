//! `opal-studio --headless script.py`: a script run against an editor with no window, the
//! files it asks for written before the run ends with the script's exit code.

use std::process::Command;

#[test]
fn a_script_runs_headless_and_its_files_are_written_before_it_ends() {
    let python = std::env::var("OPAL_PYTHON").unwrap_or_else(|_| "python".to_owned());
    if !Command::new(&python).arg("--version").output().is_ok_and(|o| o.status.success()) {
        eprintln!("No Python found (set OPAL_PYTHON): headless mode was not tried.");
        return;
    }
    let dir = std::env::temp_dir().join(format!("opal-headless-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let script = dir.join("make.py");
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
    let png = dir.join("out.png");
    let out = Command::new(env!("CARGO_BIN_EXE_opal-studio"))
        .arg("--headless")
        .arg("--python")
        .arg(&python)
        .arg(&script)
        .arg(&png)
        .output()
        .expect("ran");
    let said = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(3), "the script's exit code: {said}");
    let header = std::fs::read(&png).unwrap_or_else(|_| panic!("not written: {said}"));
    let side = |at: usize| u32::from_be_bytes(header[at..at + 4].try_into().unwrap());
    assert_eq!((side(16), side(20)), (64, 32));
    // Without a script: how to call it.
    let usage = Command::new(env!("CARGO_BIN_EXE_opal-studio")).arg("--headless").output().unwrap();
    assert_eq!(usage.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&usage.stderr).contains("usage"));
    std::fs::remove_dir_all(&dir).ok();
}
