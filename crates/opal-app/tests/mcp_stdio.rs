//! `opal-studio --mcp` as an assistant's client launches it: MCP over standard input and
//! output, without a window, the tools listed even with no editor running to call.

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
    assert_eq!(call["result"]["isError"], json!(true), "{call}");
    assert!(call["result"]["content"][0]["text"].as_str().unwrap().contains("not running"));
    drop(input);
    // The input ended: it stops.
    assert!(child.wait().unwrap().success());
}
