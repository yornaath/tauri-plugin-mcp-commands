use tauri_plugin_mcp_commands::mcp_tool;

#[mcp_tool(name = "has space")]
#[tauri::command]
async fn ping() -> &'static str {
    "pong"
}

fn main() {}
