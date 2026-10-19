use tauri::Runtime;
use tauri_plugin_mcp_server::mcp_tool;

#[mcp_tool]
#[tauri::command]
async fn focus<R: Runtime>(window: tauri::WebviewWindow<R>) -> Result<(), String> {
    window.set_focus().map_err(|e| e.to_string())
}

fn main() {}
