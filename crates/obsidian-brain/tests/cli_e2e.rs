//! The real binary end to end, with no model: the embedder is Ollama at a
//! dead port, so every embed fails and is skipped, and notes are still
//! indexed for full-text search. Ported from the parts of
//! `test/integration/{cli-vault-commands,multi-vault,server-stdin-shutdown}.test.ts`
//! that need no tools.

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Output, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

const BIN: &str = env!("CARGO_BIN_EXE_obsidian-brain");

fn fixture_vault() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/vault")
}

fn command(data_dir: &Path) -> Command {
    let mut cmd = Command::new(BIN);
    cmd.env("DATA_DIR", data_dir)
        .env("EMBEDDING_PROVIDER", "ollama")
        .env("OLLAMA_BASE_URL", "http://127.0.0.1:1")
        .env("OLLAMA_EMBEDDING_DIM", "384")
        .env("OBSIDIAN_BRAIN_NO_WATCH", "1")
        .env("OBSIDIAN_BRAIN_LOG", "info")
        .env_remove("EMBEDDING_PRESET")
        .env_remove("EMBEDDING_MODEL")
        .env_remove("OBSIDIAN_BRAIN_NO_CATCHUP");
    cmd
}

fn run(data_dir: &Path, args: &[&str]) -> Output {
    let out = command(data_dir).args(args).output().unwrap();
    assert!(
        out.status.success(),
        "{args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    out
}

fn sigterm(child: &Child) {
    let status = Command::new("kill")
        .args(["-TERM", &child.id().to_string()])
        .status()
        .unwrap();
    assert!(status.success());
}

fn wait_exit(child: &mut Child, within: Duration) -> std::process::ExitStatus {
    let deadline = Instant::now() + within;
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            return status;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            panic!("the process did not exit within {within:?}");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// A stdio MCP client over newline-delimited JSON-RPC.
struct Client {
    child: Child,
    stdin: Option<ChildStdin>,
    lines: mpsc::Receiver<String>,
    stderr: mpsc::Receiver<String>,
    next_id: u64,
}

impl Client {
    fn start(mut cmd: Command) -> Self {
        let mut child = cmd
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let (tx, lines) = mpsc::channel();
        let stdout = child.stdout.take().unwrap();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                let _ = tx.send(line);
            }
        });
        let (etx, stderr) = mpsc::channel();
        let err = child.stderr.take().unwrap();
        std::thread::spawn(move || {
            for line in BufReader::new(err).lines().map_while(Result::ok) {
                let _ = etx.send(line);
            }
        });
        let stdin = child.stdin.take();
        let mut client = Self {
            child,
            stdin,
            lines,
            stderr,
            next_id: 1,
        };
        client.request(
            "initialize",
            json!({
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": {"name": "cli-e2e", "version": "0"},
            }),
        );
        client.send(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
        client
    }

    fn send(&mut self, msg: &Value) {
        let stdin = self.stdin.as_mut().unwrap();
        writeln!(stdin, "{msg}").unwrap();
        stdin.flush().unwrap();
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let line = self
                .lines
                .recv_timeout(left)
                .unwrap_or_else(|_| panic!("no answer to {method}"));
            let msg: Value = serde_json::from_str(&line).unwrap();
            if msg["id"] == json!(id) {
                return msg;
            }
        }
    }

    fn list_vaults(&mut self) -> Value {
        let res = self.request(
            "tools/call",
            json!({"name": "list_vaults", "arguments": {}}),
        );
        let text = res["result"]["content"][0]["text"].as_str().unwrap();
        serde_json::from_str(text).unwrap()
    }

    fn stderr(&self) -> String {
        self.stderr.try_iter().collect::<Vec<_>>().join("\n")
    }
}

fn copy_dir(from: &Path, to: &Path) {
    for entry in walkdir::WalkDir::new(from) {
        let entry = entry.unwrap();
        let dest = to.join(entry.path().strip_prefix(from).unwrap());
        if entry.file_type().is_dir() {
            std::fs::create_dir_all(&dest).unwrap();
        } else {
            std::fs::copy(entry.path(), &dest).unwrap();
        }
    }
}

