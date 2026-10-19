//! Streamable HTTP transport, bound to loopback only.

use std::{
    net::{Ipv4Addr, SocketAddr, SocketAddrV4},
    sync::Arc,
};

use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
};
use tauri::{AppHandle, Runtime};
use tokio::{net::TcpListener, task::JoinHandle};
use tokio_util::sync::CancellationToken;

use crate::server::{McpHandler, Registry, ServerInfo};

/// Request bodies above this are rejected with 413.
pub(crate) const MAX_BODY_BYTES: usize = 2 * 1024 * 1024;

pub(crate) struct Running {
    pub(crate) addr: SocketAddr,
    pub(crate) cancel: CancellationToken,
    pub(crate) task: JoinHandle<()>,
}

/// Binds `127.0.0.1:port` (0 for any free port) and serves `/mcp`.
pub(crate) async fn serve<R: Runtime>(
    app: AppHandle<R>,
    registry: Arc<Registry<R>>,
    info: Arc<ServerInfo>,
    port: u16,
) -> std::io::Result<Running> {
    // The address type only admits IPv4 loopback; there is no knob to widen it.
    let listener = TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, port)).await?;
    let addr = listener.local_addr()?;
    let cancel = CancellationToken::new();

    let config = StreamableHttpServerConfig::default()
        // Stateless for every protocol version; 2026-07-28 is always stateless.
        .with_legacy_session_mode(false)
        .with_json_response(true)
        .with_sse_keep_alive(None)
        // Reject any request carrying an `Origin` header: browsers (including
        // the app's own webview) must not reach the server.
        .enforce_origin_validation()
        .with_max_request_body_bytes(MAX_BODY_BYTES)
        .with_cancellation_token(cancel.child_token());
    let handler = McpHandler { app, registry, info };
    let service: StreamableHttpService<McpHandler<R>, LocalSessionManager> =
        StreamableHttpService::new(move || Ok(handler.clone()), Default::default(), config);
    let router = axum::Router::new().nest_service("/mcp", service);

    let shutdown = cancel.clone();
    let task = tokio::spawn(async move {
        let result = axum::serve(listener, router)
            .with_graceful_shutdown(async move { shutdown.cancelled_owned().await })
            .await;
        if let Err(e) = result {
            log::error!("MCP server stopped: {e}");
        }
    });
    Ok(Running { addr, cancel, task })
}
