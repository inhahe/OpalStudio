//! `opal-studio --mcp`: an MCP server over standard input and output, for assistants whose
//! clients launch their servers (see design.md, *Scripting*). Its tools are the API's
//! methods ([`crate::api::describe`], so they cannot drift from what the editor accepts),
//! each called on the editor running on this computer through its pipe, as a script would
//! call it; `image` comes back as an image, so the assistant can see what it is doing.
//!
//! The editor need not be running to list the tools; a tool called without it says so.

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
}

impl Pipe {
    /// Connects to the editor the connection file tells of, and says hello.
    fn connect(&mut self) -> Result<(), String> {
        let file = self
            .file
            .clone()
            .or_else(|| std::env::var_os("OPAL_STUDIO_API").map(std::path::PathBuf::from))
            .or_else(crate::server::connection_file)
            .ok_or("There is no settings folder to find the editor in.")?;
        match crate::server::say_hello(&file, false) {
            Hello::Answered(conn) => {
                self.conn = Some(conn);
                Ok(())
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

impl Caller for Pipe {
    fn call(&mut self, method: &str, params: Map<String, Value>) -> Result<Value, String> {
        if self.conn.is_none() {
            self.connect()?;
        }
        self.exchange(method, params)
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
                    "int" => json!({ "type": "integer", "minimum": p["min"], "maximum": p["max"] }),
                    "number" => json!({ "type": "number", "minimum": p["min"], "maximum": p["max"] }),
                    "text" | "path" => json!({ "type": "string" }),
                    "choice" => json!({ "type": "string", "enum": p["choices"] }),
                    "document" | "layer" => json!({ "type": "integer", "minimum": 0 }),
                    "layers" => json!({ "type": "array", "items": { "type": "integer", "minimum": 0 }, "minItems": 1 }),
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
                             looks like.",
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
    Ok(match caller.call(name, arguments) {
        Ok(v) if name == "image" => json!({ "content": [
            { "type": "image", "data": v["png"], "mimeType": "image/png" },
            { "type": "text", "text": format!("{} × {} pixels", v["width"], v["height"]) },
        ] }),
        Ok(v) => json!({ "content": [{ "type": "text", "text": serde_json::to_string_pretty(&v).expect("JSON") }] }),
        Err(why) => json!({ "content": [{ "type": "text", "text": why }], "isError": true }),
    })
}

/// Serves MCP on standard input and output until the input ends; the process's exit code.
pub fn run_stdio() -> i32 {
    let mut pipe = Pipe::default();
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