#[test]
fn server_indexes_the_fixture_vault_and_search_finds_a_note() {
    let dir = tempfile::tempdir().unwrap();
    let vault = dir.path().join("vault");
    copy_dir(&fixture_vault(), &vault);
    let data = dir.path().join("data");
    let spec = format!("fixture={}", vault.display());

    let mut cmd = command(&data);
    cmd.args(["server", "--vault", &spec]);
    let mut client = Client::start(cmd);

    // The first index runs in the background after the handshake.
    let deadline = Instant::now() + Duration::from_secs(30);
    let vaults = loop {
        let v = client.list_vaults();
        if v[0]["notes"].as_u64().unwrap_or(0) >= 7 && v[0]["reindexInProgress"] == json!(false) {
            break v;
        }
        assert!(
            Instant::now() < deadline,
            "not indexed: {v}\n{}",
            client.stderr()
        );
        std::thread::sleep(Duration::from_millis(100));
    };
    assert_eq!(vaults[0]["name"], "fixture");
    assert_eq!(vaults[0]["path"], json!(vault.to_str().unwrap()));
    assert_eq!(vaults[0]["embedderReady"], json!(true));
    assert!(vaults[0].get("initError").is_none(), "{vaults}");
    assert!(data.join("fixture").join("index.db").exists());

    // Closing stdin ends the session; the server shuts down cleanly.
    drop(client.stdin.take());
    let status = wait_exit(&mut client.child, Duration::from_secs(10));
    assert!(status.success());

    let out = run(
        &data,
        &[
            "search",
            "--vault",
            &spec,
            "--mode",
            "fulltext",
            "resilient components",
        ],
    );
    let hits: Value = serde_json::from_slice(&out.stdout).unwrap();
    let ids: Vec<&str> = hits
        .as_array()
        .unwrap()
        .iter()
        .map(|h| h["nodeId"].as_str().unwrap())
        .collect();
    assert!(!ids.is_empty(), "{hits}");
    assert!(ids.iter().all(|id| vault.join(id).exists()), "{ids:?}");
}

#[test]
fn index_builds_one_index_per_vault_and_search_reads_only_its_own() {
    let dir = tempfile::tempdir().unwrap();
    let (alpha, beta, data) = (
        dir.path().join("a"),
        dir.path().join("b"),
        dir.path().join("data"),
    );
    std::fs::create_dir_all(&alpha).unwrap();
    std::fs::create_dir_all(&beta).unwrap();
    std::fs::write(
        alpha.join("Lighthouse.md"),
        "# Lighthouse\n\nThe lighthouse keeper rows to the alpha island every morning.\n",
    )
    .unwrap();
    std::fs::write(
        beta.join("Harbour.md"),
        "# Harbour\n\nThe harbour master counts the beta boats every evening.\n",
    )
    .unwrap();
    let (a, b) = (
        format!("alpha={}", alpha.display()),
        format!("beta={}", beta.display()),
    );

    let out = run(&data, &["index", "--vault", &a, "--vault", &b]);
    let stats: Value = serde_json::from_slice(&out.stdout).unwrap();
    let keys: Vec<&String> = stats.as_object().unwrap().keys().collect();
    assert_eq!(keys, ["alpha", "beta"]);
    assert_eq!(stats["alpha"]["nodesIndexed"], 1);
    assert_eq!(stats["beta"]["nodesIndexed"], 1);
    assert!(data.join("alpha/index.db").exists());
    assert!(data.join("beta/index.db").exists());

    let hit = run(
        &data,
        &["search", "--vault", &a, "--mode", "fulltext", "lighthouse"],
    );
    assert!(String::from_utf8_lossy(&hit.stdout).contains("Lighthouse.md"));
    let miss = run(
        &data,
        &["search", "--vault", &b, "--mode", "fulltext", "lighthouse"],
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&miss.stdout).unwrap(),
        json!([])
    );

    // --drop rebuilds from scratch: the unchanged note is indexed again.
    let out = run(&data, &["index", "--vault", &a, "--drop"]);
    let stats: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(stats["alpha"]["nodesIndexed"], 1);
    assert!(
        String::from_utf8_lossy(&out.stderr)
            .contains("dropped existing embeddings + sync state of \"alpha\"")
    );
    // No embed succeeds, so self-heal clears the note's sync row each pass.
    assert_eq!(stats["alpha"]["notesMissingEmbeddings"], 1);
}

#[test]
fn watch_shuts_down_cleanly_on_sigterm() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.md"), "# A\n").unwrap();
    let spec = format!("alpha={}", dir.path().display());
    let mut child = command(&dir.path().join("data"))
        .args(["watch", "--vault", &spec])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    std::thread::sleep(Duration::from_millis(1500));
    sigterm(&child);
    let status = wait_exit(&mut child, Duration::from_secs(10));
    let mut stderr = String::new();
    std::io::Read::read_to_string(child.stderr.as_mut().unwrap(), &mut stderr).unwrap();
    assert!(status.success(), "{stderr}");
    assert!(stderr.contains("shutting down (SIGTERM)"), "{stderr}");
}

