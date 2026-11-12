//! `opal-studio --mcp`: an MCP server over standard input and output, for assistants whose
//! clients launch their servers (see design.md, *Scripting*). Its tools are the API's
//! methods ([`crate::api::describe`], so they cannot drift from what the editor accepts),
//! each called on the editor running on this computer through its pipe, as a script would
//! call it; `image` comes back as an image, so the assistant can see what it is doing.
//!
//! The editor need not be running: if none is when a tool is first called, this server
//! starts one of its own, with no window, which the tools then drive (and the first call's
//! answer says so). What it holds lasts as long as this server: a script of tools saves or
//! exports what it makes.

use crate::api::describe;
use crate::server::Hello;
use interprocess::local_socket::Stream;
use serde_json::{Map, Value, json};
use std::io::{BufRead, BufReader, Write};

/// The MCP versions this server speaks, newest first: a client asking for one of them gets
/// it, any other the newest (and decides whether it can go on).
const MCP_VERSIONS: &[&str] = &["2025-06-18", "2025-03-26", "2024-11-05"];

/// What carries a tool's call to the editor.
pub trait Caller {
    fn call(&mut self, method: &str, params: Map<String, Value>) -> Result<Value, String>;

    /// What the caller has to say about how the last call went, once (that it started an
    /// editor to answer it).
    fn take_note(&mut self) -> Option<String> {
        None
    }
}

/// The editor's pipe, connected (with the token) when first needed, and again after it
/// drops.
#[derive(Default)]
struct Pipe {
    /// The connection file to read; `None` for the user's (or the one `OPAL_STUDIO_API`
    /// names).
    file: Option<std::path::PathBuf>,
    conn: Option<BufReader<Stream>>,
    next_id: u64,
    /// Whether to start an editor of this process's own if none is running.
    may_host: bool,
    /// The editor this process started, once it has.
    hosted: Option<Hosted>,
    /// What the next answer must also say (that an editor was started).
    note: Option<String>,
}

/// An editor with no window, run on a thread of this process, serving a connection file of
/// its own until this is dropped.
struct Hosted {
    file: std::path::PathBuf,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// The thread it runs on, joined when this is dropped, so that it stops (and takes its
    /// connection file away) before the process ends.
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Hosted {
    /// Starts it, loading the user's preferences (not saved back) as `--headless` does (but
    /// not in tests, which must not depend on, or rename, the user's settings).
    fn start() -> Result<Hosted, String> {
        use std::sync::atomic::{AtomicBool, Ordering};
        static HOSTED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = HOSTED.fetch_add(1, Ordering::Relaxed);
        let file = std::env::temp_dir().join(format!("opal-mcp-{}-{n}.json", std::process::id()));
        let stop = std::sync::Arc::new(AtomicBool::new(false));
        let (tx, rx) = std::sync::mpsc::channel();
        let (at, stopped) = (file.clone(), stop.clone());
        let thread = std::thread::spawn(move || {
            let ctx = eframe::egui::Context::default();
            let mut st = crate::state::AppState::new();
            st.windowless = true;
            if !cfg!(test) {
                crate::config::load(&mut st);
            }
            st.apply_tile_budget();
            let mut server = match crate::server::Server::start(at, ctx.clone()) {
                Ok(server) => server,
                Err(e) => return drop(tx.send(Err(format!("An editor could not be started: {e}")))),
            };
            let _ = tx.send(Ok(()));
            crate::headless::serve_windowless(&mut server, &mut st, &ctx, |_| stopped.load(Ordering::Relaxed));
        });
        rx.recv().map_err(|_| "An editor could not be started.".to_owned())??;
        Ok(Hosted { file, stop, thread: Some(thread) })
    }
}

impl Drop for Hosted {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// What the first answer from an editor this server started says of it.
const HOSTED_NOTE: &str = "No Opal Studio was running, so this server started one of its own, with no window. \
                           What it opens and makes lasts as long as this server: save or export what is to be kept.";

impl Pipe {
    /// Connects to the editor the connection file tells of, and says hello; if none is
    /// running and it may, starts one of its own and connects to that.
    fn connect(&mut self) -> Result<(), String> {
        let file = match &self.hosted {
            Some(h) => Some(h.file.clone()),
            None => self
                .file
                .clone()
                .or_else(|| std::env::var_os("OPAL_STUDIO_API").map(std::path::PathBuf::from))
                .or_else(crate::server::connection_file),
        };
        // (With no settings folder there is no editor to find, but one can still be started.)
        let hello = match file {
            Some(file) => crate::server::say_hello(&file, false),
            None => Hello::Nobody("There is no settings folder to find the editor in.".to_owned()),
        };
        match hello {
            Hello::Answered(conn) => {
                self.conn = Some(conn);
                Ok(())
            }
            Hello::Nobody(_) if self.may_host && self.hosted.is_none() => {
                self.hosted = Some(Hosted::start()?);
                self.note = Some(HOSTED_NOTE.to_owned());
                self.connect()
            }
            Hello::Nobody(why) | Hello::Refused(why) => Err(why),
        }
    }

