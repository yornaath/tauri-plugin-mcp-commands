//! The plugin end to end over HTTP, on tauri's mock runtime.

use std::{
    net::{SocketAddr, TcpListener as StdListener, TcpStream, UdpSocket},
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

use serde_json::{Value, json};
use tauri::{App, Listener, Manager, State, test::MockRuntime};
use tauri_plugin_mcp_commands::{
    Builder, EVENT_ACCESS_REQUESTED, EVENT_CLIENT_CONNECTED, McpServer, ToolDef, collect_mcp_tools,
    discovery::Discovery, is_owner_only, mcp_tool,
};

#[derive(Default)]
struct Counter(Mutex<u32>);

/// Adds to the counter and returns the new value.
#[mcp_tool(idempotent = false)]
#[tauri::command]
#[specta::specta]
async fn counter_add(counter: State<'_, Counter>, amount: u32) -> Result<u32, String> {
    let mut c = counter.0.lock().unwrap();
    *c += amount;
    Ok(*c)
}

#[mcp_tool(destructive)]
#[tauri::command]
#[specta::specta]
async fn fail(reason: String) -> Result<(), String> {
    Err(reason)
}

#[mcp_tool(read_only)]
#[tauri::command]
#[specta::specta]
fn sync_ping() -> String {
    "pong".into()
}

fn temp_dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("mcp-http-{tag}-{}-{:?}", std::process::id(), std::thread::current().id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}

struct Harness {
    app: App<MockRuntime>,
    server: McpServer<MockRuntime>,
    url: String,
    token: String,
    dir: PathBuf,
    client: reqwest::Client,
}

impl Drop for Harness {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

async fn start_with(tag: &str, f: impl FnOnce(Builder<MockRuntime>) -> Builder<MockRuntime>) -> Harness {
    let dir = temp_dir(tag);
    let tools: Vec<ToolDef<MockRuntime>> = collect_mcp_tools![counter_add, fail, sync_ping];
    let manual = ToolDef::manual(
        "add_numbers",
        json!({
            "type": "object",
            "properties": { "a": { "type": "number" }, "b": { "type": "number" } },
            "required": ["a", "b"]
        }),
        |_app, args| async move {
            let (a, b) = (args["a"].as_f64(), args["b"].as_f64());
            match (a, b) {
                (Some(a), Some(b)) => Ok(json!(a + b)),
                _ => Err("a and b must be numbers".to_owned()),
            }
        },
    )
    .with_description("Adds two numbers.")
    .with_output_schema(json!({ "type": "number" }));
    let builder = f(Builder::new().server_info("Test App", "1.2.3").tools(tools).tool(manual).data_dir(&dir));
    let app = tauri::test::mock_builder()
        .plugin(builder.build())
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .unwrap();
    app.manage(Counter::default());
    let server = app.state::<McpServer<MockRuntime>>().inner().clone();
    let status = server.start().await.unwrap();
    let url = status.url.clone().unwrap();
    let token = server.connection_configs().unwrap().token;
    Harness { app, server, url, token, dir, client: reqwest::Client::new() }
}

async fn start(tag: &str) -> Harness {
    start_with(tag, |b| b).await
}

fn meta(client: &str) -> Value {
    json!({
        "io.modelcontextprotocol/protocolVersion": "2026-07-28",
        "io.modelcontextprotocol/clientInfo": { "name": client, "version": "1.0.0" },
        "io.modelcontextprotocol/clientCapabilities": {},
    })
}

impl Harness {
    fn raw(&self, body: &Value) -> reqwest::RequestBuilder {
        self.client
            .post(&self.url)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream")
            .body(body.to_string())
    }

    fn post(&self, body: &Value) -> reqwest::RequestBuilder {
        self.raw(body).bearer_auth(&self.token)
    }

    async fn rpc(&self, req: reqwest::RequestBuilder) -> Value {
        let resp = req.send().await.unwrap();
        let status = resp.status();
        let text = resp.text().await.unwrap();
        assert!(status.is_success(), "HTTP {status}: {text}");
        serde_json::from_str(&text).unwrap()
    }

    /// A 2026-07-28 request from `client`.
    fn v2026(&self, client: &str, id: u32, method: &str, mut params: Value) -> reqwest::RequestBuilder {
        params["_meta"] = meta(client);
        let name = params["name"].as_str().map(str::to_owned);
        let req = self
            .post(&json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }))
            .header("MCP-Protocol-Version", "2026-07-28")
            .header("Mcp-Method", method);
        match name {
            Some(n) => req.header("Mcp-Name", n),
            None => req,
        }
    }

    fn addr(&self) -> SocketAddr {
        self.url.trim_start_matches("http://").trim_end_matches("/mcp").parse().unwrap()
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn legacy_handshake_client() {
    let s = start("legacy").await;
    let init = s
        .rpc(s.post(&json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {
                "protocolVersion": "2025-11-25",
                "capabilities": {},
                "clientInfo": { "name": "old-client", "version": "0.1" }
            }
        })))
        .await;
    assert_eq!(init["result"]["protocolVersion"], "2025-11-25", "{init}");
    assert_eq!(init["result"]["serverInfo"]["name"], "Test App");
    assert_eq!(init["result"]["serverInfo"]["version"], "1.2.3");
    assert!(init["result"]["capabilities"]["tools"].is_object(), "{init}");

    let v = "2025-11-25";
    let list = s
        .rpc(s.post(&json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" })).header("MCP-Protocol-Version", v))
        .await;
    let tools = list["result"]["tools"].as_array().unwrap_or_else(|| panic!("{list}"));
    let names: Vec<_> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["counter_add", "fail", "sync_ping", "add_numbers"]);
    let add = &tools[0];
    assert_eq!(add["description"], "Adds to the counter and returns the new value.");
    assert_eq!(add["inputSchema"]["properties"]["amount"]["type"], "integer");
    assert_eq!(add["inputSchema"]["required"], json!(["amount"]));
    assert_eq!(add["outputSchema"]["properties"]["result"]["type"], "integer");
    assert_eq!(add["annotations"], json!({ "idempotentHint": false }));
    assert!(tools[1].get("outputSchema").is_none(), "unit result has no output schema");
    assert_eq!(tools[3]["description"], "Adds two numbers.");

    let call = |id: u32, name: &str, args: Value| {
        s.post(&json!({
            "jsonrpc": "2.0", "id": id, "method": "tools/call",
            "params": { "name": name, "arguments": args }
        }))
        .header("MCP-Protocol-Version", v)
    };
    let r = s.rpc(call(3, "counter_add", json!({ "amount": 4 }))).await;
    assert_eq!(r["result"]["structuredContent"], json!({ "result": 4 }), "{r}");
    assert_eq!(r["result"]["content"][0]["text"], "{\"result\":4}");
    assert_eq!(r["result"]["isError"], false);

    let r = s.rpc(call(4, "fail", json!({ "reason": "nope" }))).await;
    assert_eq!(r["result"]["isError"], true, "{r}");
    assert_eq!(r["result"]["content"][0]["text"], "nope");

    let r = s.rpc(call(5, "counter_add", json!({}))).await;
    assert_eq!(r["result"]["content"][0]["text"], "missing required argument `amount`", "{r}");

    let r = s.rpc(call(6, "sync_ping", json!({}))).await;
    assert_eq!(r["result"]["structuredContent"], json!({ "result": "pong" }), "{r}");

    let r = s.rpc(call(7, "add_numbers", json!({ "a": 2, "b": 3.5 }))).await;
    assert_eq!(r["result"]["structuredContent"], json!({ "result": 5.5 }), "{r}");
    let r = s.rpc(call(8, "add_numbers", json!({ "a": "x", "b": 1 }))).await;
    assert_eq!(r["result"]["isError"], true, "{r}");

    let r = s.rpc(call(9, "nope", json!({}))).await;
    assert_eq!(r["error"]["code"], -32602, "{r}");
}

#[tokio::test(flavor = "multi_thread")]
async fn stateless_2026_client() {
    let s = start("stateless").await;
    let list = s.rpc(s.v2026("new-client", 1, "tools/list", json!({}))).await;
    assert_eq!(list["result"]["tools"].as_array().map(Vec::len), Some(4), "{list}");
    for (id, expected) in [(2, 7), (3, 14)] {
        let r = s
            .rpc(s.v2026("new-client", id, "tools/call", json!({ "name": "counter_add", "arguments": { "amount": 7 } })))
            .await;
        assert_eq!(r["result"]["structuredContent"], json!({ "result": expected }), "{r}");
        assert_eq!(r["result"]["resultType"], "complete", "{r}");
    }
    let status = s.server.status();
    assert_eq!(status.clients.len(), 1);
    assert_eq!(status.clients[0].name.as_deref(), Some("new-client"));
}

#[tokio::test(flavor = "multi_thread")]
async fn auth_and_browser_rejection() {
    let s = start("auth").await;
    let list = json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": { "_meta": meta("c") } });

    let resp = s.raw(&list).send().await.unwrap();
    assert_eq!(resp.status(), 401);
    assert_eq!(resp.headers()["www-authenticate"], "Bearer");
    assert_eq!(resp.headers()["cache-control"], "no-store");
    let resp = s.raw(&list).bearer_auth("wrong").send().await.unwrap();
    assert_eq!(resp.status(), 401);
    let resp = s.raw(&list).bearer_auth(format!("{}x", s.token)).send().await.unwrap();
    assert_eq!(resp.status(), 401);

    // Browser requests are refused even with a valid token.
    for origin in ["http://evil.example", "http://tauri.localhost", "tauri://localhost", "http://127.0.0.1:1420"] {
        let resp = s.post(&list).header("Origin", origin).send().await.unwrap();
        assert_eq!(resp.status(), 403, "origin {origin}");
    }
    let resp = s.post(&list).header("Sec-Fetch-Site", "cross-site").send().await.unwrap();
    assert_eq!(resp.status(), 403);
    // DNS rebinding: a loopback connection with a foreign Host header.
    let resp = s.post(&list).header("MCP-Protocol-Version", "2026-07-28").header("Host", "evil.example").send().await.unwrap();
    assert_eq!(resp.status(), 403);

    let big = json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": { "pad": "x".repeat(3 << 20) } });
    let resp = s.post(&big).header("MCP-Protocol-Version", "2026-07-28").send().await.unwrap();
    assert_eq!(resp.status(), 413);
}

#[tokio::test(flavor = "multi_thread")]
async fn loopback_only() {
    let s = start("loopback").await;
    assert!(s.addr().ip().is_loopback());
    // Find a non-loopback local address (no packets are sent).
    let lan = UdpSocket::bind("0.0.0.0:0").and_then(|u| u.connect("192.0.2.1:9").and(u.local_addr()));
    match lan {
        Ok(a) if !a.ip().is_loopback() && !a.ip().is_unspecified() => {
            let target = SocketAddr::new(a.ip(), s.addr().port());
            assert!(TcpStream::connect_timeout(&target, Duration::from_secs(2)).is_err(), "reachable on {target}");
        }
        _ => eprintln!("no non-loopback interface; skipping"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn discovery_stop_and_rotation() {
    let s = start("lifecycle").await;
    let discovery_path = s.dir.join("mcp.json");
    let d: Discovery = serde_json::from_slice(&std::fs::read(&discovery_path).unwrap()).unwrap();
    assert_eq!((d.url.as_str(), d.token.as_str(), d.pid), (s.url.as_str(), s.token.as_str(), std::process::id()));
    assert_eq!(d.app_name, "Test App");
    assert!(is_owner_only(&discovery_path).unwrap(), "discovery file must be owner-only");
    assert!(is_owner_only(&s.dir.join("token")).unwrap(), "token file must be owner-only");
    assert!(is_owner_only(&s.dir).unwrap(), "data dir must be owner-only");

    // Rotation: the old token stops working, the new one is in the discovery file.
    s.server.rotate_token().unwrap();
    let list = json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": { "_meta": meta("c") } });
    let list_with = |token: &str| {
        s.raw(&list).bearer_auth(token).header("MCP-Protocol-Version", "2026-07-28").header("Mcp-Method", "tools/list")
    };
    assert_eq!(list_with(&s.token).send().await.unwrap().status(), 401);
    let new_token = s.server.connection_configs().unwrap().token;
    assert_ne!(new_token, s.token);
    let d: Discovery = serde_json::from_slice(&std::fs::read(&discovery_path).unwrap()).unwrap();
    assert_eq!(d.token, new_token);
    assert_eq!(list_with(&new_token).send().await.unwrap().status(), 200);

    // stop() closes the port and removes the discovery file.
    let addr = s.addr();
    s.server.stop().await;
    assert!(!s.server.status().running);
    assert!(s.server.connection_configs().is_none());
    assert!(TcpStream::connect_timeout(&addr, Duration::from_secs(1)).is_err(), "port still open");
    assert!(!discovery_path.exists());

    // Restart keeps the (rotated) token.
    s.server.start().await.unwrap();
    assert_eq!(s.server.connection_configs().unwrap().token, new_token);
    s.server.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn preferred_port_with_fallback() {
    // Hold a port, then ask for it.
    let held = StdListener::bind("127.0.0.1:0").unwrap();
    let port = held.local_addr().unwrap().port();
    let s = start_with("port", |b| b.preferred_port(port)).await;
    let got = s.addr().port();
    assert_ne!(got, port);
    assert!(got > port && got <= port + 10, "expected a fallback near {port}, got {got}");
    drop(held);
}

#[tokio::test(flavor = "multi_thread")]
async fn approval_flow() {
    let s = start_with("approval", |b| b.require_approval(true).approval_timeout(Duration::from_millis(1500))).await;
    let requests = Arc::new(Mutex::new(Vec::<Value>::new()));
    let connected = Arc::new(Mutex::new(Vec::<Value>::new()));
    {
        let requests = requests.clone();
        s.app.listen(EVENT_ACCESS_REQUESTED, move |e| requests.lock().unwrap().push(serde_json::from_str(e.payload()).unwrap()));
        let connected = connected.clone();
        s.app.listen(EVENT_CLIENT_CONNECTED, move |e| connected.lock().unwrap().push(serde_json::from_str(e.payload()).unwrap()));
    }

    // Two concurrent requests from a new client share one prompt.
    let a = s.v2026("agent-a", 1, "tools/list", json!({})).send();
    let b = s.v2026("agent-a", 2, "tools/list", json!({})).send();
    let (a, b) = (tokio::spawn(a), tokio::spawn(b));
    let id = loop {
        if let Some(r) = requests.lock().unwrap().first() {
            break r["id"].as_u64().unwrap();
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    assert_eq!(s.server.status().pending_requests.len(), 1);
    assert!(s.server.respond_access(id, true));
    assert!(!s.server.respond_access(id, true), "already answered");
    for r in [a.await.unwrap().unwrap(), b.await.unwrap().unwrap()] {
        let v: Value = r.json().await.unwrap();
        assert_eq!(v["result"]["tools"].as_array().map(Vec::len), Some(4), "{v}");
    }
    assert_eq!(requests.lock().unwrap().len(), 1);
    assert_eq!(requests.lock().unwrap()[0]["client"]["name"], "agent-a");
    assert_eq!(connected.lock().unwrap().len(), 1);
    // Approved clients are not asked again.
    let r = s.rpc(s.v2026("agent-a", 3, "tools/list", json!({}))).await;
    assert!(r["result"].is_object(), "{r}");

    // A denied client gets an error, and stays denied.
    let pending = tokio::spawn(s.v2026("agent-b", 4, "tools/list", json!({})).send());
    let id = loop {
        if let Some(r) = requests.lock().unwrap().get(1) {
            break r["id"].as_u64().unwrap();
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    s.server.respond_access(id, false);
    let v: Value = pending.await.unwrap().unwrap().json().await.unwrap();
    assert_eq!(v["error"]["code"], -32001, "{v}");
    assert_eq!(v["error"]["message"], "the app denied access to `agent-b`");
    let v = s.rpc(s.v2026("agent-b", 5, "tools/list", json!({}))).await;
    assert_eq!(v["error"]["code"], -32001, "{v}");

    // Unanswered requests time out as denied.
    let v = s.rpc(s.v2026("agent-c", 6, "tools/list", json!({}))).await;
    assert_eq!(v["error"]["code"], -32001, "{v}");
    let names: Vec<_> = s.server.status().clients.into_iter().filter_map(|c| c.name).collect();
    assert_eq!(names, ["agent-a"]);
}
