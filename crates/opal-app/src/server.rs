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
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};

/// The version of the protocol, told in `hello`'s answer.
pub const PROTOCOL: u32 = 1;

/// A request passed to the interface thread, and where its answer goes.
struct Request {
    method: String,
    params: Map<String, Value>,
    answer: Sender<Result<Value, String>>,
}

/// What the connections share with the interface thread.
struct Shared {
    token: String,
    requests: Mutex<Sender<Request>>,
    /// How many connections of this generation have said hello.
    clients: AtomicUsize,
    /// Moves on to drop every connection: each made in an earlier one goes at its next
    /// request, unanswered.
    generation: AtomicU64,
    ctx: egui::Context,
}

/// The API server: its listener thread, and the requests it has passed on.
pub struct Server {
    shared: Arc<Shared>,
    requests: Receiver<Request>,
    /// The answers awaited from files being written, by the number of the call.
    later: HashMap<u64, Sender<Result<Value, String>>>,
    /// Where the pipe's name and the token were written.
    pub file: PathBuf,
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

impl Server {
    /// Starts listening on a pipe of this process's own, and writes its name and a new
    /// token to `file` (the user's [`connection_file`], or another, for tests).
    pub fn start(file: PathBuf, ctx: egui::Context) -> std::io::Result<Server> {
        let name = format!("opal-studio-{}.sock", std::process::id());
        let listener = ListenerOptions::new().name(name.as_str().to_ns_name::<GenericNamespaced>()?).create_sync()?;
        let token = new_token()?;
        if let Some(dir) = file.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let pipe = if cfg!(windows) { format!(r"\\.\pipe\{name}") } else { name.clone() };
        let info = json!({ "pipe": pipe, "token": token, "pid": std::process::id(), "protocol": PROTOCOL });
        write_private(&file, info.to_string().as_bytes())?;
        let (send, requests) = channel();
        let shared = Arc::new(Shared {
            token,
            requests: Mutex::new(send),
            clients: AtomicUsize::new(0),
            generation: AtomicU64::new(0),
            ctx,
        });
        let listening = shared.clone();
        std::thread::Builder::new().name("api listener".into()).spawn(move || {
            for conn in listener.incoming().filter_map(Result::ok) {
                let shared = listening.clone();
                let _ = std::thread::Builder::new()
                    .name("api connection".into())
                    .spawn(move || serve_connection(conn, &shared));
            }
        })?;
        Ok(Server { shared, requests, later: HashMap::new(), file })
    }

    /// How many clients are connected (have said hello).
    pub fn clients(&self) -> usize {
        self.shared.clients.load(Ordering::Relaxed)
    }

    /// Drops every connection: each goes at its next request, unanswered, and no longer
    /// counts as connected.
    pub fn disconnect_all(&self) {
        self.shared.generation.fetch_add(1, Ordering::Relaxed);
        self.shared.clients.store(0, Ordering::Relaxed);
    }

    /// Carries out the requests passed on since the last frame, and answers those whose
    /// files have been written. Called by the interface thread every frame.
    pub fn serve(&mut self, st: &mut AppState, ctx: &egui::Context) {
        while let Ok(r) = self.requests.try_recv() {
            match st.call(&r.method, &r.params, ctx) {
                Reply::Now(result) => {
                    let _ = r.answer.send(result);
                }
                Reply::Later(n) => {
                    self.later.insert(n, r.answer);
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
        // Told no more: nothing is listening.
        let _ = std::fs::remove_file(&self.file);
    }
}

/// Writes `bytes` to `path`, readable by the user alone where the system lets a program say so
/// (on Windows, a file in the user's own profile already is).
fn write_private(path: &PathBuf, bytes: &[u8]) -> std::io::Result<()> {
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

/// Serves one connection: `hello` first, then each request in turn.
fn serve_connection(conn: Stream, shared: &Shared) {
    let mut reader = BufReader::new(&conn);
    let mut writer = &conn;
    let mut hello = false;
    let generation = shared.generation.load(Ordering::Relaxed);
    let mut line = String::new();
    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        if shared.generation.load(Ordering::Relaxed) != generation {
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
                        if params.get("token").and_then(Value::as_str) == Some(shared.token.as_str()) {
                            if !hello {
                                hello = true;
                                shared.clients.fetch_add(1, Ordering::Relaxed);
                                shared.ctx.request_repaint();
                            }
                            Ok(json!({ "protocol": PROTOCOL, "editor": env!("CARGO_PKG_VERSION") }))
                        } else {
                            Err((NOT_ALLOWED, "The token is not this editor's.".to_owned()))
                        }
                    }
                    (Some(_), Some(_)) if !hello => {
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
        if !hello {
            // Refused: the connection goes.
            break;
        }
    }
    // Uncounted already if disconnected.
    if hello && shared.generation.load(Ordering::Relaxed) == generation {
        shared.clients.fetch_sub(1, Ordering::Relaxed);
        shared.ctx.request_repaint();
    }
}

/// Passes a request to the interface thread, and waits for its answer.
fn pass_on(shared: &Shared, method: String, params: Map<String, Value>) -> Result<Value, (i64, String)> {
    let (answer, answered) = channel();
    let sent = shared.requests.lock().expect("not poisoned").send(Request { method, params, answer });
    if sent.is_err() {
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
            server.serve(st, &ctx);
            st.poll_writes(false);
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        done.join().unwrap();
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