    fn exchange(&mut self, method: &str, params: Map<String, Value>) -> Result<Value, String> {
        let conn = self.conn.as_mut().expect("connected");
        self.next_id += 1;
        let request = json!({ "jsonrpc": "2.0", "id": self.next_id, "method": method, "params": params });
        let mut line = String::new();
        let sent = conn.get_mut().write_all(format!("{request}\n").as_bytes()).is_ok();
        if !sent || conn.read_line(&mut line).unwrap_or(0) == 0 {
            self.conn = None;
            return Err("The editor closed the connection.".to_owned());
        }
        let reply: Value = serde_json::from_str(&line).map_err(|e| format!("The editor's answer is not JSON: {e}"))?;
        match reply.get("error") {
            Some(e) => Err(e["message"].as_str().unwrap_or("The editor refused.").to_owned()),
            None => Ok(reply["result"].clone()),
        }
    }
}

/// What carries calls to the editor whose connection file is `file`, connecting when first
/// needed and again after the connection drops.
pub fn pipe_to(file: std::path::PathBuf) -> impl Caller + Send {
    Pipe { file: Some(file), ..Pipe::default() }
}

impl Caller for Pipe {
    fn call(&mut self, method: &str, params: Map<String, Value>) -> Result<Value, String> {
        if self.conn.is_none() {
            self.connect()?;
        }
        self.exchange(method, params)
    }

