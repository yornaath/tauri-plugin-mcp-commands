//! Proc macros for `tauri-plugin-mcp-commands`.

use proc_macro::TokenStream;
use quote::{format_ident, quote};
use syn::{ext::IdentExt, parse_macro_input, punctuated::Punctuated, ExprPath, Token};

mod attrs;
mod classify;
mod expand;

/// Exposes a command as an MCP tool.
///
/// Place it **above** `#[tauri::command]` so it can read the command's
/// `rename_all`, `rename` and `async` options:
///
/// ```ignore
/// /// Lists every note, newest first.
/// #[mcp_tool(read_only)]
/// #[tauri::command]
/// #[specta::specta]
/// async fn notes_list(repo: State<'_, NotesRepo>, limit: u32) -> Result<Vec<Note>, String> { … }
/// ```
///
/// Arguments: `name = "…"`, `title = "…"`, `read_only`, `destructive`,
/// `idempotent`, `open_world` (each optionally `= bool`), `blocking` (run a
/// sync command on the blocking pool instead of the main thread),
/// `rename_all = "camelCase" | "snake_case"`, and `specta` (use specta schemas
/// even when `#[specta::specta]` isn't below this attribute).
#[proc_macro_attribute]
pub fn mcp_tool(attr: TokenStream, item: TokenStream) -> TokenStream {
    let mut args = attrs::McpToolArgs::default();
    let parser = syn::meta::parser(|meta| args.parse_meta(meta));
    parse_macro_input!(attr with parser);
    let item = parse_macro_input!(item as syn::ItemFn);
    match expand::mcp_tool(args, item.clone()) {
        Ok(tokens) => tokens.into(),
        // Keep the command itself so the error doesn't cascade.
        Err(err) => {
            let err = err.into_compile_error();
            quote!(#item #err).into()
        }
    }
}

/// Collects `#[mcp_tool]` commands into a `Vec<ToolDef<R>>`.
///
/// ```ignore
/// collect_mcp_tools![notes_list, notes::create, generic_cmd::<tauri::Wry>]
/// ```
#[proc_macro]
pub fn collect_mcp_tools(input: TokenStream) -> TokenStream {
    let paths = parse_macro_input!(input with Punctuated::<ExprPath, Token![,]>::parse_terminated);
    let calls = paths.into_iter().map(|mut p| {
        let last = p.path.segments.last_mut().expect("path has a segment");
        last.ident = format_ident!(
            "__mcp_tool_{}",
            last.ident.unraw(),
            span = last.ident.span()
        );
        quote!(#p())
    });
    quote!(::std::vec![#(#calls),*]).into()
}
