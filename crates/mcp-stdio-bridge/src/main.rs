//! stdio ↔ streamable-HTTP bridge for Tauri apps that use
//! `tauri-plugin-mcp-commands`, for MCP clients that only speak stdio (Claude
//! Desktop's local config, `.mcpb` bundles).
//!
//! ```text
//! mcp-stdio-bridge --app-id com.example.notes
//! mcp-stdio-bridge --discovery /path/to/mcp-server/mcp.json
//! mcp-stdio-bridge --url http://127.0.0.1:43110/mcp --token-env NOTES_MCP_TOKEN
//! ```
//!
//! With `--app-id` or `--discovery` the bridge re-reads the app's discovery
//! file on every (re)connect, so it follows the app across restarts and port
//! changes. It can start before the app does: tools appear once the app's
//! server is running, announced with `notifications/tools/list_changed`.
//! Logs go to stderr; stdout carries the protocol.

use std::{path::PathBuf, sync::Arc, time::Duration};

use clap::Parser;
use rmcp::{
    ErrorData, RoleClient, RoleServer, ServerHandler, ServiceExt,
    model::{
        CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, Implementation, ListToolsResult,
        PaginatedRequestParams, ServerCapabilities, ServerConfig,
    },
    service::{Peer, RequestContext, RunningService, ServiceError},
    transport::{StreamableHttpClientTransport, stdio, streamable_http_client::StreamableHttpClientTransportConfig},
};
use serde::Deserialize;
use tokio::sync::Mutex;

/// Folder and file name inside the app's data dir; see the plugin's `discovery` module.
const DISCOVERY_DIR: &str = "mcp-server";
const DISCOVERY_FILE: &str = "mcp.json";
const DISCOVERY_VERSION: u32 = 1;

#[derive(Parser, Debug)]
#[command(version, about = "stdio <-> HTTP bridge for Tauri apps using tauri-plugin-mcp-commands")]
struct Args {
    /// The app's bundle identifier (`identifier` in tauri.conf.json). The
    /// discovery file is looked up in the app's data dir.
    #[arg(long, conflicts_with_all = ["discovery", "url"])]
    app_id: Option<String>,
    /// Path of the app's `mcp.json` discovery file.
    #[arg(long, conflicts_with = "url")]
    discovery: Option<PathBuf>,
    /// Server URL, when not using discovery. Needs `--token-env`.
    #[arg(long, requires = "token_env")]
    url: Option<String>,
    /// Environment variable holding the bearer token (with `--url`).
    #[arg(long)]
    token_env: Option<String>,
    /// How often to check whether the app's server appeared or changed.
    #[arg(long, default_value = "2", value_name = "SECONDS")]
    poll: u64,
}

/// The plugin's `discovery::Discovery`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Discovery {
    version: u32,
    url: String,
    token: String,
    #[serde(default)]
    app_name: Option<String>,
}

enum Source {
    Discovery(PathBuf),
    Direct { url: String, token: String },
}

impl Source {
    fn resolve(&self) -> Result<Discovery, String> {
        match self {
            Source::Direct { url, token } => {
                Ok(Discovery { version: DISCOVERY_VERSION, url: url.clone(), token: token.clone(), app_name: None })
            }
            Source::Discovery(path) => {
                let bytes = std::fs::read(path).map_err(|e| match e.kind() {
                    std::io::ErrorKind::NotFound => {
                        "the app isn't running, or its MCP server is turned off".to_owned()
                    }
                    _ => format!("can't read {}: {e}", path.display()),
                })?;
                let d: Discovery =
                    serde_json::from_slice(&bytes).map_err(|e| format!("bad discovery file {}: {e}", path.display()))?;
                if d.version != DISCOVERY_VERSION {
                    return Err(format!(
                        "discovery file version {} is not supported by this bridge (expects {DISCOVERY_VERSION}); update the bridge",
                        d.version
                    ));
                }
                Ok(d)
            }
        }
    }

    fn describe(&self) -> String {
        match self {
            Source::Discovery(p) => p.display().to_string(),
            Source::Direct { url, .. } => url.clone(),
        }
    }
}

struct Upstream {
    service: RunningService<RoleClient, ()>,
    discovery: Discovery,
}

struct Bridge {
    source: Source,
    upstream: Mutex<Option<Upstream>>,
}

impl Bridge {
    async fn connect(&self) -> Result<Upstream, String> {
        let discovery = self.source.resolve()?;
        let config = StreamableHttpClientTransportConfig::with_uri(discovery.url.clone()).auth_header(discovery.token.clone());
        let transport = StreamableHttpClientTransport::from_config(config);
        let service = ().serve(transport).await.map_err(|e| format!("can't reach the app at {}: {e}", discovery.url))?;
        log(&format!("connected to {}", discovery.url));
        Ok(Upstream { service, discovery })
    }