    fn take_note(&mut self) -> Option<String> {
        self.note.take()
    }
}

/// The tools: each method of the API but those clients give in their own way
/// ([`crate::api::OWN_WAY`]: `methods`, which the tool list is, and pixels, which an
/// assistant sees through `image`), with a JSON Schema of its parameters. Made once.
pub fn tools() -> &'static Value {
    static TOOLS: std::sync::OnceLock<Value> = std::sync::OnceLock::new();
    TOOLS.get_or_init(made_tools)
}

fn made_tools() -> Value {
    let tools: Vec<Value> = describe()
        .as_array()
        .expect("a list")
        .iter()
        .filter(|m| !crate::api::OWN_WAY.contains(&m["name"].as_str().expect("named")))
        .map(|m| {
            let mut properties = Map::new();
            let mut required = Vec::new();
            for p in m["params"].as_array().expect("a list") {
                let name = p["name"].as_str().expect("named");
                let mut schema = match p["kind"].as_str().expect("a kind") {
                    "bool" => json!({ "type": "boolean" }),
                    // A number, or an expression as text (whose range is checked once worked out).
                    "int" | "pixels" => json!({ "type": ["integer", "string"], "minimum": p["min"], "maximum": p["max"] }),
                    "number" | "angle" => json!({ "type": ["number", "string"], "minimum": p["min"], "maximum": p["max"] }),
                    // A number in the call's unit, or an expression: the range is in pixels.
                    "length" => json!({ "type": ["number", "string"] }),
                    "any" => json!({}),
                    "text" | "path" => json!({ "type": "string" }),
                    "paths" => json!({ "type": "array", "items": { "type": "string" }, "minItems": 1 }),
                    "choice" => json!({ "type": "string", "enum": p["choices"] }),
                    "document" | "layer" => json!({ "type": "integer", "minimum": 0 }),
                    "layers" => json!({ "type": "array", "items": { "type": "integer", "minimum": 0 }, "minItems": 1 }),
                    "points" => {
                        let far = crate::box_fields::FAR;
                        let coordinate = json!({ "type": "number", "minimum": -far, "maximum": far });
                        json!({
                            "type": "array",
                            "items": { "type": "array", "items": coordinate, "minItems": 2, "maxItems": 2 },
                            "minItems": p["min"],
                            "maxItems": opal_core::selection::MOST_POINTS,
                        })
                    }
                    "outline" => {
                        let far = crate::box_fields::FAR;
                        let point = json!({
                            "type": "array",
                            "items": { "type": "number", "minimum": -far, "maximum": far },
                            "minItems": 2,
                            "maxItems": 2,
                        });
                        json!({
                            "type": "object",
                            "properties": {
                                "start": point,
                                "segments": {
                                    "type": "array",
                                    "items": {
                                        "oneOf": [
                                            {
                                                "type": "object",
                                                "properties": { "kind": { "const": "line" }, "to": point },
                                                "required": ["kind", "to"],
                                            },
                                            {
                                                "type": "object",
                                                "properties": {
                                                    "kind": { "const": "curve" },
                                                    "c1": point, "c2": point, "to": point,
                                                },
                                                "required": ["kind", "c1", "c2", "to"],
                                            },
                                        ],
                                    },
                                    // (A symmetric outline's slice may have one.)
                                    "minItems": 1,
                                    "maxItems": opal_core::selection::MOST_POINTS - 1,
                                },
                                "symmetry": {
                                    "type": "object",
                                    "properties": {
                                        "order": { "type": "integer", "minimum": 1, "maximum": opal_core::symmetry::MOST_ORDER },
                                        "mirror": { "type": "boolean" },
                                        "center": point,
                                        "angle": { "type": "number" },
                                    },
                                    "required": ["order", "mirror", "center", "angle"],
                                },
                                "joins": {
                                    "type": "array",
                                    "items": { "type": "string", "enum": ["sharp", "smooth", "symmetric"] },
                                },
                                "smooth": { "type": "boolean" },
                            },
                            "required": ["start", "segments"],
                        })
                    }
                    other => unreachable!("a parameter of kind {other}"),
                };
                let mut help = p["help"].as_str().expect("helped").to_owned();
                if let Some(d) = p.get("default") {
                    schema["default"] = d.clone();
                } else if let Some(o) = p["otherwise"].as_str() {
                    help += &format!(" Left out: {o}.");
                }
                schema["description"] = json!(help);
                if p.get("required").is_some() {
                    required.push(json!(name));
                }
                properties.insert(name.to_owned(), schema);
            }
            json!({
                "name": m["name"],
                "description": m["help"],
                "inputSchema": { "type": "object", "properties": properties, "required": required, "additionalProperties": false },
            })
        })
        .collect();
    json!(tools)
}

/// The answer to MCP message `message`, calling the editor through `caller`; `None` for a
/// notification, which is not answered.
pub fn handle(message: &Value, caller: &mut dyn Caller) -> Option<Value> {
    let id = message.get("id")?.clone();
    let method = message["method"].as_str().unwrap_or("");
    let params = &message["params"];
    let result = match method {
        "initialize" => Ok(json!({
            "protocolVersion": params["protocolVersion"]
                .as_str()
                .filter(|v| MCP_VERSIONS.contains(v))
                .unwrap_or(MCP_VERSIONS[0]),
            "capabilities": { "tools": {} },
            "serverInfo": { "name": "opal-studio", "version": env!("CARGO_PKG_VERSION") },
            "instructions": "Drives the Opal Studio graphics editor running on this computer. Images and layers \
                             are named by id: `documents` and `layers` tell them; most tools act on the active \
                             image and the selected layers when none is named. `image` shows what an image \
                             looks like. Calls between begin_step and end_step are one step of the image's \
                             history, which one undo takes back.",
        })),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": tools() })),
        "tools/call" => call_tool(params, caller),
        _ => Err((-32601, format!("There is no method \"{method}\"."))),
    };
    Some(match result {
        Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
        Err((code, message)) => json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } }),
    })
}

