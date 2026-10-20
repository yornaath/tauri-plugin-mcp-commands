use tauri::ipc::Channel;
use tauri_plugin_mcp_commands::mcp_tool;

#[mcp_tool]
#[tauri::command]
async fn stream(on_progress: Channel<u32>) -> Result<(), String> {
    on_progress.send(1).map_err(|e| e.to_string())
}

fn main() {}
