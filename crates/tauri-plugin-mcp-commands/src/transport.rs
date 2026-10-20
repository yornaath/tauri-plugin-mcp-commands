//! Streamable HTTP transport, bound to loopback only.

use std::{
    io,
    net::{Ipv4Addr, SocketAddr, SocketAddrV4},
    sync::Arc,
    time::Duration,
};

use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
};
use tauri::Runtime;
use tokio::{net::TcpListener, task::JoinHandle};
use tokio_util::sync::CancellationToken;

use crate::{
    auth::{self, Guard},
    server::McpHandler,
};

/// Request bodies above this are rejected with 413.
pub(crate) const MAX_BODY_BYTES: usize = 2 * 1024 * 1024;
/// Ports tried after the preferred one before falling back to any free port.
const PORT_FALLBACKS: u16 = 10;

pub(crate) struct Running {
    pub(crate) addr: SocketAddr,
    cancel: CancellationToken,
    task: JoinHandle<()>,
}

impl Running {
    /// Stops accepting connections and waits for the listener to close.
    pub(crate) async fn stop(self) {
        self.cancel.cancel();
        // In-flight requests get a moment to finish; then the task is dropped.
        if tokio::time::timeout(Duration::from_secs(5), self.task).await.is_err() {
            log::warn!("MCP server did not shut down within 5s");
        }
    }
}

/// `127.0.0.1` only. The address type admits nothing else, and there is no
/// option to widen it.
async fn bind(preferred: Option<u16>) -> io::Result<TcpListener> {
    let at = |port| TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, port));
    if let Some(p) = preferred.filter(|p| *p != 0) {
        for port in (p..=p.saturating_add(PORT_FALLBACKS)).filter(|q| *q != 0) {
            match at(port).await {
                Ok(l) => return Ok(l),
                Err(e) if e.kind() == io::ErrorKind::AddrInUse || e.kind() == io::ErrorKind::PermissionDenied => {
                    continue;
                }
                Err(e) => return Err(e),
            }
        }
        log::warn!("ports {p}-{} are taken; using a random port", p.saturating_add(PORT_FALLBACKS));
    }
    at(0).await
}

pub(crate) async fn serve<R: Runtime>(
    handler: McpHandler<R>,
    guard: Arc<Guard>,
    preferred_port: Option<u16>,
) -> io::Result<Running> {
    let listener = bind(preferred_port).await?;
    let addr = listener.local_addr()?;
    let cancel = CancellationToken::new();

    let config = StreamableHttpServerConfig::default()
        // Stateless for every protocol version; 2026-07-28 is always stateless.
        .with_legacy_session_mode(false)
        .with_json_response(true)
        .with_sse_keep_alive(None)
        // Belt and braces: the guard already rejects requests with an Origin.
        .enforce_origin_validation()
        .with_max_request_body_bytes(MAX_BODY_BYTES)
        .with_cancellation_token(cancel.child_token());
    let service: StreamableHttpService<McpHandler<R>, LocalSessionManager> =
        StreamableHttpService::new(move || Ok(handler.clone()), Default::default(), config);
    let router = axum::Router::new()
        .nest_service("/mcp", service)
        .layer(axum::middleware::from_fn_with_state(guard, auth::guard));

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
