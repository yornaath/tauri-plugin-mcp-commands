//! Serves a few tools from tauri's mock runtime, for poking at with an MCP
//! client: `cargo run -p tauri-plugin-mcp-server --example mock_server [port]`.
//! Prints the URL and bearer token.

use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::Manager;
use tauri_plugin_mcp_server::{Builder, McpServer, collect_mcp_tools, mcp_tool};

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
    let dir = std::env::temp_dir().join("mcp-mock-server");
    let app = tauri::test::mock_builder()
        .plugin(
            Builder::new()
                .server_info("mock-app", "0.1.0")
                .tools(collect_mcp_tools![shout, explode])
                .preferred_port(port)
                .data_dir(dir)
                .build(),
        )
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .unwrap();
    let server = app.state::<McpServer<tauri::test::MockRuntime>>();
    server.start().await.unwrap();
    let c = server.connection_configs().unwrap();
    println!("url:   {}\ntoken: {}\n\n{}", c.url, c.token, c.claude_code);
    tokio::signal::ctrl_c().await.unwrap();
    server.stop().await;
}
