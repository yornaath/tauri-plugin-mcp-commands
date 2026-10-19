//! The Tauri plugin: builder, runtime control (`McpServer`) and lifecycle.

use std::{
    net::SocketAddr,
    path::PathBuf,
    sync::{Arc, Mutex, RwLock},
    time::Duration,
};

use serde::{Serialize, Serializer};
use serde_json::{Value, json};
use tauri::{
    AppHandle, Manager, RunEvent, Runtime,
    plugin::{Builder as PluginBuilder, TauriPlugin},
};

use crate::{
    ToolDef,
    auth::{self, Guard},
    clients::{AccessRequest, Clients, ConnectedClient},
    commands,
    discovery::{self, Discovery},
    secure_fs,
    server::{McpHandler, Registry, ServerInfo},
    transport::{self, Running},
};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Tauri(#[from] tauri::Error),
}

impl Serialize for Error {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

/// Configures the plugin. Nothing is exposed unless passed to [`Builder::tools`]
/// or [`Builder::tool`], and the server only runs once started.
pub struct Builder<R: Runtime> {
    info: Option<ServerInfo>,
    tools: Vec<ToolDef<R>>,
    preferred_port: Option<u16>,
    autostart: bool,
    require_approval: bool,
    approval_timeout: Duration,
    data_dir: Option<PathBuf>,
    bridge_path: Option<PathBuf>,
}

impl<R: Runtime> Default for Builder<R> {
    fn default() -> Self {
        Self::new()
    }
}

impl<R: Runtime> Builder<R> {
    pub fn new() -> Self {
        Self {
            info: None,
            tools: Vec::new(),
            preferred_port: None,
            autostart: false,
            require_approval: false,
            approval_timeout: Duration::from_secs(120),
            data_dir: None,
            bridge_path: None,
        }
    }

    /// Name and version reported to clients. Defaults to the app's.
    pub fn server_info(mut self, name: impl Into<String>, version: impl Into<String>) -> Self {
        self.info = Some(ServerInfo { name: name.into(), version: version.into() });
        self
    }

    /// Adds tools, typically `collect_mcp_tools![…]`.
    pub fn tools(mut self, tools: impl IntoIterator<Item = ToolDef<R>>) -> Self {
        self.tools.extend(tools);
        self
    }

    /// Adds one tool, e.g. a [`ToolDef::manual`] one.
    pub fn tool(mut self, tool: ToolDef<R>) -> Self {
        self.tools.push(tool);
        self
    }

    /// Port to try first. If it and the next 10 are taken, any free port is
    /// used. Without this, the port is random on every start. HTTP clients
    /// are configured with a fixed URL, so set one.
    pub fn preferred_port(mut self, port: u16) -> Self {
        self.preferred_port = Some(port);
        self
    }

    /// Start the server when the app starts. Off by default; the usual
    /// pattern is to start it from a user setting via [`McpServer::start`].
    pub fn autostart(mut self, autostart: bool) -> Self {
        self.autostart = autostart;
        self
    }

    /// Ask the host app before serving a new client: emits
    /// `mcp://access-requested` and waits for [`McpServer::respond_access`].
    pub fn require_approval(mut self, require: bool) -> Self {
        self.require_approval = require;
        self
    }

    /// How long an access request waits before it is denied. Default 120 s.
    pub fn approval_timeout(mut self, timeout: Duration) -> Self {
        self.approval_timeout = timeout;
        self
    }

    /// Where the token and discovery file live. Defaults to
    /// `<app_data_dir>/mcp-server`.
    pub fn data_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.data_dir = Some(dir.into());
        self
    }

    /// Path of the bundled `mcp-stdio-bridge`, used to render the Claude
    /// Desktop config in [`McpServer::connection_configs`].
    pub fn bridge_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.bridge_path = Some(path.into());
        self
    }

    pub fn build(self) -> TauriPlugin<R> {
        let config = Mutex::new(Some(self));
        PluginBuilder::<R>::new("mcp-server")
            .invoke_handler(tauri::generate_handler![
                commands::start,
                commands::stop,
                commands::status,
                commands::connection_configs,
                commands::respond_access,
                commands::rotate_token
            ])
            .setup(move |app, _api| {
                let b = config.lock().unwrap().take().expect("setup runs once");
                let autostart = b.autostart;
                let server = McpServer::new(app, b)?;
                app.manage(server.clone());
                if autostart {
                    tauri::async_runtime::spawn(async move {
                        if let Err(e) = server.start().await {
                            log::error!("failed to start the MCP server: {e}");
                        }
                    });
                }
                Ok(())
            })
            .on_event(|app, event| {
                if let RunEvent::Exit = event
                    && let Some(server) = app.try_state::<McpServer<R>>()
                {
                    tauri::async_runtime::block_on(server.stop());
                }
            })
            .build()
    }
}

