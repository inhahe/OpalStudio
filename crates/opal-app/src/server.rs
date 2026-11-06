//! The API's local connection (see design.md, *Scripting*): JSON-RPC 2.0, one request or
//! response a line, over a named pipe (a Unix socket elsewhere) that only this computer can
//! reach.
//!
//! A connection's first request must be `hello`, with the token the editor wrote, beside the
//! pipe's name, to `api.json` in the user's settings folder (which only they can read): so
//! only the user's own programs can drive the editor. Every later request is handed to the
//! interface thread, which carries it out between frames ([`Server::serve`]) and answers it,
//! or, for a file being written, answers it once the file is.

use crate::api::{Reply, Role};
use crate::state::AppState;
use eframe::egui;
use interprocess::local_socket::{GenericNamespaced, ListenerOptions, Stream, prelude::*};
use serde_json::{Map, Value, json};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The version of the protocol, told in `hello`'s answer.
pub const PROTOCOL: u32 = 1;

/// The longest line read from a connection that has not said hello yet: a hello is short.
const STRANGER_LINE: usize = 4 << 10;
/// The longest line read from one that has.
const CLIENT_LINE: usize = 1 << 20;
/// How many connections may wait to say hello at once; more are dropped at once.
const MAX_STRANGERS: usize = 16;
/// How long a frame goes on carrying out requests that arrive while it does (a script's
/// next call comes a moment after the last's answer), before it lets itself be drawn.
const SERVE_FOR: Duration = Duration::from_millis(6);
/// How long, within that, the next request is waited for.
const NEXT_WITHIN: Duration = Duration::from_millis(1);

/// How long a step may go without a call before it ends by itself, when the next call comes:
/// one an assistant forgot to end would otherwise take in all its calls after.
const STEP_IDLE: Duration = if cfg!(test) { Duration::from_millis(300) } else { Duration::from_secs(10 * 60) };

/// How long a connection has to say hello before it is dropped, so that connections that
/// never do cannot keep every place among the strangers.
const HELLO_WITHIN: Duration = if cfg!(test) { Duration::from_millis(400) } else { Duration::from_secs(5) };

/// What a connection passes to the interface thread.
enum Request {
    /// A call, and where its answer goes: from a connection of `generation` (refused if the
    /// user has disconnected since), in `groups` (the script run the connection is, the step
    /// the call is in), and `heard` unless it is a notification, whose answer no one reads.
    Call {
        method: String,
        params: Map<String, Value>,
        generation: u64,
        groups: Vec<u64>,
        heard: bool,
        answer: Sender<Result<Value, String>>,
    },
    /// Group `group` (a script run, its connection closed, or a `step`, ended) has ended:
    /// what it made is joined into one state labeled `label`.
    Ended { group: u64, label: String, step: bool },
}

/// Who may connect, and who has. One lock for all of it, so that a disconnect cannot pass a
/// connection saying hello or going.
struct Clients {
    /// The token a hello must give: replaced when everyone is disconnected, so that no one
    /// dropped can come back with the one it had.
    token: String,
    /// Whether the user has shut scripts out (by disconnecting them): until they let them in
    /// again, a hello is refused, but for one that only opens files ([`hand_to_running`]).
    shut_out: bool,
    /// Moving on drops every connection made in an earlier generation.
    generation: u64,
    /// The connections of this generation that have said hello (but those that only open
    /// files).
    connected: usize,
}

/// What the connections share with the interface thread.
struct Shared {
    requests: Sender<Request>,
    clients: Mutex<Clients>,
    /// How many connections have not said hello yet.
    strangers: AtomicUsize,
    /// The number the next group (a script run, a step) gets.
    runs: AtomicU64,
    /// The steps connections have begun and not ended, by group: their names.
    open_steps: Mutex<HashMap<u64, String>>,
    /// Whether the user has asked to quit: hellos are refused.
    quitting: AtomicBool,
    ctx: egui::Context,
    /// Where the pipe's name and the token are written, and the pipe's name.
    file: PathBuf,
    pipe: String,
}

/// The API server: its listener thread, and the requests it has passed on.
pub struct Server {
    shared: Arc<Shared>,
    requests: Receiver<Request>,
    /// The answers awaited from files being written, by the number of the call.
    later: HashMap<u64, Sender<Result<Value, String>>>,
}

/// Where the pipe's name and the token are written, for clients to find.
pub fn connection_file() -> Option<PathBuf> {
    directories::ProjectDirs::from("", "", "Opal Studio").map(|d| d.config_dir().join("api.json"))
}

/// A random token: 32 bytes from the operating system's generator, in hex.
pub(crate) fn new_token() -> std::io::Result<String> {
    random_hex(32)
}

