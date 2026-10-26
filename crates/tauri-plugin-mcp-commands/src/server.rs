//! The MCP `ServerHandler`, serving `tools/list` and `tools/call` from the
//! tool registry.

use std::{collections::HashMap, sync::Arc};

use rmcp::{
    ErrorData, RoleServer, ServerHandler,
    model::{
        CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, ErrorCode, Extensions, Implementation,
        InitializeRequestParams, InitializeResult, ListToolsResult, PaginatedRequestParams, RequestMetaObject,
        ServerCapabilities, ServerConfig, Tool, ToolAnnotations as McpAnnotations,
    },
    service::RequestContext,
};
use serde_json::{Value, json};
use tauri::{AppHandle, Runtime};

use crate::{
    ToolArgs, ToolDef, ToolError,
    clients::{ClientIdent, Clients},
};

/// Server name and version reported to clients.
#[derive(Debug, Clone)]
pub struct ServerInfo {
    pub name: String,
    pub version: String,
}

/// The exposed tools, by name.
pub(crate) struct Registry<R: Runtime> {
    tools: Vec<ToolDef<R>>,
    by_name: HashMap<String, usize>,
}

impl<R: Runtime> Registry<R> {
    /// Fails on duplicate tool names.
    pub(crate) fn new(tools: Vec<ToolDef<R>>) -> Result<Self, String> {
        let mut by_name = HashMap::new();
        for (i, tool) in tools.iter().enumerate() {
            let n = tool.name();
            if n.is_empty() || n.len() > 128 || !n.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.')) {
                return Err(format!("invalid MCP tool name `{n}`: use 1-128 characters from [A-Za-z0-9_.-]"));
            }
            if by_name.insert(tool.name().to_owned(), i).is_some() {
                return Err(format!("duplicate MCP tool name `{}`", tool.name()));
            }
        }
        Ok(Self { tools, by_name })
    }

    pub(crate) fn names(&self) -> impl Iterator<Item = &str> {
        self.tools.iter().map(ToolDef::name)
    }

    fn get(&self, name: &str) -> Option<&ToolDef<R>> {
        self.by_name.get(name).map(|&i| &self.tools[i])
    }
}

pub(crate) struct McpHandler<R: Runtime> {
    pub(crate) app: AppHandle<R>,
    pub(crate) registry: Arc<Registry<R>>,
    pub(crate) info: Arc<ServerInfo>,
    pub(crate) clients: Arc<Clients>,
}

impl<R: Runtime> Clone for McpHandler<R> {
    fn clone(&self) -> Self {
        Self {
            app: self.app.clone(),
            registry: self.registry.clone(),
            info: self.info.clone(),
            clients: self.clients.clone(),
        }
    }
}

impl<R: Runtime> McpHandler<R> {
    /// Who sent this request: `clientInfo` from the 2026-07-28 `_meta` or
    /// the `initialize` params, plus the HTTP User-Agent.
    async fn admit(
        &self,
        meta: &RequestMetaObject,
        extensions: &Extensions,
        init: Option<&Implementation>,
    ) -> Result<(), ErrorData> {
        let info = init.cloned().or_else(|| meta.client_info());
        let user_agent = extensions
            .get::<http::request::Parts>()
            .and_then(|p| p.headers.get(http::header::USER_AGENT))
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);
        let ident = ClientIdent {
            name: info.as_ref().map(|i| i.name.clone()),
            version: info.as_ref().map(|i| i.version.clone()),
            user_agent,
        };
        // -32001: a server-defined error, so clients show the message.
        self.clients.admit(&self.app, ident).await.map_err(|m| ErrorData::new(ErrorCode(-32001), m, None))
    }
}

fn to_mcp_tool<R: Runtime>(tool: &ToolDef<R>) -> Result<Tool, String> {
    let schemas = tool.schemas()?;
    let mut out = Tool::new_with_raw(
        tool.name().to_owned(),
        tool.description().map(|d| d.to_owned().into()),
        Arc::new(schemas.input.clone()),
    );
    if let Some(output) = &schemas.output {
        out = out.with_raw_output_schema(Arc::new(output.clone()));
    }
    let a = tool.annotations();
    if a != &Default::default() {
        let mut ann = McpAnnotations::new();
        ann.title = a.title.clone();
        ann.read_only_hint = a.read_only;
        ann.destructive_hint = a.destructive;
        ann.idempotent_hint = a.idempotent;
        ann.open_world_hint = a.open_world;
        out = out.with_annotations(ann);
    }
    if let Some(title) = &a.title {
        out = out.with_title(title.clone());
    }
    Ok(out)
}

