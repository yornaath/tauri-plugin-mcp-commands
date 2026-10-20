//! Demo app for tauri-plugin-mcp-commands.
//!
//! Flags:
//! - `--autostart`: start the MCP server on launch (otherwise use the UI)
//! - `--approval`: ask before serving each new client
//! - `--hidden`: create the main window invisible
//! - `--tray-only`: no windows at all, just a tray icon
//! - `--self-test`: start the server, call it over HTTP, print a report, exit
//! - `--port N`: preferred MCP port (default 43110)

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod diagnostics;
mod notes;

use std::time::{Duration, Instant};

use diagnostics::{MainThread, WebviewReports};
use notes::NotesRepo;
use serde_json::{Value, json};
use tauri::{AppHandle, Emitter, Manager, RunEvent, Runtime, WebviewUrl, WebviewWindowBuilder, tray::TrayIconBuilder};
use tauri_plugin_mcp_commands::{McpServer, ToolAnnotations, ToolDef, collect_mcp_tools};

struct Flags {
    autostart: bool,
    approval: bool,
    hidden: bool,
    tray_only: bool,
    self_test: bool,
    port: u16,
}

fn flags() -> Flags {
    let args: Vec<String> = std::env::args().collect();
    let has = |f: &str| args.iter().any(|a| a == f);
    let port = args
        .iter()
        .position(|a| a == "--port")
        .and_then(|i| args.get(i + 1))
        .and_then(|p| p.parse().ok())
        .unwrap_or(43110);
    Flags {
        autostart: has("--autostart"),
        approval: has("--approval"),
        hidden: has("--hidden"),
        tray_only: has("--tray-only"),
        self_test: has("--self-test"),
        port,
    }
}

/// A tool that isn't a command: a closure with a hand-written schema.
fn start_timer_tool<R: Runtime>() -> ToolDef<R> {
    ToolDef::manual(
        "start_timer",
        json!({
            "type": "object",
            "properties": {
                "seconds": { "type": "integer", "minimum": 1, "maximum": 3600 },
                "label": { "type": "string" }
            },
            "required": ["seconds"]
        }),
        |app: AppHandle<R>, args| async move {
            let seconds = args.get("seconds").and_then(Value::as_u64).filter(|s| (1..=3600).contains(s));
            let seconds = seconds.ok_or("`seconds` must be an integer between 1 and 3600")?;
            let label = args.get("label").and_then(Value::as_str).unwrap_or("Timer").to_owned();
            let _ = app.emit("timer-started", json!({ "label": label, "seconds": seconds }));
            tauri::async_runtime::spawn(async move {
                tokio::time::sleep(Duration::from_secs(seconds)).await;
                let _ = app.emit("timer-finished", json!({ "label": label }));
            });
            Ok(json!({ "started": true, "seconds": seconds }))
        },
    )
    .with_description("Starts a countdown timer in the app; the window shows it and announces when it ends.")
    .with_annotations(ToolAnnotations { idempotent: Some(false), ..Default::default() })
    .with_output_schema(json!({
        "type": "object",
        "properties": { "started": { "type": "boolean" }, "seconds": { "type": "integer" } },
        "required": ["started", "seconds"]
    }))
}

/// The stdio bridge sits next to the app binary: `cargo build` puts both in
/// `target/<profile>`, and Tauri bundles `externalBin` sidecars there too.
fn bridge_path() -> std::path::PathBuf {
    let exe = std::env::current_exe().unwrap_or_default();
    exe.with_file_name(format!("mcp-stdio-bridge{}", std::env::consts::EXE_SUFFIX))
}