/// A `tools/call`: the editor's answer as the tool's content, an image as an image; a
/// refusal as the tool's error, which the assistant reads.
fn call_tool(params: &Value, caller: &mut dyn Caller) -> Result<Value, (i64, String)> {
    let name = params["name"].as_str().ok_or((-32602, "A tool call needs a name.".to_owned()))?;
    if !tools().as_array().expect("a list").iter().any(|t| t["name"] == json!(name)) {
        return Err((-32602, format!("There is no tool \"{name}\".")));
    }
    let arguments = match &params["arguments"] {
        Value::Null => Map::new(),
        Value::Object(a) => a.clone(),
        _ => return Err((-32602, "A tool's arguments are an object.".to_owned())),
    };
    let mut answer = match caller.call(name, arguments) {
        Ok(v) if name == "image" => json!({ "content": [
            { "type": "image", "data": v["png"], "mimeType": "image/png" },
            { "type": "text", "text": format!("{} × {} pixels", v["width"], v["height"]) },
        ] }),
        Ok(v) => json!({ "content": [{ "type": "text", "text": serde_json::to_string_pretty(&v).expect("JSON") }] }),
        Err(why) => json!({ "content": [{ "type": "text", "text": why }], "isError": true }),
    };
    if let Some(note) = caller.take_note()
        && let Some(content) = answer["content"].as_array_mut()
    {
        content.push(json!({ "type": "text", "text": note }));
    }
    Ok(answer)
}

