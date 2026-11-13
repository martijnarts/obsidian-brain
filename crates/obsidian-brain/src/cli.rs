//! The `obsidian-brain` command line.

use std::net::SocketAddr;
use std::process::ExitCode;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use clap::{Parser, Subcommand, ValueEnum};
use rmcp::ServiceExt;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::config::resolve_data_dir;
use crate::mcp::{Server, http};
use crate::pipeline::{WatcherOptions, read_watcher_opts_from_env};
use crate::search::SearchUnique;
use crate::store::db::drop_embedding_state;
use crate::vaults::{VaultSpec, Vaults, index_vaults_in_turn, run_startup_index};

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
    let code = match runtime.block_on(run(cli.command)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("obsidian-brain: {err:#}");
            ExitCode::FAILURE
        }
    };
    // A blocked stdin read or a stuck task must not keep the process alive
    // after a clean shutdown.
    runtime.shutdown_timeout(HARD_EXIT);
    code
}

/// How long shutdown waits for stray tasks before the process exits.
const HARD_EXIT: Duration = Duration::from_secs(4);

/// How often the stdio server checks that the process that spawned it is
/// still alive.
const ORPHAN_CHECK: Duration = Duration::from_secs(60);

/// A shutdown trigger that remembers the first reason given.
#[derive(Clone, Default)]
struct Shutdown {
    token: CancellationToken,
    reason: Arc<OnceLock<String>>,
}

impl Shutdown {
    fn trigger(&self, reason: &str) {
        let _ = self.reason.set(reason.to_string());
        self.token.cancel();
    }

    fn reason(&self) -> &str {
        self.reason.get().map_or("unknown", String::as_str)
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
            // Armed before the vaults open, so an early SIGTERM still shuts
            // down cleanly.
            let shutdown = Shutdown::default();
            spawn_signal_handler(shutdown.clone());
            let vaults = Arc::new(Vaults::open(&vaults, &data_dir).await?);
            let server = Server::new(vaults.clone(), crate::tools::all());
            let result = match transport {
                Transport::Stdio => {
                    // The model loads and the first index runs in the
                    // background once the handshake is done, so the client
                    // never waits for them.
                    tokio::select! {
                        served = server.serve(rmcp::transport::stdio()) => match served {
                            Err(err) => {
                                shutdown.trigger("MCP transport closed");
                                Err(err.into())
                            }
                            Ok(running) => {
                                start_background(&vaults, &shutdown);
                                spawn_orphan_watchdog(shutdown.clone());
                                let service = running.cancellation_token();
                                tokio::select! {
                                    r = running.waiting() => {
                                        shutdown.trigger("MCP transport closed");
                                        r.map(|_| ()).map_err(Into::into)
                                    }
                                    _ = shutdown.token.cancelled() => {
                                        service.cancel();
                                        Ok(())
                                    }
                                }
                            }
                        },
                        _ = shutdown.token.cancelled() => Ok(()),
                    }
                }
                Transport::Http => {
                    let listen = listen.unwrap_or_else(|| "127.0.0.1:8080".parse().unwrap());
                    start_background(&vaults, &shutdown);
                    http::serve(
                        server,
                        http::HttpOptions {
                            listen,
                            allowed_hosts,
                        },
                        shutdown.token.clone(),
                    )
                    .await
                }
            };
            tracing::info!(
                reason = shutdown.reason(),
                "shutting down ({}).",
                shutdown.reason()
            );
            vaults.close().await;
            result
        }
        Command::Index { vaults, drop } => {
            let vaults = Vaults::open(&vaults, &data_dir).await?;
            let result = index_all(&vaults, drop).await;
            vaults.close().await;
            let stats = result?;
            println!("{}", serde_json::to_string_pretty(&stats)?);
            Ok(())
        }
        Command::Watch { vaults, debounce } => {
            let shutdown = Shutdown::default();
            spawn_signal_handler(shutdown.clone());
            spawn_stdin_eof_watch(shutdown.clone());
            let vaults = Vaults::open(&vaults, &data_dir).await?;
            for ctx in vaults.iter() {
                if let Err(err) = ctx.ensure_embedder_ready().await {
                    vaults.close().await;
                    return Err(err);
                }
            }
            vaults.watch_all(WatcherOptions {
                debounce_ms: Some(debounce),
                stability_ms: None,
            });
            shutdown.token.cancelled().await;
            tracing::info!(
                reason = shutdown.reason(),
                "shutting down ({}).",
                shutdown.reason()
            );
            vaults.close().await;
            Ok(())
        }
        Command::Search {
            query,
            vault,
            limit,
            mode,
        } => {
            let vaults = Vaults::open(&[vault], &data_dir).await?;
            let result = search(&vaults, &query, limit, mode).await;
            vaults.close().await;
            println!("{}", serde_json::to_string_pretty(&result?)?);
            Ok(())
        }
    }
}

