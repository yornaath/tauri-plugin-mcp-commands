//! Syntax-level parameter checks. Real classification is type-based (see the
//! core crate's `dispatch` module); this only gives early, well-placed errors
//! and picks the runtime type parameter for the generated `ToolDef`.

use syn::{
    GenericArgument, GenericParam, Generics, Lifetime, PathArguments, Type, TypeParamBound,
    visit::{self, Visit},
    visit_mut::VisitMut,
};

const WINDOW_BOUND: &[&str] = &["Window", "WebviewWindow", "Webview"];

/// If `ty` is syntactically a Tauri window type, returns its name.
/// Only bare names (`Window<R>`) and `tauri::` paths count, so an unrelated
/// `my_crate::Window` is left to the type-based check.
pub fn window_bound(ty: &Type) -> Option<&'static str> {
    match ty {
        Type::Reference(r) => window_bound(&r.elem),
        Type::Paren(p) => window_bound(&p.elem),
        Type::Group(g) => window_bound(&g.elem),
        Type::Path(p) if p.qself.is_none() => {
            let segs = &p.path.segments;
            let last = segs.last()?.ident.to_string();
            let name = WINDOW_BOUND.iter().find(|w| **w == last)?;
            let tauri_path = segs.len() == 1
                || segs.first().is_some_and(|s| s.ident == "tauri") && segs.len() <= 3;
            tauri_path.then_some(*name)
        }
        _ => None,
    }
}

/// The runtime type the generated `ToolDef<_>` is parameterised by.
pub enum RuntimeParam {
    /// The command is generic over a `Runtime` param; reuse it.
    Existing(syn::Ident),
    /// The command takes `AppHandle` (i.e. `AppHandle<Wry>`) or `AppHandle<X>`
    /// for a concrete `X`, so the tool only works with that runtime.
    Concrete(Type),
    /// The command works with any runtime; the generated fn adds `__R`.
    Fresh,
}

pub fn runtime_param(generics: &Generics, arg_types: &[&Type]) -> RuntimeParam {
    for param in &generics.params {
        if let GenericParam::Type(tp) = param {
            let is_runtime = tp.bounds.iter().any(|b| match b {
                TypeParamBound::Trait(t) => t.path.segments.last().is_some_and(|s| s.ident == "Runtime"),
                _ => false,
            });
            if is_runtime {
                return RuntimeParam::Existing(tp.ident.clone());
            }
        }
    }
    struct FindAppHandle(Option<Type>);
    impl<'ast> Visit<'ast> for FindAppHandle {
        fn visit_path_segment(&mut self, seg: &'ast syn::PathSegment) {
            if seg.ident == "AppHandle" && self.0.is_none() {
                self.0 = match &seg.arguments {
                    PathArguments::None => Some(syn::parse_quote!(::tauri_plugin_mcp_server::__private::tauri::Wry)),
                    PathArguments::AngleBracketed(a) => a.args.iter().find_map(|g| match g {
                        GenericArgument::Type(t) => Some(t.clone()),
                        _ => None,
                    }),
                    PathArguments::Parenthesized(_) => None,
                };
            }
            visit::visit_path_segment(self, seg);
        }
    }
    let mut finder = FindAppHandle(None);
    for ty in arg_types {
        finder.visit_type(ty);
    }
    match finder.0 {
        Some(rt) => RuntimeParam::Concrete(rt),
        None => RuntimeParam::Fresh,
    }
}

/// Replaces every named lifetime with `'_`, so a parameter type like
/// `State<'r, Repo>` can be named outside the command's signature.
pub fn erase_lifetimes(ty: &Type) -> Type {
    struct Erase;
    impl VisitMut for Erase {
        fn visit_lifetime_mut(&mut self, lt: &mut Lifetime) {
            *lt = Lifetime::new("'_", lt.apostrophe);
        }
    }
    let mut ty = ty.clone();
    Erase.visit_type_mut(&mut ty);
    ty
}

#[cfg(test)]
mod tests {
    use super::*;
    use quote::ToTokens;
    use syn::parse_quote;

    #[test]
    fn detects_window_types() {
        let cases: &[(Type, Option<&str>)] = &[
            (parse_quote!(Window), Some("Window")),
            (parse_quote!(Window<R>), Some("Window")),
            (parse_quote!(tauri::Window<R>), Some("Window")),
            (parse_quote!(::tauri::WebviewWindow), Some("WebviewWindow")),
            (parse_quote!(tauri::webview::Webview<R>), Some("Webview")),
            (parse_quote!(&Webview), Some("Webview")),
            (parse_quote!(my_crate::Window), None),
            (parse_quote!(State<'_, Window>), None),
            (parse_quote!(AppHandle), None),
            (parse_quote!(String), None),
        ];
        for (ty, want) in cases {
            assert_eq!(window_bound(ty), *want, "{}", ty.to_token_stream());
        }
    }

    #[test]
    fn picks_runtime_param() {
        let g: Generics = parse_quote!(<R: tauri::Runtime>);
        assert!(matches!(runtime_param(&g, &[]), RuntimeParam::Existing(i) if i == "R"));

        let g: Generics = parse_quote!(<Rt: Runtime + Send>);
        assert!(matches!(runtime_param(&g, &[]), RuntimeParam::Existing(i) if i == "Rt"));

        let rt = |tys: &[&Type]| match runtime_param(&Generics::default(), tys) {
            RuntimeParam::Concrete(t) => Some(t.to_token_stream().to_string()),
            RuntimeParam::Fresh => None,
            RuntimeParam::Existing(_) => unreachable!(),
        };
        let bare: Type = parse_quote!(tauri::AppHandle);
        let wry: Type = parse_quote!(AppHandle<tauri::Wry>);
        let state: Type = parse_quote!(State<'_, Repo>);
        let input: Type = parse_quote!(u32);
        assert_eq!(rt(&[&bare]).as_deref(), Some(":: tauri_plugin_mcp_server :: __private :: tauri :: Wry"));
        assert_eq!(rt(&[&state, &wry]).as_deref(), Some("tauri :: Wry"));
        assert_eq!(rt(&[&state, &input]), None);
    }

    #[test]
    fn erases_named_lifetimes() {
        let ty: Type = parse_quote!(State<'r, Vec<&'a str>>);
        assert_eq!(erase_lifetimes(&ty).to_token_stream().to_string(), "State < '_ , Vec < & '_ str > >");
    }
}
