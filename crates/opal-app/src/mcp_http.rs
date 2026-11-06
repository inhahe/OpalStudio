//! MCP over HTTP on this computer alone (see design.md, *Scripting*), for assistants whose
//! clients connect to a server by its URL rather than launch one: the "streamable HTTP"
//! transport, every request answered in JSON (nothing is streamed). Each MCP session talks
//! to the editor through a connection of its own, as the stdio shim does, so the editor
//! treats it as any other client (disconnecting, waiting while the user is busy).
//!
//! The HTTP is read strictly, and only as much of it as MCP needs: `POST` (and `DELETE`, to
//! end a session) on `/mcp`, bodies framed by `Content-Length` alone. A request is refused
//! unless its `Host` is this computer, and its `Origin` too if it has one (against DNS
//! rebinding, by which a web page reaches a local server under a name of its own), and it
//! gives the bearer token: kept in a file only the user can read, and the same from one
//! start to the next, so that a client is set up once.

use crate::mcp::Caller;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, SocketAddrV4, TcpListener, TcpStream};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The longest request line or header line read.
const LINE: usize = 8 << 10;
/// The most header lines a request may have.
const HEADERS: usize = 64;
/// The longest body read: as long as a line the editor's pipe reads.
const BODY: usize = 1 << 20;
/// How many connections are served at once; more are closed.
const CONNECTIONS: usize = 16;
/// How many sessions are kept; past that, the one used longest ago goes.
const SESSIONS: usize = 32;
/// How long a connection may wait for a request and take to send it (its every byte, however
/// they trickle in), before it is closed.
const IDLE: Duration = if cfg!(test) { Duration::from_secs(1) } else { Duration::from_secs(30) };

/// The port served on by default.
pub const PORT: u16 = 47860;

/// A request, as read.
#[derive(Debug)]
struct Request {
    method: String,
    path: String,
    /// By name, in lower case.
    headers: HashMap<String, String>,
    body: Vec<u8>,
    /// Whether the connection goes on after the answer.
    keep_alive: bool,
}

/// Why a request could not be read: the status to answer with (none if the connection just
/// ended or stalled), and why.
#[derive(Debug)]
struct Unreadable(Option<u16>, &'static str);

/// The next line from `reader`, ended by CR LF (not kept), at most [`LINE`] bytes, with no
/// control character but tabs: `None` if the connection ended before one began.
fn read_line(reader: &mut impl BufRead) -> Result<Option<String>, Unreadable> {
    let mut bytes = Vec::new();
    let read = reader
        .by_ref()
        .take(LINE as u64 + 2)
        .read_until(b'\n', &mut bytes)
        .map_err(|_| Unreadable(None, "The connection failed or stalled."))?;
    if read == 0 {
        return Ok(None);
    }
    if !bytes.ends_with(b"\r\n") {
        return Err(if bytes.len() > LINE {
            Unreadable(Some(431), "A line is too long.")
        } else {
            Unreadable(Some(400), "A line does not end in CR LF.")
        });
    }
    bytes.truncate(bytes.len() - 2);
    if bytes.iter().any(|&b| (b < 0x20 && b != b'\t') || b == 0x7f) {
        return Err(Unreadable(Some(400), "A line holds a control character."));
    }
    String::from_utf8(bytes).map(Some).map_err(|_| Unreadable(Some(400), "A line is not UTF-8."))
}

/// Whether `name` is an HTTP token (what a header's name must be).
fn is_token(name: &str) -> bool {
    !name.is_empty() && name.bytes().all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b))
}

