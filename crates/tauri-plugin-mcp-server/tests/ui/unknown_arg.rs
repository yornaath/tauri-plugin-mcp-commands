use tauri_plugin_mcp_server::mcp_tool;

#[mcp_tool(readonly)]
#[tauri::command]
async fn ping() -> &'static str {
    "pong"
}

fn main() {}
