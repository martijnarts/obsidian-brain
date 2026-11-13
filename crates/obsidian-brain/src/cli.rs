//! The `obsidian-brain` command line.

use std::net::SocketAddr;
use std::process::ExitCode;
use std::sync::Arc;

use clap::{Parser, Subcommand, ValueEnum};
use rmcp::ServiceExt;
use tokio_util::sync::CancellationToken;

use crate::config::resolve_data_dir;
use crate::mcp::{Server, http};
use crate::vaults::{VaultSpec, Vaults};

#[derive(Parser)]
#[command(
    name = "obsidian-brain",
    version,
    about = "Semantic search, a link graph and editing over Obsidian vaults, as an MCP server."
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Clone, Copy, ValueEnum, PartialEq)]
enum Transport {
    /// For an MCP client that spawns the server.
    Stdio,
    /// For a long-running server at /mcp.
    Http,
}

#[derive(Clone, Copy, ValueEnum)]
enum SearchMode {
    Hybrid,
    Semantic,
    Fulltext,
}

#[derive(Subcommand)]
enum Command {
    /// Start the MCP server. Every tool takes a required `vault` argument
    /// naming one of the --vault names. Indexes live in <DATA_DIR>/<name>.
    Server {
        /// A vault to serve, as name=path. Repeat for more vaults.
        #[arg(long = "vault", required = true)]
        vaults: Vec<VaultSpec>,
        #[arg(long, value_enum, default_value = "stdio")]
        transport: Transport,
        /// Address to listen on with --transport http.
        #[arg(long, requires_if("http", "transport"))]
        listen: Option<SocketAddr>,
        /// A `Host` header to accept with --transport http, besides
        /// loopback. Repeat for more hosts.
        #[arg(long = "allowed-host")]
        allowed_hosts: Vec<String>,
    },
    /// Scan each vault and update its index, one vault at a time.
    Index {
        /// A vault to index, as name=path. Repeat for more vaults.
        #[arg(long = "vault", required = true)]
        vaults: Vec<VaultSpec>,
        /// Drop all embeddings and sync state before indexing.
        #[arg(long)]
        drop: bool,
    },
    /// Keep each vault index live by reindexing on vault changes.
    Watch {
        /// A vault to watch, as name=path. Repeat for more vaults.
        #[arg(long = "vault", required = true)]
        vaults: Vec<VaultSpec>,
        /// Per-file reindex debounce in milliseconds.
        #[arg(long, default_value_t = 3000)]
        debounce: u64,
    },
    /// Search one vault.
    Search {
        query: String,
        /// The vault to search, as name=path.
        #[arg(long = "vault")]
        vault: VaultSpec,
        #[arg(short, long, default_value_t = 10)]
        limit: usize,
        #[arg(short, long, value_enum, default_value = "hybrid")]
        mode: SearchMode,
    },
}

pub fn main() -> ExitCode {
    let cli = Cli::parse();
    // Logs go to stderr: stdout carries the stdio transport.
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("OBSIDIAN_BRAIN_LOG")
                .unwrap_or_else(|_| "info".into()),
        )
        .init();
    let runtime = match tokio::runtime::Runtime::new() {
        Ok(rt) => rt,
        Err(err) => {
            eprintln!("obsidian-brain: {err}");
            return ExitCode::FAILURE;
        }
    };
    match runtime.block_on(run(cli.command)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("obsidian-brain: {err:#}");
            ExitCode::FAILURE
        }
    }
}

async fn run(command: Command) -> anyhow::Result<()> {
    let data_dir = resolve_data_dir(None);
    match command {
        Command::Server {
            vaults,
            transport,
            listen,
            allowed_hosts,
        } => {
            if transport == Transport::Stdio && listen.is_some() {
                anyhow::bail!("--listen only applies with --transport http.");
            }
            let vaults = Arc::new(Vaults::open(&vaults, &data_dir).await?);
            let server = Server::new(vaults.clone(), crate::tools::all());
            let cancel = CancellationToken::new();
            spawn_signal_handler(cancel.clone());
            let result = match transport {
                Transport::Stdio => {
                    let running = server.serve(rmcp::transport::stdio()).await?;
                    tokio::select! {
                        r = running.waiting() => r.map(|_| ()).map_err(Into::into),
                        _ = cancel.cancelled() => Ok(()),
                    }
                }
                Transport::Http => {
                    let listen = listen.unwrap_or_else(|| "127.0.0.1:8080".parse().unwrap());
                    http::serve(
                        server,
                        http::HttpOptions {
                            listen,
                            allowed_hosts,
                        },
                        cancel,
                    )
                    .await
                }
            };
            vaults.close().await;
            result
        }
        Command::Index { .. } | Command::Watch { .. } | Command::Search { .. } => {
            anyhow::bail!("not ported yet")
        }
    }
}

fn spawn_signal_handler(cancel: CancellationToken) {
    tokio::spawn(async move {
        let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("SIGTERM handler");
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = term.recv() => {}
        }
        tracing::info!("shutting down");
        cancel.cancel();
    });
}