/// Reads the next request from `reader`: `Ok(None)` if the connection ended before one.
fn read_request(reader: &mut impl BufRead) -> Result<Option<Request>, Unreadable> {
    let Some(first) = read_line(reader)? else { return Ok(None) };
    let mut parts = first.split(' ');
    let (Some(method), Some(path), Some(version), None) = (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(Unreadable(Some(400), "The request line is not METHOD PATH VERSION."));
    };
    let mut keep_alive = match version {
        "HTTP/1.1" => true,
        "HTTP/1.0" => false,
        _ => return Err(Unreadable(Some(505), "Only HTTP/1.1 and 1.0 are spoken.")),
    };
    if !is_token(method) {
        return Err(Unreadable(Some(400), "The method is not a word."));
    }
    let mut headers = HashMap::new();
    let mut lines = 0;
    loop {
        let Some(line) = read_line(reader)? else {
            return Err(Unreadable(None, "The connection ended in the headers."));
        };
        if line.is_empty() {
            break;
        }
        lines += 1;
        if lines > HEADERS {
            return Err(Unreadable(Some(431), "There are too many headers."));
        }
        let Some((name, value)) = line.split_once(':') else {
            // A folded line (one going on from the last) among them.
            return Err(Unreadable(Some(400), "A header line is not NAME: VALUE."));
        };
        if !is_token(name) {
            return Err(Unreadable(Some(400), "A header's name is not a word."));
        }
        let name = name.to_ascii_lowercase();
        let value = value.trim_matches([' ', '\t']).to_owned();
        let once = matches!(name.as_str(), "content-length" | "host" | "authorization" | "origin" | "mcp-session-id");
        if headers.insert(name, value).is_some() && once {
            return Err(Unreadable(Some(400), "A header that may be given once is given twice."));
        }
    }
    // Another framing would be another way to read where the body ends (request smuggling).
    if headers.contains_key("transfer-encoding") {
        return Err(Unreadable(Some(501), "A body is framed by Content-Length alone."));
    }
    let len = match headers.get("content-length") {
        None => 0,
        Some(v) if (1..=9).contains(&v.len()) && v.bytes().all(|b| b.is_ascii_digit()) => {
            v.parse::<usize>().expect("digits")
        }
        Some(_) => return Err(Unreadable(Some(400), "Content-Length is not a number.")),
    };
    if len > BODY {
        return Err(Unreadable(Some(413), "The body is longer than 1 MB."));
    }
    let mut body = vec![0; len];
    reader.read_exact(&mut body).map_err(|_| Unreadable(None, "The connection ended in the body."))?;
    if let Some(c) = headers.get("connection") {
        let says = |word: &str| c.split(',').any(|t| t.trim().eq_ignore_ascii_case(word));
        keep_alive = if says("close") { false } else { keep_alive || says("keep-alive") };
    }
    Ok(Some(Request { method: method.to_owned(), path: path.to_owned(), headers, body, keep_alive }))
}

