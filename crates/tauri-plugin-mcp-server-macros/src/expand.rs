use heck::{ToLowerCamelCase, ToSnakeCase};
use proc_macro2::TokenStream;
use quote::{format_ident, quote, quote_spanned};
use syn::{FnArg, GenericParam, ItemFn, Pat, ext::IdentExt, spanned::Spanned};

use crate::{
    attrs::{self, ArgCase, McpToolArgs},
    classify::{self, RuntimeParam},
};

enum Executor {
    Async,
    MainThread,
    Blocking,
}

pub fn mcp_tool(args: McpToolArgs, item: ItemFn) -> syn::Result<TokenStream> {
    let cmd = attrs::parse_command_attrs(&item.attrs)?;
    let sig = &item.sig;
    let ident = &sig.ident;

    if let Some(recv) = sig.receiver() {
        return Err(syn::Error::new_spanned(recv, "`#[mcp_tool]` can't be used on methods"));
    }

    let case = args.rename_all.or(cmd.rename_all).unwrap_or(ArgCase::Camel);
    let tool_name = match (&args.name, &cmd.rename) {
        (Some(lit), _) | (None, Some(lit)) => {
            attrs::validate_tool_name(&lit.value(), lit)?;
            lit.value()
        }
        (None, None) => {
            let name = ident.unraw().to_string();
            attrs::validate_tool_name(&name, ident)?;
            name
        }
    };

    let mut types = Vec::new();
    let mut keys = Vec::new();
    for input in &sig.inputs {
        let FnArg::Typed(pt) = input else { unreachable!() };
        if let Some(window) = classify::window_bound(&pt.ty) {
            return Err(syn::Error::new_spanned(
                &pt.ty,
                format!(
                    "`{window}` can't be a parameter of an `#[mcp_tool]` command: an MCP tool call has no \
                     calling window. Take an `AppHandle` and look the window up instead, e.g. \
                     `app.get_webview_window(\"main\")`"
                ),
            ));
        }
        let Pat::Ident(pi) = &*pt.pat else {
            return Err(syn::Error::new_spanned(&pt.pat, "`#[mcp_tool]` command parameters must be named"));
        };
        let raw = pi.ident.unraw().to_string();
        keys.push(match case {
            ArgCase::Camel => raw.to_lower_camel_case(),
            ArgCase::Snake => raw.to_snake_case(),
        });
        types.push(&*pt.ty);
    }

    let executor = if sig.asyncness.is_some() {
        Executor::Async
    } else if args.blocking || cmd.run_async {
        Executor::Blocking
    } else {
        Executor::MainThread
    };

    let m = quote!(::tauri_plugin_mcp_server);
    let p = quote!(#m::__private);

    // Generics of the generated fn: the command's type/const params, plus
    // `__R` when the command is runtime-agnostic.
    let mut gen_generics = item.sig.generics.clone();
    gen_generics.params = gen_generics
        .params
        .into_iter()
        .filter(|g| !matches!(g, GenericParam::Lifetime(_)))
        .collect();
    let runtime = match classify::runtime_param(&sig.generics, &types) {
        RuntimeParam::Existing(r) => quote!(#r),
        RuntimeParam::Concrete(t) => quote!(#t),
        RuntimeParam::Fresh => {
            gen_generics.params.push(syn::parse_quote!(__R: #p::tauri::Runtime));
            quote!(__R)
        }
    };
    let turbofish_args: Vec<_> = sig
        .generics
        .params
        .iter()
        .filter_map(|g| match g {
            GenericParam::Type(t) => Some(t.ident.clone()),
            GenericParam::Const(c) => Some(c.ident.clone()),
            GenericParam::Lifetime(_) => None,
        })
        .collect();
    let call_path = if turbofish_args.is_empty() { quote!(#ident) } else { quote!(#ident::<#(#turbofish_args),*>) };
    let (impl_generics, _, where_clause) = gen_generics.split_for_impl();

    let bindings: Vec<_> = (0..types.len()).map(|i| format_ident!("__mcp_arg{i}")).collect();
    // Spanned to the parameter type so type-based errors point at it.
    let extract = types.iter().zip(&keys).zip(&bindings).map(|((ty, key), b)| {
        let span = ty.span();
        let ty = classify::erase_lifetimes(ty);
        quote_spanned!(span=> let #b = (&&&#p::Arg::<#ty>::new()).__mcp_extract(&__cx, #key)?;)
    });
    let is_input = types.iter().map(|ty| {
        let ty = classify::erase_lifetimes(ty);
        quote!((&&&#p::Arg::<#ty>::new()).__mcp_is_input())
    });

    let body = match executor {
        Executor::Async => quote! {
            ::std::boxed::Box::pin(async move {
                let __cx = #p::Cx::new(&__app, &__args, #tool_name);
                #(#extract)*
                let __ret = #call_path(#(#bindings),*).await;
                (&__ret).__mcp_kind().into_value(__ret)
            })
        },
        Executor::MainThread | Executor::Blocking => {
            let run = match executor {
                Executor::MainThread => quote!(#p::__run_on_main_thread),
                _ => quote!(#p::__run_blocking),
            };
            quote! {
                #run(__app, move |__app| {
                    let __cx = #p::Cx::new(__app, &__args, #tool_name);
                    #(#extract)*
                    let __ret = #call_path(#(#bindings),*);
                    (&__ret).__mcp_kind().into_value(__ret)
                })
            }
        }
    };

    let opt_str = |s: Option<String>| match s {
        Some(s) => quote!(::std::option::Option::Some(#s)),
        None => quote!(::std::option::Option::None),
    };
    let opt_bool = |b: Option<bool>| match b {
        Some(b) => quote!(::std::option::Option::Some(#b)),
        None => quote!(::std::option::Option::None),
    };
    let description = opt_str(attrs::doc_string(&item.attrs));
    let title = match &args.title {
        Some(t) => quote!(::std::option::Option::Some(::std::string::String::from(#t))),
        None => quote!(::std::option::Option::None),
    };
    let (read_only, destructive, idempotent, open_world) = (
        opt_bool(args.read_only),
        opt_bool(args.destructive),
        opt_bool(args.idempotent),
        opt_bool(args.open_world),
    );

    let vis = &item.vis;
    let gen_ident = format_ident!("__mcp_tool_{}", ident.unraw());
    let keys_lit = &keys;

    Ok(quote! {
        #item

        #[doc(hidden)]
        #[allow(non_snake_case, clippy::needless_question_mark)]
        #vis fn #gen_ident #impl_generics () -> #m::ToolDef<#runtime> #where_clause {
            #[allow(unused_imports)]
            use #p::{
                ExtractAppHandle as _, ExtractInput as _, ExtractState as _, ExtractWindowBound as _,
                IsInputAppHandle as _, IsInputState as _, IsInputValue as _, IsInputWindowBound as _,
                ResultKind as _, ValueKind as _,
            };
            const __KEYS: &[&str] = &[#(#keys_lit),*];
            let __is_input: &[bool] = &[#(#is_input),*];
            let __inputs: ::std::vec::Vec<&'static str> = __KEYS
                .iter()
                .zip(__is_input)
                .filter_map(|(k, i)| i.then_some(*k))
                .collect();
            #m::ToolDef::__from_macro(
                #p::MacroMeta {
                    name: #tool_name,
                    description: #description,
                    annotations: #m::ToolAnnotations {
                        title: #title,
                        read_only: #read_only,
                        destructive: #destructive,
                        idempotent: #idempotent,
                        open_world: #open_world,
                    },
                    inputs: __inputs,
                },
                |__app: #p::tauri::AppHandle<#runtime>, __args: #m::ToolArgs| -> #m::ToolFuture { #body },
            )
        }
    })
}
