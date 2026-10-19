//! Expose selected Tauri commands as an MCP server.

mod dispatch;
#[cfg(feature = "specta")]
mod schema;
mod server;
mod tool;
mod transport;

pub use tauri_plugin_mcp_server_macros::{collect_mcp_tools, mcp_tool};
pub use server::ServerInfo;
pub use tool::{JsonObject, ToolAnnotations, ToolArgs, ToolDef, ToolError, ToolFuture, ToolHandler, ToolSchemas};

/// Used by generated code. Not public API.
#[doc(hidden)]
pub mod __private {
    pub use crate::dispatch::*;
    pub use crate::tool::{__run_blocking, __run_on_main_thread, MacroMeta, SchemaSource};
    pub use serde_json;
    #[cfg(feature = "specta")]
    pub use specta;
    pub use tauri;

    /// Spike entry point until the plugin builder exists: serves `tools` on
    /// `127.0.0.1:port` and returns the bound address and a stop token.
    pub async fn serve<R: tauri::Runtime>(
        app: tauri::AppHandle<R>,
        tools: Vec<crate::ToolDef<R>>,
        info: crate::ServerInfo,
        port: u16,
    ) -> std::io::Result<(std::net::SocketAddr, tokio_util::sync::CancellationToken)> {
        let registry = crate::server::Registry::new(tools).map_err(std::io::Error::other)?;
        let running = crate::transport::serve(app, std::sync::Arc::new(registry), std::sync::Arc::new(info), port).await?;
        Ok((running.addr, running.cancel))
    }
}

/// Schema source for a `#[specta::specta]` command. Used by generated code.
#[cfg(feature = "specta")]
#[doc(hidden)]
#[macro_export]
macro_rules! __specta_source {
    ($($f:tt)*) => {
        $crate::__private::SchemaSource::Specta(|types| {
            $crate::__private::specta::function::fn_datatype!($($f)*)(types)
        })
    };
}

#[cfg(not(feature = "specta"))]
#[doc(hidden)]
#[macro_export]
macro_rules! __specta_source {
    ($($f:tt)*) => {
        ::core::compile_error!(
            "this command uses `#[specta::specta]`; enable the `specta` feature of tauri-plugin-mcp-server"
        )
    };
}