/// An answer: its status, headers and body.
struct Answer {
    status: u16,
    headers: Vec<(&'static str, String)>,
    body: Vec<u8>,
}

impl Answer {
    /// A refusal, `why` in a JSON-RPC error with no id (as MCP has HTTP errors carry).
    fn refusal(status: u16, why: &str) -> Answer {
        let body = json!({ "jsonrpc": "2.0", "id": null, "error": { "code": -32600, "message": why } });
        Answer {
            status,
            headers: vec![("Content-Type", "application/json".into())],
            body: body.to_string().into_bytes(),
        }
    }
}

/// Writes `answer` to `out`, saying the connection closes unless `keep_alive`.
fn respond(out: &mut impl Write, answer: &Answer, keep_alive: bool) -> std::io::Result<()> {
    let reason = match answer.status {
        200 => "OK",
        202 => "Accepted",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        413 => "Content Too Large",
        415 => "Unsupported Media Type",
        431 => "Request Header Fields Too Large",
        501 => "Not Implemented",
        503 => "Service Unavailable",
        505 => "HTTP Version Not Supported",
        _ => "Error",
    };
    let mut head = format!("HTTP/1.1 {} {reason}\r\nContent-Length: {}\r\n", answer.status, answer.body.len());
    for (name, value) in &answer.headers {
        // Only values of the editor's own making, which never hold a line break.
        assert!(!value.contains(['\r', '\n']), "a header value with a line break");
        head += &format!("{name}: {value}\r\n");
    }
    if !keep_alive {
        head += "Connection: close\r\n";
    }
    head += "\r\n";
    out.write_all(head.as_bytes())?;
    out.write_all(&answer.body)?;
    out.flush()
}

/// An MCP session: what carries its calls to the editor, and when it was last used.
struct Session {
    caller: Box<dyn Caller + Send>,
    used: Instant,
}

/// What the connections share.
struct Shared {
    port: u16,
    token: String,
    /// Makes what carries a new session's calls to the editor.
    new_caller: Box<dyn Fn() -> Box<dyn Caller + Send> + Send + Sync>,
    sessions: Mutex<HashMap<String, Arc<Mutex<Session>>>>,
    connections: AtomicUsize,
    stop: AtomicBool,
}

/// The HTTP server: listening until dropped.
pub struct HttpServer {
    shared: Arc<Shared>,
    listener: Option<std::thread::JoinHandle<()>>,
}

impl HttpServer {
    /// Listens on `port` of this computer's own address (127.0.0.1; 0 for any port free),
    /// for clients giving `token`; each session's calls go to the editor through what
    /// `new_caller` makes.
    pub fn start(
        port: u16,
        token: String,
        new_caller: impl Fn() -> Box<dyn Caller + Send> + Send + Sync + 'static,
    ) -> std::io::Result<HttpServer> {
        let listener = TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, port))?;
        let port = listener.local_addr()?.port();
        let shared = Arc::new(Shared {
            port,
            token,
            new_caller: Box::new(new_caller),
            sessions: Mutex::new(HashMap::new()),
            connections: AtomicUsize::new(0),
            stop: AtomicBool::new(false),
        });
        let listening = shared.clone();
        let listen = move || {
            for conn in listener.incoming() {
                if listening.stop.load(Ordering::Relaxed) {
                    break;
                }
                // A failure to accept is waited out, not spun on.
                let Ok(conn) = conn else {
                    std::thread::sleep(Duration::from_millis(250));
                    continue;
                };
                if listening.connections.load(Ordering::Relaxed) >= CONNECTIONS {
                    continue;
                }
                listening.connections.fetch_add(1, Ordering::Relaxed);
                let shared = listening.clone();
                let started = std::thread::Builder::new().name("mcp http connection".into()).spawn(move || {
                    serve(conn, &shared);
                    shared.connections.fetch_sub(1, Ordering::Relaxed);
                });
                if started.is_err() {
                    listening.connections.fetch_sub(1, Ordering::Relaxed);
                }
            }
        };
        let thread = std::thread::Builder::new().name("mcp http".into()).spawn(listen)?;
        Ok(HttpServer { shared, listener: Some(thread) })
    }

    /// The port it listens on.
    pub fn port(&self) -> u16 {
        self.shared.port
    }
}

impl Drop for HttpServer {
    /// Stops listening (the connections still open are refused at their next request).
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Relaxed);
        // Wakes the listener, waiting for a connection, to see it has stopped.
        let _ = TcpStream::connect_timeout(&SocketAddrV4::new(Ipv4Addr::LOCALHOST, self.shared.port).into(), IDLE);
        if let Some(thread) = self.listener.take() {
            let _ = thread.join();
        }
    }
}

/// A connection read against a deadline for the whole request: each read waits no longer
/// than what is left, so that a client trickling bytes cannot hold a place for ever.
struct Timed {
    conn: TcpStream,
    until: Instant,
}

impl Read for Timed {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let left = self.until.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(std::io::ErrorKind::TimedOut.into());
        }
        self.conn.set_read_timeout(Some(left))?;
        self.conn.read(buf)
    }
}

/// Serves one connection: request after request, while it is kept alive, each waited for
/// and read within [`IDLE`].
fn serve(conn: TcpStream, shared: &Shared) {
    let _ = conn.set_write_timeout(Some(IDLE));
    let _ = conn.set_nodelay(true);
    let Ok(mut out) = conn.try_clone() else { return };
    let mut reader = BufReader::new(Timed { conn, until: Instant::now() });
    loop {
        reader.get_mut().until = Instant::now() + IDLE;
        let request = match read_request(&mut reader) {
            Ok(Some(request)) => request,
            Ok(None) | Err(Unreadable(None, _)) => return,
            Err(Unreadable(Some(status), why)) => {
                let _ = respond(&mut out, &Answer::refusal(status, why), false);
                return;
            }
        };
        if shared.stop.load(Ordering::Relaxed) {
            let _ = respond(&mut out, &Answer::refusal(503, "MCP over HTTP has been turned off."), false);
            return;
        }
        let answer = answer(&request, shared);
        if respond(&mut out, &answer, request.keep_alive).is_err() || !request.keep_alive {
            return;
        }
    }
}

