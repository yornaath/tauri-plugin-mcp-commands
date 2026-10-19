//! MCP over streamable HTTP: 2025-11-25 (initialize handshake) and
//! 2026-07-28 (stateless, per-request `_meta`) clients.

use std::sync::Mutex;

use serde_json::{Value, json};
use tauri::{AppHandle, Manager, State, test::MockRuntime};
use tauri_plugin_mcp_server::{ServerInfo, ToolDef, __private::serve, collect_mcp_tools, mcp_tool};

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

struct Server {
    url: String,
    client: reqwest::Client,
    _app: AppHandle<MockRuntime>,
    cancel: tokio_util::sync::CancellationToken,
}

impl Drop for Server {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

async fn start() -> Server {
    let app = tauri::test::mock_app();
    app.manage(Counter::default());
    let handle = app.handle().clone();
    let tools: Vec<ToolDef<MockRuntime>> = collect_mcp_tools![counter_add, fail, sync_ping];
    let info = ServerInfo { name: "test-app".into(), version: "1.2.3".into() };
    let (addr, cancel) = serve(handle.clone(), tools, info, 0).await.unwrap();
    assert!(addr.ip().is_loopback());
    Server { url: format!("http://{addr}/mcp"), client: reqwest::Client::new(), _app: handle, cancel }
}

impl Server {
    fn post(&self, body: &Value) -> reqwest::RequestBuilder {
        self.client
            .post(&self.url)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream")
            .body(body.to_string())
    }

    async fn rpc(&self, req: reqwest::RequestBuilder) -> Value {
        let resp = req.send().await.unwrap();
        let status = resp.status();
        let ct = resp.headers().get("content-type").map(|v| v.to_str().unwrap().to_owned());
        let text = resp.text().await.unwrap();
        assert!(status.is_success(), "HTTP {status}: {text}");
        assert_eq!(ct.as_deref(), Some("application/json"), "{text}");
        serde_json::from_str(&text).unwrap()
    }
}

fn meta() -> Value {
    json!({
        "io.modelcontextprotocol/protocolVersion": "2026-07-28",
        "io.modelcontextprotocol/clientInfo": { "name": "test-client", "version": "1.0.0" },
        "io.modelcontextprotocol/clientCapabilities": {},
    })
}

#[tokio::test(flavor = "multi_thread")]
async fn legacy_handshake_client() {
    let s = start().await;
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
    assert_eq!(init["result"]["serverInfo"]["name"], "test-app");
    assert!(init["result"]["capabilities"]["tools"].is_object(), "{init}");

    let v = "2025-11-25";
    let list = s
        .rpc(s.post(&json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" })).header("MCP-Protocol-Version", v))
        .await;
    let tools = list["result"]["tools"].as_array().unwrap_or_else(|| panic!("{list}"));
    let names: Vec<_> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["counter_add", "fail", "sync_ping"]);
    let add = &tools[0];
    assert_eq!(add["description"], "Adds to the counter and returns the new value.");
    assert_eq!(add["inputSchema"]["properties"]["amount"]["type"], "integer");
    assert_eq!(add["inputSchema"]["required"], json!(["amount"]));
    assert_eq!(add["outputSchema"]["properties"]["result"]["type"], "integer");
    assert_eq!(add["annotations"], json!({ "idempotentHint": false }));
    assert!(tools[1].get("outputSchema").is_none(), "unit result has no output schema");

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
    assert!(r["result"].get("resultType").is_none(), "pre-2026 peers don't get resultType: {r}");

    let r = s.rpc(call(4, "fail", json!({ "reason": "nope" }))).await;
    assert_eq!(r["result"]["isError"], true, "{r}");
    assert_eq!(r["result"]["content"][0]["text"], "nope");

    let r = s.rpc(call(5, "counter_add", json!({}))).await;
    assert_eq!(r["result"]["isError"], true, "{r}");
    assert_eq!(r["result"]["content"][0]["text"], "missing required argument `amount`");

    let r = s.rpc(call(6, "sync_ping", json!({}))).await;
    assert_eq!(r["result"]["structuredContent"], json!({ "result": "pong" }), "{r}");

    let r = s.rpc(call(7, "nope", json!({}))).await;
    assert_eq!(r["error"]["code"], -32602, "{r}");
}

#[tokio::test(flavor = "multi_thread")]
async fn stateless_2026_client() {
    let s = start().await;
    let v = "2026-07-28";
    let list = s
        .rpc(
            s.post(&json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": { "_meta": meta() } }))
                .header("MCP-Protocol-Version", v)
                .header("Mcp-Method", "tools/list"),
        )
        .await;
    assert_eq!(list["result"]["tools"].as_array().map(Vec::len), Some(3), "{list}");

    for (id, expected) in [(2, 7), (3, 14)] {
        let r = s
            .rpc(
                s.post(&json!({
                    "jsonrpc": "2.0", "id": id, "method": "tools/call",
                    "params": { "name": "counter_add", "arguments": { "amount": 7 }, "_meta": meta() }
                }))
                .header("MCP-Protocol-Version", v)
                .header("Mcp-Method", "tools/call")
                .header("Mcp-Name", "counter_add"),
            )
            .await;
        assert_eq!(r["result"]["structuredContent"], json!({ "result": expected }), "{r}");
        assert_eq!(r["result"]["resultType"], "complete", "{r}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn rejects_browser_origins_and_foreign_hosts() {
    let s = start().await;
    let list = json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": { "_meta": meta() } });
    for origin in ["http://evil.example", "http://tauri.localhost", "tauri://localhost", "http://127.0.0.1:1420"] {
        let resp = s.post(&list).header("MCP-Protocol-Version", "2026-07-28").header("Origin", origin).send().await.unwrap();
        assert_eq!(resp.status(), 403, "origin {origin}");
    }
    // DNS rebinding: a loopback connection with a foreign Host header.
    let resp = s.post(&list).header("MCP-Protocol-Version", "2026-07-28").header("Host", "evil.example").send().await.unwrap();
    assert_eq!(resp.status(), 403);

    let big = "x".repeat(3 * 1024 * 1024);
    let resp = s
        .post(&json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": { "pad": big } }))
        .header("MCP-Protocol-Version", "2026-07-28")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 413);
}
