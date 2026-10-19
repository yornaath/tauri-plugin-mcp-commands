//! Expose selected Tauri commands as an MCP server.

mod dispatch;
#[cfg(feature = "specta")]
mod schema;
mod tool;

pub use tauri_plugin_mcp_server_macros::{collect_mcp_tools, mcp_tool};
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
