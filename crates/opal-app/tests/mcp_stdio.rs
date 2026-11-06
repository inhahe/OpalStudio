//! `opal-studio --mcp` as an assistant's client launches it: MCP over standard input and
//! output, without a window; with no editor running, it starts one of its own.

use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

#[test]
fn the_mcp_server_answers_over_stdio_without_a_window() {
    let missing = std::env::temp_dir().join(format!("opal-mcp-test-{}-none.json", std::process::id()));
    let mut child = Command::new(env!("CARGO_BIN_EXE_opal-studio"))
        .arg("--mcp")
        .env("OPAL_STUDIO_API", &missing)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("started");
    let mut input = child.stdin.take().unwrap();
    let mut output = BufReader::new(child.stdout.take().unwrap());
    let mut ask = |message: Value| -> Value {
        writeln!(input, "{message}").unwrap();
        input.flush().unwrap();
        let mut line = String::new();
        output.read_line(&mut line).unwrap();
        serde_json::from_str(&line).unwrap_or_else(|_| panic!("not JSON: {line:?}"))
    };
    let hello = ask(
        json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": { "protocolVersion": "2025-06-18" } }),
    );
    assert_eq!(hello["result"]["serverInfo"]["name"], json!("opal-studio"), "{hello}");
    let tools = ask(json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }));
    let names: Vec<&str> =
        tools["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert!(names.contains(&"new") && names.contains(&"image") && names.contains(&"layers"), "{names:?}");
    let call = ask(json!({ "jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": { "name": "documents" } }));
    assert_eq!(call["result"].get("isError"), None, "{call}");
    assert!(call["result"]["content"][1]["text"].as_str().unwrap().contains("started one of its own"), "{call}");
    let new = json!({ "name": "new", "arguments": { "width": "1in", "height": 10, "resolution": 50 } });
    ask(json!({ "jsonrpc": "2.0", "id": 4, "method": "tools/call", "params": new }));
    let docs = ask(json!({ "jsonrpc": "2.0", "id": 5, "method": "tools/call", "params": { "name": "documents" } }));
    assert!(docs["result"]["content"][0]["text"].as_str().unwrap().contains("\"width\": 50"), "{docs}");
    let pid = child.id();
    drop(input);
    // The input ended: it stops, the editor it started with it, taking its file away.
    assert!(child.wait().unwrap().success());
    let hosted = std::env::temp_dir().join(format!("opal-mcp-{pid}-0.json"));
    assert!(!hosted.exists(), "{} was left behind", hosted.display());
}