fn main() {
    let flags = flags();
    let tray_only = flags.tray_only;
    let mcp = tauri_plugin_mcp_commands::Builder::new()
        .server_info("MCP Demo", env!("CARGO_PKG_VERSION"))
        .tools(collect_mcp_tools![
            notes::notes_list,
            notes::notes_create,
            notes::notes_delete,
            notes::word_count,
            diagnostics::where_sync,
            diagnostics::where_blocking,
            diagnostics::where_threadpool,
            diagnostics::where_async,
            diagnostics::hold_main,
        ])
        .tool(start_timer_tool())
        .preferred_port(flags.port)
        .autostart(flags.autostart && !flags.self_test)
        .require_approval(flags.approval)
        .bridge_path(bridge_path())
        .build();

    tauri::Builder::default()
        .plugin(mcp)
        .manage(NotesRepo::default())
        .manage(WebviewReports::default())
        .invoke_handler(tauri::generate_handler![
            notes::notes_list,
            notes::notes_create,
            notes::notes_delete,
            notes::word_count,
            diagnostics::where_sync,
            diagnostics::where_blocking,
            diagnostics::where_threadpool,
            diagnostics::where_async,
            diagnostics::hold_main,
            diagnostics::report,
        ])
        .setup(move |app| {
            // `setup` runs on the main thread.
            app.manage(MainThread(std::thread::current().id()));
            if flags.tray_only {
                TrayIconBuilder::new()
                    .icon(app.default_window_icon().unwrap().clone())
                    .tooltip("MCP Demo")
                    .build(app)?;
            } else {
                WebviewWindowBuilder::new(app, "main", WebviewUrl::default())
                    .title("MCP Demo")
                    .inner_size(760.0, 820.0)
                    .visible(!flags.hidden)
                    .build()?;
            }
            if flags.self_test {
                let handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    let ok = self_test(&handle).await;
                    handle.exit(if ok { 0 } else { 1 });
                });
            }
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building the app")
        .run(move |_app, event| {
            // Tray-only: closing every window must not quit; `app.exit(code)` still does.
            if let RunEvent::ExitRequested { api, code: None, .. } = event
                && tray_only
            {
                api.prevent_exit();
            }
        });
}

struct Client {
    http: reqwest::Client,
    url: String,
    token: String,
}

impl Client {
    async fn call(&self, id: u32, tool: &str, args: Value) -> Result<Value, String> {
        let body = json!({
            "jsonrpc": "2.0", "id": id, "method": "tools/call",
            "params": {
                "name": tool,
                "arguments": args,
                "_meta": {
                    "io.modelcontextprotocol/protocolVersion": "2026-07-28",
                    "io.modelcontextprotocol/clientInfo": { "name": "demo-self-test", "version": "0" },
                    "io.modelcontextprotocol/clientCapabilities": {}
                }
            }
        });
        let resp = self
            .http
            .post(&self.url)
            .bearer_auth(&self.token)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream")
            .header("MCP-Protocol-Version", "2026-07-28")
            .header("Mcp-Method", "tools/call")
            .header("Mcp-Name", tool)
            .body(body.to_string())
            .timeout(Duration::from_secs(40))
            .send()
            .await
            .map_err(|e| e.to_string())?;
        let v: Value = resp.json().await.map_err(|e| e.to_string())?;
        if v["result"]["isError"] == true || v.get("error").is_some() {
            return Err(v.to_string());
        }
        Ok(v["result"]["structuredContent"].clone())
    }
}