#[test]
fn server_shuts_down_cleanly_on_sigterm() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("note.md"), "# Note\n\nhello\n").unwrap();
    let spec = format!("notes={}", dir.path().display());
    let mut cmd = command(&dir.path().join("data"));
    cmd.args(["server", "--vault", &spec]);
    let mut client = Client::start(cmd);
    std::thread::sleep(Duration::from_millis(500));
    sigterm(&client.child);
    let status = wait_exit(&mut client.child, Duration::from_secs(10));
    std::thread::sleep(Duration::from_millis(100));
    let stderr = client.stderr();
    assert!(status.success(), "{stderr}");
    assert!(stderr.contains("shutting down (SIGTERM)"), "{stderr}");
}

#[test]
fn no_catchup_skips_the_startup_catch_up_of_a_non_empty_index() {
    let dir = tempfile::tempdir().unwrap();
    let (vault, data) = (dir.path().join("v"), dir.path().join("data"));
    std::fs::create_dir_all(&vault).unwrap();
    std::fs::write(vault.join("one.md"), "# One\n").unwrap();
    let spec = format!("v={}", vault.display());
    run(&data, &["index", "--vault", &spec]);
    std::fs::write(vault.join("two.md"), "# Two\n").unwrap();

    let mut cmd = command(&data);
    cmd.env("OBSIDIAN_BRAIN_NO_CATCHUP", "1")
        .args(["server", "--vault", &spec]);
    let mut client = Client::start(cmd);
    std::thread::sleep(Duration::from_millis(1500));
    let v = client.list_vaults();
    assert_eq!(v[0]["notes"], 1, "{v}");
    assert_eq!(v[0]["embedderReady"], json!(true), "{v}");
    drop(client.stdin.take());
    wait_exit(&mut client.child, Duration::from_secs(10));

    // Without the switch, the catch-up picks the new note up.
    let mut cmd = command(&data);
    cmd.args(["server", "--vault", &spec]);
    let mut client = Client::start(cmd);
    let deadline = Instant::now() + Duration::from_secs(20);
    while client.list_vaults()[0]["notes"] != 2 {
        assert!(Instant::now() < deadline, "{}", client.stderr());
        std::thread::sleep(Duration::from_millis(100));
    }
    drop(client.stdin.take());
    wait_exit(&mut client.child, Duration::from_secs(10));
}

/// One raw HTTP/1.1 POST; returns the status code and the body.
fn http_post(port: u16, path: &str, host: &str, body: &str) -> (u16, String) {
    use std::io::{Read, Write};
    let mut stream = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
    write!(
        stream,
        "POST {path} HTTP/1.1\r\nHost: {host}\r\nContent-Type: application/json\r\n\
         Accept: application/json, text/event-stream\r\nMCP-Protocol-Version: 2025-06-18\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    let status = response.split(' ').nth(1).unwrap().parse().unwrap();
    let body = response
        .split_once("\r\n\r\n")
        .map(|(_, b)| b.to_string())
        .unwrap_or_default();
    (status, body)
}

#[test]
fn http_server_answers_at_mcp_and_refuses_foreign_hosts() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("note.md"), "# Note\n\nhello\n").unwrap();
    let spec = format!("notes={}", dir.path().display());
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let listen = format!("127.0.0.1:{port}");
    let mut child = command(&dir.path().join("data"))
        .args([
            "server",
            "--transport",
            "http",
            "--listen",
            &listen,
            "--vault",
            &spec,
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while std::net::TcpStream::connect(("127.0.0.1", port)).is_err() {
        assert!(Instant::now() < deadline, "the server never listened");
        std::thread::sleep(Duration::from_millis(50));
    }
    let list = r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"list_vaults","arguments":{}}}"#;
    for path in ["/mcp", "/mcp/"] {
        let (status, body) = http_post(port, path, &listen, list);
        assert_eq!(status, 200, "{path}: {body}");
        assert!(body.contains(r#"\"name\": \"notes\""#), "{path}: {body}");
    }
    let (status, _) = http_post(port, "/mcp", "evil.example", list);
    assert_eq!(status, 403, "a foreign Host header");
    let (status, _) = http_post(port, "/other", &listen, list);
    assert_eq!(status, 404);
    sigterm(&child);
    let status = wait_exit(&mut child, Duration::from_secs(10));
    assert!(status.success());
}
