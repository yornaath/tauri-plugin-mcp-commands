//! The MCP `ServerHandler`, serving `tools/list` and `tools/call` from the
//! tool registry.

use std::{collections::HashMap, sync::Arc};

use rmcp::{
    ErrorData, RoleServer, ServerHandler,
    model::{
        CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, Implementation, ListToolsResult,
        PaginatedRequestParams, ServerCapabilities, ServerConfig, Tool, ToolAnnotations as McpAnnotations,
    },
    service::RequestContext,
};
use serde_json::{Value, json};
use tauri::{AppHandle, Runtime};

use crate::{ToolArgs, ToolDef, ToolError};

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
            if by_name.insert(tool.name().to_owned(), i).is_some() {
                return Err(format!("duplicate MCP tool name `{}`", tool.name()));
            }
        }
        Ok(Self { tools, by_name })
    }

    fn get(&self, name: &str) -> Option<&ToolDef<R>> {
        self.by_name.get(name).map(|&i| &self.tools[i])
    }
}

pub(crate) struct McpHandler<R: Runtime> {
    pub(crate) app: AppHandle<R>,
    pub(crate) registry: Arc<Registry<R>>,
    pub(crate) info: Arc<ServerInfo>,
}

impl<R: Runtime> Clone for McpHandler<R> {
    fn clone(&self) -> Self {
        Self { app: self.app.clone(), registry: self.registry.clone(), info: self.info.clone() }
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
        Ok(value) => match wrap {
            // `()` return: no structured content.
            None => CallToolResult::success(vec![ContentBlock::text(value.to_string())]),
            Some(true) => CallToolResult::structured(json!({ "result": value })),
            Some(false) => CallToolResult::structured(value),
        },
        Err(err) => CallToolResult::error(vec![ContentBlock::text(err.to_string())]),
    }
}

impl<R: Runtime> ServerHandler for McpHandler<R> {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new(self.info.name.clone(), self.info.version.clone()))
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
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
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
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