    /// The upstream peer, connecting if needed.
    async fn peer(&self) -> Result<Peer<RoleClient>, String> {
        let mut up = self.upstream.lock().await;
        if up.is_none() {
            *up = Some(self.connect().await?);
        }
        Ok(up.as_ref().unwrap().service.peer().clone())
    }

    /// Runs `f` upstream; on a transport failure, reconnects (re-reading
    /// discovery) and tries once more.
    async fn forward<T, F, Fut>(&self, f: F) -> Result<T, String>
    where
        F: Fn(Peer<RoleClient>) -> Fut,
        Fut: Future<Output = Result<T, ServiceError>>,
    {
        let mut last = String::new();
        for _ in 0..2 {
            let peer = self.peer().await?;
            match f(peer).await {
                Ok(v) => return Ok(v),
                // The app answered with a protocol error: pass it on as is.
                Err(ServiceError::McpError(e)) => return Err(e.message.to_string()),
                Err(e) => {
                    last = e.to_string();
                    log(&format!("upstream request failed ({last}); reconnecting"));
                    *self.upstream.lock().await = None;
                }
            }
        }
        Err(format!("the app stopped responding: {last}"))
    }

    /// Reconnects when the discovery file appears or changes. Returns true
    /// when the tool list may have changed.
    async fn refresh(&self) -> bool {
        let current = self.source.resolve().ok();
        let mut up = self.upstream.lock().await;
        let connected = up.as_ref().map(|u| u.discovery.clone());
        if current == connected {
            return false;
        }
        *up = None;
        drop(up);
        match current {
            Some(_) => self.peer().await.is_ok(),
            // Server went away: the (now empty) tool list changed too.
            None => connected.is_some(),
        }
    }
}

#[derive(Clone)]
struct Handler(Arc<Bridge>);

impl ServerHandler for Handler {
    fn get_info(&self) -> ServerConfig {
        let name = self.0.source.resolve().ok().and_then(|d| d.app_name).unwrap_or_else(|| "tauri-app".into());
        let caps = ServerCapabilities::builder().enable_tools().enable_tool_list_changed().build();
        ServerConfig::new(caps).with_server_info(Implementation::new(name, env!("CARGO_PKG_VERSION")))
    }

    async fn list_tools(
        &self,
        request: Option<PaginatedRequestParams>,
        _: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        match self.0.forward(|peer| {
            let request = request.clone();
            async move { peer.list_tools(request).await }
        })
        .await
        {
            Ok(list) => Ok(list),
            // Not running yet: no tools for now; we announce them when it starts.
            Err(e) => {
                log(&format!("tools/list: {e}"));
                Ok(ListToolsResult::default())
            }
        }
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let result = self
            .0
            .forward(|peer| {
                let mut request = request.clone();
                // The upstream client adds its own protocol metadata.
                request.meta = None;
                async move { peer.call_tool(request).await }
            })
            .await;
        Ok(match result {
            Ok(r) => r,
            Err(e) => CallToolResult::error(vec![ContentBlock::text(e)]),
        }
        .into())
    }
}

fn log(msg: &str) {
    eprintln!("[mcp-stdio-bridge] {msg}");
}

#[tokio::main]
async fn main() {
    let args = Args::parse();
    let source = match (&args.app_id, &args.discovery, &args.url) {
        (Some(id), _, _) => match dirs::data_dir() {
            Some(dir) => Source::Discovery(dir.join(id).join(DISCOVERY_DIR).join(DISCOVERY_FILE)),
            None => fatal("can't determine the user data directory"),
        },
        (_, Some(path), _) => Source::Discovery(path.clone()),
        (_, _, Some(url)) => {
            let var = args.token_env.as_deref().unwrap();
            match std::env::var(var) {
                Ok(token) => Source::Direct { url: url.clone(), token },
                Err(_) => fatal(&format!("environment variable {var} is not set")),
            }
        }
        _ => fatal("pass --app-id, --discovery or --url (see --help)"),
    };
    log(&format!("forwarding to {}", source.describe()));
    let bridge = Arc::new(Bridge { source, upstream: Mutex::new(None) });

    let server = match Handler(bridge.clone()).serve(stdio()).await {
        Ok(s) => s,
        Err(e) => fatal(&format!("stdio handshake failed: {e}")),
    };

    // Follow the app: reconnect when it (re)starts and tell the client.
    let peer = server.peer().clone();
    let poll = Duration::from_secs(args.poll.max(1));
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(poll).await;
            if bridge.refresh().await {
                let _ = peer.notify_tool_list_changed().await;
            }
        }
    });

    if let Err(e) = server.waiting().await {
        log(&format!("stopped: {e}"));
    }
}

fn fatal(msg: &str) -> ! {
    log(msg);
    std::process::exit(2)
}
