use std::{
    borrow::Cow,
    future::Future,
    pin::Pin,
    sync::{Arc, OnceLock},
    time::Duration,
};

use serde_json::{Map, Value, json};
use tauri::{AppHandle, Runtime};

/// Arguments of one tool call: the MCP `arguments` object.
pub type ToolArgs = Map<String, Value>;

pub type ToolFuture = Pin<Box<dyn Future<Output = Result<Value, ToolError>> + Send + 'static>>;

pub type ToolHandler<R> = Arc<dyn Fn(AppHandle<R>, ToolArgs) -> ToolFuture + Send + Sync + 'static>;

/// Why a tool call failed. Every variant is reported to the client as a tool
/// result with `isError: true`, so the agent sees the message and can retry.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ToolError {
    /// The command returned `Err(e)`. Holds `e` serialized the way Tauri
    /// would send it to the webview.
    #[error("{}", display_command_error(.0))]
    Command(Value),
    /// The arguments did not match the tool's parameters.
    #[error("{0}")]
    InvalidArgs(String),
    /// The plugin could not run the command.
    #[error("{0}")]
    Internal(String),
}

fn display_command_error(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// Behavior hints for clients, mapped to MCP `ToolAnnotations`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ToolAnnotations {
    pub title: Option<String>,
    pub read_only: Option<bool>,
    pub destructive: Option<bool>,
    pub idempotent: Option<bool>,
    pub open_world: Option<bool>,
}

pub type JsonObject = Map<String, Value>;

/// JSON Schemas for one tool.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolSchemas {
    pub input: JsonObject,
    /// `None` when the command returns `()`.
    pub output: Option<JsonObject>,
    /// MCP requires an object output schema. Non-object results are wrapped as
    /// `{ "result": … }`, and so is the structured content.
    pub wrap_output: bool,
}

/// Where a tool's schemas come from.
#[doc(hidden)]
pub enum SchemaSource {
    /// No type information: every input is accepted as any JSON value.
    Untyped,
    #[cfg(feature = "specta")]
    Specta(fn(&mut specta::Types) -> specta::datatype::Function),
}

/// One tool the MCP server exposes.
pub struct ToolDef<R: Runtime> {
    pub(crate) name: Cow<'static, str>,
    pub(crate) description: Option<Cow<'static, str>>,
    pub(crate) annotations: ToolAnnotations,
    /// IPC names of the parameters that are MCP inputs, in declaration order.
    pub(crate) inputs: Vec<&'static str>,
    pub(crate) schema_source: SchemaSource,
    pub(crate) schemas: OnceLock<Result<ToolSchemas, String>>,
    pub(crate) handler: ToolHandler<R>,
}

impl<R: Runtime> ToolDef<R> {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn description(&self) -> Option<&str> {
        self.description.as_deref()
    }

    pub fn annotations(&self) -> &ToolAnnotations {
        &self.annotations
    }

    /// IPC names of the parameters that are MCP inputs, in declaration order.
    pub fn inputs(&self) -> &[&'static str] {
        &self.inputs
    }

    /// The tool's schemas, built on first use.
    pub fn schemas(&self) -> Result<&ToolSchemas, &str> {
        self.schemas
            .get_or_init(|| match &self.schema_source {
                SchemaSource::Untyped => Ok(untyped_schemas(&self.inputs)),
                #[cfg(feature = "specta")]
                SchemaSource::Specta(f) => {
                    let mut types = specta::Types::default();
                    let func = f(&mut types);
                    crate::schema::from_function(&func, &types, &self.inputs)
                }
            })
            .as_ref()
            .map_err(String::as_str)
    }

    /// Runs the tool.
    pub fn call(&self, app: AppHandle<R>, args: ToolArgs) -> ToolFuture {
        (self.handler)(app, args)
    }
}

/// How long a sync command may wait for the main thread before the call fails.
pub(crate) const MAIN_THREAD_TIMEOUT: Duration = Duration::from_secs(30);

/// Metadata the macro passes alongside the handler.
#[doc(hidden)]
pub struct MacroMeta {
    pub name: &'static str,
    pub description: Option<&'static str>,
    pub annotations: ToolAnnotations,
    pub inputs: Vec<&'static str>,
    pub schema: SchemaSource,
}

#[doc(hidden)]
impl<R: Runtime> ToolDef<R> {
    pub fn __from_macro(
        meta: MacroMeta,
        handler: impl Fn(AppHandle<R>, ToolArgs) -> ToolFuture + Send + Sync + 'static,
    ) -> Self {
        Self {
            name: Cow::Borrowed(meta.name),
            description: meta.description.map(Cow::Borrowed),
            annotations: meta.annotations,
            inputs: meta.inputs,
            schema_source: meta.schema,
            schemas: OnceLock::new(),
            handler: Arc::new(handler),
        }
    }
}

fn untyped_schemas(inputs: &[&str]) -> ToolSchemas {
    let properties: JsonObject = inputs.iter().map(|n| ((*n).to_owned(), json!({}))).collect();
    let Value::Object(input) = json!({ "type": "object", "properties": properties }) else { unreachable!() };
    ToolSchemas { input, output: None, wrap_output: false }
}

/// Runs a sync command on the main thread, like Tauri does for IPC calls.
#[doc(hidden)]
pub fn __run_on_main_thread<R, F>(app: AppHandle<R>, f: F) -> ToolFuture
where
    R: Runtime,
    F: FnOnce(&AppHandle<R>) -> Result<Value, ToolError> + Send + 'static,
{
    Box::pin(async move {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let handle = app.clone();
        app.run_on_main_thread(move || {
            let _ = tx.send(f(&handle));
        })
        .map_err(|e| ToolError::Internal(format!("failed to schedule on the main thread: {e}")))?;
        match tokio::time::timeout(MAIN_THREAD_TIMEOUT, rx).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(ToolError::Internal("the main thread dropped the call".into())),
            Err(_) => Err(ToolError::Internal(format!(
                "timed out after {}s waiting for the main thread",
                MAIN_THREAD_TIMEOUT.as_secs()
            ))),
        }
    })
}

/// Runs a sync command on the blocking thread pool, like Tauri does for
/// `#[tauri::command(async)]` on a sync fn.
#[doc(hidden)]
pub fn __run_blocking<R, F>(app: AppHandle<R>, f: F) -> ToolFuture
where
    R: Runtime,
    F: FnOnce(&AppHandle<R>) -> Result<Value, ToolError> + Send + 'static,
{
    Box::pin(async move {
        tauri::async_runtime::spawn_blocking(move || f(&app))
            .await
            .map_err(|e| ToolError::Internal(format!("command panicked: {e}")))?
    })
}