/// `n` bytes from the operating system's generator, in hex.
pub(crate) fn random_hex(n: usize) -> std::io::Result<String> {
    let mut bytes = vec![0u8; n];
    getrandom::fill(&mut bytes).map_err(|e| std::io::Error::other(e.to_string()))?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

/// What a connection file says: the pipe the editor listens on, the token a hello must
/// give, and the editor's process.
struct ConnectionInfo {
    pipe: String,
    token: String,
    pid: Option<u32>,
}

/// Reads connection file `file`; why not, if it cannot be.
fn read_connection_file(file: &Path) -> Result<ConnectionInfo, String> {
    let text = std::fs::read_to_string(file).map_err(|_| "Opal Studio is not running: start it first.".to_owned())?;
    let info: Value = serde_json::from_str(&text).map_err(|e| format!("{} is not readable: {e}", file.display()))?;
    match (info["pipe"].as_str(), info["token"].as_str()) {
        (Some(pipe), Some(token)) => Ok(ConnectionInfo {
            pipe: pipe.to_owned(),
            token: token.to_owned(),
            pid: info["pid"].as_u64().and_then(|p| u32::try_from(p).ok()),
        }),
        _ => Err(format!("{} does not name a pipe and a token.", file.display())),
    }
}

/// What came of saying hello to the editor a connection file tells of.
pub enum Hello {
    /// It answered: the connection, past hello.
    Answered(BufReader<Stream>),
    /// No editor is there to answer: why not.
    Nobody(String),
    /// An editor is there, but refused (or was too busy to answer): its reason.
    Refused(String),
}

/// How many times a busy pipe is tried, 25 ms apart, before giving up.
const CONNECT_TRIES: usize = 20;

/// Connects to the editor connection file `file` tells of, and says hello with its token, as
/// a connection that only opens files if `files_only`.
pub fn say_hello(file: &Path, files_only: bool) -> Hello {
    match read_connection_file(file) {
        Ok(info) => hello_to(&info, files_only, CONNECT_TRIES),
        Err(why) => Hello::Nobody(why),
    }
}

/// [`say_hello`] to the editor `info` tells of, trying its pipe `tries` times while it is
/// busy.
fn hello_to(info: &ConnectionInfo, files_only: bool, tries: usize) -> Hello {
    let gone = || Hello::Nobody("Opal Studio is not running (its pipe is gone): start it again.".to_owned());
    let Some(name) = info.pipe.rsplit(['\\', '/']).next().and_then(|n| n.to_ns_name::<GenericNamespaced>().ok()) else {
        return gone();
    };
    let stream = match connect(name, tries) {
        Ok(stream) => stream,
        Err(Unconnected::Absent) => return gone(),
        Err(Unconnected::Busy) => return Hello::Refused("Opal Studio is busy: try again in a moment.".to_owned()),
    };
    let mut conn = BufReader::new(stream);
    let mut params = json!({ "token": info.token });
    if files_only {
        params["files"] = json!(true);
    }
    let hello = json!({ "jsonrpc": "2.0", "id": 0, "method": "hello", "params": params });
    let mut line = String::new();
    if conn.get_mut().write_all(format!("{hello}\n").as_bytes()).is_err() || conn.read_line(&mut line).unwrap_or(0) == 0
    {
        // Someone took the connection: an editor with no room for it just then.
        return Hello::Refused("Opal Studio did not answer: try again in a moment.".to_owned());
    }
    let Ok(answer) = serde_json::from_str::<Value>(&line) else {
        return Hello::Refused("The editor's answer is not JSON.".to_owned());
    };
    if let Some(e) = answer.get("error") {
        return Hello::Refused(e["message"].as_str().unwrap_or("The editor refused.").to_owned());
    }
    match answer["result"]["protocol"].as_u64() {
        Some(p) if p == u64::from(PROTOCOL) => Hello::Answered(conn),
        p => Hello::Refused(format!(
            "The editor speaks protocol {}, this program {PROTOCOL}: use the same version of both.",
            p.map_or_else(|| "?".to_owned(), |p| p.to_string())
        )),
    }
}

/// Why a pipe could not be connected to.
enum Unconnected {
    /// No one listens on it.
    Absent,
    /// It was busy each time it was tried (accepting another client, it had no instance
    /// free): someone listens on it.
    Busy,
}

/// Connects to pipe `name`, trying it `tries` times, 25 ms apart, while it is busy.
fn connect(name: interprocess::local_socket::Name<'_>, tries: usize) -> Result<Stream, Unconnected> {
    for tried in 1..=tries {
        match Stream::connect(name.borrow()) {
            Ok(s) => return Ok(s),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(Unconnected::Absent),
            Err(_) if tried < tries => std::thread::sleep(Duration::from_millis(25)),
            Err(_) => {}
        }
    }
    Err(Unconnected::Busy)
}

/// Whether an editor is running as `file` tells: someone listens on its pipe (each editor's
/// is of its own, named after its process). Only connected to, not said hello to, so that
/// nothing waits on an answer (an editor stopped in a debugger never gives one), and the
/// interface thread can ask.
pub fn is_running(file: &Path) -> bool {
    let Ok(info) = read_connection_file(file) else { return false };
    let Some(name) = info.pipe.rsplit(['\\', '/']).next().and_then(|n| n.to_ns_name::<GenericNamespaced>().ok()) else {
        return false;
    };
    !matches!(connect(name, 1), Err(Unconnected::Absent))
}

/// Hands `files` to the editor already running (as `file` tells), to be opened there, and
/// lets it come to the front; whether one was. Each goes as a notification, carried out but
/// not answered (what opening it has to say is told there), so that this process need not
/// wait for a dialog the user has open there. An editor that is quitting refuses them, and
/// this process opens them itself.
pub fn hand_to_running(file: &Path, files: &[PathBuf]) -> bool {
    let Ok(info) = read_connection_file(file) else { return false };
    let Hello::Answered(mut conn) = hello_to(&info, true, CONNECT_TRIES) else { return false };
    allow_to_front(info.pid);
    for f in files {
        let path = std::path::absolute(f).unwrap_or_else(|_| f.clone());
        let open = json!({ "jsonrpc": "2.0", "method": "open", "params": { "path": path.display().to_string() } });
        if conn.get_mut().write_all(format!("{open}\n").as_bytes()).is_err() {
            return false;
        }
    }
    true
}

/// Lets the editor's process `pid` bring its window to the front: Windows lets only the
/// program the user is working in hand that on.
fn allow_to_front(pid: Option<u32>) {
    #[cfg(windows)]
    if let Some(pid) = pid {
        // SAFETY: AllowSetForegroundWindow takes a process id and has no other
        // preconditions; failing, it only leaves the window where it is.
        unsafe {
            let _ = windows::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow(pid);
        }
    }
    #[cfg(not(windows))]
    let _ = pid;
}

impl Server {
    /// Starts listening on a pipe of this process's own, and writes its name and a new token
    /// to `file` (the user's [`connection_file`], or another, for tests). Fails, with
    /// [`std::io::ErrorKind::AddrInUse`], if an editor already running answers on the pipe
    /// `file` names: scripts reach the first.
    pub fn start(file: PathBuf, ctx: egui::Context) -> std::io::Result<Server> {
        if is_running(&file) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AddrInUse,
                "another Opal Studio is running, which scripts connect to",
            ));
        }
        // This process's own (a test makes several servers).
        static SERVERS: AtomicU64 = AtomicU64::new(0);
        let n = SERVERS.fetch_add(1, Ordering::Relaxed);
        let name = format!("opal-studio-{}-{n}.sock", std::process::id());
        let listener = ListenerOptions::new().name(name.as_str().to_ns_name::<GenericNamespaced>()?).create_sync()?;
        let pipe = if cfg!(windows) { format!(r"\\.\pipe\{name}") } else { name.clone() };
        let (send, requests) = channel();
        let token = new_token()?;
        let shared = Arc::new(Shared {
            requests: send,
            clients: Mutex::new(Clients { token: token.clone(), shut_out: false, generation: 0, connected: 0 }),
            strangers: AtomicUsize::new(0),
            runs: AtomicU64::new(0),
            open_steps: Mutex::new(HashMap::new()),
            quitting: AtomicBool::new(false),
            ctx,
            file,
            pipe,
        });
        // Told before the listener's thread starts, so that a failure leaves nothing behind
        // (the pipe takes connections already, which wait for it).
        if let Some(dir) = shared.file.parent() {
            std::fs::create_dir_all(dir)?;
        }
        write_connection_file(&shared, &token)?;
        let listening = shared.clone();
        let listen = move || {
            for conn in listener.incoming() {
                // A failure to accept is waited out, not spun on (one that lasts would
                // otherwise take a core for the life of the editor).
                let Ok(conn) = conn else {
                    std::thread::sleep(Duration::from_millis(250));
                    continue;
                };
                if listening.strangers.load(Ordering::Relaxed) >= MAX_STRANGERS {
                    continue;
                }
                listening.strangers.fetch_add(1, Ordering::Relaxed);
                let shared = listening.clone();
                let started = std::thread::Builder::new()
                    .name("api connection".into())
                    .spawn(move || serve_connection(conn, &shared));
                if started.is_err() {
                    listening.strangers.fetch_sub(1, Ordering::Relaxed);
                }
            }
        };
        if let Err(e) = std::thread::Builder::new().name("api listener".into()).spawn(listen) {
            let _ = std::fs::remove_file(&shared.file);
            return Err(e);
        }
        Ok(Server { shared, requests, later: HashMap::new() })
    }

    /// How many clients are connected (have said hello).
    pub fn clients(&self) -> usize {
        self.shared.clients.lock().expect("not poisoned").connected
    }

    /// Drops every connection (each goes at its next request, unanswered, and no longer
    /// counts as connected), refuses what they asked that is still waiting, makes a new
    /// token, and shuts scripts out until [`let_in`](Self::let_in): a client that reads the
    /// connection file again (as the MCP shim does) cannot come straight back.
    pub fn disconnect_all(&self) {
        {
            let mut c = self.shared.clients.lock().expect("not poisoned");
            c.generation += 1;
            c.connected = 0;
            c.shut_out = true;
        }
        // The new token is kept only once the file holds it: one it did not get would leave
        // every client, even let in again, with a token that is not the editor's.
        if let Ok(token) = new_token()
            && write_connection_file(&self.shared, &token).is_ok()
        {
            self.shared.clients.lock().expect("not poisoned").token = token;
        }
        self.shared.ctx.request_repaint();
    }

    /// The names of the steps clients have begun and not ended, in no order.
    pub fn open_steps(&self) -> Vec<String> {
        self.shared.open_steps.lock().expect("not poisoned").values().cloned().collect()
    }

    /// Whether scripts are shut out, since the user disconnected them.
    pub fn shut_out(&self) -> bool {
        self.shared.clients.lock().expect("not poisoned").shut_out
    }

    /// Lets scripts connect again after a disconnect.
    pub fn let_in(&self) {
        self.shared.clients.lock().expect("not poisoned").shut_out = false;
        self.shared.ctx.request_repaint();
    }

    /// Carries out the requests passed on since the last frame, and answers those whose
    /// files have been written. Called by the interface thread every frame; while the user
    /// is `busy` with something (a dialog, a drag) that a request could change under them,
    /// the requests wait. Once the user has asked to quit, they are refused.
    ///
    /// A frame that carries out a request goes on with those that arrive meanwhile, for a
    /// few milliseconds ([`SERVE_FOR`]): a script calling one method after another then
    /// makes many calls a frame, not one.
    pub fn serve(&mut self, st: &mut AppState, ctx: &egui::Context, busy: bool) {
        let quitting = st.exit_requested || st.exit_after_writes;
        // Told to those saying hello, which are refused meanwhile.
        self.shared.quitting.store(quitting, Ordering::Relaxed);
        if !busy || quitting {
            let generation = self.shared.clients.lock().expect("not poisoned").generation;
            let until = Instant::now() + SERVE_FOR;
            let mut next = self.requests.try_recv().ok();
            while let Some(r) = next {
                match r {
                    Request::Ended { group, label, step } => st.group_ended(group, &label, step),
                    Request::Call { answer, generation: theirs, .. } if theirs != generation => {
                        let _ = answer.send(Err("Disconnected by the user.".to_owned()));
                    }
                    Request::Call { answer, .. } if quitting => {
                        let _ = answer.send(Err("The editor is closing.".to_owned()));
                    }
                    Request::Call { method, params, groups, heard, answer, .. } => {
                        match st.call_from(&groups, heard, &method, &params, ctx) {
                            Reply::Now(result) => {
                                let _ = answer.send(result);
                            }
                            Reply::Later(n) => {
                                self.later.insert(n, answer);
                            }
                        }
                    }
                }
                next = self.next_before(until);
            }
            // Any left for the next frame asked for it as they came (pass_on).
        }
        for (n, result) in st.answers_while(busy, quitting) {
            if let Some(answer) = self.later.remove(&n) {
                let _ = answer.send(result);
            }
        }
    }

    /// The next request, if it is not yet `until` and one comes within [`NEXT_WITHIN`] (but
    /// no later than `until`). Looked for again and again rather than slept for: a wait
    /// shorter than the system timer's tick (15.6 ms on Windows, unless some program asks for
    /// finer) lasts the whole tick.
    fn next_before(&self, until: Instant) -> Option<Request> {
        let now = Instant::now();
        if now >= until {
            return None;
        }
        let until = until.min(now + NEXT_WITHIN);
        loop {
            if let Ok(r) = self.requests.try_recv() {
                return Some(r);
            }
            if Instant::now() >= until {
                return None;
            }
            std::thread::yield_now();
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        // Told no more, unless the file has become another's.
        let token = self.shared.clients.lock().expect("not poisoned").token.clone();
        let mine = read_connection_file(&self.shared.file).is_ok_and(|info| info.token == token);
        if mine {
            let _ = std::fs::remove_file(&self.shared.file);
        }
    }
}

