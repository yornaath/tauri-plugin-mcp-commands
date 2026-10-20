//! Expose selected Tauri commands as an MCP server.

mod auth;
mod clients;
mod commands;
pub mod discovery;
mod dispatch;
mod plugin;
#[cfg(feature = "specta")]
mod schema;
mod schema_util;
#[cfg(feature = "schemars")]
mod schemars_schema;
mod secure_fs;
mod server;
mod tool;
mod transport;

pub use clients::{AccessRequest, ConnectedClient, EVENT_ACCESS_REQUESTED, EVENT_CLIENT_CONNECTED};
#[cfg(feature = "tauri-specta")]
pub use commands::specta_builder;
pub use plugin::{Builder, ConnectionConfigs, Error, McpServer, Status};
pub use secure_fs::is_owner_only;
pub use server::ServerInfo;
pub use tauri_plugin_mcp_commands_macros::{collect_mcp_tools, mcp_tool};
pub use tool::{
    JsonObject, ToolAnnotations, ToolArgs, ToolDef, ToolError, ToolFuture, ToolHandler, ToolSchemas,
};

/// Used by generated code. Not public API.
#[doc(hidden)]
pub mod __private {
    pub use crate::dispatch::*;
    #[cfg(feature = "schemars")]
    pub use crate::tool::SchemarsSig;
    #[cfg(feature = "specta")]
    pub use crate::tool::SpectaSig;
    pub use crate::tool::{__run_blocking, __run_on_main_thread, MacroMeta, SchemaSource};
    #[cfg(feature = "schemars")]
    pub use schemars;
    pub use serde_json;
    #[cfg(feature = "specta")]
    pub use specta;
    pub use tauri;
}

// Schema sources, as macro_rules so generated code compiles whichever features
// are enabled. Called as `__x_source!(inputs: [T, ...], result: [R])`.

/// Schema source for a specta command. Used by generated code.
#[cfg(feature = "specta")]
#[doc(hidden)]
#[macro_export]
macro_rules! __specta_source {
    (inputs: [$($ty:ty),*], result: [$($ret:ty)?]) => {
        $crate::__private::SchemaSource::Specta(|types| $crate::__private::SpectaSig {
            inputs: ::std::vec![$(
                <$ty as $crate::__private::specta::function::FunctionArg>::to_datatype(types)
            ),*],
            result: $crate::__specta_source!(@result types $($ret)?),
        })
    };
    (@result $types:ident) => { ::std::option::Option::None };
    (@result $types:ident $ret:ty) => {
        ::std::option::Option::Some(<$ret as $crate::__private::specta::Type>::definition($types))
    };
}

#[cfg(not(feature = "specta"))]
#[doc(hidden)]
#[macro_export]
macro_rules! __specta_source {
    ($($t:tt)*) => {
        ::core::compile_error!(
            "this command uses specta (`#[specta::specta]` or `#[mcp_tool(specta)]`); enable the `specta` feature of tauri-plugin-mcp-commands"
        )
    };
}

/// Schema source for other commands: schemars when that feature is on,
/// otherwise untyped. Used by generated code.
#[cfg(feature = "schemars")]
#[doc(hidden)]
#[macro_export]
macro_rules! __fallback_source {
    (inputs: [$($ty:ty),*], result: [$($ret:ty)?]) => {
        $crate::__private::SchemaSource::Schemars(|g| {
            #[allow(unused_imports)]
            use $crate::__private::schemars_glue::{
                Arg, Ret, SchemarsAppHandle as _, SchemarsOption as _, SchemarsOutput as _, SchemarsResult as _,
                SchemarsState as _, SchemarsValue as _,
            };
            $crate::__private::SchemarsSig {
                inputs: ::std::vec![$((&&&Arg::<$ty>::new()).__mcp_schemars(g)),*],
                result: $crate::__fallback_source!(@result g $($ret)?),
            }
        })
    };
    (@result $g:ident) => { ::std::option::Option::None };
    (@result $g:ident $ret:ty) => { (&Ret::<$ret>::new()).__mcp_schemars_out($g) };
}

#[cfg(not(feature = "schemars"))]
#[doc(hidden)]
#[macro_export]
macro_rules! __fallback_source {
    ($($t:tt)*) => {
        $crate::__private::SchemaSource::Untyped
    };
}