/// Whether `given` (an `Authorization` header) is the bearer token `token` (the scheme's
/// name in any case), compared in the same time however much of it matches.
fn token_matches(given: Option<&String>, token: &str) -> bool {
    let Some(given) = given.filter(|g| g.get(..7).is_some_and(|s| s.eq_ignore_ascii_case("bearer "))).map(|g| &g[7..])
    else {
        return false;
    };
    given.len() == token.len() && given.bytes().zip(token.bytes()).fold(0, |d, (a, b)| d | (a ^ b)) == 0
}

/// The answer to `request`.
fn answer(request: &Request, shared: &Shared) -> Answer {
    // This computer, by address or name: a page that reached it by another name (DNS
    // rebinding) gives that name.
    let here = [format!("127.0.0.1:{}", shared.port), format!("localhost:{}", shared.port)];
    let is_here = |h: &str| here.iter().any(|l| l.eq_ignore_ascii_case(h));
    if !request.headers.get("host").is_some_and(|h| is_here(h)) {
        return Answer::refusal(403, "The request is not addressed to this computer.");
    }
    // A web page's request says where the page is from: one served from anywhere is refused.
    if let Some(origin) = request.headers.get("origin")
        && !origin.strip_prefix("http://").is_some_and(is_here)
    {
        return Answer::refusal(403, "Requests from web pages are refused.");
    }
    if !token_matches(request.headers.get("authorization"), &shared.token) {
        let mut refusal = Answer::refusal(
            401,
            "The bearer token is not this editor's: Connect an Assistant (File › Scripts) shows it.",
        );
        refusal.headers.push(("WWW-Authenticate", "Bearer".into()));
        return refusal;
    }
    if request.path != "/mcp" {
        return Answer::refusal(404, "MCP is served at /mcp.");
    }
    match request.method.as_str() {
        "POST" => post(request, shared),
        "DELETE" => match request.headers.get("mcp-session-id") {
            Some(id) if shared.sessions.lock().expect("not poisoned").remove(id).is_some() => {
                Answer { status: 200, headers: Vec::new(), body: Vec::new() }
            }
            Some(_) => Answer::refusal(404, "There is no such session."),
            None => Answer::refusal(400, "Mcp-Session-Id names the session to end."),
        },
        _ => {
            // Nothing is streamed: the editor sends nothing it is not asked for.
            let mut refusal = Answer::refusal(405, "Only POST, and DELETE to end a session.");
            refusal.headers.push(("Allow", "POST, DELETE".into()));
            refusal
        }
    }
}

/// The answer to a `POST`: the MCP messages it carries, handled in their session (made by
/// an `initialize`).
fn post(request: &Request, shared: &Shared) -> Answer {
    if !request.headers.get("content-type").is_some_and(|t| t.to_ascii_lowercase().starts_with("application/json")) {
        return Answer::refusal(415, "The body is application/json.");
    }
    let Ok(body) = serde_json::from_slice::<Value>(&request.body) else {
        return Answer::refusal(400, "The body is not JSON.");
    };
    let batch = body.is_array();
    let messages = match body {
        Value::Array(messages) if !messages.is_empty() => messages,
        message @ Value::Object(_) => vec![message],
        _ => return Answer::refusal(400, "The body is a JSON-RPC message, or a list of them."),
    };
    let initializing = messages.iter().any(|m| m["method"] == json!("initialize"));
    let (session, made) = match request.headers.get("mcp-session-id") {
        Some(id) => match shared.sessions.lock().expect("not poisoned").get(id) {
            Some(session) => (session.clone(), None),
            None => return Answer::refusal(404, "There is no such session: initialize again."),
        },
        None if initializing => {
            let id = crate::server::new_token().unwrap_or_default();
            if id.is_empty() {
                return Answer::refusal(503, "No session could be made.");
            }
            let session = Arc::new(Mutex::new(Session { caller: (shared.new_caller)(), used: Instant::now() }));
            let mut sessions = shared.sessions.lock().expect("not poisoned");
            if sessions.len() >= SESSIONS
                && let Some(oldest) = sessions
                    .iter()
                    .min_by_key(|(_, s)| s.try_lock().map_or(Instant::now(), |s| s.used))
                    .map(|(id, _)| id.clone())
            {
                sessions.remove(&oldest);
            }
            sessions.insert(id.clone(), session.clone());
            (session, Some(id))
        }
        None => return Answer::refusal(400, "Mcp-Session-Id names the session (initialize first)."),
    };
    let mut session = session.lock().expect("not poisoned");
    session.used = Instant::now();
    // Requests and notifications are handled; a response (the editor asks nothing) is not.
    let answers: Vec<Value> = messages
        .iter()
        .filter(|m| m.get("method").is_some())
        .filter_map(|m| crate::mcp::handle(m, session.caller.as_mut()))
        .collect();
    let mut headers = made.map(|id| vec![("Mcp-Session-Id", id)]).unwrap_or_default();
    if answers.is_empty() {
        return Answer { status: 202, headers, body: Vec::new() };
    }
    headers.push(("Content-Type", "application/json".into()));
    let body = if batch { json!(answers) } else { answers.into_iter().next().expect("one") };
    Answer { status: 200, headers, body: body.to_string().into_bytes() }
}

