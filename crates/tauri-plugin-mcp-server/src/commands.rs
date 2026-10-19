//! Plugin commands for the guest JS bindings. They take `AppHandle<R>` so the
//! runtime is inferred from the invoke.

use tauri::{AppHandle, Manager, Runtime};

use crate::plugin::{ConnectionConfigs, Error, McpServer, Status};

fn server<R: Runtime>(app: &AppHandle<R>) -> McpServer<R> {
    app.state::<McpServer<R>>().inner().clone()
}

#[tauri::command]
pub(crate) async fn start<R: Runtime>(app: AppHandle<R>) -> Result<Status, Error> {
    server(&app).start().await
}

#[tauri::command]
pub(crate) async fn stop<R: Runtime>(app: AppHandle<R>) -> Result<Status, Error> {
    let server = server(&app);
    server.stop().await;
    Ok(server.status())
}

#[tauri::command]
pub(crate) fn status<R: Runtime>(app: AppHandle<R>) -> Status {
    server(&app).status()
}

#[tauri::command]
pub(crate) fn connection_configs<R: Runtime>(app: AppHandle<R>) -> Option<ConnectionConfigs> {
    server(&app).connection_configs()
}

#[tauri::command]
pub(crate) fn respond_access<R: Runtime>(app: AppHandle<R>, id: u64, allow: bool) -> bool {
    server(&app).respond_access(id, allow)
}

#[tauri::command]
pub(crate) fn rotate_token<R: Runtime>(app: AppHandle<R>) -> Result<(), Error> {
    server(&app).rotate_token()
}
