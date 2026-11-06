//! Run Script and the Console panel (see design.md, *Scripting*): a Python script the editor
//! runs, in the Python the preferences name, connected to this editor as any script is;
//! what it prints is shown as it comes.

use crate::commands::Cmd;
use crate::icons;
use crate::state::{AppState, MessageKind};
use eframe::egui::{self, RichText};
use std::collections::HashMap;
use std::collections::VecDeque;
use std::ffi::OsStr;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How many scripts run lately the console remembers.
pub const RECENT: usize = 10;

/// The most lines the console keeps; older ones go.
const MAX_LINES: usize = 5000;

/// How long, once the script has ended, what it printed is waited for (a process it started
/// may hold its output open after it).
const OUTPUT_AFTER: Duration = Duration::from_secs(1);

/// Who said a line of the console.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Said {
    /// The script, on its standard output.
    Output,
    /// The script, on its standard error: its errors and tracebacks.
    Error,
    /// The editor: that a script started or ended.
    Editor,
}

#[derive(Clone, Debug)]
pub struct Line {
    pub said: Said,
    pub text: String,
}

/// The script running.
struct Running {
    /// Its file's name.
    name: String,
    child: Child,
    /// Its lines as they come; `None` when one of its two streams ends.
    lines: Receiver<Option<(Said, String)>>,
    /// How many of its streams are still open.
    open: usize,
    started: Instant,
    /// How it ended, and when that was seen, once it has.
    ended: Option<(ExitStatus, Instant)>,
    /// Whether the user stopped it.
    stopped: bool,
    /// It and the processes it starts, to be ended together.
    tree: Tree,
}

impl Running {
    /// Ends the script and every process it started.
    fn end(&mut self) {
        self.tree.end();
        let _ = self.child.kill();
    }
}

impl Drop for Running {
    /// A script the editor started does not outlive it, while it runs (once it has ended by
    /// itself, a process it left running goes on).
    fn drop(&mut self) {
        if matches!(self.child.try_wait(), Ok(None)) {
            self.end();
            let _ = self.child.wait();
        }
    }
}

/// A script's process and those it starts (on Windows, a job object: what a process starts
/// joins its job); elsewhere, the script alone.
struct Tree {
    #[cfg(windows)]
    job: Option<windows::Win32::Foundation::HANDLE>,
}

impl Tree {
    /// The tree `child` heads, just started. (A process it starts in the moment before it is
    /// put in the job is not in it: a script has hardly begun by then.)
    fn of(child: &Child) -> Tree {
        #[cfg(windows)]
        {
            use std::os::windows::io::AsRawHandle;
            use windows::Win32::System::JobObjects::{AssignProcessToJobObject, CreateJobObjectW};
            // SAFETY: a new job of the default security, and the child's own handle, open
            // while `child` is; the job is closed if it cannot be given the child.
            let job = unsafe {
                CreateJobObjectW(None, windows::core::PCWSTR::null()).ok().filter(|&job| {
                    let assigned =
                        AssignProcessToJobObject(job, windows::Win32::Foundation::HANDLE(child.as_raw_handle()))
                            .is_ok();
                    if !assigned {
                        let _ = windows::Win32::Foundation::CloseHandle(job);
                    }
                    assigned
                })
            };
            Tree { job }
        }
        #[cfg(not(windows))]
        {
            let _ = child;
            Tree {}
        }
    }

    /// Ends every process of the tree (the script's own is ended by its caller too).
    fn end(&self) {
        #[cfg(windows)]
        if let Some(job) = self.job {
            // SAFETY: the job is this tree's own, open until it is dropped.
            unsafe {
                let _ = windows::Win32::System::JobObjects::TerminateJobObject(job, 1);
            }
        }
    }
}

impl Drop for Tree {
    /// Lets go of the job (which, made without a limit, ends nothing by going).
    fn drop(&mut self) {
        #[cfg(windows)]
        if let Some(job) = self.job {
            // SAFETY: as in `end`; not used again.
            unsafe {
                let _ = windows::Win32::Foundation::CloseHandle(job);
            }
        }
    }
}