/// The bearer token clients give, from `file` (made there, readable by the user alone, the
/// first time); `renew` makes a new one, so that clients given the old one are refused.
pub fn token(file: &Path, renew: bool) -> std::io::Result<String> {
    if !renew
        && let Ok(text) = std::fs::read_to_string(file)
        && text.trim().len() == 64
    {
        return Ok(text.trim().to_owned());
    }
    let token = crate::server::new_token()?;
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir)?;
    }
    crate::server::write_private(file, token.as_bytes())?;
    Ok(token)
}

/// Where the token is kept: beside the connection file.
pub fn token_file() -> Option<std::path::PathBuf> {
    crate::server::connection_file().map(|f| f.with_file_name("mcp-http-token.txt"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Map;

    /// Answers every tool call with its name and arguments.
    struct Echo;

    impl Caller for Echo {
        fn call(&mut self, method: &str, params: Map<String, Value>) -> Result<Value, String> {
            Ok(json!({ "called": method, "with": params }))
        }
    }

    const TOKEN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    /// Sends `raw` to `port` and reads what comes back until the connection closes (each
    /// request here asks it to), as the status and the head and body.
    fn exchange(port: u16, raw: &str) -> (u16, String, String) {
        let mut conn = TcpStream::connect(("127.0.0.1", port)).unwrap();
        conn.write_all(raw.as_bytes()).unwrap();
        let mut got = String::new();
        let _ = conn.read_to_string(&mut got);
        let (head, body) = got.split_once("\r\n\r\n").unwrap_or((&got, ""));
        let status = head.split(' ').nth(1).and_then(|s| s.parse().ok()).unwrap_or(0);
        (status, head.to_owned(), body.to_owned())
    }

    fn post(port: u16, session: Option<&str>, body: &str, extra: &str) -> (u16, String, String) {
        let session = session.map_or(String::new(), |s| format!("Mcp-Session-Id: {s}\r\n"));
        exchange(
            port,
            &format!(
                "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAuthorization: Bearer {TOKEN}\r\n\
                 Content-Type: application/json\r\nAccept: application/json, text/event-stream\r\n\
                 Connection: close\r\n{session}{extra}Content-Length: {}\r\n\r\n{body}",
                body.len()
            ),
        )
    }

    #[test]
    fn an_assistant_initializes_a_session_and_calls_a_tool() {
        let server = HttpServer::start(0, TOKEN.to_owned(), || Box::new(Echo)).unwrap();
        let port = server.port();
        let init = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18"}}"#;
        let (status, head, body) = post(port, None, init, "");
        assert_eq!(status, 200, "{head}\n{body}");
        let session = head.lines().find_map(|l| l.strip_prefix("Mcp-Session-Id: ")).expect("a session").to_owned();
        assert_eq!(serde_json::from_str::<Value>(&body).unwrap()["result"]["protocolVersion"], json!("2025-06-18"));
        // A notification: accepted, nothing to say.
        let (status, ..) = post(port, Some(&session), r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#, "");
        assert_eq!(status, 202);
        let call = r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"documents","arguments":{}}}"#;
        let (status, _, body) = post(port, Some(&session), call, "");
        assert_eq!(status, 200);
        let text = serde_json::from_str::<Value>(&body).unwrap()["result"]["content"][0]["text"].clone();
        let said: Value = serde_json::from_str(text.as_str().unwrap()).unwrap();
        assert_eq!(said["called"], json!("documents"), "{body}");
        // Without a session, or with one that has ended: refused.
        assert_eq!(post(port, None, call, "").0, 400);
        let delete = format!(
            "DELETE /mcp HTTP/1.1\r\nHost: localhost:{port}\r\nAuthorization: Bearer {TOKEN}\r\n\
             Mcp-Session-Id: {session}\r\nConnection: close\r\n\r\n"
        );
        assert_eq!(exchange(port, &delete).0, 200);
        assert_eq!(post(port, Some(&session), call, "").0, 404);
    }

    #[test]
    fn requests_from_elsewhere_or_without_the_token_are_refused() {
        let server = HttpServer::start(0, TOKEN.to_owned(), || Box::new(Echo)).unwrap();
        let port = server.port();
        let init = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#;
        let ask = |host: &str, auth: &str, extra: &str| {
            exchange(
                port,
                &format!(
                    "POST /mcp HTTP/1.1\r\nHost: {host}\r\n{auth}Content-Type: application/json\r\n\
                     Connection: close\r\n{extra}Content-Length: {}\r\n\r\n{init}",
                    init.len()
                ),
            )
            .0
        };
        let bearer = format!("Authorization: Bearer {TOKEN}\r\n");
        assert_eq!(ask(&format!("127.0.0.1:{port}"), &bearer, ""), 200);
        // A page that reached this port under a name of its own (DNS rebinding).
        assert_eq!(ask(&format!("evil.example:{port}"), &bearer, ""), 403);
        assert_eq!(ask(&format!("localhost:{port}"), &bearer, "Origin: https://evil.example\r\n"), 403);
        assert_eq!(ask(&format!("localhost:{port}"), &bearer, &format!("Origin: http://localhost:{port}\r\n")), 200);
        // No token, or another.
        assert_eq!(ask(&format!("localhost:{port}"), "", ""), 401);
        let wrong = format!("Authorization: Bearer {}\r\n", "f".repeat(64));
        assert_eq!(ask(&format!("localhost:{port}"), &wrong, ""), 401);
        // Nothing streamed.
        let get = format!("GET /mcp HTTP/1.1\r\nHost: localhost:{port}\r\n{bearer}Connection: close\r\n\r\n");
        assert_eq!(exchange(port, &get).0, 405);
    }

    #[test]
    fn what_is_not_strictly_http_is_refused() {
        let server = HttpServer::start(0, TOKEN.to_owned(), || Box::new(Echo)).unwrap();
        let port = server.port();
        let head = format!("POST /mcp HTTP/1.1\r\nHost: localhost:{port}\r\nAuthorization: Bearer {TOKEN}\r\n");
        // Another framing of the body, two lengths, a folded line, a bare line feed, too long.
        let cases = [
            (format!("{head}Transfer-Encoding: chunked\r\n\r\n0\r\n\r\n"), 501),
            (format!("{head}Content-Length: 2\r\nContent-Length: 3\r\n\r\n{{}}"), 400),
            (format!("{head}X-A: b\r\n c\r\n\r\n"), 400),
            (format!("{head}X-A: b\n\r\n"), 400),
            (format!("{head}Content-Length: 99999999\r\n\r\n"), 413),
            (format!("{head}X-A: {}\r\n\r\n", "b".repeat(LINE + 10)), 431),
            ("POST /mcp HTTP/2.0\r\n\r\n".to_owned(), 505),
        ];
        for (raw, want) in cases {
            assert_eq!(exchange(port, &raw).0, want, "{raw:.80}");
        }
    }

    #[test]
    fn a_client_trickling_its_request_or_repeating_a_header_is_cut_off() {
        let server = HttpServer::start(0, TOKEN.to_owned(), || Box::new(Echo)).unwrap();
        let port = server.port();
        // A header line now and then, never ending the request.
        let mut conn = TcpStream::connect(("127.0.0.1", port)).unwrap();
        conn.write_all(format!("POST /mcp HTTP/1.1\r\nHost: localhost:{port}\r\n").as_bytes()).unwrap();
        let started = Instant::now();
        let mut cut = false;
        while started.elapsed() < Duration::from_secs(4) {
            std::thread::sleep(Duration::from_millis(200));
            if conn.write_all(b"X-A: b\r\n").is_err() {
                cut = true;
                break;
            }
        }
        let mut rest = Vec::new();
        let _ = conn.set_read_timeout(Some(Duration::from_secs(1)));
        cut |= conn.read_to_end(&mut rest).is_ok();
        assert!(cut && started.elapsed() < Duration::from_secs(4), "held for {:?}", started.elapsed());
        // The same header line over and over counts as many lines.
        let many = format!("POST /mcp HTTP/1.1\r\nHost: localhost:{port}\r\n{}\r\n", "X-A: b\r\n".repeat(HEADERS + 1));
        assert_eq!(exchange(port, &many).0, 431);
        // The scheme's name in any case.
        let init = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#;
        let lower = format!(
            "POST /mcp HTTP/1.1\r\nHost: localhost:{port}\r\nAuthorization: bearer {TOKEN}\r\n\
             Content-Type: application/json\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{init}",
            init.len()
        );
        assert_eq!(exchange(port, &lower).0, 200);
    }

    #[test]
    fn a_kept_connection_serves_request_after_request_and_dropping_stops_it_listening() {
        let server = HttpServer::start(0, TOKEN.to_owned(), || Box::new(Echo)).unwrap();
        let port = server.port();
        let mut conn = TcpStream::connect(("127.0.0.1", port)).unwrap();
        let body = r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#;
        let one = format!(
            "POST /mcp HTTP/1.1\r\nHost: localhost:{port}\r\nAuthorization: Bearer {TOKEN}\r\n\
             Content-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        );
        // Without a session, a ping is refused, but the connection stays.
        let mut reader = BufReader::new(conn.try_clone().unwrap());
        for _ in 0..2 {
            conn.write_all(one.as_bytes()).unwrap();
            let mut status = String::new();
            reader.read_line(&mut status).unwrap();
            assert!(status.starts_with("HTTP/1.1 400"), "{status}");
            let mut len = 0;
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if let Some(v) = line.strip_prefix("Content-Length: ") {
                    len = v.trim().parse().unwrap();
                }
                if line == "\r\n" {
                    break;
                }
            }
            let mut rest = vec![0; len];
            reader.read_exact(&mut rest).unwrap();
        }
        drop(server);
        assert!(TcpStream::connect(("127.0.0.1", port)).is_err(), "still listening");
    }

    #[test]
    fn a_session_reaches_the_editor_through_its_pipe() {
        let file = std::env::temp_dir().join(format!("opal-mcp-http-api-{}.json", std::process::id()));
        let mut server = crate::server::Server::start(file.clone(), eframe::egui::Context::default()).unwrap();
        let mut st = crate::state::AppState::new();
        let http = HttpServer::start(0, TOKEN.to_owned(), move || Box::new(crate::mcp::pipe_to(file.clone()))).unwrap();
        let port = http.port();
        let asked = std::thread::spawn(move || {
            let init = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#;
            let (_, head, _) = post(port, None, init, "");
            let session = head.lines().find_map(|l| l.strip_prefix("Mcp-Session-Id: ")).expect("a session").to_owned();
            let call = r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"new","arguments":{"width":8,"height":4}}}"#;
            post(port, Some(&session), call, "")
        });
        let ctx = eframe::egui::Context::default();
        while !asked.is_finished() {
            server.serve(&mut st, &ctx, false);
            std::thread::sleep(Duration::from_millis(2));
        }
        let (status, _, body) = asked.join().unwrap();
        assert_eq!(status, 200, "{body}");
        let result = &serde_json::from_str::<Value>(&body).unwrap()["result"];
        assert!(result["isError"] != json!(true) && result["content"][0]["text"].is_string(), "{body}");
        assert_eq!(st.docs.len(), 1, "no image was made");
        assert_eq!(st.docs[0].doc().canvas.width, 8);
    }

    #[test]
    fn the_token_lasts_from_one_start_to_the_next_until_renewed() {
        let file = std::env::temp_dir().join(format!("opal-mcp-token-{}.txt", std::process::id()));
        let _ = std::fs::remove_file(&file);
        let first = token(&file, false).unwrap();
        assert_eq!(first.len(), 64);
        assert_eq!(token(&file, false).unwrap(), first);
        let renewed = token(&file, true).unwrap();
        assert_ne!(renewed, first);
        assert_eq!(token(&file, false).unwrap(), renewed);
        std::fs::remove_file(&file).unwrap();
    }
}