/// Serves MCP on standard input and output until the input ends; the process's exit code.
pub fn run_stdio() -> i32 {
    let mut pipe = Pipe { may_host: true, ..Pipe::default() };
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout().lock();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { return 1 };
        if line.trim().is_empty() {
            continue;
        }
        let answer = match serde_json::from_str::<Value>(&line) {
            Ok(message) => handle(&message, &mut pipe),
            Err(e) => Some(
                json!({ "jsonrpc": "2.0", "id": null, "error": { "code": -32700, "message": format!("Not JSON: {e}") } }),
            ),
        };
        if let Some(answer) = answer
            && (writeln!(stdout, "{answer}").is_err() || stdout.flush().is_err())
        {
            return 1;
        }
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::Reply;
    use crate::state::AppState;
    use eframe::egui;

    /// Calls the editor's state in this process, waiting for a write's answer.
    struct InProcess(AppState);

    impl Caller for InProcess {
        fn call(&mut self, method: &str, params: Map<String, Value>) -> Result<Value, String> {
            match self.0.call(method, &params, &egui::Context::default()) {
                Reply::Now(r) => r,
                Reply::Later(n) => loop {
                    self.0.poll_writes(false);
                    if let Some((_, r)) = self.0.answers().into_iter().find(|(m, _)| *m == n) {
                        return r;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(5));
                },
            }
        }
    }

    fn ask(caller: &mut dyn Caller, method: &str, params: Value) -> Value {
        handle(&json!({ "jsonrpc": "2.0", "id": 7, "method": method, "params": params }), caller).expect("answered")
    }

    #[test]
    fn an_assistant_lists_the_tools_and_calls_them() {
        let _turn = crate::background::testing::turn();
        let mut editor = InProcess(AppState::new());
        let hello = ask(&mut editor, "initialize", json!({ "protocolVersion": "2025-03-26" }));
        assert_eq!(hello["result"]["protocolVersion"], json!("2025-03-26"));
        assert_eq!(hello["result"]["serverInfo"]["name"], json!("opal-studio"));
        // A version it does not speak: answered with the newest it does.
        let unknown = ask(&mut editor, "initialize", json!({ "protocolVersion": "2099-01-01" }));
        assert_eq!(unknown["result"]["protocolVersion"], json!(MCP_VERSIONS[0]));
        assert!(handle(&json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }), &mut editor).is_none());

        let tools = ask(&mut editor, "tools/list", json!({}))["result"]["tools"].clone();
        let new = tools.as_array().unwrap().iter().find(|t| t["name"] == json!("new")).unwrap();
        assert_eq!(new["inputSchema"]["required"], json!(["width", "height"]));
        assert_eq!(new["inputSchema"]["properties"]["background"]["enum"], json!(["white", "transparent"]));
        assert!(!tools.as_array().unwrap().iter().any(|t| t["name"] == json!("methods")));

        let made = ask(&mut editor, "tools/call", json!({ "name": "new", "arguments": { "width": 64, "height": 48 } }));
        let text = made["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("document"), "{made}");
        // Refused: the reason, as the tool's error, for the assistant to read.
        let bad = ask(&mut editor, "tools/call", json!({ "name": "new", "arguments": { "width": 0, "height": 1 } }));
        assert_eq!(bad["result"]["isError"], json!(true));
        assert!(bad["result"]["content"][0]["text"].as_str().unwrap().contains("between 1 and"));
        // Seen: an image.
        let seen = ask(&mut editor, "tools/call", json!({ "name": "image", "arguments": { "max_side": 32 } }));
        assert_eq!(seen["result"]["content"][0]["type"], json!("image"), "{seen}");
        assert_eq!(seen["result"]["content"][0]["mimeType"], json!("image/png"));
        assert!(seen["result"]["content"][1]["text"].as_str().unwrap().starts_with("32 × 24"));

        let unknown = ask(&mut editor, "tools/call", json!({ "name": "zoom_in" }));
        assert_eq!(unknown["error"]["code"], json!(-32602));
        assert_eq!(ask(&mut editor, "resources/list", json!({}))["error"]["code"], json!(-32601));
    }

    #[test]
    fn without_the_editor_a_tool_says_so() {
        // A connection file that does not exist: the editor is not running.
        let missing = std::env::temp_dir().join("opal-no-editor-api.json");
        let mut pipe = Pipe { file: Some(missing), ..Pipe::default() };
        let answer = ask(&mut pipe, "tools/call", json!({ "name": "documents", "arguments": {} }));
        assert_eq!(answer["result"]["isError"], json!(true));
        assert!(answer["result"]["content"][0]["text"].as_str().unwrap().contains("not running"), "{answer}");
    }

    #[test]
    fn without_the_editor_this_server_starts_one_of_its_own() {
        let missing = std::env::temp_dir().join("opal-no-editor-api.json");
        let mut pipe = Pipe { file: Some(missing), may_host: true, ..Pipe::default() };
        let text = |answer: &Value| answer["result"]["content"][0]["text"].as_str().unwrap().to_owned();
        let first = ask(&mut pipe, "tools/call", json!({ "name": "documents", "arguments": {} }));
        assert_eq!(first["result"].get("isError"), None, "{first}");
        let said = first["result"]["content"][1]["text"].as_str().unwrap_or_default();
        assert!(said.contains("started one of its own"), "the first answer does not say so: {first}");
        let made =
            ask(&mut pipe, "tools/call", json!({ "name": "new", "arguments": { "width": "2in", "height": 50 } }));
        assert_eq!(made["result"]["content"].as_array().unwrap().len(), 1, "said again: {made}");
        assert!(text(&made).contains("document"), "{made}");
        let docs = ask(&mut pipe, "tools/call", json!({ "name": "documents", "arguments": {} }));
        assert!(text(&docs).contains("\"width\": 144"), "{docs}");
        let hosted = pipe.hosted.as_ref().unwrap().file.clone();
        drop(pipe);
        // Stopped with the server, before the process can end: its file goes with it.
        assert!(!hosted.exists(), "the editor it started is still serving");
    }

    #[test]
    fn a_refused_hello_is_told_and_the_next_call_connects_afresh() {
        let file = std::env::temp_dir().join(format!("opal-mcp-refused-{}.json", std::process::id()));
        let mut server = crate::server::Server::start(file.clone(), egui::Context::default()).unwrap();
        let mut st = AppState::new();
        let mut pipe = Pipe { file: Some(file), ..Pipe::default() };
        // Scripts shut out: the editor's reason, as the tool's error.
        server.disconnect_all();
        let refused = pipe.call("documents", Map::new()).unwrap_err();
        assert!(refused.contains("disconnected"), "{refused}");
        // Let in again: the next call connects (not on the connection the editor dropped).
        server.let_in();
        let ctx = egui::Context::default();
        let asked = std::thread::spawn(move || pipe.call("documents", Map::new()));
        while !asked.is_finished() {
            server.serve(&mut st, &ctx, false);
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        let answer = asked.join().unwrap();
        assert!(answer.is_ok(), "{answer:?}");
    }
}
