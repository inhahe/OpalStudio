//! Run Script and the Console panel (see design.md, *Scripting*): a Python script the editor
//! runs, in the Python the preferences name, connected to this editor as any script is;
//! what it prints is shown as it comes.

use crate::commands::Cmd;
use crate::icons;
use crate::state::{AppState, MessageKind};
use eframe::egui::{self, RichText};
use std::collections::VecDeque;
use std::ffi::OsStr;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

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
}

impl Drop for Running {
    /// A script the editor started does not outlive it.
    fn drop(&mut self) {
        if matches!(self.child.try_wait(), Ok(None)) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

/// The console: what scripts printed, and the script running, if one is.
#[derive(Default)]
pub struct Console {
    lines: VecDeque<Line>,
    running: Option<Running>,
    /// The script run last, for Run Again.
    pub last: Option<PathBuf>,
}

/// A command running `script` (with `args`) in `python`, connected to the editor whose
/// connection file is `api`, with the `opal` package on its path, its output unbuffered and
/// in UTF-8.
pub fn script_command(python: &OsStr, script: &OsStr, args: &[std::ffi::OsString], api: &Path) -> Command {
    let mut command = Command::new(python);
    command
        .arg(script)
        .args(args)
        .env("OPAL_STUDIO_API", api)
        .env("PYTHONUNBUFFERED", "1")
        .env("PYTHONIOENCODING", "utf-8");
    if let Some(package) = crate::headless::package_dir() {
        let mut path = std::env::split_paths(&std::env::var_os("PYTHONPATH").unwrap_or_default()).collect::<Vec<_>>();
        path.insert(0, package);
        if let Ok(joined) = std::env::join_paths(path) {
            command.env("PYTHONPATH", joined);
        }
    }
    command
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
        let name =
            script.file_name().map_or_else(|| script.display().to_string(), |n| n.to_string_lossy().into_owned());
        let mut command = script_command(python.as_ref(), script.as_os_str(), &[], api);
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
        let (send, lines) = channel();
        let streams: [(Option<Box<dyn Read + Send>>, Said); 2] = [
            (child.stdout.take().map(|s| Box::new(s) as Box<dyn Read + Send>), Said::Output),
            (child.stderr.take().map(|s| Box::new(s) as Box<dyn Read + Send>), Said::Error),
        ];
        for (stream, said) in streams {
            let (out, ctx) = (send.clone(), ctx.clone());
            let Some(stream) = stream else {
                let _ = send.send(None);
                continue;
            };
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
            Some(Running { name, child, lines, open: 2, started: Instant::now(), ended: None, stopped: false });
        self.last = Some(script.to_owned());
        Ok(())
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
    pub fn stop(&mut self) {
        if let Some(r) = &mut self.running
            && r.ended.is_none()
        {
            r.stopped = true;
            let _ = r.child.kill();
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
            self.message(MessageKind::Info, "Scripts run in the other Opal Studio", NOT_SERVING);
            return;
        };
        let python = self.prefs.text("scripting.python");
        match self.console.run(script, &python, &api, ctx) {
            Ok(()) => {
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

/// Why Run Script is not available in an editor that does not serve scripts.
pub const NOT_SERVING: &str = "Another Opal Studio was open first, and scripts connect to that one: run them there.";

/// The Console panel: the script running (and a way to stop it), or ways to run one; then
/// what scripts printed.
pub fn console_panel(ui: &mut egui::Ui, st: &mut AppState) {
    let weak = st.palette.weak_text;
    ui.horizontal(|ui| {
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
                if let Some(last) = st.console.last.clone() {
                    let name = last.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned());
                    let again = ui
                        .add_enabled(available.is_ok(), egui::Button::new(format!("{}  Run {name} again", icons::PLAY)))
                        .on_hover_text(last.display().to_string());
                    if again.clicked() {
                        st.run_script(&last, ui.ctx());
                    }
                }
            }
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let some = !st.console.lines().is_empty();
            if ui.add_enabled(some, egui::Button::new(format!("{}  Clear", icons::BROOM))).clicked() {
                st.console.clear();
            }
            if ui
                .add_enabled(some, egui::Button::new(format!("{}  Copy", icons::COPY)))
                .on_hover_text("Copy everything shown, to paste into a bug report or an editor.")
                .clicked()
            {
                ui.ctx().copy_text(st.console.text());
            }
        });
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

    /// The Python to try scripts with, if there is one.
    fn python() -> Option<String> {
        let python = std::env::var("OPAL_PYTHON").unwrap_or_else(|_| "python".to_owned());
        Command::new(&python).arg("--version").output().is_ok_and(|o| o.status.success()).then_some(python)
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

    fn script(name: &str, text: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("opal-console-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, text).unwrap();
        path
    }

    #[test]
    fn what_a_script_prints_is_shown_and_how_it_ended() {
        let Some(python) = python() else { return eprintln!("No Python found (set OPAL_PYTHON).") };
        let path = script(
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
        assert_eq!(console.last.as_deref(), Some(path.as_path()));
    }

    #[test]
    fn a_script_can_be_stopped() {
        let Some(python) = python() else { return eprintln!("No Python found (set OPAL_PYTHON).") };
        let path = script("forever.py", "import time\nprint('started', flush=True)\ntime.sleep(60)\n");
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
        let path = script(
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

    #[test]
    fn a_python_that_is_not_there_is_said_so() {
        let mut console = Console::default();
        let path = script("any.py", "");
        let err = console
            .run(&path, "no-such-python-anywhere", Path::new("unused.json"), &egui::Context::default())
            .unwrap_err();
        assert!(err.contains("Preferences › Scripting"), "{err}");
        assert!(console.running().is_none());
    }
}