/// The console: what scripts printed, and the script running, if one is.
#[derive(Default)]
pub struct Console {
    lines: VecDeque<Line>,
    running: Option<Running>,
    /// The scripts run lately, the last first, each once: at most [`RECENT`], kept between
    /// sessions (see `config`).
    pub recent: Vec<PathBuf>,
    /// Whether each of them was there when last looked (see [`Console::there`]).
    seen: Arc<Mutex<HashMap<PathBuf, bool>>>,
    /// Whether they are being looked at, and when they last were.
    looking: Arc<AtomicBool>,
    looked: Option<Instant>,
}

/// How often the recent scripts are looked at again, while the Console panel is shown.
const LOOK_EVERY: Duration = Duration::from_secs(5);

/// A command running `script` (with `args`) in `python` (see [`python_program`]),
/// connected to the editor whose connection file is `api`, with the `opal` package on its
/// path, its output unbuffered and in UTF-8.
pub fn script_command(python: &str, script: &OsStr, args: &[std::ffi::OsString], api: &Path) -> Command {
    let (program, python_args) = python_program(python);
    let mut command = Command::new(program);
    command
        .args(python_args)
        .arg(script)
        .args(args)
        .env("OPAL_STUDIO_API", api)
        .env("PYTHONUNBUFFERED", "1")
        .env("PYTHONIOENCODING", "utf-8");
    if let Some(package) = crate::headless::package_dir()
        && let Some(path) = python_path(package, std::env::var_os("PYTHONPATH").as_deref())
    {
        command.env("PYTHONPATH", path);
    }
    command
}

/// The program and its arguments that `python` (the preference, or `--python`) names: a
/// path, in quotes or not (as Explorer's Copy as Path gives one), or a command with
/// arguments (`py -3.12`).
pub fn python_program(python: &str) -> (String, Vec<String>) {
    let python = python.trim();
    if let Some(rest) = python.strip_prefix('"')
        && let Some((program, args)) = rest.split_once('"')
    {
        return (program.to_owned(), args.split_whitespace().map(str::to_owned).collect());
    }
    if Path::new(python).is_file() {
        return (python.to_owned(), Vec::new());
    }
    let mut words = python.split_whitespace();
    (words.next().unwrap_or("python").to_owned(), words.map(str::to_owned).collect())
}

/// `PYTHONPATH` with `package` first, before `existing`'s folders. An empty one is left
/// out: Python reads it as the current folder, ahead of its own library, where a script's
/// `json.py` would hide the real one.
fn python_path(package: PathBuf, existing: Option<&OsStr>) -> Option<std::ffi::OsString> {
    let rest = existing.map(std::env::split_paths).into_iter().flatten().filter(|p| !p.as_os_str().is_empty());
    std::env::join_paths(std::iter::once(package).chain(rest)).ok()
}

impl Console {
    pub fn lines(&self) -> &VecDeque<Line> {
        &self.lines
    }

    /// The name of the script running, if one is.
    pub fn running(&self) -> Option<&str> {
        self.running.as_ref().map(|r| r.name.as_str())
    }

    fn say(&mut self, said: Said, text: String) {
        if self.lines.len() == MAX_LINES {
            self.lines.pop_front();
        }
        self.lines.push_back(Line { said, text });
    }

    pub fn clear(&mut self) {
        self.lines.clear();
    }

