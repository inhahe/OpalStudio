//! The API's local connection (see design.md, *Scripting*): JSON-RPC 2.0, one request or
//! response a line, over a named pipe (a Unix socket elsewhere) that only this computer can
//! reach.
//!
//! A connection's first request must be `hello`, with the token the editor wrote, beside the
//! pipe's name, to `api.json` in the user's settings folder (which only they can read): so
//! only the user's own programs can drive the editor. Every later request is handed to the
//! interface thread, which carries it out between frames ([`Server::serve`]) and answers it,
//! or, for a file being written, answers it once the file is.

use crate::api::Reply;
use crate::state::AppState;
use eframe::egui;
use interprocess::local_socket::{GenericNamespaced, ListenerOptions, Stream, prelude::*};
use serde_json::{Map, Value, json};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};

/// The version of the protocol, told in `hello`'s answer.
pub const PROTOCOL: u32 = 1;

/// The longest line read from a connection that has not said hello yet: a hello is short.
const STRANGER_LINE: usize = 4 << 10;
/// The longest line read from one that has.
const CLIENT_LINE: usize = 1 << 20;
/// How many connections may wait to say hello at once; more are dropped at once.
const MAX_STRANGERS: usize = 16;

/// A request passed to the interface thread, and where its answer goes.
struct Request {
    method: String,
    params: Map<String, Value>,
    answer: Sender<Result<Value, String>>,
}

/// Which connections count: those of the current generation (moving on drops every
/// connection made in an earlier one) that have said hello. One lock for both, so that a
/// disconnect cannot pass a connection saying hello or going.
#[derive(Default)]
struct Clients {
    generation: u64,
    connected: usize,
}

/// What the connections share with the interface thread.
struct Shared {
    /// The token a hello must give: replaced when everyone is disconnected, so that no one
    /// dropped can come back with it.
    token: Mutex<String>,
    requests: Sender<Request>,
    clients: Mutex<Clients>,
    /// How many connections have not said hello yet.
    strangers: AtomicUsize,
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
fn new_token() -> std::io::Result<String> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|e| std::io::Error::other(e.to_string()))?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

/// Connects to the pipe `info` (a connection file's) names, and says hello with its token:
/// the connection, if an editor is there to answer.
fn hello(info: &Value) -> Option<BufReader<Stream>> {
    let (pipe, token) = (info["pipe"].as_str()?, info["token"].as_str()?);
    let name = pipe.rsplit(['\\', '/']).next()?.to_ns_name::<GenericNamespaced>().ok()?;
    let mut conn = BufReader::new(connect(name)?);
    let hello = json!({ "jsonrpc": "2.0", "id": 0, "method": "hello", "params": { "token": token } });
    conn.get_mut().write_all(format!("{hello}\n").as_bytes()).ok()?;
    let mut line = String::new();
    conn.read_line(&mut line).ok()?;
    let answer: Value = serde_json::from_str(&line).ok()?;
    answer.get("result").is_some().then_some(conn)
}

/// Connects to pipe `name`, trying again for a moment while it is busy (accepting another
/// client, it has no instance free); `None` if no editor is there.
pub fn connect(name: interprocess::local_socket::Name<'_>) -> Option<Stream> {
    for _ in 0..20 {
        match Stream::connect(name.borrow()) {
            Ok(s) => return Some(s),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
            Err(_) => std::thread::sleep(std::time::Duration::from_millis(25)),
        }
    }
    None
}

/// The editor `file` tells of, if it is running: a connection to it, past hello.
fn running(file: &Path) -> Option<BufReader<Stream>> {
    hello(&serde_json::from_str(&std::fs::read_to_string(file).ok()?).ok()?)
}

/// Whether an editor is running as `file` tells (it answers hello).
pub fn is_running(file: &Path) -> bool {
    running(file).is_some()
}

/// Hands `files` to the editor already running (as `file` tells), to be opened there;
/// whether one was. Each goes as a notification, carried out but not answered, so that this
/// process need not wait for a dialog the user has open there.
pub fn hand_to_running(file: &Path, files: &[PathBuf]) -> bool {
    let Some(mut conn) = running(file) else { return false };
    for f in files {
        let path = std::path::absolute(f).unwrap_or_else(|_| f.clone());
        let open = json!({ "jsonrpc": "2.0", "method": "open", "params": { "path": path.display().to_string() } });
        if conn.get_mut().write_all(format!("{open}\n").as_bytes()).is_err() {
            return false;
        }
    }
    true
}