/// Writes the pipe's name and `token` to the connection file.
fn write_connection_file(shared: &Shared, token: &str) -> std::io::Result<()> {
    let info = json!({ "pipe": shared.pipe, "token": token, "pid": std::process::id(), "protocol": PROTOCOL });
    write_private(&shared.file, info.to_string().as_bytes())
}

/// Writes `bytes` to `path`, readable by the user alone where the system lets a program say so
/// (on Windows, a file in the user's own profile already is).
pub(crate) fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    options.open(path)?.write_all(bytes)
}

/// A JSON-RPC response to request `id`.
fn response(id: &Value, result: Result<Value, (i64, String)>) -> String {
    let body = match result {
        Ok(value) => json!({ "jsonrpc": "2.0", "id": id, "result": value }),
        Err((code, message)) => json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } }),
    };
    body.to_string() + "\n"
}

/// The JSON-RPC error codes used.
const PARSE_ERROR: i64 = -32700;
const INVALID_REQUEST: i64 = -32600;
/// A method that failed, with the reason.
const FAILED: i64 = -32000;
/// A request before `hello`, or with the wrong token.
const NOT_ALLOWED: i64 = -32001;

/// The next line from `reader`, at most `limit` bytes: `None` at the end, or past the limit
/// (the connection then goes: a request is never that long).
fn read_line(reader: &mut impl BufRead, limit: usize) -> Option<String> {
    let mut bytes = Vec::new();
    let read = reader.take(limit as u64 + 1).read_until(b'\n', &mut bytes).ok()?;
    if read == 0 || (bytes.len() > limit && !bytes.ends_with(b"\n")) {
        return None;
    }
    String::from_utf8(bytes).ok()
}