async fn self_test(app: &AppHandle) -> bool {
    let server = app.state::<McpServer<tauri::Wry>>().inner().clone();
    let status = match server.start().await {
        Ok(s) => s,
        Err(e) => {
            println!("[FAIL] start: {e}");
            return false;
        }
    };
    let configs = server.connection_configs().expect("running");
    println!("MCP server: {} ({} tools)", configs.url, status.tools.len());
    let client = std::sync::Arc::new(Client { http: reqwest::Client::new(), url: configs.url, token: configs.token });

    let mut ok = true;
    let mut check = |name: &str, pass: bool, detail: String| {
        println!("[{}] {name}: {detail}", if pass { "PASS" } else { "FAIL" });
        ok &= pass;
    };

    for (i, (tool, want_main)) in
        [("where_sync", true), ("where_blocking", false), ("where_threadpool", false), ("where_async", false)]
            .into_iter()
            .enumerate()
    {
        match client.call(i as u32, tool, json!({})).await {
            Ok(v) => check(tool, v["onMainThread"] == want_main, format!("{v}")),
            Err(e) => check(tool, false, e),
        }
    }

    // Concurrent sync calls queue on the main thread behind a slow one.
    let start = Instant::now();
    let spawn = |id: u32, tool: &'static str, args: Value| {
        let client = client.clone();
        tokio::spawn(async move { client.call(id, tool, args).await })
    };
    let slow = spawn(10, "hold_main", json!({ "ms": 500 }));
    let fast: Vec<_> = (0..5).map(|i| spawn(20 + i, "where_sync", json!({}))).collect();
    let mut all_ok = slow.await.unwrap().is_ok();
    for f in fast {
        all_ok &= f.await.unwrap().is_ok_and(|v| v["onMainThread"] == true);
    }
    check("concurrent main-thread calls", all_ok, format!("6 calls in {:?}", start.elapsed()));

    // The demo's own tools.
    let created = client.call(30, "notes_create", json!({ "title": "From MCP", "tags": ["agent"] })).await;
    check("notes_create", created.as_ref().is_ok_and(|v| v["title"] == "From MCP"), format!("{created:?}"));
    let listed = client.call(31, "notes_list", json!({ "tag": "agent" })).await;
    check("notes_list", listed.as_ref().is_ok_and(|v| v["result"][0]["title"] == "From MCP"), format!("{listed:?}"));
    let words = client.call(32, "word_count", json!({ "text": "one two three" })).await;
    check("word_count", words.as_ref().is_ok_and(|v| v["result"] == 3), format!("{words:?}"));
    let timer = client.call(33, "start_timer", json!({ "seconds": 1 })).await;
    check("start_timer (manual)", timer.as_ref().is_ok_and(|v| v["started"] == true), format!("{timer:?}"));
    let missing = client.call(34, "notes_delete", json!({ "id": 999 })).await;
    check("notes_delete error", missing.as_ref().is_err_and(|e| e.contains("no note with id 999")), format!("{missing:?}"));

    // The same commands still work over IPC from the webview.
    if let Some(window) = app.get_webview_window("main") {
        // App commands and plugin commands (through the ACL).
        let _ = window.eval(
            "const i = window.__TAURI__.core.invoke;\
             Promise.all([i('where_sync'), i('notes_list', { tag: 'agent' }), i('plugin:mcp-commands|status'),\
                          i('plugin:mcp-commands|connection_configs')])\
               .then(v => i('report', { label: 'ipc', value: v }))\
               .catch(e => i('report', { label: 'ipc-error', value: String(e) }))",
        );
        match wait_report(app, "ipc").await {
            Some((label, v)) if label == "ipc" => check(
                "webview IPC",
                v[0]["onMainThread"] == true
                    && v[1][0]["title"] == "From MCP"
                    && v[2]["running"] == true
                    && v[3]["claudeCode"].as_str().is_some_and(|c| c.starts_with("claude mcp add")),
                // Not the raw configs: they contain the bearer token.
                json!({ "where": v[0], "notes": v[1].as_array().map(Vec::len), "running": v[2]["running"] }).to_string(),
            ),
            Some((_, error)) => check("webview IPC", false, error.to_string()),
            None => check("webview IPC", false, "no report within 20s".into()),
        }

        // The UI script ran and rendered. It may still be loading (WebKit can
        // run `eval` on the parsed page before deferred module scripts), so
        // wait for its ready flag rather than reading the DOM once.
        let _ = window.eval(
            "(async () => {\
               const deadline = Date.now() + 15000;\
               while (!window.__demoReady && Date.now() < deadline) await new Promise(r => setTimeout(r, 100));\
               await window.__TAURI__.core.invoke('report', { label: 'ui', value: {\
                 ready: !!window.__demoReady,\
                 readyState: document.readyState,\
                 tools: document.getElementById('tools')?.textContent ?? null,\
                 errors: window.__demoErrors ?? 'errors.js did not run',\
               } });\
             })()",
        );
        match wait_report(app, "ui").await {
            Some((_, v)) => check(
                "webview UI",
                v["ready"] == true && v["tools"].as_str().is_some_and(|t| t.contains("notes_list")),
                v.to_string(),
            ),
            None => check("webview UI", false, "no report within 20s".into()),
        }
    } else {
        println!("[SKIP] webview IPC: no window");
    }

    server.stop().await;
    check("stop", !server.status().running, "stopped".into());
    ok
}

/// Waits for a `report` from the webview whose label starts with `prefix`.
async fn wait_report(app: &AppHandle, prefix: &str) -> Option<(String, Value)> {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let found = app.state::<WebviewReports>().0.lock().unwrap().iter().find(|(l, _)| l.starts_with(prefix)).cloned();
        if found.is_some() || Instant::now() > deadline {
            return found;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}