/// Index each vault in turn. `drop` first clears its embeddings and sync
/// state, forcing a rebuild from scratch.
async fn index_all(vaults: &Vaults, drop: bool) -> anyhow::Result<serde_json::Map<String, Value>> {
    let mut stats = serde_json::Map::new();
    for name in vaults.names() {
        let ctx = vaults.get(&name).expect("a listed vault");
        if drop {
            ctx.db.with(drop_embedding_state)?;
            eprintln!("obsidian-brain: dropped existing embeddings + sync state of \"{name}\"");
        }
        ctx.ensure_embedder_ready().await?;
        let s = ctx.pipeline.index(&ctx.config.vault_path).await?;
        stats.insert(name, serde_json::to_value(s)?);
    }
    Ok(stats)
}

async fn search(
    vaults: &Vaults,
    query: &str,
    limit: usize,
    mode: SearchMode,
) -> anyhow::Result<Value> {
    let ctx = vaults.iter().next().expect("one vault").clone();
    Ok(match mode {
        SearchMode::Fulltext => serde_json::to_value(ctx.search.fulltext(query, limit)?)?,
        SearchMode::Semantic => {
            ctx.ensure_embedder_ready().await?;
            serde_json::to_value(ctx.search.semantic(query, limit).await?)?
        }
        SearchMode::Hybrid => {
            ctx.ensure_embedder_ready().await?;
            serde_json::to_value(ctx.search.hybrid(query, limit, SearchUnique::Notes).await?)?
        }
    })
}

/// The startup index of every vault, one at a time, and a watcher per
/// vault unless `OBSIDIAN_BRAIN_NO_WATCH=1`.
fn start_background(vaults: &Arc<Vaults>, shutdown: &Shutdown) {
    let v = vaults.clone();
    let stopped = shutdown.token.clone();
    tokio::spawn(async move {
        index_vaults_in_turn(v.iter(), run_startup_index, || stopped.is_cancelled()).await;
    });
    vaults.start_watchers(read_watcher_opts_from_env());
}

fn spawn_signal_handler(shutdown: Shutdown) {
    tokio::spawn(async move {
        let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("SIGTERM handler");
        let reason = tokio::select! {
            _ = tokio::signal::ctrl_c() => "SIGINT",
            _ = term.recv() => "SIGTERM",
        };
        shutdown.trigger(reason);
    });
}

/// A host that crashes or is force-quit sends no SIGTERM, and the server
/// would run on forever. Once a minute, check that the parent is the one
/// that spawned us; a dead parent reparents us. Closed stdin is not the
/// signal: some hosts close it briefly while they load.
fn spawn_orphan_watchdog(shutdown: Shutdown) {
    let original = std::os::unix::process::parent_id();
    tracing::debug!(parent_pid = original, "orphan watchdog armed");
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(ORPHAN_CHECK);
        tick.tick().await;
        loop {
            tokio::select! {
                _ = shutdown.token.cancelled() => return,
                _ = tick.tick() => {
                    if std::os::unix::process::parent_id() != original {
                        shutdown.trigger("parent process died (orphaned)");
                        return;
                    }
                }
            }
        }
    });
}

/// `watch` also stops when its stdin closes. A plain thread reads it, so a
/// blocked read never holds up the runtime.
fn spawn_stdin_eof_watch(shutdown: Shutdown) {
    std::thread::spawn(move || {
        let mut sink = [0u8; 4096];
        let mut stdin = std::io::stdin();
        loop {
            match std::io::Read::read(&mut stdin, &mut sink) {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
        }
        shutdown.trigger("stdin EOF");
    });
}