/// The first line from connection `conn` (nothing read from it yet), which must come within
/// [`HELLO_WITHIN`] and be at most [`STRANGER_LINE`] bytes: `None` past then, at the end, or
/// past the limit. A named pipe has no timeouts, and one that does not block reads nothing
/// as it reads the end; so what has come is looked at without being read, every few
/// milliseconds, and the line read once it is all there (what follows it is left).
#[cfg(windows)]
fn read_first_line(_: &mut BufReader<&Stream>, conn: &Stream) -> Option<String> {
    use std::os::windows::io::{AsHandle, AsRawHandle};
    let Stream::NamedPipe(pipe) = conn;
    let handle = windows::Win32::Foundation::HANDLE(pipe.inner().as_handle().as_raw_handle());
    let until = Instant::now() + HELLO_WITHIN;
    let mut seen = vec![0u8; STRANGER_LINE + 1];
    loop {
        let mut got = 0u32;
        // SAFETY: the handle is the pipe's, open while `conn` is; `seen` has room for the
        // bytes asked for. It fails once the other end has closed.
        unsafe {
            windows::Win32::System::Pipes::PeekNamedPipe(
                handle,
                Some(seen.as_mut_ptr().cast()),
                seen.len() as u32,
                Some(&mut got),
                None,
                None,
            )
        }
        .ok()?;
        let seen = &seen[..got as usize];
        if let Some(end) = seen.iter().position(|&b| b == b'\n') {
            let mut line = vec![0u8; end + 1];
            (&mut &*conn).read_exact(&mut line).ok()?;
            return String::from_utf8(line).ok();
        }
        if seen.len() > STRANGER_LINE || Instant::now() >= until {
            return None;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// The first line from `reader`, which must come within [`HELLO_WITHIN`] and be at most
/// [`STRANGER_LINE`] bytes: `None` past then, at the end, or past the limit.
#[cfg(not(windows))]
fn read_first_line(reader: &mut BufReader<&Stream>, conn: &Stream) -> Option<String> {
    conn.set_recv_timeout(Some(HELLO_WITHIN)).ok()?;
    let line = read_line(reader, STRANGER_LINE);
    conn.set_recv_timeout(None).ok()?;
    line
}

/// Answers a hello with `params`: lets the connection in if it gives the token, and scripts
/// are not shut out (or it only opens files). Once it is in, `hello` holds its generation
/// and whether it counts as connected (unless it only opens files), and `script` the run
/// it is, if it names a script.
fn answer_hello(
    shared: &Shared,
    params: &Map<String, Value>,
    hello: &mut Option<(u64, bool)>,
    script: &mut Option<(u64, String)>,
) -> Result<Value, (i64, String)> {
    let files_only = params.get("files").and_then(Value::as_bool) == Some(true);
    if shared.quitting.load(Ordering::Relaxed) {
        return Err((NOT_ALLOWED, "The editor is closing.".to_owned()));
    }
    let mut c = shared.clients.lock().expect("not poisoned");
    if params.get("token").and_then(Value::as_str) != Some(c.token.as_str()) {
        return Err((NOT_ALLOWED, "The token is not this editor's.".to_owned()));
    }
    if c.shut_out && !files_only {
        return Err((
            NOT_ALLOWED,
            "The user has disconnected scripts: they can let them in again from Opal Studio's status bar.".to_owned(),
        ));
    }
    if hello.is_none() {
        if !files_only {
            c.connected += 1;
            *script = params
                .get("script")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(|s| (shared.runs.fetch_add(1, Ordering::Relaxed), s.to_owned()));
        }
        *hello = Some((c.generation, !files_only));
        drop(c);
        shared.strangers.fetch_sub(1, Ordering::Relaxed);
        shared.ctx.request_repaint();
    }
    Ok(json!({ "protocol": PROTOCOL, "editor": env!("CARGO_PKG_VERSION") }))
}

/// Serves one connection: `hello` first, then each request in turn.
fn serve_connection(conn: Stream, shared: &Shared) {
    let mut reader = BufReader::new(&conn);
    let mut writer = &conn;
    // Once it has said hello: the generation it is of, and whether it counts as connected.
    let mut hello: Option<(u64, bool)> = None;
    // The script run it is, if its hello named a script: its number and the script's name.
    let mut script: Option<(u64, String)> = None;
    // The step it has begun, if one.
    let mut step: Option<Step> = None;
    loop {
        let line = match hello {
            None => read_first_line(&mut reader, &conn),
            Some(_) => read_line(&mut reader, CLIENT_LINE),
        };
        let Some(line) = line else { break };
        if hello.is_some_and(|(g, _)| shared.clients.lock().expect("not poisoned").generation != g) {
            // Disconnected by the user.
            break;
        }
        // A step left open and unused this long was forgotten (an assistant's, say): it
        // ends before this call, which is not part of it.
        if step.as_ref().is_some_and(|s| s.used.elapsed() >= STEP_IDLE) {
            end_step(shared, step.take().expect("there"));
        }
        // A notification (a request without an id) is carried out but not answered; a line
        // that is not a request at all is answered, with no id.
        let (id, answer) = match serde_json::from_str::<Value>(&line) {
            Err(e) => (Some(Value::Null), Err((PARSE_ERROR, format!("Not JSON: {e}")))),
            Ok(request) => {
                let id = request.get("id").cloned();
                let method = request.get("method").and_then(Value::as_str).map(str::to_owned);
                let params = match request.get("params") {
                    None | Some(Value::Null) => Some(Map::new()),
                    Some(Value::Object(p)) => Some(p.clone()),
                    Some(_) => None,
                };
                let answer = match (method, params) {
                    (Some(method), Some(params)) if method == "hello" => {
                        answer_hello(shared, &params, &mut hello, &mut script)
                    }
                    (Some(_), Some(_)) if hello.is_none() => {
                        Err((NOT_ALLOWED, "Say hello first, with the token from api.json.".to_owned()))
                    }
                    (Some(method), Some(_)) if hello.is_some_and(|(_, counted)| !counted) && method != "open" => {
                        Err((NOT_ALLOWED, "This connection only opens files.".to_owned()))
                    }
                    (Some(method), Some(params)) if crate::api::role_of(&method) == Some(Role::Connection) => {
                        connection_answer(shared, &method, &params, &mut step)
                    }
                    (Some(method), Some(params)) => {
                        let (generation, _) = hello.expect("said hello");
                        let heard = id.is_some();
                        // In the script run the connection is, and the step it has begun.
                        let groups = script.iter().map(|s| s.0).chain(step.iter().map(|s| s.group)).collect();
                        if let Some(s) = &mut step {
                            s.used = Instant::now();
                        }
                        pass_on(shared, method, params, generation, groups, heard)
                    }
                    _ => Err((INVALID_REQUEST, "A request needs a method, and its params as an object.".to_owned())),
                };
                (id, answer)
            }
        };
        if let Some(id) = id
            && writer.write_all(response(&id, answer).as_bytes()).is_err()
        {
            break;
        }
        if hello.is_none() {
            // Refused: the connection goes.
            break;
        }
    }
    // After all its calls (each was answered before the next was read): a step left open,
    // then the run.
    if let Some(step) = step {
        end_step(shared, step);
    }
    if let Some((group, name)) = script {
        let _ = shared.requests.send(Request::Ended { group, label: format!("Script: {name}"), step: false });
    }
    shared.ctx.request_repaint();
    match hello {
        None => {
            shared.strangers.fetch_sub(1, Ordering::Relaxed);
        }
        Some((g, true)) => {
            // Uncounted already if disconnected.
            let mut c = shared.clients.lock().expect("not poisoned");
            if c.generation == g {
                c.connected -= 1;
                shared.ctx.request_repaint();
            }
        }
        Some((_, false)) => {}
    }
}

/// A step a connection has begun: its group's number, its name, how many steps were begun
/// inside it, and when a call was last made in it.
struct Step {
    group: u64,
    name: String,
    inside: usize,
    used: Instant,
}

/// Ends `step`: its states joined (before any later call, the channel keeping their order),
/// and no longer shown as open.
fn end_step(shared: &Shared, step: Step) {
    shared.open_steps.lock().expect("not poisoned").remove(&step.group);
    let _ = shared.requests.send(Request::Ended { group: step.group, label: step.name, step: true });
    shared.ctx.request_repaint();
}

/// Answers `method`, one of those about the connection (see `api::CONNECTION`), with
/// `params`, for a connection whose step is `step`.
fn connection_answer(
    shared: &Shared,
    method: &str,
    params: &Map<String, Value>,
    step: &mut Option<Step>,
) -> Result<Value, (i64, String)> {
    crate::api::check_args(method, params).map_err(|why| (FAILED, why))?;
    match method {
        "begin_step" => {
            match step {
                Some(outer) => outer.inside += 1,
                None => {
                    let name = params["name"].as_str().expect("checked").trim().to_owned();
                    let group = shared.runs.fetch_add(1, Ordering::Relaxed);
                    shared.open_steps.lock().expect("not poisoned").insert(group, name.clone());
                    shared.ctx.request_repaint();
                    *step = Some(Step { group, name, inside: 0, used: Instant::now() });
                }
            }
            Ok(json!({}))
        }
        "end_step" => match step {
            None => Err((FAILED, "end_step: no step has begun.".to_owned())),
            Some(s) if s.inside > 0 => {
                s.inside -= 1;
                Ok(json!({}))
            }
            Some(_) => {
                end_step(shared, step.take().expect("there"));
                Ok(json!({}))
            }
        },
        other => Err((FAILED, format!("{other} is not answered yet."))),
    }
}

/// Passes a request to the interface thread (from a connection of `generation`, in
/// `groups`, `heard` unless a notification), and waits for its answer.
fn pass_on(
    shared: &Shared,
    method: String,
    params: Map<String, Value>,
    generation: u64,
    groups: Vec<u64>,
    heard: bool,
) -> Result<Value, (i64, String)> {
    let (answer, answered) = channel();
    if shared.requests.send(Request::Call { method, params, generation, groups, heard, answer }).is_err() {
        return Err((FAILED, "The editor is closing.".to_owned()));
    }
    shared.ctx.request_repaint();
    match answered.recv() {
        Ok(result) => result.map_err(|why| (FAILED, why)),
        Err(_) => Err((FAILED, "The editor closed before answering.".to_owned())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    /// A client of `server`'s pipe: sends each line, returning each response read back.
    struct Client {
        reader: BufReader<Stream>,
    }

    impl Client {
        /// Connects to `pipe`, as the connection file names it.
        fn connect(pipe: &str) -> Client {
            let name = pipe.rsplit(['\\', '/']).next().unwrap().to_ns_name::<GenericNamespaced>().unwrap();
            Client { reader: BufReader::new(Stream::connect(name).unwrap()) }
        }

        fn send(&mut self, line: &str) -> Option<Value> {
            if self.reader.get_mut().write_all(format!("{line}\n").as_bytes()).is_err() {
                return None;
            }
            let mut out = String::new();
            match self.reader.read_line(&mut out) {
                Ok(0) | Err(_) => None,
                Ok(_) => Some(serde_json::from_str(&out).unwrap()),
            }
        }
    }

    /// Runs `server` for `st` on this thread while `client` runs on another.
    fn with_client(server: &mut Server, st: &mut AppState, client: impl FnOnce() + Send + 'static) {
        let ctx = egui::Context::default();
        let done = std::thread::spawn(client);
        while !done.is_finished() {
            server.serve(st, &ctx, false);
            st.poll_writes(false);
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        done.join().unwrap();
    }

    /// The connection file's pipe and token.
    fn info(file: &Path) -> (String, String) {
        let info: Value = serde_json::from_str(&std::fs::read_to_string(file).unwrap()).unwrap();
        (info["pipe"].as_str().unwrap().to_owned(), info["token"].as_str().unwrap().to_owned())
    }

    fn hello_line(token: &str) -> String {
        format!(r#"{{"jsonrpc":"2.0","id":1,"method":"hello","params":{{"token":"{token}"}}}}"#)
    }

    #[test]
    fn disconnecting_drops_everyone_and_keeps_them_out_until_let_in() {
        let file = std::env::temp_dir().join(format!("opal-api-drop-{}.json", std::process::id()));
        let mut server = Server::start(file.clone(), egui::Context::default()).unwrap();
        let mut st = AppState::new();
        let (pipe, old) = info(&file);
        let mut client = Client::connect(&pipe);
        assert!(client.send(&hello_line(&old)).unwrap().get("result").is_some());
        // A request waiting (the user busy) when they disconnect is refused, not carried out.
        let mut waiting = Client::connect(&pipe);
        waiting.send(&hello_line(&old)).unwrap();
        let asked = std::thread::spawn(move || {
            waiting.send(r#"{"jsonrpc":"2.0","id":2,"method":"new","params":{"width":8,"height":8}}"#)
        });
        // Passed on, and waiting.
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(server.clients(), 2);
        server.disconnect_all();
        assert_eq!(server.clients(), 0);
        server.serve(&mut st, &egui::Context::default(), false);
        let refused = asked.join().unwrap().expect("answered");
        assert!(refused["error"]["message"].as_str().is_some_and(|m| m.contains("Disconnected")), "{refused}");
        assert!(st.docs.is_empty(), "carried out after the disconnect");
        // Dropped at its next request, unanswered; and the old token is no use.
        assert!(client.send(r#"{"jsonrpc":"2.0","id":2,"method":"documents"}"#).is_none());
        let (_, new) = info(&file);
        assert_ne!(new, old, "the token stayed");
        let refused = Client::connect(&pipe).send(&hello_line(&old)).unwrap();
        assert_eq!(refused["error"]["code"], json!(NOT_ALLOWED));
        // Nor can a client that reads the new one come straight back (as the MCP shim
        // would); one that only opens files can, and does only that.
        assert!(server.shut_out());
        let back = Client::connect(&pipe).send(&hello_line(&new)).unwrap();
        assert!(back["error"]["message"].as_str().is_some_and(|m| m.contains("disconnected")), "{back}");
        let mut opener = Client::connect(&pipe);
        let files = format!(r#"{{"jsonrpc":"2.0","id":1,"method":"hello","params":{{"token":"{new}","files":true}}}}"#);
        assert!(opener.send(&files).unwrap().get("result").is_some());
        let other = opener.send(r#"{"jsonrpc":"2.0","id":2,"method":"documents"}"#).unwrap();
        assert!(other["error"]["message"].as_str().is_some_and(|m| m.contains("only opens files")), "{other}");
        assert_eq!(server.clients(), 0, "one that only opens files counts as no script");
        server.let_in();
        assert!(Client::connect(&pipe).send(&hello_line(&new)).unwrap().get("result").is_some());
        // A stranger's endless line is cut off, not kept.
        let mut flood = Client::connect(&pipe);
        assert!(flood.send(&"x".repeat(STRANGER_LINE * 4)).is_none(), "an endless line was read");
        drop(server);
        assert!(!file.exists());
    }

    #[test]
    fn a_frame_ends_however_fast_the_calls_come() {
        let file = std::env::temp_dir().join(format!("opal-api-flood-{}.json", std::process::id()));
        let mut server = Server::start(file.clone(), egui::Context::default()).unwrap();
        let (pipe, token) = info(&file);
        let mut st = AppState::new();
        let ctx = egui::Context::default();
        // Scripts calling as fast as they can, several at once, with calls that take longer to
        // carry out than to make: whenever one is carried out, another is already waiting.
        let calls: Vec<_> = (0..8)
            .map(|_| {
                let mut client = Client::connect(&pipe);
                client.send(&hello_line(&token)).unwrap();
                std::thread::spawn(move || {
                    for i in 0..300 {
                        client.send(&format!(r#"{{"jsonrpc":"2.0","id":{i},"method":"methods"}}"#)).unwrap();
                    }
                })
            })
            .collect();
        let (mut frames, mut longest) = (0, Duration::ZERO);
        while !calls.iter().all(|c| c.is_finished()) {
            let t = Instant::now();
            server.serve(&mut st, &ctx, false);
            longest = longest.max(t.elapsed());
            frames += 1;
        }
        calls.into_iter().for_each(|c| c.join().unwrap());
        // Each frame stops after its few milliseconds, to be drawn.
        assert!(longest < Duration::from_millis(100), "a frame took {longest:?} ({frames} frames)");
    }

    #[test]
    fn an_editor_quitting_refuses_hellos_so_files_open_elsewhere() {
        let file = std::env::temp_dir().join(format!("opal-api-quit-{}.json", std::process::id()));
        let mut server = Server::start(file.clone(), egui::Context::default()).unwrap();
        let mut st = AppState::new();
        st.exit_requested = true;
        server.serve(&mut st, &egui::Context::default(), false);
        assert!(is_running(&file), "it is still there");
        let photo = std::env::temp_dir().join("opal-api-photo.png");
        assert!(!hand_to_running(&file, &[photo]), "handed to an editor that will not open it");
        st.exit_requested = false;
        server.serve(&mut st, &egui::Context::default(), false);
        assert!(matches!(say_hello(&file, false), Hello::Answered(_)), "still refused once it was not quitting");
    }

    #[test]
    fn an_editor_is_seen_running_though_it_has_no_room_or_never_answers() {
        // Every place among the strangers taken: the editor takes a connection and drops it.
        let file = std::env::temp_dir().join(format!("opal-api-full-{}.json", std::process::id()));
        let server = Server::start(file.clone(), egui::Context::default()).unwrap();
        let (pipe, _) = info(&file);
        let silent: Vec<Client> = (0..MAX_STRANGERS).map(|_| Client::connect(&pipe)).collect();
        std::thread::sleep(Duration::from_millis(50));
        assert!(is_running(&file), "taken for gone while full");
        drop((silent, server));
        // One that takes the connection and never answers (stopped in a debugger, say).
        let name = format!("opal-api-mute-{}.sock", std::process::id());
        let listener = ListenerOptions::new()
            .name(name.as_str().to_ns_name::<GenericNamespaced>().unwrap())
            .create_sync()
            .unwrap();
        let held = std::thread::spawn(move || {
            let taken = listener.incoming().next();
            std::thread::sleep(Duration::from_secs(2));
            drop(taken);
        });
        let mute = std::env::temp_dir().join(format!("opal-api-mute-{}.json", std::process::id()));
        std::fs::write(&mute, json!({ "pipe": format!(r"\\.\pipe\{name}"), "token": "t" }).to_string()).unwrap();
        let (told, asked) = std::sync::mpsc::channel();
        let probe = mute.clone();
        std::thread::spawn(move || told.send(is_running(&probe)));
        assert_eq!(asked.recv_timeout(Duration::from_secs(1)), Ok(true), "waited on an editor that never answers");
        let _ = held.join();
        std::fs::remove_file(&mute).ok();
    }

    #[test]
    fn a_disconnect_keeps_the_token_the_file_could_not_be_given() {
        let file = std::env::temp_dir().join(format!("opal-api-locked-{}.json", std::process::id()));
        let server = Server::start(file.clone(), egui::Context::default()).unwrap();
        let (pipe, old) = info(&file);
        // The file cannot be written (a folder is in its place).
        std::fs::remove_file(&file).unwrap();
        std::fs::create_dir(&file).unwrap();
        server.disconnect_all();
        server.let_in();
        let back = Client::connect(&pipe).send(&hello_line(&old)).unwrap();
        std::fs::remove_dir(&file).unwrap();
        assert!(back.get("result").is_some(), "the token became one no client can have: {back}");
    }

    #[test]
    fn connections_that_never_say_hello_are_dropped_and_make_room() {
        let file = std::env::temp_dir().join(format!("opal-api-silent-{}.json", std::process::id()));
        let server = Server::start(file.clone(), egui::Context::default()).unwrap();
        let (pipe, token) = info(&file);
        let silent: Vec<Client> = (0..MAX_STRANGERS).map(|_| Client::connect(&pipe)).collect();
        // Every place among the strangers is taken until they have had their time.
        std::thread::sleep(HELLO_WITHIN + Duration::from_millis(300));
        let mut late = Client::connect(&pipe);
        assert!(late.send(&hello_line(&token)).is_some_and(|r| r.get("result").is_some()), "no room was made");
        drop((silent, server));
    }

    #[test]
    fn a_frame_waits_for_the_next_call_no_longer_than_it_should() {
        let file = std::env::temp_dir().join(format!("opal-api-pause-{}.json", std::process::id()));
        let mut server = Server::start(file.clone(), egui::Context::default()).unwrap();
        let (pipe, token) = info(&file);
        let mut st = AppState::new();
        let ctx = egui::Context::default();
        let mut client = Client::connect(&pipe);
        client.send(&hello_line(&token)).unwrap();
        // A script that thinks between its calls, longer than a tick of the system's timer
        // (which a wait for the next call must not take).
        let calls = std::thread::spawn(move || {
            for i in 0..20 {
                client.send(&format!(r#"{{"jsonrpc":"2.0","id":{i},"method":"documents"}}"#)).unwrap();
                let think = Instant::now() + Duration::from_millis(25);
                while Instant::now() < think {
                    std::hint::spin_loop();
                }
            }
        });
        let mut slow = Vec::new();
        while !calls.is_finished() {
            let t = Instant::now();
            server.serve(&mut st, &ctx, false);
            if t.elapsed() >= Duration::from_millis(8) {
                slow.push(t.elapsed());
            }
        }
        calls.join().unwrap();
        // Each frame that serves a call takes it and the wait for one more (1 ms), not a
        // tick (15.6 ms on Windows) on top. (A few, on a busy computer, are held up anyway.)
        assert!(slow.len() <= 5, "{} of the frames serving 20 calls were slow: {slow:?}", slow.len());
    }

    #[test]
    fn a_frame_carries_out_the_calls_that_follow_one_another() {
        let file = std::env::temp_dir().join(format!("opal-api-burst-{}.json", std::process::id()));
        let mut server = Server::start(file.clone(), egui::Context::default()).unwrap();
        let (pipe, token) = info(&file);
        let mut st = AppState::new();
        let ctx = egui::Context::default();
        let mut client = Client::connect(&pipe);
        client.send(&hello_line(&token)).unwrap();
        let calls = std::thread::spawn(move || {
            for i in 0..200 {
                let r = client.send(&format!(r#"{{"jsonrpc":"2.0","id":{i},"method":"documents"}}"#)).unwrap();
                assert!(r.get("result").is_some(), "{r}");
            }
        });
        // Frames 16 ms apart, as vsync paces them.
        let mut frames = 0;
        while !calls.is_finished() {
            server.serve(&mut st, &ctx, false);
            frames += 1;
            std::thread::sleep(std::time::Duration::from_millis(16));
        }
        calls.join().unwrap();
        assert!(frames < 40, "200 calls took {frames} frames");
    }

    #[test]
    fn a_script_s_changes_to_an_image_become_one_history_state_when_it_ends() {
        let file = std::env::temp_dir().join(format!("opal-api-script-{}.json", std::process::id()));
        let mut server = Server::start(file.clone(), egui::Context::default()).unwrap();
        let (pipe, token) = info(&file);
        let mut st = AppState::new();
        // A script (its hello names it), then a client that is none (an assistant, say).
        for script in [Some("go.py"), None] {
            let (pipe, token) = (pipe.clone(), token.clone());
            with_client(&mut server, &mut st, move || {
                let mut client = Client::connect(&pipe);
                let named = script.map_or(String::new(), |s| format!(r#","script":"{s}""#));
                let hello =
                    format!(r#"{{"jsonrpc":"2.0","id":1,"method":"hello","params":{{"token":"{token}"{named}}}}}"#);
                client.send(&hello).unwrap();
                if script.is_some() {
                    client
                        .send(r#"{"jsonrpc":"2.0","id":2,"method":"new","params":{"width":64,"height":32}}"#)
                        .unwrap();
                }
                for (i, name) in ["A", "B"].iter().enumerate() {
                    let call = format!(
                        r#"{{"jsonrpc":"2.0","id":{},"method":"new_layer","params":{{"name":"{name}"}}}}"#,
                        i + 3
                    );
                    assert!(client.send(&call).unwrap().get("result").is_some());
                }
            });
            // Its end is told once its connection has closed.
            let ctx = egui::Context::default();
            let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
            let joined = |st: &AppState| st.docs[0].history.undo_label() == Some("Script: go.py");
            while script.is_some() && !joined(&st) && std::time::Instant::now() < until {
                server.serve(&mut st, &ctx, false);
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
        }
        let h = &st.docs[0].history;
        let labels: Vec<&str> = h.path_from_root(h.current()).iter().map(|&n| h.node(n).label.as_str()).collect();
        assert_eq!(labels[1..], ["Script: go.py", "New layer", "New layer"], "{labels:?}");
        let names: Vec<&str> = h.doc().layers.iter().map(|l| l.name.as_str()).collect();
        assert_eq!(names[names.len() - 4..], ["A", "B", "A", "B"]);
    }

    #[test]
    fn a_step_a_client_begins_and_ends_is_one_history_state_named_as_it_says() {
        let file = std::env::temp_dir().join(format!("opal-api-step-{}.json", std::process::id()));
        let mut server = Server::start(file.clone(), egui::Context::default()).unwrap();
        let (pipe, token) = info(&file);
        let mut st = AppState::new();
        let ctx = egui::Context::default();
        let call = |id: u64, method: &str, params: &str| {
            format!(r#"{{"jsonrpc":"2.0","id":{id},"method":"{method}","params":{params}}}"#)
        };
        // A client that is no script (each call its own state, but for its steps), then a
        // script (its run one state, but for its steps).
        for script in [None, Some("go.py")] {
            let (pipe, token, call) = (pipe.clone(), token.clone(), call);
            with_client(&mut server, &mut st, move || {
                let mut client = Client::connect(&pipe);
                let named = script.map_or(String::new(), |s| format!(r#","script":"{s}""#));
                client.send(&format!(
                    r#"{{"jsonrpc":"2.0","id":1,"method":"hello","params":{{"token":"{token}"{named}}}}}"#
                ));
                let layer = call(9, "new_layer", "{}");
                let ok = |client: &mut Client, line: String| {
                    let r = client.send(&line).unwrap();
                    assert!(r.get("result").is_some(), "{line}: {r}");
                };
                if script.is_none() {
                    ok(&mut client, call(2, "new", r#"{"width":8,"height":8}"#));
                    ok(&mut client, call(3, "begin_step", r#"{"name":"Two layers"}"#));
                    ok(&mut client, call(4, "begin_step", r#"{"name":"Inner"}"#));
                    ok(&mut client, layer.clone());
                    ok(&mut client, call(5, "end_step", "{}"));
                    ok(&mut client, layer.clone());
                    ok(&mut client, call(6, "end_step", "{}"));
                    ok(&mut client, call(7, "begin_step", r#"{"name":"Just one"}"#));
                    ok(&mut client, layer.clone());
                    ok(&mut client, call(8, "end_step", "{}"));
                    ok(&mut client, layer.clone());
                    let r = client.send(&call(10, "end_step", "{}")).unwrap();
                    assert!(r["error"]["message"].as_str().unwrap().contains("no step"), "{r}");
                    // Left open when the connection ends: ended with it.
                    ok(&mut client, call(11, "begin_step", r#"{"name":"Left open"}"#));
                    ok(&mut client, layer.clone());
                } else {
                    ok(&mut client, layer.clone());
                    ok(&mut client, layer.clone());
                    ok(&mut client, call(3, "begin_step", r#"{"name":"S"}"#));
                    ok(&mut client, layer.clone());
                    ok(&mut client, call(4, "end_step", "{}"));
                    ok(&mut client, layer.clone());
                    // A step the script undoes: gone with the run, not left beside it.
                    ok(&mut client, call(5, "begin_step", r#"{"name":"Undone"}"#));
                    ok(&mut client, layer.clone());
                    ok(&mut client, call(6, "undo", "{}"));
                    ok(&mut client, call(7, "end_step", "{}"));
                    ok(&mut client, layer.clone());
                }
            });
            let until = std::time::Instant::now() + Duration::from_secs(5);
            let want: &[&str] = if script.is_none() {
                &["Two layers", "Just one", "New layer", "Left open"]
            } else {
                &["Two layers", "Just one", "New layer", "Left open", "Script: go.py", "S", "Script: go.py"]
            };
            let labels = |st: &AppState| {
                let h = &st.docs[0].history;
                h.path_from_root(h.current()).iter().skip(1).map(|&n| h.node(n).label.clone()).collect::<Vec<_>>()
            };
            while labels(&st) != want && std::time::Instant::now() < until {
                server.serve(&mut st, &ctx, false);
                std::thread::sleep(Duration::from_millis(2));
            }
            assert_eq!(labels(&st), want);
        }
    }

    #[test]
    fn a_step_shows_while_open_ends_when_forgotten_and_takes_only_its_own_arguments() {
        let file = std::env::temp_dir().join(format!("opal-api-open-{}.json", std::process::id()));
        let mut server = Server::start(file.clone(), egui::Context::default()).unwrap();
        let (pipe, token) = info(&file);
        let mut st = AppState::new();
        let ctx = egui::Context::default();
        let mut client = Client::connect(&pipe);
        client.send(&hello_line(&token)).unwrap();
        let ask = |client: &mut Client, line: &str, st: &mut AppState, server: &mut Server| {
            let (send, got) = std::sync::mpsc::channel();
            std::thread::scope(|scope| {
                let line = line.to_owned();
                scope.spawn(move || send.send(client.send(&line)).unwrap());
                loop {
                    server.serve(st, &ctx, false);
                    if let Ok(r) = got.try_recv() {
                        return r.unwrap();
                    }
                    std::thread::sleep(Duration::from_millis(2));
                }
            })
        };
        ask(
            &mut client,
            r#"{"jsonrpc":"2.0","id":1,"method":"new","params":{"width":8,"height":8}}"#,
            &mut st,
            &mut server,
        );
        // Arguments as for any method: none it does not take.
        let wrong = ask(
            &mut client,
            r#"{"jsonrpc":"2.0","id":2,"method":"end_step","params":{"name":"x"}}"#,
            &mut st,
            &mut server,
        );
        assert!(wrong["error"]["message"].as_str().unwrap().contains("\"name\""), "{wrong}");
        ask(
            &mut client,
            r#"{"jsonrpc":"2.0","id":3,"method":"begin_step","params":{"name":"Forgotten"}}"#,
            &mut st,
            &mut server,
        );
        assert_eq!(server.open_steps(), ["Forgotten"]);
        ask(&mut client, r#"{"jsonrpc":"2.0","id":4,"method":"new_layer","params":{}}"#, &mut st, &mut server);
        // Unused past its time: the next call is not part of it.
        std::thread::sleep(STEP_IDLE + Duration::from_millis(100));
        ask(&mut client, r#"{"jsonrpc":"2.0","id":5,"method":"new_layer","params":{}}"#, &mut st, &mut server);
        server.serve(&mut st, &ctx, false);
        assert!(server.open_steps().is_empty(), "still shown as open");
        let h = &st.docs[0].history;
        let labels: Vec<&str> = h.path_from_root(h.current()).iter().map(|&n| h.node(n).label.as_str()).collect();
        assert_eq!(labels, ["New image", "Forgotten", "New layer"]);
    }

    #[test]
    fn every_method_about_the_connection_is_answered_by_it() {
        let file = std::env::temp_dir().join(format!("opal-api-conn-{}.json", std::process::id()));
        let server = Server::start(file, egui::Context::default()).unwrap();
        let mut step = None;
        for m in crate::api::methods().iter().filter(|m| m.role == Role::Connection) {
            let mut params = Map::new();
            for p in m.params {
                params.insert(p.name.to_owned(), json!("A name"));
            }
            let answer = connection_answer(&server.shared, &m.name, &params, &mut step);
            assert!(!answer.is_err_and(|(_, why)| why.contains("not answered")), "{} is not answered", m.name);
        }
    }

    #[test]
    fn requests_wait_while_the_user_is_busy_and_are_refused_while_quitting() {
        let file = std::env::temp_dir().join(format!("opal-api-busy-{}.json", std::process::id()));
        let mut server = Server::start(file.clone(), egui::Context::default()).unwrap();
        let (pipe, token) = info(&file);
        let mut st = AppState::new();
        let ctx = egui::Context::default();
        let mut client = Client::connect(&pipe);
        client.send(&hello_line(&token)).unwrap();
        // The second asked only once told to (a frame serving the first would otherwise
        // carry it out too, before the quit).
        let (go, told) = std::sync::mpsc::channel::<()>();
        let asked = std::thread::spawn(move || {
            let first = client.send(r#"{"jsonrpc":"2.0","id":2,"method":"documents"}"#).unwrap();
            told.recv().unwrap();
            let second = client.send(r#"{"jsonrpc":"2.0","id":3,"method":"documents"}"#).unwrap();
            (first, second)
        });
        // Busy: the request waits.
        for _ in 0..50 {
            server.serve(&mut st, &ctx, true);
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        assert!(!asked.is_finished(), "answered while the user was busy");
        server.serve(&mut st, &ctx, false);
        // Quitting: refused.
        st.exit_requested = true;
        go.send(()).unwrap();
        while !asked.is_finished() {
            server.serve(&mut st, &ctx, true);
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        let (first, second) = asked.join().unwrap();
        assert!(first.get("result").is_some(), "{first}");
        assert!(second["error"]["message"].as_str().unwrap().contains("closing"), "{second}");
    }

    #[test]
    fn files_opened_with_an_editor_running_open_in_it() {
        let _turn = crate::background::testing::turn();
        let file = std::env::temp_dir().join(format!("opal-api-hand-{}.json", std::process::id()));
        let mut server = Server::start(file.clone(), egui::Context::default()).unwrap();
        // Another editor would not serve scripts beside it.
        assert!(Server::start(file.clone(), egui::Context::default()).is_err());
        let png = std::env::temp_dir().join(format!("opal-api-hand-{}.png", std::process::id()));
        let mut doc = opal_core::document::Document::new(8, 8);
        let mut ids = opal_core::document::IdGen::default();
        let layer = opal_core::document::Layer::new_raster(ids.layer(), ids.object(), "l", Default::default());
        doc.insert(opal_core::document::Position { parent: None, index: 0 }, layer).unwrap();
        opal_io::image_files::export(&doc, &png, &opal_io::image_files::ExportFormat::Png { sixteen_bit: false }, None)
            .unwrap();
        assert!(hand_to_running(&file, std::slice::from_ref(&png)));
        let mut st = AppState::new();
        let ctx = egui::Context::default();
        for _ in 0..500 {
            server.serve(&mut st, &ctx, false);
            if !st.docs.is_empty() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(st.docs.len(), 1, "not opened");
        std::fs::remove_file(&png).ok();
        drop(server);
        assert!(!hand_to_running(&file, &[png]), "handed to an editor that is gone");
    }

    /// The Python to run the package's test with: `OPAL_PYTHON`, or `python` if it runs.
    use crate::console::test_python as python;

    #[test]
    fn the_python_package_drives_the_editor() {
        let Some(python) = python() else {
            eprintln!("No Python found (set OPAL_PYTHON): the package was not tried.");
            return;
        };
        let _turn = crate::background::testing::turn();
        let file = std::env::temp_dir().join(format!("opal-api-py-{}.json", std::process::id()));
        let mut server = Server::start(file.clone(), egui::Context::default()).unwrap();
        let mut st = AppState::new();
        let png = std::env::temp_dir().join(format!("opal-api-py-{}.png", std::process::id()));
        let script = r#"
import ast, json, keyword, os, pathlib, sys
import opal
# The stubs are Python.
ast.parse(pathlib.Path(opal.__file__).with_suffix(".pyi").read_text(encoding="utf-8"))
class Count:
    # Like a numpy integer: not an int, but one in all but name.
    def __index__(self):
        return 320
with opal.connect() as editor:
    # No method of the editor's is hidden by the client's own, or is a keyword.
    names = [m["name"] for m in editor.methods()]
    own_way = {"methods", "pixels", "put_pixels", "release_pixels"}
    assert not [n for n in names if n not in own_way and hasattr(opal.Editor, n)], names
    assert not [n for n in names if keyword.iskeyword(n)], names
    # A step, as the client gives it: two layers, one state; and one whose body fails, its
    # own error come through, the step ended all the same.
    stepped = editor.new(width=4, height=4)["document"]
    with editor.step("Sky and sea"):
        editor.new_layer(name="Sky")
        editor.new_layer(name="Sea")
    try:
        with editor.step("Fails"):
            editor.new_layer(name="Lost")
            raise RuntimeError("the script's own")
    except RuntimeError as e:
        assert str(e) == "the script's own", e
    labels = [s["label"] for s in editor.history(document=stepped)["states"]]
    assert labels == ["New image", "Sky and sea", "Fails"], labels
    editor.close(document=stepped, discard=True)
    doc = editor.new(width=Count(), height=200)["document"]
    sky = editor.new_layer(name="Sky")["layer"]
    editor.toggle_visible(layers=[sky], visible=False)
    layers = editor.layers(document=doc)
    # The answer to a call cut short (interrupted while it waited) still on its way is
    # passed over, not taken for the next call's.
    editor._pipe._send((json.dumps({"jsonrpc": "2.0", "id": 10**6, "method": "documents"}) + "\n").encode())
    assert editor.layers(document=doc) == layers, "took another call's answer"
    try:
        editor.rename(layer=999999, name="x")
        refused = None
    except opal.OpalError as e:
        refused = str(e)
    # A relative path, against the current folder.
    os.chdir(pathlib.Path(sys.argv[1]).parent)
    # A connection file without a token: said so, as the package's own error.
    bad = pathlib.Path(sys.argv[1]).with_suffix(".json")
    bad.write_text("{}", encoding="utf-8")
    try:
        opal.connect(bad)
        raise SystemExit("connected without a token")
    except opal.OpalError as e:
        assert "pipe and a token" in str(e), e
    bad.unlink()
    exported = editor.export(path=pathlib.Path(sys.argv[1]).name)
    editor.close(document=doc, discard=True)
    # Pixels, through shared memory, as numpy arrays.
    import numpy as np
    pic = editor.new(width=4, height=3, background="transparent")["document"]
    red = np.zeros((3, 4, 4), dtype=np.uint8)
    red[..., 0] = 255
    red[..., 3] = 255
    red[0, 0] = [0, 0, 255, 128]
    put = editor.put_pixels(red, name="Red")
    back = editor.pixels()
    assert back.shape == (3, 4, 4) and (back == red).all(), back
    linear = editor.pixels(layer=put, format="linear")
    assert linear.dtype == np.float32 and abs(linear[1, 1, 0] - 1) < 1e-3 and abs(linear[0, 0, 3] - 128 / 255) < 1e-2
    sixteen = editor.pixels(format="srgb16")
    assert sixteen.dtype == np.uint16 and sixteen[1, 1, 0] == 65535 and sixteen[1, 1, 1] == 0, sixteen[1, 1]
    # Into the layer, replacing its pixels there; and opaque ones, from floats.
    editor.put_pixels(np.array([[[0, 255, 0, 255]]], dtype=np.uint8), layer=put, x=2, y=1)
    assert (editor.pixels(x=2, y=1, width=1, height=1) == [0, 255, 0, 255]).all()
    editor.put_pixels(np.ones((1, 2, 3)), layer=put, x=0, y=2)
    assert (editor.pixels(x=0, y=2, width=2, height=1) == 255).all()
    editor.close(document=pic, discard=True)
    # As elsewhere, with no shared memory: in the lines, a band of rows a request.
    opal._SHARED = False
    wide = editor.new(width=600, height=400, background="transparent")["document"]
    big = (np.arange(600 * 400 * 4) % 251).astype(np.uint8).reshape(400, 600, 4)
    big[..., 3] = 255
    banded = editor.put_pixels(big, name="Banded")
    assert (editor.pixels(layer=banded) == big).all()
    assert len(editor.layers(document=wide)) == 2, "a layer a band"
    opal._SHARED = True
    editor.close(document=wide, discard=True)
    try:
        editor.put_pixels(np.zeros((0, 4, 4), np.uint8))
        raise SystemExit("put no pixels without a word")
    except opal.OpalError as e:
        assert "no pixels" in str(e), e
    unreadable = pathlib.Path(sys.argv[1]).with_suffix(".dir")
    unreadable.mkdir(exist_ok=True)
    try:
        opal.connect(unreadable)
        raise SystemExit("connected through a folder")
    except opal.OpalError as e:
        assert "not readable" in str(e), e
    unreadable.rmdir()
    left = editor.documents()
    # Changes to an image the script leaves open, one history state once it ends.
    editor.new(width=8, height=8)
    editor.new_layer(name="One")
    editor.new_layer(name="Two")
    print(json.dumps({"layers": layers, "refused": refused, "exported": exported, "methods": len(editor.methods()), "left": left}))
"#;
        // Run from a file, as a script is (its name names its history state).
        let file_of_script = std::env::temp_dir().join(format!("opal-api-py-{}.py", std::process::id()));
        std::fs::write(&file_of_script, script).unwrap();
        // As Run Script and headless runs start it.
        let child = crate::console::script_command(&python, file_of_script.as_os_str(), &[png.clone().into()], &file)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        // The script runs while this thread serves it.
        let ctx = egui::Context::default();
        let done = std::thread::spawn(move || child.wait_with_output().unwrap());
        while !done.is_finished() {
            server.serve(&mut st, &ctx, false);
            st.poll_writes(false);
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        let out = done.join().unwrap();
        std::fs::remove_file(&file_of_script).ok();
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(out.status.success(), "the script failed:\n{stderr}");
        let label = format!("Script: {}", file_of_script.file_name().unwrap().to_string_lossy());
        let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while st.docs[0].history.undo_label() != Some(label.as_str()) && std::time::Instant::now() < until {
            server.serve(&mut st, &ctx, false);
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        let h = &st.docs[0].history;
        assert_eq!((h.undo_label(), h.path_from_root(h.current()).len()), (Some(label.as_str()), 2));
        let said: Value = serde_json::from_slice(&out.stdout).unwrap_or_else(|_| panic!("{stderr}"));
        assert_eq!(said["layers"][0]["name"], json!("Sky"), "{said}");
        assert_eq!(said["layers"][0]["visible"], json!(false));
        assert!(said["refused"].as_str().is_some_and(|r| r.contains("no layer")), "{said}");
        assert_eq!(said["exported"]["path"], json!(png.display().to_string()));
        assert_eq!(said["methods"].as_u64(), Some(crate::api::methods().len() as u64));
        assert_eq!(said["left"], json!([]), "close closed the connection, not the image");
        assert!(std::fs::metadata(&png).is_ok_and(|m| m.len() > 0));
        std::fs::remove_file(&png).ok();
    }

    #[test]
    fn a_client_says_hello_with_the_token_then_calls_the_api() {
        // It exports, on the pools another test may hold.
        let _turn = crate::background::testing::turn();
        let file = std::env::temp_dir().join(format!("opal-api-test-{}.json", std::process::id()));
        let mut server = Server::start(file.clone(), egui::Context::default()).unwrap();
        let mut info = String::new();
        std::fs::File::open(&file).unwrap().read_to_string(&mut info).unwrap();
        let info: Value = serde_json::from_str(&info).unwrap();
        let token = info["token"].as_str().unwrap().to_owned();
        assert_eq!(token.len(), 64);
        let pipe = info["pipe"].as_str().unwrap();
        assert!(pipe.contains("opal-studio-"));
        let mut st = AppState::new();

        // Without the token: refused, and the connection dropped.
        let mut stranger = Client::connect(pipe);
        let refused = stranger.send(r#"{"jsonrpc":"2.0","id":1,"method":"documents"}"#).unwrap();
        assert_eq!(refused["error"]["code"], json!(NOT_ALLOWED));
        assert!(stranger.send(r#"{"jsonrpc":"2.0","id":2,"method":"documents"}"#).is_none(), "still connected");
        let mut liar = Client::connect(pipe);
        let wrong = liar.send(r#"{"jsonrpc":"2.0","id":1,"method":"hello","params":{"token":"0000"}}"#).unwrap();
        assert_eq!(wrong["error"]["code"], json!(NOT_ALLOWED));

        let mut client = Client::connect(pipe);
        let path = std::env::temp_dir().join(format!("opal-api-test-{}.png", std::process::id()));
        let png = path.display().to_string();
        with_client(&mut server, &mut st, move || {
            let hello = format!(r#"{{"jsonrpc":"2.0","id":1,"method":"hello","params":{{"token":"{token}"}}}}"#);
            assert_eq!(client.send(&hello).unwrap()["result"]["protocol"], json!(PROTOCOL));
            let made =
                client.send(r#"{"jsonrpc":"2.0","id":2,"method":"new","params":{"width":40,"height":30}}"#).unwrap();
            assert!(made["result"]["document"].is_u64(), "{made}");
            let bad =
                client.send(r#"{"jsonrpc":"2.0","id":3,"method":"new","params":{"width":0,"height":30}}"#).unwrap();
            assert_eq!((bad["id"].as_u64(), bad["error"]["code"].as_i64()), (Some(3), Some(FAILED)), "{bad}");
            assert!(client.send("not json").unwrap()["error"]["code"] == json!(PARSE_ERROR));
            // Answered once the file is written.
            let export = json!({ "jsonrpc": "2.0", "id": 4, "method": "export", "params": { "path": png } });
            let exported = client.send(&export.to_string()).unwrap();
            assert_eq!(exported["result"]["path"], json!(png), "{exported}");
        });
        assert!(std::fs::metadata(&path).is_ok_and(|m| m.len() > 0), "not written");
        std::fs::remove_file(&path).ok();
        drop(server);
        assert!(!file.exists(), "the connection file outlived the server");
    }
}