/// Server state, as returned by [`McpServer::status`].
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub running: bool,
    pub url: Option<String>,
    pub port: Option<u16>,
    pub tools: Vec<String>,
    pub clients: Vec<ConnectedClient>,
    pub pending_requests: Vec<AccessRequest>,
}

/// Ready-to-paste client configurations, for the host app's settings UI.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionConfigs {
    /// Server name used in the snippets.
    pub name: String,
    pub url: String,
    pub token: String,
    /// `claude mcp add …` command line.
    pub claude_code: String,
    /// Entry for Cursor's `mcp.json`.
    pub cursor: Value,
    /// Entry for VS Code's `.vscode/mcp.json`.
    pub vscode: Value,
    /// Table for Codex's `~/.codex/config.toml`.
    pub codex: String,
    /// Entry for `claude_desktop_config.json`, via the stdio bridge. `None`
    /// unless [`Builder::bridge_path`] is set.
    pub claude_desktop: Option<Value>,
}

/// Runtime control of the MCP server: `app.state::<McpServer<R>>()`.
pub struct McpServer<R: Runtime>(Arc<Inner<R>>);

impl<R: Runtime> Clone for McpServer<R> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

struct Inner<R: Runtime> {
    app: AppHandle<R>,
    registry: Arc<Registry<R>>,
    info: Arc<ServerInfo>,
    clients: Arc<Clients>,
    guard: Arc<Guard>,
    dir: PathBuf,
    preferred_port: Option<u16>,
    bridge_path: Option<PathBuf>,
    /// Serializes start/stop.
    lifecycle: tokio::sync::Mutex<Option<Running>>,
    addr: RwLock<Option<SocketAddr>>,
}

impl<R: Runtime> McpServer<R> {
    fn new(app: &AppHandle<R>, b: Builder<R>) -> Result<Self, Box<dyn std::error::Error>> {
        let registry = Registry::new(b.tools)?;
        let info = b.info.unwrap_or_else(|| {
            let p = app.package_info();
            ServerInfo { name: p.name.clone(), version: p.version.to_string() }
        });
        let dir = match b.data_dir {
            Some(d) => d,
            None => app.path().app_data_dir()?.join(discovery::DIR_NAME),
        };
        Ok(Self(Arc::new(Inner {
            app: app.clone(),
            registry: Arc::new(registry),
            info: Arc::new(info),
            clients: Arc::new(Clients::new(b.require_approval, b.approval_timeout)),
            guard: Arc::new(Guard { token: RwLock::new(String::new()) }),
            dir,
            preferred_port: b.preferred_port,
            bridge_path: b.bridge_path,
            lifecycle: tokio::sync::Mutex::new(None),
            addr: RwLock::new(None),
        })))
    }

    /// Starts the server, or does nothing if it is running.
    pub async fn start(&self) -> Result<Status, Error> {
        let inner = &self.0;
        let mut running = inner.lifecycle.lock().await;
        if running.is_none() {
            secure_fs::create_private_dir(&inner.dir)?;
            *inner.guard.token.write().unwrap() = auth::load_or_create(&inner.dir)?;
            let handler = McpHandler {
                app: inner.app.clone(),
                registry: inner.registry.clone(),
                info: inner.info.clone(),
                clients: inner.clients.clone(),
            };
            let r = transport::serve(handler, inner.guard.clone(), inner.preferred_port).await?;
            *inner.addr.write().unwrap() = Some(r.addr);
            if let Err(e) = self.write_discovery() {
                *inner.addr.write().unwrap() = None;
                r.stop().await;
                return Err(e.into());
            }
            log::info!("MCP server listening on http://{}/mcp", r.addr);
            *running = Some(r);
        }
        drop(running);
        Ok(self.status())
    }

    /// Stops the server and closes its port. Does nothing if it is stopped.
    pub async fn stop(&self) {
        let inner = &self.0;
        let mut running = inner.lifecycle.lock().await;
        if let Some(r) = running.take() {
            *inner.addr.write().unwrap() = None;
            if let Err(e) = discovery::remove(&inner.dir) {
                log::warn!("failed to remove the MCP discovery file: {e}");
            }
            r.stop().await;
            inner.clients.reset();
            log::info!("MCP server stopped");
        }
    }