    /// Runs `script` with `python` (a command, or a path), in the script's folder,
    /// connected to the editor whose connection file is `api`. Nothing else may be running.
    pub fn run(&mut self, script: &Path, python: &str, api: &Path, ctx: &egui::Context) -> Result<(), String> {
        assert!(self.running.is_none(), "a script is running already");
        let name = file_name(script);
        let mut command = script_command(python, script.as_os_str(), &[], api);
        command.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
        if let Some(folder) = script.parent().filter(|f| !f.as_os_str().is_empty()) {
            command.current_dir(folder);
        }
        #[cfg(windows)]
        {
            // No console window of its own: what it prints shows here.
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            std::os::windows::process::CommandExt::creation_flags(&mut command, CREATE_NO_WINDOW);
        }
        let mut child = command.spawn().map_err(|e| {
            format!("\"{python}\" could not be started: {e}. Preferences › Scripting names the Python to use.")
        })?;
        let tree = Tree::of(&child);
        let (send, lines) = channel();
        let streams: [(Box<dyn Read + Send>, Said); 2] = [
            (Box::new(child.stdout.take().expect("piped")), Said::Output),
            (Box::new(child.stderr.take().expect("piped")), Said::Error),
        ];
        for (stream, said) in streams {
            let (out, ctx) = (send.clone(), ctx.clone());
            let read = move || {
                let mut reader = BufReader::new(stream);
                let mut bytes = Vec::new();
                while reader.read_until(b'\n', &mut bytes).is_ok_and(|n| n > 0) {
                    let text = String::from_utf8_lossy(&bytes).trim_end_matches(['\r', '\n']).to_owned();
                    bytes.clear();
                    if out.send(Some((said, text))).is_err() {
                        return;
                    }
                    ctx.request_repaint();
                }
                let _ = out.send(None);
                ctx.request_repaint();
            };
            if std::thread::Builder::new().name("script output".into()).spawn(read).is_err() {
                let _ = send.send(None);
            }
        }
        self.say(Said::Editor, format!("Running {name}."));
        self.running =
            Some(Running { name, child, lines, open: 2, started: Instant::now(), ended: None, stopped: false, tree });
        self.remember(script);
        Ok(())
    }

    /// Puts `script` first among the recent ones.
    fn remember(&mut self, script: &Path) {
        self.recent.retain(|p| p != script);
        self.recent.insert(0, script.to_owned());
        self.recent.truncate(RECENT);
        self.looked = None;
    }

    /// Forgets the recent scripts.
    pub fn forget_recent(&mut self) {
        self.recent.clear();
        self.looked = None;
    }

    /// Whether `script` was there when the recent scripts were last looked at (one not yet
    /// looked at is taken to be).
    pub fn there(&self, script: &Path) -> bool {
        self.seen.lock().expect("not poisoned").get(script).copied().unwrap_or(true)
    }

    /// Looks whether the recent scripts are there, if they were not looked at lately, on a
    /// thread of its own: a script on a share that does not answer would otherwise hold
    /// the window up while asked about.
    pub fn look_at_recent(&mut self, ctx: &egui::Context) {
        // Looked at again while the panel shows, even with nothing else to draw it.
        ctx.request_repaint_after(LOOK_EVERY);
        if self.looking.load(Ordering::Relaxed) || self.looked.is_some_and(|t| t.elapsed() < LOOK_EVERY) {
            return;
        }
        self.looked = Some(Instant::now());
        self.looking.store(true, Ordering::Relaxed);
        let (recent, seen, looking, ctx) = (self.recent.clone(), self.seen.clone(), self.looking.clone(), ctx.clone());
        let look = move || {
            let found: HashMap<PathBuf, bool> = recent
                .into_iter()
                .map(|p| {
                    let there = p.is_file();
                    (p, there)
                })
                .collect();
            *seen.lock().expect("not poisoned") = found;
            looking.store(false, Ordering::Relaxed);
            ctx.request_repaint();
        };
        if std::thread::Builder::new().name("recent scripts".into()).spawn(look).is_err() {
            self.looking.store(false, Ordering::Relaxed);
        }
    }

    /// Takes what the script has printed since asked last; once it has ended (and what it
    /// printed has all come), says how.
    pub fn poll(&mut self, ctx: &egui::Context) {
        let Some(r) = &mut self.running else { return };
        let mut got = Vec::new();
        while let Ok(line) = r.lines.try_recv() {
            match line {
                Some(line) => got.push(line),
                None => r.open -= 1,
            }
        }
        if r.ended.is_none()
            && let Ok(Some(status)) = r.child.try_wait()
        {
            r.ended = Some((status, Instant::now()));
        }
        let done = r.ended.filter(|(_, seen)| r.open == 0 || seen.elapsed() >= OUTPUT_AFTER);
        // Its end is looked for even if it prints nothing.
        ctx.request_repaint_after(Duration::from_millis(250));
        for (said, text) in got {
            self.say(said, text);
        }
        if let Some((status, _)) = done {
            let r = self.running.take().expect("running");
            let how = match status.code() {
                _ if r.stopped => " (stopped)".to_owned(),
                Some(0) => String::new(),
                Some(code) => format!(" with exit code {code}"),
                None => " (stopped by the system)".to_owned(),
            };
            let took = r.started.elapsed().as_secs_f64();
            self.say(Said::Editor, format!("{} ended{how}, after {took:.1} s.", r.name));
        }
    }

