//! Expose selected Tauri commands as an MCP server.

mod dispatch;
mod tool;

pub use tauri_plugin_mcp_server_macros::{collect_mcp_tools, mcp_tool};
pub use tool::{ToolAnnotations, ToolArgs, ToolDef, ToolError, ToolFuture, ToolHandler};

/// Used by generated code. Not public API.
#[doc(hidden)]
pub mod __private {
    pub use crate::dispatch::*;
    pub use crate::tool::{__run_blocking, __run_on_main_thread, MacroMeta};
    pub use serde_json;
    pub use tauri;
}
