//! Drives the real bridge binary over stdio, like Claude Desktop does,
//! against the plugin running on tauri's mock runtime.

use std::{
    io::{BufRead, BufReader, Write},
    path::Path,
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc,
    time::{Duration, Instant},
};

use serde_json::{Value, json};
use tauri::{Manager, test::MockRuntime};
use tauri_plugin_mcp_commands::{Builder, McpServer, collect_mcp_tools, mcp_tool};

/// Greets someone.
#[mcp_tool(read_only)]
#[tauri::command]
#[specta::specta]
async fn greet(name: String) -> Result<String, String> {
    if name.is_empty() { Err("name is empty".into()) } else { Ok(format!("Hello, {name}!")) }
}

struct Bridge {
    child: Child,
    stdin: ChildStdin,
    lines: mpsc::Receiver<Value>,
    next_id: u64,
}

impl Bridge {
    fn spawn(discovery: &Path) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_mcp-stdio-bridge"))
            .args(["--discovery", discovery.to_str().unwrap(), "--poll", "1"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let (tx, lines) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if let Ok(v) = serde_json::from_str(&line) {
                    let _ = tx.send(v);
                }
            }
        });
        Self { child, stdin, lines, next_id: 0 }
    }

    fn send(&mut self, v: Value) {
        writeln!(self.stdin, "{v}").unwrap();
        self.stdin.flush().unwrap();
    }

    /// Next message matching `pred`, skipping others.
    fn wait_for(&self, what: &str, pred: impl Fn(&Value) -> bool) -> Value {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.lines.recv_timeout(left) {
                Ok(v) if pred(&v) => return v,
                Ok(_) => continue,
                Err(_) => panic!("timed out waiting for {what}"),
            }
        }
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
        self.next_id += 1;
        let id = self.next_id;
        self.send(json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }));
        self.wait_for(method, |v| v["id"] == id)
    }
}

impl Drop for Bridge {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn bridge_follows_the_app() {
    let dir = std::env::temp_dir().join(format!("mcp-bridge-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let discovery = dir.join("mcp.json");

    let app = tauri::test::mock_builder()
        .plugin(
            Builder::new().server_info("Bridge Test", "0.0.1").tools(collect_mcp_tools![greet]).data_dir(&dir).build(),
        )
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .unwrap();
    let server = app.state::<McpServer<MockRuntime>>().inner().clone();

    // The client starts the bridge before the app's server is on.
    let mut bridge = Bridge::spawn(&discovery);
    let init = bridge.request(
        "initialize",
        json!({ "protocolVersion": "2025-11-25", "capabilities": {}, "clientInfo": { "name": "desktop-like", "version": "1" } }),
    );
    assert_eq!(init["result"]["capabilities"]["tools"]["listChanged"], true, "{init}");
    bridge.send(json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }));
    let list = bridge.request("tools/list", json!({}));
    assert_eq!(list["result"]["tools"], json!([]), "{list}");
    let call = bridge.request("tools/call", json!({ "name": "greet", "arguments": { "name": "x" } }));
    assert_eq!(call["result"]["isError"], true, "{call}");
    assert!(call["result"]["content"][0]["text"].as_str().unwrap().contains("isn't running"), "{call}");

    // The app turns its server on: the bridge notices and announces tools.
    server.start().await.unwrap();
    bridge.wait_for("tools/list_changed", |v| v["method"] == "notifications/tools/list_changed");
    let list = bridge.request("tools/list", json!({}));
    assert_eq!(list["result"]["tools"][0]["name"], "greet", "{list}");
    assert_eq!(list["result"]["tools"][0]["description"], "Greets someone.");
    let call = bridge.request("tools/call", json!({ "name": "greet", "arguments": { "name": "Ada" } }));
    assert_eq!(call["result"]["structuredContent"], json!({ "result": "Hello, Ada!" }), "{call}");
    let call = bridge.request("tools/call", json!({ "name": "greet", "arguments": { "name": "" } }));
    assert_eq!(call["result"]["isError"], true, "{call}");
    assert_eq!(call["result"]["content"][0]["text"], "name is empty");

    // The app restarts its server (new port, rotated token): the bridge follows.
    let old_url = server.status().url.unwrap();
    server.stop().await;
    let blocker = std::net::TcpListener::bind(old_url.trim_start_matches("http://").trim_end_matches("/mcp")).ok();
    server.rotate_token().unwrap();
    server.start().await.unwrap();
    assert_ne!(server.status().url.unwrap(), old_url);
    let call = bridge.request("tools/call", json!({ "name": "greet", "arguments": { "name": "again" } }));
    assert_eq!(call["result"]["structuredContent"], json!({ "result": "Hello, again!" }), "{call}");
    drop(blocker);

    server.stop().await;
    drop(bridge);
    let _ = std::fs::remove_dir_all(&dir);
}