    /// Stops the script running, at once; what it has done so far stays.
    /// Stops the script running, and every process it started, at once; what they have done
    /// so far stays.
    pub fn stop(&mut self) {
        if let Some(r) = &mut self.running
            && r.ended.is_none()
        {
            r.stopped = true;
            r.end();
        }
    }

    /// Everything shown, as text, for the clipboard.
    fn text(&self) -> String {
        self.lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>().join("\n")
    }
}

impl AppState {
    /// Runs `script` (Run Script, or Run Again), telling the user if it cannot be.
    pub fn run_script(&mut self, script: &Path, ctx: &egui::Context) {
        let Some(api) = self.scripts_file.clone() else {
            let why = self.scripts_unserved.clone().unwrap_or_else(|| NOT_SERVING.to_owned());
            self.message(MessageKind::Info, "Scripts cannot run here", why);
            return;
        };
        let python = self.prefs.text("scripting.python");
        let recent = self.console.recent.clone();
        match self.console.run(script, &python, &api, ctx) {
            Ok(()) => {
                // The recent scripts are kept with the settings.
                if self.console.recent != recent {
                    self.settings_touched();
                }
                self.show_console = true;
                // The user's own doing: if they had shut scripts out, this one is let in.
                self.let_scripts_in = true;
            }
            Err(why) => self.message(MessageKind::Error, "The script could not be run", why),
        }
    }

    /// Run Script: asks for the script, then runs it.
    pub(crate) fn run_script_dialog(&mut self, ctx: &egui::Context) {
        let picked =
            rfd::FileDialog::new().set_title("Run Script").add_filter("Python scripts", &["py", "pyw"]).pick_file();
        if let Some(script) = picked {
            self.run_script(&script, ctx);
        }
    }
}

/// The Python to try scripts with in tests, if there is one (`OPAL_PYTHON`, or `python`).
#[cfg(test)]
pub fn test_python() -> Option<String> {
    let python = std::env::var("OPAL_PYTHON").unwrap_or_else(|_| "python".to_owned());
    Command::new(&python).arg("--version").output().is_ok_and(|o| o.status.success()).then_some(python)
}

/// Why Run Script is not available in an editor that does not serve scripts.
pub const NOT_SERVING: &str = "Another Opal Studio was open first, and scripts connect to that one: run them there.";

use crate::state::file_title as file_name;

/// The scripts run lately, to run one again (if `can_run`), and a way to forget them.
fn recent_menu(ui: &mut egui::Ui, st: &mut AppState, can_run: bool) {
    ui.menu_button(format!("{}  Recent", icons::HISTORY), |ui| {
        let recent = st.console.recent.clone();
        for script in &recent {
            let there = st.console.there(script);
            // Two of one name (in two folders) told apart by their folder.
            let name = file_name(script);
            let twice = recent.iter().filter(|p| file_name(p) == name).count() > 1;
            let folder = script.parent().map(|f| f.display().to_string()).unwrap_or_default();
            let label = if twice { format!("{name}  ({folder})") } else { name };
            let item = ui.add_enabled(can_run && there, egui::Button::new(label));
            let shown = script.display().to_string();
            let item = if there {
                item.on_hover_text(shown)
            } else {
                item.on_disabled_hover_text(format!("No longer there: {shown}"))
            };
            if item.clicked() {
                st.run_script(script, ui.ctx());
                ui.close();
            }
        }
        ui.separator();
        if ui.button("Forget them").clicked() {
            st.console.forget_recent();
            st.settings_touched();
            ui.close();
        }
    })
    .response
    .on_hover_text("The scripts run lately, to run one again.");
}