    pub fn status(&self) -> Status {
        let addr = *self.0.addr.read().unwrap();
        Status {
            running: addr.is_some(),
            url: addr.map(|a| format!("http://{a}/mcp")),
            port: addr.map(|a| a.port()),
            tools: self.0.registry.names().map(str::to_owned).collect(),
            clients: self.0.clients.connected(),
            pending_requests: self.0.clients.pending(),
        }
    }

    /// Replaces the bearer token. Every client must be reconfigured, and
    /// approvals are forgotten.
    pub fn rotate_token(&self) -> Result<(), Error> {
        let inner = &self.0;
        secure_fs::create_private_dir(&inner.dir)?;
        *inner.guard.token.write().unwrap() = auth::rotate(&inner.dir)?;
        inner.clients.reset();
        if inner.addr.read().unwrap().is_some() {
            self.write_discovery()?;
        }
        Ok(())
    }

    /// Answers an `mcp://access-requested` event. Returns false if `id` is
    /// not pending (already answered, timed out or cancelled).
    pub fn respond_access(&self, id: u64, allow: bool) -> bool {
        self.0.clients.respond(&self.0.app, id, allow)
    }

    /// Client configurations for the running server. `None` while stopped,
    /// since the URL depends on the bound port.
    pub fn connection_configs(&self) -> Option<ConnectionConfigs> {
        let addr = (*self.0.addr.read().unwrap())?;
        let token = self.0.guard.token.read().unwrap().clone();
        Some(render_configs(&self.0.info.name, &format!("http://{addr}/mcp"), &token, self.bridge_args()))
    }

    fn bridge_args(&self) -> Option<(String, Vec<String>)> {
        let path = self.0.bridge_path.as_ref()?;
        let identifier = self.0.app.config().identifier.clone();
        Some((path.display().to_string(), vec!["--app-id".into(), identifier]))
    }

    fn write_discovery(&self) -> std::io::Result<()> {
        let inner = &self.0;
        let Some(addr) = *inner.addr.read().unwrap() else { return Ok(()) };
        discovery::write(
            &inner.dir,
            &Discovery {
                version: discovery::FORMAT_VERSION,
                url: format!("http://{addr}/mcp"),
                port: addr.port(),
                token: inner.guard.token.read().unwrap().clone(),
                pid: std::process::id(),
                app_name: inner.info.name.clone(),
                identifier: inner.app.config().identifier.clone(),
            },
        )
    }
}

/// `"My App"` → `"my-app"`.
fn slug(name: &str) -> String {
    let s: String = name.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' }).collect();
    let s = s.split('-').filter(|p| !p.is_empty()).collect::<Vec<_>>().join("-");
    if s.is_empty() { "tauri-app".into() } else { s }
}

fn render_configs(app_name: &str, url: &str, token: &str, bridge: Option<(String, Vec<String>)>) -> ConnectionConfigs {
    let name = slug(app_name);
    let auth = format!("Bearer {token}");
    ConnectionConfigs {
        claude_code: format!("claude mcp add --transport http {name} {url} --header \"Authorization: {auth}\""),
        cursor: json!({ "mcpServers": { &name: { "url": url, "headers": { "Authorization": &auth } } } }),
        vscode: json!({ "servers": { &name: { "type": "http", "url": url, "headers": { "Authorization": &auth } } } }),
        codex: format!("[mcp_servers.{name}]\nurl = \"{url}\"\nhttp_headers = {{ \"Authorization\" = \"{auth}\" }}\n"),
        claude_desktop: bridge
            .map(|(command, args)| json!({ "mcpServers": { &name: { "command": command, "args": args } } })),
        name,
        url: url.to_owned(),
        token: token.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configs() {
        assert_eq!(slug("My App 2!"), "my-app-2");
        assert_eq!(slug("!!"), "tauri-app");
        let c = render_configs("My App", "http://127.0.0.1:43110/mcp", "tok", Some(("/b".into(), vec!["--app-id".into(), "x".into()])));
        assert_eq!(c.claude_code, "claude mcp add --transport http my-app http://127.0.0.1:43110/mcp --header \"Authorization: Bearer tok\"");
        assert_eq!(c.cursor["mcpServers"]["my-app"]["headers"]["Authorization"], "Bearer tok");
        assert_eq!(c.codex, "[mcp_servers.my-app]\nurl = \"http://127.0.0.1:43110/mcp\"\nhttp_headers = { \"Authorization\" = \"Bearer tok\" }\n");
        assert_eq!(c.claude_desktop.unwrap()["mcpServers"]["my-app"]["args"], json!(["--app-id", "x"]));
    }
}
