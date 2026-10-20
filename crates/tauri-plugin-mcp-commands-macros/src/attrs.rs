use syn::{
    Attribute, Expr, ExprLit, Lit, LitBool, LitStr, Meta, Token, meta::ParseNestedMeta, parse::Parse,
    punctuated::Punctuated, spanned::Spanned,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArgCase {
    Camel,
    Snake,
}

impl ArgCase {
    fn parse(lit: &LitStr) -> syn::Result<Self> {
        match lit.value().as_str() {
            "camelCase" => Ok(Self::Camel),
            "snake_case" => Ok(Self::Snake),
            _ => Err(syn::Error::new(lit.span(), "expected \"camelCase\" or \"snake_case\"")),
        }
    }
}

/// Arguments of `#[mcp_tool(...)]`.
#[derive(Default, Debug)]
pub struct McpToolArgs {
    pub name: Option<LitStr>,
    pub title: Option<LitStr>,
    pub read_only: Option<bool>,
    pub destructive: Option<bool>,
    pub idempotent: Option<bool>,
    pub open_world: Option<bool>,
    pub blocking: bool,
    pub rename_all: Option<ArgCase>,
    /// Use specta schemas even without a visible `#[specta::specta]`.
    pub specta: bool,
}

impl McpToolArgs {
    pub fn parse_meta(&mut self, meta: ParseNestedMeta) -> syn::Result<()> {
        let flag = |meta: &ParseNestedMeta| -> syn::Result<bool> {
            if meta.input.peek(Token![=]) {
                Ok(meta.value()?.parse::<LitBool>()?.value)
            } else {
                Ok(true)
            }
        };
        if meta.path.is_ident("name") {
            self.name = Some(meta.value()?.parse()?);
        } else if meta.path.is_ident("title") {
            self.title = Some(meta.value()?.parse()?);
        } else if meta.path.is_ident("read_only") {
            self.read_only = Some(flag(&meta)?);
        } else if meta.path.is_ident("destructive") {
            self.destructive = Some(flag(&meta)?);
        } else if meta.path.is_ident("idempotent") {
            self.idempotent = Some(flag(&meta)?);
        } else if meta.path.is_ident("open_world") {
            self.open_world = Some(flag(&meta)?);
        } else if meta.path.is_ident("blocking") {
            self.blocking = flag(&meta)?;
        } else if meta.path.is_ident("specta") {
            self.specta = flag(&meta)?;
        } else if meta.path.is_ident("rename_all") {
            self.rename_all = Some(ArgCase::parse(&meta.value()?.parse()?)?);
        } else {
            return Err(meta.error(
                "unknown `mcp_tool` argument; expected one of `name`, `title`, `read_only`, \
                 `destructive`, `idempotent`, `open_world`, `blocking`, `rename_all`, `specta`",
            ));
        }
        Ok(())
    }
}

/// What `#[tauri::command(...)]` says, when `#[mcp_tool]` sits above it.
#[derive(Default, Debug)]
pub struct CommandAttrs {
    pub present: bool,
    pub rename_all: Option<ArgCase>,
    pub rename: Option<LitStr>,
    pub run_async: bool,
}

#[allow(clippy::large_enum_variant)] // parse-time only
enum CommandItem {
    Meta(Meta),
    Async,
}

impl Parse for CommandItem {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        if input.peek(Token![async]) {
            input.parse::<Token![async]>()?;
            Ok(Self::Async)
        } else {
            input.parse().map(Self::Meta)
        }
    }
}

fn is_tauri_command(attr: &Attribute) -> bool {
    let segs: Vec<_> = attr.path().segments.iter().map(|s| s.ident.to_string()).collect();
    matches!(segs.as_slice(), [c] if c == "command") || matches!(segs.as_slice(), [t, c] if t == "tauri" && c == "command")
}

pub fn is_specta(attr: &Attribute) -> bool {
    let segs: Vec<_> = attr.path().segments.iter().map(|s| s.ident.to_string()).collect();
    matches!(segs.as_slice(), [s] if s == "specta") || matches!(segs.as_slice(), [a, b] if a == "specta" && b == "specta")
}

pub fn parse_command_attrs(attrs: &[Attribute]) -> syn::Result<CommandAttrs> {
    let mut out = CommandAttrs::default();
    for attr in attrs.iter().filter(|a| is_tauri_command(a)) {
        out.present = true;
        let Meta::List(list) = &attr.meta else { continue };
        let items = list.parse_args_with(Punctuated::<CommandItem, Token![,]>::parse_terminated)?;
        for item in items {
            match item {
                CommandItem::Async => out.run_async = true,
                CommandItem::Meta(Meta::NameValue(nv)) => {
                    let Expr::Lit(ExprLit { lit: Lit::Str(s), .. }) = &nv.value else { continue };
                    if nv.path.is_ident("rename_all") {
                        out.rename_all = Some(ArgCase::parse(s)?);
                    } else if nv.path.is_ident("rename") {
                        out.rename = Some(s.clone());
                    }
                }
                // Anything else is Tauri's to validate.
                CommandItem::Meta(_) => {}
            }
        }
    }
    Ok(out)
}

/// Joins `///` doc comments into one description.
pub fn doc_string(attrs: &[Attribute]) -> Option<String> {
    let lines: Vec<String> = attrs
        .iter()
        .filter(|a| a.path().is_ident("doc"))
        .filter_map(|a| match &a.meta {
            Meta::NameValue(nv) => match &nv.value {
                Expr::Lit(ExprLit { lit: Lit::Str(s), .. }) => Some(s.value()),
                _ => None,
            },
            _ => None,
        })
        .map(|l| l.strip_prefix(' ').map(str::to_owned).unwrap_or(l))
        .collect();
    let doc = lines.join("\n").trim().to_owned();
    (!doc.is_empty()).then_some(doc)
}

/// Same rules rmcp applies: 1..=128 chars of `[A-Za-z0-9_.-]`.
pub fn validate_tool_name(name: &str, span: &impl Spanned) -> syn::Result<()> {
    let ok = !name.is_empty()
        && name.len() <= 128
        && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'));
    if ok {
        Ok(())
    } else {
        Err(syn::Error::new(
            span.span(),
            format!("invalid MCP tool name `{name}`: use 1-128 characters from [A-Za-z0-9_.-]"),
        ))
    }
}