/// The Console panel: the script running (and a way to stop it), or ways to run one; then
/// what scripts printed.
pub fn console_panel(ui: &mut egui::Ui, st: &mut AppState) {
    let weak = st.palette.weak_text;
    // Wrapped: in a narrow panel, a long script name would push the rest past its edge.
    ui.horizontal_wrapped(|ui| {
        match st.console.running() {
            Some(name) => {
                ui.label(RichText::new(format!("{}  {name} is running", icons::PLAY)).color(st.palette.strong_text));
                if ui
                    .button(format!("{}  Stop", icons::STOP))
                    .on_hover_text("End the script now. What it has done so far stays.")
                    .clicked()
                {
                    st.console.stop();
                }
            }
            None => {
                let available = crate::commands::availability(st, Cmd::RunScript);
                let run =
                    ui.add_enabled(available.is_ok(), egui::Button::new(format!("{}  Run Script…", icons::SCRIPT)));
                let run = match &available {
                    Ok(()) => run.on_hover_text(Cmd::RunScript.info().description),
                    Err(why) => run.on_disabled_hover_text(why),
                };
                if run.clicked() {
                    st.queue.push(Cmd::RunScript);
                }
                st.console.look_at_recent(ui.ctx());
                if let Some(last) = st.console.recent.first().cloned() {
                    let there = st.console.there(&last);
                    let shown = last.display().to_string();
                    let again = ui
                        .add_enabled(
                            available.is_ok() && there,
                            egui::Button::new(format!("{}  Run {} again", icons::PLAY, file_name(&last))),
                        )
                        .on_hover_text(&shown)
                        .on_disabled_hover_text(match &available {
                            Err(why) => why.clone(),
                            Ok(()) => format!("No longer there: {shown}"),
                        });
                    if again.clicked() {
                        st.run_script(&last, ui.ctx());
                    }
                    recent_menu(ui, st, available.is_ok());
                }
            }
        }
        let some = !st.console.lines().is_empty();
        if ui
            .add_enabled(some, egui::Button::new(format!("{}  Copy", icons::COPY)))
            .on_hover_text("Copy everything shown, to paste into a bug report or an editor.")
            .clicked()
        {
            ui.ctx().copy_text(st.console.text());
        }
        if ui.add_enabled(some, egui::Button::new(format!("{}  Clear", icons::BROOM))).clicked() {
            st.console.clear();
        }
    });
    ui.separator();
    let lines = st.console.lines();
    if lines.is_empty() {
        ui.label(
            RichText::new(
                "What scripts print shows here. Run Script (File › Scripts) runs one; scripts started \
                           elsewhere print where they were started.",
            )
            .color(weak),
        );
        return;
    }
    let error = ui.visuals().error_fg_color;
    let row = ui.text_style_height(&egui::TextStyle::Monospace) + ui.spacing().item_spacing.y;
    egui::ScrollArea::both().stick_to_bottom(true).auto_shrink([false; 2]).show_rows(
        ui,
        row,
        lines.len(),
        |ui, rows| {
            for line in lines.range(rows) {
                let text = RichText::new(&line.text).monospace();
                let text = match line.said {
                    Said::Output => text,
                    Said::Error => text.color(error),
                    Said::Editor => text.color(weak).italics(),
                };
                ui.add(egui::Label::new(text).wrap_mode(egui::TextWrapMode::Extend).selectable(true));
            }
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    use super::test_python as python;

    /// A folder of a test's own for its scripts, removed when it goes.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(test: &str) -> Scratch {
            let dir = std::env::temp_dir().join(format!("opal-console-{}-{test}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            Scratch(dir)
        }

        fn script(&self, name: &str, text: &str) -> PathBuf {
            let path = self.0.join(name);
            std::fs::write(&path, text).unwrap();
            path
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn a_python_is_a_path_quoted_or_not_or_a_command_with_arguments() {
        let words = |p: &str| python_program(p);
        assert_eq!(words("py -3.12"), ("py".to_owned(), vec!["-3.12".to_owned()]));
        assert_eq!(
            words(r#""C:\Program Files\Python\python.exe""#),
            (r"C:\Program Files\Python\python.exe".to_owned(), vec![])
        );
        assert_eq!(
            words(r#""C:\Py\python.exe" -X utf8"#),
            (r"C:\Py\python.exe".to_owned(), vec!["-X".to_owned(), "utf8".to_owned()])
        );
        let here = std::env::current_exe().unwrap().display().to_string();
        assert_eq!(words(&here), (here.clone(), vec![]), "a path with spaces, not split");
        let package = PathBuf::from("pkg");
        let sep = if cfg!(windows) { ";" } else { ":" };
        assert_eq!(python_path(package.clone(), None), Some("pkg".into()));
        assert_eq!(python_path(package.clone(), Some(OsStr::new(""))), Some("pkg".into()), "an empty folder added");
        assert_eq!(
            python_path(package, Some(OsStr::new(&format!("a{sep}{sep}b")))),
            Some(format!("pkg{sep}a{sep}b").into())
        );
    }

    /// Polls `console` until its script has ended (at most 30 s).
    fn until_ended(console: &mut Console) {
        let ctx = egui::Context::default();
        let until = Instant::now() + Duration::from_secs(30);
        while console.running().is_some() && Instant::now() < until {
            console.poll(&ctx);
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(console.running().is_none(), "the script did not end");
    }

    #[test]
    fn what_a_script_prints_is_shown_and_how_it_ended() {
        let Some(python) = python() else { return eprintln!("No Python found (set OPAL_PYTHON).") };
        let scratch = Scratch::new("speak");
        let path = scratch.script(
            "speak.py",
            "import os, sys\nprint('hello', os.path.basename(os.getcwd()))\n\
             print('oops', file=sys.stderr)\nprint('é')\nsys.exit(3)\n",
        );
        let mut console = Console::default();
        console.run(&path, &python, Path::new("unused.json"), &egui::Context::default()).unwrap();
        assert!(console.running().is_some());
        until_ended(&mut console);
        let folder = path.parent().unwrap().file_name().unwrap().to_string_lossy().into_owned();
        let said = |s: Said| console.lines().iter().filter(|l| l.said == s).map(|l| l.text.clone()).collect::<Vec<_>>();
        // Run in its own folder; printed in UTF-8.
        assert_eq!(said(Said::Output), [format!("hello {folder}"), "é".to_owned()]);
        assert_eq!(said(Said::Error), ["oops"]);
        let told = said(Said::Editor);
        assert!(told.last().is_some_and(|l| l.contains("speak.py ended with exit code 3")), "{told:?}");
        assert_eq!(console.recent, std::slice::from_ref(&path));
    }

    #[test]
    fn a_script_can_be_stopped() {
        let Some(python) = python() else { return eprintln!("No Python found (set OPAL_PYTHON).") };
        let scratch = Scratch::new("forever");
        let path = scratch.script("forever.py", "import time\nprint('started', flush=True)\ntime.sleep(60)\n");
        let mut console = Console::default();
        console.run(&path, &python, Path::new("unused.json"), &egui::Context::default()).unwrap();
        let started = Instant::now();
        console.stop();
        until_ended(&mut console);
        assert!(started.elapsed() < Duration::from_secs(10));
        let last = &console.lines().back().unwrap().text;
        assert!(last.contains("forever.py ended (stopped)"), "{last}");
    }

    #[test]
    fn run_script_runs_one_connected_to_this_editor_as_one_step_of_history() {
        let Some(python) = python() else { return eprintln!("No Python found (set OPAL_PYTHON).") };
        let file = std::env::temp_dir().join(format!("opal-console-api-{}.json", std::process::id()));
        let mut server = crate::server::Server::start(file.clone(), egui::Context::default()).unwrap();
        let mut st = AppState::new();
        st.prefs.set("scripting.python", crate::settings::Value::Text(python));
        assert!(crate::commands::availability(&st, Cmd::RunScript).is_err(), "available without a server");
        st.scripts_file = Some(file);
        let scratch = Scratch::new("draw");
        let path = scratch.script(
            "draw.py",
            "import opal\nwith opal.connect() as e:\n    e.new(width=8, height=8)\n    \
             e.new_layer(name='A')\n    e.new_layer(name='B')\nprint('done')\n",
        );
        let ctx = egui::Context::default();
        st.run_script(&path, &ctx);
        assert!(st.show_console && st.console.running().is_some(), "{:?}", st.messages);
        let err = crate::commands::availability(&st, Cmd::RunScript).unwrap_err();
        assert!(err.contains("draw.py is running"), "{err}");
        let joined = |st: &AppState| st.docs.first().is_some_and(|d| d.history.undo_label() == Some("Script: draw.py"));
        let until = Instant::now() + Duration::from_secs(30);
        while (st.console.running().is_some() || !joined(&st)) && Instant::now() < until {
            server.serve(&mut st, &ctx, false);
            st.console.poll(&ctx);
            std::thread::sleep(Duration::from_millis(2));
        }
        let printed: Vec<&str> = st.console.lines().iter().map(|l| l.text.as_str()).collect();
        assert!(printed.contains(&"done"), "{printed:?}");
        assert!(joined(&st), "not one step: {:?}", st.docs.first().map(|d| d.history.undo_label()));
    }

    #[cfg(windows)]
    #[test]
    fn stopping_a_script_ends_what_it_started_too() {
        let Some(python) = python() else { return eprintln!("No Python found (set OPAL_PYTHON).") };
        let scratch = Scratch::new("tree");
        let beat = scratch.0.join("beat.txt");
        scratch.script(
            "worker.py",
            "import os, sys, time\nopen(sys.argv[2], 'w').write(str(os.getpid()))\n\
             while True:\n    open(sys.argv[1], 'a').write('.')\n    time.sleep(0.05)\n",
        );
        let path = scratch.script(
            "starts.py",
            "import pathlib, subprocess, sys, time\nhere = pathlib.Path(__file__).parent\n\
             subprocess.Popen([sys.executable, str(here / 'worker.py'), str(here / 'beat.txt'), str(here / 'pid.txt')])\n\
             print('started', flush=True)\ntime.sleep(60)\n",
        );
        let mut console = Console::default();
        console.run(&path, &python, Path::new("unused.json"), &egui::Context::default()).unwrap();
        let until = Instant::now() + Duration::from_secs(20);
        while !beat.is_file() && Instant::now() < until {
            console.poll(&egui::Context::default());
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(beat.is_file(), "the worker never began");
        console.stop();
        until_ended(&mut console);
        std::thread::sleep(Duration::from_millis(300));
        let size = || std::fs::metadata(&beat).map_or(0, |m| m.len());
        let before = size();
        std::thread::sleep(Duration::from_millis(500));
        let alive = size() != before;
        if alive {
            // Not left running whatever the test finds.
            let pid = std::fs::read_to_string(scratch.0.join("pid.txt")).unwrap_or_default();
            let _ = Command::new("taskkill").args(["/F", "/PID", pid.trim()]).output();
        }
        assert!(!alive, "what the script started went on after Stop");
    }

    #[test]
    fn the_scripts_run_lately_are_kept_the_last_first_each_once() {
        let mut console = Console::default();
        let at = |i: usize| PathBuf::from(format!("C:/scripts/{i}.py"));
        for i in 0..RECENT + 3 {
            console.remember(&at(i));
        }
        console.remember(&at(RECENT));
        assert_eq!(console.recent.len(), RECENT);
        assert_eq!(console.recent[..3], [at(RECENT), at(RECENT + 2), at(RECENT + 1)]);
        assert_eq!(console.recent.iter().filter(|p| **p == at(RECENT)).count(), 1);
    }

    #[test]
    fn whether_the_recent_scripts_are_there_is_looked_at_off_this_thread() {
        let scratch = Scratch::new("seen");
        let here = scratch.script("here.py", "");
        let gone = scratch.0.join("gone.py");
        let mut console = Console::default();
        console.remember(&gone);
        console.remember(&here);
        assert!(console.there(&gone), "taken to be there before it is looked at");
        console.look_at_recent(&egui::Context::default());
        let until = Instant::now() + Duration::from_secs(10);
        while console.looking.load(Ordering::Relaxed) && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(console.there(&here) && !console.there(&gone));
    }

    #[test]
    fn a_python_that_is_not_there_is_said_so() {
        let mut console = Console::default();
        let scratch = Scratch::new("any");
        let path = scratch.script("any.py", "");
        let err = console
            .run(&path, "no-such-python-anywhere", Path::new("unused.json"), &egui::Context::default())
            .unwrap_err();
        assert!(err.contains("Preferences › Scripting"), "{err}");
        assert!(console.running().is_none());
    }
}
