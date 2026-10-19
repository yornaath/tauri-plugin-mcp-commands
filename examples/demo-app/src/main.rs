//! Demo app for tauri-plugin-mcp-server.
//!
//! Flags:
//! - `--hidden`: create the main window invisible
//! - `--tray-only`: no windows at all, just a tray icon
//! - `--self-test`: call the app's own MCP server over HTTP, print a report, exit
//! - `--port N`: MCP port (default 43110)

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::{
    sync::Mutex,
    thread::ThreadId,
    time::{Duration, Instant},
};

use serde::Serialize;
use serde_json::{Value, json};
use specta::Type;
use tauri::{AppHandle, Manager, RunEvent, State, WebviewUrl, WebviewWindowBuilder, tray::TrayIconBuilder};
use tauri_plugin_mcp_server::{ServerInfo, __private::serve, collect_mcp_tools, mcp_tool};

struct MainThread(ThreadId);

/// Results the webview reports back during `--self-test`.
#[derive(Default)]
struct WebviewReports(Mutex<Vec<(String, Value)>>);

#[derive(Serialize, Type)]
#[serde(rename_all = "camelCase")]
struct Placement {
    on_main_thread: bool,
    thread: String,
}

fn placement(main: &MainThread) -> Placement {
    let t = std::thread::current();
    Placement { on_main_thread: t.id() == main.0, thread: format!("{:?} {}", t.id(), t.name().unwrap_or("-")) }
}

/// Reports which thread a plain sync command runs on.
#[mcp_tool(read_only)]
#[tauri::command]
#[specta::specta]
fn where_sync(main: State<'_, MainThread>) -> Placement {
    placement(&main)
}

/// A sync command forced onto the blocking pool with `#[mcp_tool(blocking)]`.
#[mcp_tool(read_only, blocking)]
#[tauri::command]
#[specta::specta]
fn where_blocking(main: State<'_, MainThread>) -> Placement {
    placement(&main)
}

/// A sync command Tauri itself runs on its thread pool.
#[mcp_tool(read_only)]
#[tauri::command(async)]
#[specta::specta]
fn where_threadpool(main: State<'_, MainThread>) -> Placement {
    placement(&main)
}

/// An async command.
#[mcp_tool(read_only)]
#[tauri::command]
#[specta::specta]
async fn where_async(main: State<'_, MainThread>) -> Result<Placement, String> {
    Ok(placement(&main))
}

/// Holds the main thread for `ms` milliseconds.
#[mcp_tool(idempotent)]
#[tauri::command]
#[specta::specta]
fn hold_main(ms: u64) -> u64 {
    std::thread::sleep(Duration::from_millis(ms));
    ms
}

#[tauri::command]
fn report(reports: State<'_, WebviewReports>, label: String, value: Value) {
    reports.0.lock().unwrap().push((label, value));
}

struct Flags {
    hidden: bool,
    tray_only: bool,
    self_test: bool,
    port: u16,
}

fn flags() -> Flags {
    let args: Vec<String> = std::env::args().collect();
    let port = args
        .iter()
        .position(|a| a == "--port")
        .and_then(|i| args.get(i + 1))
        .and_then(|p| p.parse().ok())
        .unwrap_or(43110);
    Flags {
        hidden: args.iter().any(|a| a == "--hidden"),
        tray_only: args.iter().any(|a| a == "--tray-only"),
        self_test: args.iter().any(|a| a == "--self-test"),
        port,
    }
}

fn main() {
    let flags = flags();
    let tray_only = flags.tray_only;
    tauri::Builder::default()
        .manage(WebviewReports::default())
        .invoke_handler(tauri::generate_handler![
            where_sync,
            where_blocking,
            where_threadpool,
            where_async,
            hold_main,
            report
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
                    .visible(!flags.hidden)
                    .build()?;
            }

            let handle = app.handle().clone();
            let port = flags.port;
            let self_test = flags.self_test;
            tauri::async_runtime::spawn(async move {
                let tools = collect_mcp_tools![where_sync, where_blocking, where_threadpool, where_async, hold_main];
                let info = ServerInfo { name: "MCP Demo".into(), version: env!("CARGO_PKG_VERSION").into() };
                let (addr, cancel) = match serve(handle.clone(), tools, info, port).await {
                    Ok(v) => v,
                    Err(e) => {
                        eprintln!("failed to start MCP server: {e}");
                        handle.exit(2);
                        return;
                    }
                };
                println!("MCP server: http://{addr}/mcp");
                // Keep the server for the app's lifetime.
                std::mem::forget(cancel);
                if self_test {
                    let ok = self_test_run(&handle, &format!("http://{addr}/mcp")).await;
                    handle.exit(if ok { 0 } else { 1 });
                }
            });
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building the app")
        .run(move |_app, event| {
            // Tray-only: closing every window must not quit; `app.exit(code)` still does.
            if let RunEvent::ExitRequested { api, code: None, .. } = event {
                if tray_only {
                    api.prevent_exit();
                }
            }
        });
}

async fn call(client: &reqwest::Client, url: &str, id: u32, tool: &str, args: Value) -> Result<Value, String> {
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
    let resp = client
        .post(url)
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

async fn self_test_run(app: &AppHandle, url: &str) -> bool {
    let client = reqwest::Client::new();
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
        match call(&client, url, i as u32, tool, json!({})).await {
            Ok(v) => check(tool, v["onMainThread"] == want_main, format!("{v}")),
            Err(e) => check(tool, false, e),
        }
    }

    // Concurrent sync calls queue on the main thread behind a slow one.
    let start = Instant::now();
    let spawn = |id: u32, tool: &'static str, args: Value| {
        let (client, url) = (client.clone(), url.to_owned());
        tokio::spawn(async move { call(&client, &url, id, tool, args).await })
    };
    let slow = spawn(10, "hold_main", json!({ "ms": 500 }));
    let fast: Vec<_> = (0..5).map(|i| spawn(20 + i, "where_sync", json!({}))).collect();
    let slow = slow.await.unwrap();
    let mut all_ok = slow.is_ok();
    for f in fast {
        all_ok &= f.await.unwrap().is_ok_and(|v| v["onMainThread"] == true);
    }
    check("concurrent main-thread calls", all_ok, format!("6 calls in {:?}", start.elapsed()));

    // The same commands still work over IPC from the webview.
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.eval(
            "window.__TAURI__.core.invoke('where_sync')\
               .then(v => window.__TAURI__.core.invoke('report', { label: 'where_sync', value: v }))\
               .catch(e => window.__TAURI__.core.invoke('report', { label: 'error', value: String(e) }))",
        );
        let deadline = Instant::now() + Duration::from_secs(10);
        let reported = loop {
            if let Some(r) = app.state::<WebviewReports>().0.lock().unwrap().first().cloned() {
                break Some(r);
            }
            if Instant::now() > deadline {
                break None;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        };
        match reported {
            Some((label, v)) => check("webview IPC", label == "where_sync" && v["onMainThread"] == true, format!("{v}")),
            None => check("webview IPC", false, "no report within 10s".into()),
        }
    } else {
        println!("[SKIP] webview IPC: no window");
    }
    ok
}
