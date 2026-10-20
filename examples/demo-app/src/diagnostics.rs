//! Thread-placement probes, used by `--self-test`.

use std::{thread::ThreadId, time::Duration};

use serde::Serialize;
use serde_json::Value;
use specta::Type;
use tauri::State;
use tauri_plugin_mcp_commands::mcp_tool;

pub struct MainThread(pub ThreadId);

/// Results the webview reports back during `--self-test`.
#[derive(Default)]
pub struct WebviewReports(pub std::sync::Mutex<Vec<(String, Value)>>);

#[derive(Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct Placement {
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
pub fn where_sync(main: State<'_, MainThread>) -> Placement {
    placement(&main)
}

/// A sync command forced onto the blocking pool with `#[mcp_tool(blocking)]`.
#[mcp_tool(read_only, blocking)]
#[tauri::command]
#[specta::specta]
pub fn where_blocking(main: State<'_, MainThread>) -> Placement {
    placement(&main)
}

/// A sync command Tauri itself runs on its thread pool.
#[mcp_tool(read_only)]
#[tauri::command(async)]
#[specta::specta]
pub fn where_threadpool(main: State<'_, MainThread>) -> Placement {
    placement(&main)
}

/// An async command.
#[mcp_tool(read_only)]
#[tauri::command]
#[specta::specta]
pub async fn where_async(main: State<'_, MainThread>) -> Result<Placement, String> {
    Ok(placement(&main))
}

/// Holds the main thread for `ms` milliseconds.
#[mcp_tool(idempotent)]
#[tauri::command]
#[specta::specta]
pub fn hold_main(ms: u64) -> u64 {
    std::thread::sleep(Duration::from_millis(ms.min(5_000)));
    ms
}

#[tauri::command]
pub fn report(reports: State<'_, WebviewReports>, label: String, value: Value) {
    reports.0.lock().unwrap().push((label, value));
}
