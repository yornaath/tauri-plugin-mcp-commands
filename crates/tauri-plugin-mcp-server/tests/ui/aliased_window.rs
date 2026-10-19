use tauri::test::MockRuntime;
use tauri_plugin_mcp_server::mcp_tool;

/// An alias hides the window type from the macro; the type-based check still catches it.
type MainWindow = tauri::Window<MockRuntime>;

#[mcp_tool]
#[tauri::command]
async fn title(window: MainWindow) -> Result<String, String> {
    window.title().map_err(|e| e.to_string())
}

fn main() {}