impl Server {
    /// Starts listening on a pipe of this process's own, then writes its name and a new
    /// token to `file` (the user's [`connection_file`], or another, for tests). Fails if an
    /// editor already running answers on the pipe `file` names: scripts reach the first.
    pub fn start(file: PathBuf, ctx: egui::Context) -> std::io::Result<Server> {
        if running(&file).is_some() {
            return Err(std::io::Error::other("another Opal Studio is running, which scripts connect to"));
        }
        // This process's own (a test makes several servers).
        static SERVERS: AtomicU64 = AtomicU64::new(0);
        let n = SERVERS.fetch_add(1, Ordering::Relaxed);
        let name = format!("opal-studio-{}-{n}.sock", std::process::id());
        let listener = ListenerOptions::new().name(name.as_str().to_ns_name::<GenericNamespaced>()?).create_sync()?;
        let pipe = if cfg!(windows) { format!(r"\\.\pipe\{name}") } else { name.clone() };
        let (send, requests) = channel();
        let shared = Arc::new(Shared {
            token: Mutex::new(new_token()?),
            requests: send,
            clients: Mutex::new(Clients::default()),
            strangers: AtomicUsize::new(0),
            ctx,
            file,
            pipe,
        });
        let listening = shared.clone();
        std::thread::Builder::new().name("api listener".into()).spawn(move || {
            for conn in listener.incoming() {
                // A failure to accept is waited out, not spun on (one that lasts would
                // otherwise take a core for the life of the editor).
                let Ok(conn) = conn else {
                    std::thread::sleep(std::time::Duration::from_millis(250));
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
        })?;
        // Told once there is someone listening.
        if let Some(dir) = shared.file.parent() {
            std::fs::create_dir_all(dir)?;
        }
        write_connection_file(&shared)?;
        Ok(Server { shared, requests, later: HashMap::new() })
    }

    /// How many clients are connected (have said hello).
    pub fn clients(&self) -> usize {
        self.shared.clients.lock().expect("not poisoned").connected
    }

    /// Drops every connection (each goes at its next request, unanswered, and no longer
    /// counts as connected), and makes a new token, so that none of them can come back
    /// with the one it had.
    pub fn disconnect_all(&self) {
        {
            let mut c = self.shared.clients.lock().expect("not poisoned");
            c.generation += 1;
            c.connected = 0;
        }
        if let Ok(token) = new_token() {
            *self.shared.token.lock().expect("not poisoned") = token;
            let _ = write_connection_file(&self.shared);
        }
        self.shared.ctx.request_repaint();
    }

    /// Carries out the requests passed on since the last frame, and answers those whose
    /// files have been written. Called by the interface thread every frame; while the user
    /// is `busy` with something (a dialog, a drag) that a request could change under them,
    /// the requests wait. Once the user has asked to quit, they are refused.
    pub fn serve(&mut self, st: &mut AppState, ctx: &egui::Context, busy: bool) {
        let quitting = st.exit_requested || st.exit_after_writes;
        if !busy || quitting {
            while let Ok(r) = self.requests.try_recv() {
                if quitting {
                    let _ = r.answer.send(Err("The editor is closing.".to_owned()));
                    continue;
                }
                match st.call(&r.method, &r.params, ctx) {
                    Reply::Now(result) => {
                        let _ = r.answer.send(result);
                    }
                    Reply::Later(n) => {
                        self.later.insert(n, r.answer);
                    }
                }
            }
        }
        for (n, result) in st.answers() {
            if let Some(answer) = self.later.remove(&n) {
                let _ = answer.send(result);
            }
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        // Told no more, unless the file has become another's.
        let mine = std::fs::read_to_string(&self.shared.file)
            .ok()
            .and_then(|t| serde_json::from_str::<Value>(&t).ok())
            .is_some_and(|info| {
                info["token"].as_str() == Some(self.shared.token.lock().expect("not poisoned").as_str())
            });
        if mine {
            let _ = std::fs::remove_file(&self.shared.file);
        }
    }
}

/// Writes the pipe's name and the token to the connection file.
fn write_connection_file(shared: &Shared) -> std::io::Result<()> {
    let token = shared.token.lock().expect("not poisoned").clone();
    let info = json!({ "pipe": shared.pipe, "token": token, "pid": std::process::id(), "protocol": PROTOCOL });
    write_private(&shared.file, info.to_string().as_bytes())
}

/// Writes `bytes` to `path`, readable by the user alone where the system lets a program say so
/// (on Windows, a file in the user's own profile already is).
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
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

/// Serves one connection: `hello` first, then each request in turn.
fn serve_connection(conn: Stream, shared: &Shared) {
    let mut reader = BufReader::new(&conn);
    let mut writer = &conn;
    // The generation it is of, once it has said hello.
    let mut hello: Option<u64> = None;
    while let Some(line) = read_line(&mut reader, if hello.is_some() { CLIENT_LINE } else { STRANGER_LINE }) {
        if hello.is_some_and(|g| shared.clients.lock().expect("not poisoned").generation != g) {
            // Disconnected by the user.
            break;
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
                        let token = shared.token.lock().expect("not poisoned").clone();
                        if params.get("token").and_then(Value::as_str) == Some(token.as_str()) {
                            if hello.is_none() {
                                let mut c = shared.clients.lock().expect("not poisoned");
                                c.connected += 1;
                                hello = Some(c.generation);
                                shared.strangers.fetch_sub(1, Ordering::Relaxed);
                                shared.ctx.request_repaint();
                            }
                            Ok(json!({ "protocol": PROTOCOL, "editor": env!("CARGO_PKG_VERSION") }))
                        } else {
                            Err((NOT_ALLOWED, "The token is not this editor's.".to_owned()))
                        }
                    }
                    (Some(_), Some(_)) if hello.is_none() => {
                        Err((NOT_ALLOWED, "Say hello first, with the token from api.json.".to_owned()))
                    }
                    (Some(method), Some(params)) => pass_on(shared, method, params),
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
    match hello {
        None => {
            shared.strangers.fetch_sub(1, Ordering::Relaxed);
        }
        Some(g) => {
            // Uncounted already if disconnected.
            let mut c = shared.clients.lock().expect("not poisoned");
            if c.generation == g {
                c.connected -= 1;
                shared.ctx.request_repaint();
            }
        }
    }
}

/// Passes a request to the interface thread, and waits for its answer.
fn pass_on(shared: &Shared, method: String, params: Map<String, Value>) -> Result<Value, (i64, String)> {
    let (answer, answered) = channel();
    if shared.requests.send(Request { method, params, answer }).is_err() {
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
    fn disconnecting_drops_everyone_and_their_token() {
        let file = std::env::temp_dir().join(format!("opal-api-drop-{}.json", std::process::id()));
        let server = Server::start(file.clone(), egui::Context::default()).unwrap();
        let (pipe, old) = info(&file);
        let mut client = Client::connect(&pipe);
        assert!(client.send(&hello_line(&old)).unwrap().get("result").is_some());
        assert_eq!(server.clients(), 1);
        server.disconnect_all();
        assert_eq!(server.clients(), 0);
        // Dropped at its next request, unanswered; and the old token is no use.
        assert!(client.send(r#"{"jsonrpc":"2.0","id":2,"method":"documents"}"#).is_none());
        let (_, new) = info(&file);
        assert_ne!(new, old, "the token stayed");
        let refused = Client::connect(&pipe).send(&hello_line(&old)).unwrap();
        assert_eq!(refused["error"]["code"], json!(NOT_ALLOWED));
        assert!(Client::connect(&pipe).send(&hello_line(&new)).unwrap().get("result").is_some());
        // A stranger's endless line is cut off, not kept.
        let mut flood = Client::connect(&pipe);
        assert!(flood.send(&"x".repeat(STRANGER_LINE * 4)).is_none(), "an endless line was read");
        drop(server);
        assert!(!file.exists());
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
        let asked = std::thread::spawn(move || {
            let first = client.send(r#"{"jsonrpc":"2.0","id":2,"method":"documents"}"#).unwrap();
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
    fn python() -> Option<String> {
        let python = std::env::var("OPAL_PYTHON").unwrap_or_else(|_| "python".to_owned());
        let runs = std::process::Command::new(&python).arg("--version").output().is_ok_and(|o| o.status.success());
        runs.then_some(python)
    }

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
        let package = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../python");
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
    assert not [n for n in names if n != "methods" and hasattr(opal.Editor, n)], names
    assert not [n for n in names if keyword.iskeyword(n)], names
    doc = editor.new(width=Count(), height=200)["document"]
    sky = editor.new_layer(name="Sky")["layer"]
    editor.toggle_visible(layers=[sky], visible=False)
    layers = editor.layers(document=doc)
    try:
        editor.rename(layer=999999, name="x")
        refused = None
    except opal.OpalError as e:
        refused = str(e)
    # A relative path, against this script's folder.
    os.chdir(pathlib.Path(sys.argv[1]).parent)
    exported = editor.export(path=pathlib.Path(sys.argv[1]).name)
    editor.close(document=doc, discard=True)
    left = editor.documents()
    print(json.dumps({"layers": layers, "refused": refused, "exported": exported, "methods": len(editor.methods()), "left": left}))
"#;
        let child = std::process::Command::new(&python)
            .arg("-c")
            .arg(script)
            .arg(&png)
            .env("PYTHONPATH", &package)
            .env("OPAL_STUDIO_API", &file)
            .env("PYTHONIOENCODING", "utf-8")
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
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(out.status.success(), "the script failed:\n{stderr}");
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