fn to_call_result(value: Result<Value, ToolError>, wrap: Option<bool>) -> CallToolResult {
    match value {
        Ok(value) => match (wrap, crate::content::blocks_of(&value)) {
            // A `ToolContent` result: its blocks, as they are.
            (None, Some(blocks)) => CallToolResult::success(content_blocks(blocks)),
            // `()` return: no structured content.
            (None, None) => CallToolResult::success(vec![ContentBlock::text(value.to_string())]),
            (Some(true), _) => CallToolResult::structured(json!({ "result": value })),
            (Some(false), _) => CallToolResult::structured(value),
        },
        Err(err) => CallToolResult::error(vec![ContentBlock::text(err.to_string())]),
    }
}

/// Content blocks from a serialized `ToolContent`. Anything unexpected is
/// passed on as text rather than dropped.
fn content_blocks(blocks: &[Value]) -> Vec<ContentBlock> {
    blocks
        .iter()
        .map(|block| {
            let field = |name: &str| block.get(name).and_then(Value::as_str);
            match field("type") {
                Some("text") => ContentBlock::text(field("text").unwrap_or_default()),
                Some("image") => match (field("data"), field("mimeType")) {
                    (Some(data), Some(mime_type)) => ContentBlock::image(data, mime_type),
                    _ => ContentBlock::text(block.to_string()),
                },
                _ => ContentBlock::text(block.to_string()),
            }
        })
        .collect()
}

impl<R: Runtime> ServerHandler for McpHandler<R> {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new(self.info.name.clone(), self.info.version.clone()))
    }

    async fn initialize(
        &self,
        request: InitializeRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<InitializeResult, ErrorData> {
        self.admit(&context.meta, &context.extensions, Some(&request.client_info)).await?;
        context.peer.set_peer_info(request.clone());
        self.negotiate_initialize(&request)
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        self.admit(&context.meta, &context.extensions, None).await?;
        let tools = self
            .registry
            .tools
            .iter()
            .filter_map(|t| {
                to_mcp_tool(t)
                    .inspect_err(|e| log::error!("not listing MCP tool `{}`: {e}", t.name()))
                    .ok()
            })
            .collect();
        Ok(ListToolsResult::with_all_items(tools))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        self.admit(&context.meta, &context.extensions, None).await?;
        let Some(tool) = self.registry.get(&request.name) else {
            return Err(ErrorData::invalid_params(format!("unknown tool `{}`", request.name), None));
        };
        let wrap = match tool.schemas() {
            Ok(s) => s.output.as_ref().map(|_| s.wrap_output),
            Err(e) => return Err(ErrorData::internal_error(format!("tool `{}` is unavailable: {e}", tool.name()), None)),
        };
        let args: ToolArgs = request.arguments.unwrap_or_default();
        let result = tool.call(self.app.clone(), args).await;
        if let Err(e) = &result {
            log::debug!("MCP tool `{}` failed: {e}", tool.name());
        }
        Ok(to_call_result(result, wrap).into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ToolContent;

    #[test]
    fn tool_content_becomes_content_blocks() {
        let value = serde_json::to_value(ToolContent::new().text("two frames").image_png(&[137, 80])).unwrap();
        let result = to_call_result(Ok(value), None);

        assert_eq!(result.structured_content, None);
        assert_eq!(result.is_error, Some(false));
        let blocks = serde_json::to_value(&result.content).unwrap();
        assert_eq!(
            blocks,
            json!([
                { "type": "text", "text": "two frames" },
                { "type": "image", "data": "iVA=", "mimeType": "image/png" },
            ])
        );
    }

    #[test]
    fn malformed_blocks_fall_back_to_text() {
        let value = json!({ "__mcp_tool_content": [{ "type": "image" }, { "type": "audio" }] });
        let result = to_call_result(Ok(value), None);
        let blocks = serde_json::to_value(&result.content).unwrap();
        assert_eq!(blocks[0]["type"], "text");
        assert_eq!(blocks[1]["type"], "text");
    }
}
