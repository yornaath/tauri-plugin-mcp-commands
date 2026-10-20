//! Plugin commands for the guest JS bindings. They take `AppHandle<R>` so the
//! runtime is inferred from the invoke.

use tauri::{AppHandle, Manager, Runtime};

use crate::plugin::{ConnectionConfigs, Error, McpServer, Status};

fn server<R: Runtime>(app: &AppHandle<R>) -> McpServer<R> {
    app.state::<McpServer<R>>().inner().clone()
}

#[tauri::command]
#[cfg_attr(feature = "specta", specta::specta)]
pub(crate) async fn start<R: Runtime>(app: AppHandle<R>) -> Result<Status, Error> {
    server(&app).start().await
}

#[tauri::command]
#[cfg_attr(feature = "specta", specta::specta)]
pub(crate) async fn stop<R: Runtime>(app: AppHandle<R>) -> Result<Status, Error> {
    let server = server(&app);
    server.stop().await;
    Ok(server.status())
}

#[tauri::command]
#[cfg_attr(feature = "specta", specta::specta)]
pub(crate) fn status<R: Runtime>(app: AppHandle<R>) -> Status {
    server(&app).status()
}

#[tauri::command]
#[cfg_attr(feature = "specta", specta::specta)]
pub(crate) fn connection_configs<R: Runtime>(app: AppHandle<R>) -> Option<ConnectionConfigs> {
    server(&app).connection_configs()
}

#[tauri::command]
#[cfg_attr(feature = "specta", specta::specta)]
pub(crate) fn respond_access<R: Runtime>(app: AppHandle<R>, id: u64, allow: bool) -> bool {
    server(&app).respond_access(id, allow)
}

#[tauri::command]
#[cfg_attr(feature = "specta", specta::specta)]
pub(crate) fn rotate_token<R: Runtime>(app: AppHandle<R>) -> Result<(), Error> {
    server(&app).rotate_token()
}

/// The plugin's commands and types, for generating bindings with tauri-specta:
///
/// ```ignore
/// tauri_plugin_mcp_commands::specta_builder()
///     .export(specta_typescript::Typescript::default(), "../src/bindings/mcp-commands.ts")?;
/// ```
///
/// Commands are invoked as `plugin:mcp-commands|<name>`. The event payloads
/// [`ConnectedClient`](crate::ConnectedClient) and
/// [`AccessRequest`](crate::AccessRequest) are exported as types.
#[cfg(feature = "tauri-specta")]
pub fn specta_builder() -> tauri_specta::Builder<tauri::Wry> {
    tauri_specta::Builder::<tauri::Wry>::new()
        .plugin_name("mcp-commands")
        // Everything crosses the IPC as JSON, so 64-bit integers (request ids,
        // numbers inside `serde_json::Value`) arrive as JS numbers anyway.
        .dangerously_cast_bigints_to_number()
        .commands(tauri_specta::collect_commands![
            start::<tauri::Wry>,
            stop::<tauri::Wry>,
            status::<tauri::Wry>,
            connection_configs::<tauri::Wry>,
            respond_access::<tauri::Wry>,
            rotate_token::<tauri::Wry>,
        ])
        .typ::<crate::ConnectedClient>()
        .typ::<crate::AccessRequest>()
}
