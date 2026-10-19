//! Serves a few tools from tauri's mock runtime, for poking at with an MCP
//! client: `cargo run -p tauri-plugin-mcp-server --example mock_server [port]`.

use serde::{Deserialize, Serialize};
use specta::Type;
use tauri_plugin_mcp_server::{ServerInfo, __private::serve, collect_mcp_tools, mcp_tool};

#[derive(Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct Note {
    title: String,
    body: Option<String>,
    is_pinned: bool,
}

/// Echoes a note back with its title upper-cased.
#[mcp_tool(read_only)]
#[tauri::command]
#[specta::specta]
async fn shout(note: Note, repeat: Option<u8>) -> Result<Note, String> {
    let n = repeat.unwrap_or(1).max(1) as usize;
    Ok(Note { title: note.title.to_uppercase().repeat(n), ..note })
}

/// Always fails.
#[mcp_tool(destructive)]
#[tauri::command]
#[specta::specta]
async fn explode(reason: String) -> Result<(), String> {
    Err(format!("boom: {reason}"))
}

#[tokio::main]
async fn main() {
    let port = std::env::args().nth(1).and_then(|p| p.parse().ok()).unwrap_or(0);
    let app = tauri::test::mock_app();
    let info = ServerInfo { name: "mock-app".into(), version: "0.1.0".into() };
    let (addr, _cancel) = serve(app.handle().clone(), collect_mcp_tools![shout, explode], info, port).await.unwrap();
    println!("listening on http://{addr}/mcp");
    tokio::signal::ctrl_c().await.unwrap();
}
