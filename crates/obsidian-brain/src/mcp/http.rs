//! Streamable HTTP at `/mcp`, stateless: no tool sends a message the
//! client did not ask for.

use std::net::SocketAddr;
use std::sync::Arc;

use rmcp::transport::streamable_http_server::session::never::NeverSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use tokio_util::sync::CancellationToken;

use super::Server;

pub struct HttpOptions {
    pub listen: SocketAddr,
    /// `Host` values to accept besides loopback, for a server behind a proxy.
    pub allowed_hosts: Vec<String>,
}

pub async fn serve(
    server: Server,
    opts: HttpOptions,
    cancel: CancellationToken,
) -> anyhow::Result<()> {
    let mut hosts: Vec<String> = vec!["localhost".into(), "127.0.0.1".into(), "::1".into()];
    hosts.extend(opts.allowed_hosts);
    let mut config = StreamableHttpServerConfig::default().with_allowed_hosts(hosts);
    config.legacy_session_mode = false;
    config.json_response = true;
    config.cancellation_token = cancel.child_token();

    let service = StreamableHttpService::new(
        move || Ok(server.clone()),
        Arc::new(NeverSessionManager::default()),
        config,
    );
    let router = axum::Router::new()
        .nest_service("/mcp", service.clone())
        .route_service("/mcp/", service);
    let listener = tokio::net::TcpListener::bind(opts.listen).await?;
    tracing::info!(addr = %listener.local_addr()?, "serving MCP over HTTP at /mcp");
    axum::serve(listener, router)
        .with_graceful_shutdown(async move { cancel.cancelled().await })
        .await?;
    Ok(())
}
