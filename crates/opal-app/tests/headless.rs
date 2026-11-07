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

#[test]
fn a_script_gives_expressions_built_in_python_and_the_editor_works_them_out() {
    let python = std::env::var("OPAL_PYTHON").unwrap_or_else(|_| "python".to_owned());
    if !Command::new(&python).arg("--version").output().is_ok_and(|o| o.status.success()) {
        eprintln!("No Python found (set OPAL_PYTHON): expressions from Python were not tried.");
        return;
    }
    let scratch = Scratch(std::env::temp_dir().join(format!("opal-headless-expr-{}", std::process::id())));
    std::fs::create_dir_all(&scratch.0).unwrap();
    let script = scratch.0.join("expr.py");
    std::fs::write(&script, EXPR_SCRIPT).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_opal-studio"))
        .args(["--headless", "--python", &python])
        .arg(&script)
        .output()
        .expect("ran");
    let said = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(5), "{said}");
}

/// Builds expressions, checks their text (as the editor's parser reads it: parentheses
/// where, and only where, they are needed), and has the editor work some out.
const EXPR_SCRIPT: &str = r#"
import sys, opal
from opal import layer, canvas, box, fn, mm, inch, deg

def text(e, want):
    assert str(e) == want, f"{str(e)!r} is not {want!r}"

text(layer("Logo").right + mm(5), 'layer("Logo").right + 5mm')
text(layer('Say "hi" \\ there').top, 'layer("Say \\"hi\\" \\\\ there").top')
text(layer(12).center.x * 2, "layer(12).center.x * 2")
text((box.x - 1) - (box.width - 2), "x - 1 - (width - 2)")
text(box.x - (1 - box.width), "x - (1 - width)")
text((box.x + 1) * 2, "(x + 1) * 2")
text(-box.x ** 2, "-x^2")
text((-box.x) ** 2, "(-x)^2")
text(2 ** box.x ** 2, "2^x^2")
text((2 ** box.x) ** 2, "(2^x)^2")
text(box.width * -3, "width * -3")
text(inch(-1) / 4, "-1in / 4")
text(1.5 + box.y, "1.5 + y")
text(fn.mid(layer(3).center, canvas.center).x, "mid(layer(3).center, canvas.center).x")
text(fn.round(box.x, mm(1)), "round(x, 1mm)")
text(abs(box.x), "abs(x)")
text(round(box.angle), "round(angle)")
text(deg(90) - box.angle, "90deg - angle")
for bad in (float("inf"), True, "1in"):
    try:
        box.x + bad
    except (TypeError, ValueError):
        pass
    else:
        raise AssertionError(f"{bad!r} was taken into an expression")
try:
    bool(box.x)
except TypeError:
    pass
else:
    raise AssertionError("an expression had a truth value")

with opal.connect() as editor:
    # 2 in by 1 in at 144 ppi.
    editor.new(width=inch(2), height=inch(1), resolution=144)
    c = editor.crop(width=canvas.width - inch(0.5), height=box.height)
    assert (c["width"], c["height"]) == (216, 144), c
    # The background keeps its pixels past the canvas: it is 2 in wide still.
    c = editor.crop(width=layer("Background").width / 2 + 1, height="height / 2")
    assert (c["width"], c["height"]) == (145, 72), c
    try:
        editor.crop(width=layer("Nope").width)
    except opal.OpalError as e:
        assert "There is no layer called “Nope”" in str(e), str(e)
    else:
        raise AssertionError("an expression naming no layer was not refused")
sys.exit(5)
"#;
