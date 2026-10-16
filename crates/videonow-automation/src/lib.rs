//! The studio's MCP server: five tools over every engine.
//!
//! | Tool | What it does |
//! |---|---|
//! | `command_list` | every command of every engine, with its effect class (`read`, `edit`, `destructive`, `fileWrite`) |
//! | `command_run` | run one command; the reply carries its result and its receipt |
//! | `command_batch` | run several in order; stops at the first error unless told not to |
//! | `doc_inspect` | the whole project: sequence, compositions, images, assets |
//! | `render_preview` | a PNG of the active sequence at a time (reads only) |
//!
//! The studio lives on one thread (the engines are not shared between threads); tool calls reach
//! it through a channel, one at a time, in the order they arrive. `serve_stdio` gives the server a
//! studio of its own on a worker thread. The desktop application keeps its studio on its UI
//! thread and runs the jobs between frames ([`attach`], [`serve_tcp`]): an agent then edits the
//! project the person sees, and every change shows up in the application's receipts.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::mpsc;

use base64::Engine as _;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ContentBlock as Content};
use rmcp::{ErrorData as McpError, ServerHandler, ServiceExt, tool, tool_handler, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};
use videonow_core::{Policy, Studio};

#[derive(Debug, thiserror::Error)]
pub enum AutomationError {
    #[error("the studio worker stopped")]
    WorkerGone,
    #[error("{0}")]
    Io(String),
}

/// Work for the thread that owns the studio.
pub type Job = Box<dyn FnOnce(&mut Studio) + Send>;
/// Called after a job is queued, so an idle owner wakes up to run it.
pub type Wake = std::sync::Arc<dyn Fn() + Send + Sync>;

/// A handle to the thread that owns the studio.
#[derive(Clone)]
struct Worker {
    tx: mpsc::Sender<Job>,
    wake: Option<Wake>,
}

impl Worker {
    fn spawn(policy: Policy) -> Self {
        let (tx, rx) = mpsc::channel::<Job>();
        std::thread::spawn(move || {
            let mut studio = Studio::new();
            studio.set_policy(policy);
            while let Ok(job) = rx.recv() {
                // a panic in one command must not take the session down (never-crash rule)
                let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| job(&mut studio)));
                if r.is_err() {
                    eprintln!("videonow: a command panicked; the session keeps serving");
                }
            }
        });
        Self { tx, wake: None }
    }

    async fn call<T: Send + 'static>(&self, f: impl FnOnce(&mut Studio) -> T + Send + 'static) -> Result<T, AutomationError> {
        let (rtx, rrx) = tokio::sync::oneshot::channel();
        self.tx
            .send(Box::new(move |s: &mut Studio| {
                let _ = rtx.send(f(s));
            }))
            .map_err(|_| AutomationError::WorkerGone)?;
        if let Some(w) = &self.wake {
            w();
        }
        rrx.await.map_err(|_| AutomationError::WorkerGone)
    }
}

#[derive(Clone)]
pub struct StudioMcp {
    worker: Worker,
    tool_router: ToolRouter<Self>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListParams {
    /// Only commands whose id or label contains this text (case-insensitive).
    #[serde(default)]
    pub filter: Option<String>,
    /// Only one engine: `timeline`, `motion`, `image` or `studio`.
    #[serde(default)]
    pub domain: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RunParams {
    /// `domain:command`, e.g. `studio:comp.new`, `motion:layer.newText`, `timeline:timeline.place`.
    pub id: String,
    /// The command's parameters (see `params` in `command_list`). `motion:` commands take an
    /// optional composition `asset`; `image:` commands need an image `asset`.
    #[serde(default)]
    pub params: Option<Value>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Step {
    pub id: String,
    #[serde(default)]
    pub params: Option<Value>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BatchParams {
    pub steps: Vec<Step>,
    /// Stop at the first failing step (default true). Steps that ran stay applied: this is not a
    /// transaction (`studio:undo` once per changed step rolls them back).
    #[serde(default)]
    pub stop_on_error: Option<bool>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PreviewParams {
    /// Sequence time in seconds (default 0).
    #[serde(default)]
    pub seconds: Option<f64>,
    /// Longest side of the PNG in pixels (default 960, at most the sequence size).
    #[serde(default)]
    pub max_side: Option<u32>,
}

fn ok_json(v: &Value) -> CallToolResult {
    CallToolResult::success(vec![Content::text(serde_json::to_string_pretty(v).unwrap_or_default())])
}
fn fail(e: impl std::fmt::Display) -> CallToolResult {
    CallToolResult::error(vec![Content::text(e.to_string())])
}

const INSTRUCTIONS: &str = "VideoNow: one video project across three engines. `timeline:` commands edit sequences (FilmCraft), \
`motion:` commands edit compositions (EffectCraft; pass a composition `asset`), `image:` commands edit image documents \
(PhotoCraft; pass an image `asset`), `studio:` commands create assets and connect them (`studio:comp.new`, `studio:image.new`, \
`studio:image.toComp`, `studio:asset.place`). Every reply carries a receipt: what changed, revisions, what was checked and \
what was not. Look at `render_preview` before claiming a visual result.";

#[tool_router]
impl StudioMcp {
    #[tool(
        title = "List commands",
        description = "Every command of every engine: id (`domain:command`), label, parameter doc, and effect class (read, edit, destructive, fileWrite; `basis` says whether the class is declared or inferred).",
        annotations(read_only_hint = true, destructive_hint = false, idempotent_hint = true, open_world_hint = false)
    )]
    async fn command_list(&self, Parameters(p): Parameters<ListParams>) -> Result<CallToolResult, McpError> {
        let mut q = serde_json::Map::new();
        if let Some(f) = p.filter {
            q.insert("filter".into(), json!(f));
        }
        if let Some(d) = p.domain {
            q.insert("domain".into(), json!(d));
        }
        let r = self.worker.call(move |s| s.execute("studio:commands", Value::Object(q)).map(|v| v["result"].clone())).await;
        Ok(match r {
            Ok(Ok(v)) => ok_json(&v),
            Ok(Err(e)) => fail(e),
            Err(e) => fail(e),
        })
    }

    #[tool(
        title = "Run a command",
        description = "Run one command: {\"id\":\"studio:comp.new\",\"params\":{\"name\":\"Title\"}}. The reply is {result, receipt}; the receipt lists what changed, the revisions, what was checked and what was not.",
        annotations(read_only_hint = false, destructive_hint = true, idempotent_hint = false, open_world_hint = false)
    )]
    async fn command_run(&self, Parameters(p): Parameters<RunParams>) -> Result<CallToolResult, McpError> {
        let r = self.worker.call(move |s| s.execute(&p.id, p.params.unwrap_or(Value::Null))).await;
        Ok(match r {
            Ok(Ok(v)) => ok_json(&v),
            Ok(Err(e)) => fail(e),
            Err(e) => fail(e),
        })
    }

    #[tool(
        title = "Run several commands",
        description = "Run commands in order: {steps: [{id, params?}], stop_on_error?: true}. Returns {completed, failed, results}. Not a transaction: steps that ran stay applied.",
        annotations(read_only_hint = false, destructive_hint = true, idempotent_hint = false, open_world_hint = false)
    )]
    async fn command_batch(&self, Parameters(p): Parameters<BatchParams>) -> Result<CallToolResult, McpError> {
        let stop = p.stop_on_error.unwrap_or(true);
        let r = self
            .worker
            .call(move |s| {
                let (mut completed, mut failed, mut results) = (0u32, 0u32, Vec::new());
                for st in p.steps {
                    match s.execute(&st.id, st.params.unwrap_or(Value::Null)) {
                        Ok(v) => {
                            completed += 1;
                            results.push(json!({"ok": true, "id": st.id, "result": v["result"], "receipt": v["receipt"]}));
                        }
                        Err(e) => {
                            failed += 1;
                            results.push(json!({"ok": false, "id": st.id, "error": e.to_string()}));
                            if stop {
                                break;
                            }
                        }
                    }
                }
                json!({"completed": completed, "failed": failed, "results": results})
            })
            .await;
        Ok(match r {
            Ok(v) if v["failed"].as_u64().unwrap_or(0) > 0 => CallToolResult::error(vec![Content::text(v.to_string())]),
            Ok(v) => ok_json(&v),
            Err(e) => fail(e),
        })
    }

    #[tool(
        title = "Inspect the project",
        description = "The whole project: the active sequence, every composition and image document with its asset id, and the studio revision.",
        annotations(read_only_hint = true, destructive_hint = false, idempotent_hint = true, open_world_hint = false)
    )]
    async fn doc_inspect(&self) -> Result<CallToolResult, McpError> {
        let r = self.worker.call(|s| s.execute("studio:inspect", json!({})).map(|v| v["result"].clone())).await;
        Ok(match r {
            Ok(Ok(v)) => ok_json(&v),
            Ok(Err(e)) => fail(e),
            Err(e) => fail(e),
        })
    }

    #[tool(
        title = "Render a preview",
        description = "A PNG of the active sequence at `seconds` (compositions and images on it render live). Reads only; names any offline media drawn as a slate.",
        annotations(read_only_hint = true, destructive_hint = false, idempotent_hint = true, open_world_hint = false)
    )]
    async fn render_preview(&self, Parameters(p): Parameters<PreviewParams>) -> Result<CallToolResult, McpError> {
        let secs = p.seconds.unwrap_or(0.0);
        let max_side = p.max_side.unwrap_or(960).clamp(16, 4096);
        let r = self
            .worker
            .call(move |s| {
                let full = s.timeline.active_sequence().map(|q| q.settings.width.max(q.settings.height)).unwrap_or(1920).max(1);
                let scale = (f64::from(max_side) / f64::from(full)).min(1.0);
                s.render_rgba(secs, scale)
            })
            .await;
        Ok(match r {
            Ok(Ok((w, h, rgba, offline))) => {
                let mut png = Vec::new();
                match image::write_buffer_with_format(&mut std::io::Cursor::new(&mut png), &rgba, w, h, image::ExtendedColorType::Rgba8, image::ImageFormat::Png) {
                    Ok(()) => {
                        let note = if offline.is_empty() { format!("{w}x{h} at {secs}s") } else { format!("{w}x{h} at {secs}s; offline (slate): {}", offline.join(", ")) };
                        CallToolResult::success(vec![Content::image(base64::engine::general_purpose::STANDARD.encode(&png), "image/png"), Content::text(note)])
                    }
                    Err(e) => fail(e),
                }
            }
            Ok(Err(e)) => fail(e),
            Err(e) => fail(e),
        })
    }
}

impl StudioMcp {
    /// A server whose studio enforces `policy` (see `videonow_core::policy`).
    pub fn new(policy: Policy) -> Self {
        Self { worker: Worker::spawn(policy), tool_router: Self::tool_router() }
    }

    /// A server for a studio another thread owns: tool calls become [`Job`]s on `tx`, and
    /// `wake` is called after each one is queued.
    pub fn attach(tx: mpsc::Sender<Job>, wake: Option<Wake>) -> Self {
        Self { worker: Worker { tx, wake }, tool_router: Self::tool_router() }
    }
}

/// See [`StudioMcp::attach`].
pub fn attach(tx: mpsc::Sender<Job>, wake: Option<Wake>) -> StudioMcp {
    StudioMcp::attach(tx, wake)
}

/// The first line a client sends on the desktop application's agent port, before MCP starts:
/// `VIDEONOW <token>`. Anything else closes the connection (a web page cannot speak it, and the
/// token is only in the user's own connection file).
pub const HELLO: &str = "VIDEONOW";

/// What happened on the agent port, for the application to show.
#[derive(Clone, Debug)]
pub enum AgentEvent {
    Listening(std::net::SocketAddr),
    Connected(std::net::SocketAddr),
    Disconnected(std::net::SocketAddr),
    Refused(std::net::SocketAddr, String),
    Failed(String),
}

/// Serve MCP on `addr` (a localhost address) for a studio another thread owns: every connection
/// that starts with `VIDEONOW <token>` is one MCP session over [`attach`]. Runs until `stop` is
/// set; `events` hears about connections.
pub fn serve_tcp(
    addr: std::net::SocketAddr,
    token: String,
    tx: mpsc::Sender<Job>,
    wake: Option<Wake>,
    events: impl Fn(AgentEvent) + Send + Sync + 'static,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> Result<std::net::SocketAddr, AutomationError> {
    use tokio::io::AsyncBufReadExt as _;
    let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().map_err(|e| AutomationError::Io(e.to_string()))?;
    let listener = rt.block_on(tokio::net::TcpListener::bind(addr)).map_err(|e| AutomationError::Io(format!("{addr}: {e}")))?;
    let bound = listener.local_addr().map_err(|e| AutomationError::Io(e.to_string()))?;
    let events = std::sync::Arc::new(events);
    events(AgentEvent::Listening(bound));
    std::thread::spawn(move || {
        rt.block_on(async move {
            loop {
                if stop.load(std::sync::atomic::Ordering::Relaxed) {
                    break;
                }
                let accepted = tokio::time::timeout(std::time::Duration::from_millis(250), listener.accept()).await;
                let (stream, peer) = match accepted {
                    Err(_) => continue,
                    Ok(Err(e)) => {
                        events(AgentEvent::Failed(e.to_string()));
                        continue;
                    }
                    Ok(Ok(x)) => x,
                };
                if !peer.ip().is_loopback() {
                    events(AgentEvent::Refused(peer, "not from this computer".into()));
                    continue;
                }
                let (tx, wake, token, events, stop) = (tx.clone(), wake.clone(), token.clone(), events.clone(), stop.clone());
                tokio::spawn(async move {
                    let (r, w) = stream.into_split();
                    let mut r = tokio::io::BufReader::new(r);
                    let mut first = String::new();
                    let hello = tokio::time::timeout(std::time::Duration::from_secs(5), r.read_line(&mut first)).await;
                    if !matches!(hello, Ok(Ok(_))) || first.trim_end() != format!("{HELLO} {token}") {
                        events(AgentEvent::Refused(peer, "wrong or missing token".into()));
                        return;
                    }
                    events(AgentEvent::Connected(peer));
                    match StudioMcp::attach(tx, wake).serve((r, w)).await {
                        Ok(service) => {
                            let cancel = service.cancellation_token();
                            let watch = tokio::spawn(async move {
                                while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                                    tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                                }
                                cancel.cancel();
                            });
                            let _ = service.waiting().await;
                            watch.abort();
                        }
                        Err(e) => events(AgentEvent::Failed(e.to_string())),
                    }
                    events(AgentEvent::Disconnected(peer));
                });
            }
        });
    });
    Ok(bound)
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for StudioMcp {
    fn get_info(&self) -> rmcp::model::ServerConfig {
        rmcp::model::ServerConfig::new(rmcp::model::ServerCapabilities::builder().enable_tools().build())
            .with_server_info(rmcp::model::Implementation::new("videonow", env!("CARGO_PKG_VERSION")))
            .with_instructions(INSTRUCTIONS.to_string())
    }
}

/// Serve MCP on stdin/stdout until the client disconnects.
pub fn serve_stdio(policy: Policy) -> Result<(), AutomationError> {
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().map_err(|e| AutomationError::Io(e.to_string()))?;
    rt.block_on(async {
        let service = StudioMcp::new(policy).serve((tokio::io::stdin(), tokio::io::stdout())).await.map_err(|e| AutomationError::Io(e.to_string()))?;
        service.waiting().await.map_err(|e| AutomationError::Io(e.to_string()))?;
        Ok(())
    })
}

/// Where the desktop application writes how to reach its agent port (`{"addr", "token"}`):
/// `$XDG_RUNTIME_DIR/videonow/agent.json`, else the platform's local data folder. Readable by the
/// user only.
pub fn connection_file() -> Option<std::path::PathBuf> {
    let base = std::env::var_os("XDG_RUNTIME_DIR")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("LOCALAPPDATA").map(std::path::PathBuf::from))
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".local/state")))?;
    Some(base.join("videonow").join("agent.json"))
}

/// Record the agent port for `videonow mcp --attach`.
pub fn write_connection(addr: std::net::SocketAddr, token: &str) -> Result<std::path::PathBuf, AutomationError> {
    let path = connection_file().ok_or_else(|| AutomationError::Io("no folder for the connection file (set XDG_RUNTIME_DIR or HOME)".into()))?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| AutomationError::Io(format!("{}: {e}", dir.display())))?;
    }
    let text = json!({"addr": addr.to_string(), "token": token, "pid": std::process::id()}).to_string();
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        opts.mode(0o600);
    }
    let mut f = opts.open(&path).map_err(|e| AutomationError::Io(format!("{}: {e}", path.display())))?;
    std::io::Write::write_all(&mut f, text.as_bytes()).map_err(|e| AutomationError::Io(format!("{}: {e}", path.display())))?;
    Ok(path)
}

/// Remove the connection file if it still names this process's port.
pub fn remove_connection(addr: std::net::SocketAddr) {
    if let Some(path) = connection_file()
        && let Ok(text) = std::fs::read_to_string(&path)
        && let Ok(v) = serde_json::from_str::<Value>(&text)
        && v["addr"].as_str() == Some(addr.to_string().as_str())
    {
        let _ = std::fs::remove_file(path);
    }
}

/// A random token for the agent port: not guessable by another user or a web page.
pub fn new_token() -> String {
    use std::hash::{BuildHasher as _, Hasher as _};
    let mut out = String::new();
    for i in 0..4u64 {
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u64(i);
        h.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0));
        h.write_u32(std::process::id());
        out.push_str(&format!("{:016x}", h.finish()));
    }
    out
}

/// `videonow mcp --attach`: connect stdin/stdout to the desktop application's agent port, so an
/// MCP client that starts a process (Claude Desktop, Claude Code) edits the open project.
/// `target` is a connection file or `host:port` (then `token` is needed); `None` reads
/// [`connection_file`].
pub fn proxy_stdio(target: Option<&str>, token: Option<&str>) -> Result<(), AutomationError> {
    use std::io::{Read as _, Write as _};
    let (addr, token) = match (target, token) {
        (Some(t), Some(k)) if t.contains(':') && !std::path::Path::new(t).exists() => (t.to_string(), k.to_string()),
        _ => {
            let path = match target {
                Some(t) => std::path::PathBuf::from(t),
                None => connection_file().ok_or_else(|| AutomationError::Io("no connection file location".into()))?,
            };
            let text = std::fs::read_to_string(&path)
                .map_err(|e| AutomationError::Io(format!("{}: {e} (is VideoNow open with agent access turned on?)", path.display())))?;
            let v: Value = serde_json::from_str(&text).map_err(|e| AutomationError::Io(format!("{}: {e}", path.display())))?;
            let addr = v["addr"].as_str().ok_or_else(|| AutomationError::Io(format!("{}: no addr", path.display())))?.to_string();
            let token = token.map(str::to_string).or_else(|| v["token"].as_str().map(str::to_string)).ok_or_else(|| AutomationError::Io(format!("{}: no token", path.display())))?;
            (addr, token)
        }
    };
    let mut stream = std::net::TcpStream::connect(&addr).map_err(|e| AutomationError::Io(format!("{addr}: {e} (is VideoNow open with agent access turned on?)")))?;
    stream.write_all(format!("{HELLO} {token}\n").as_bytes()).map_err(|e| AutomationError::Io(e.to_string()))?;
    let mut up = stream.try_clone().map_err(|e| AutomationError::Io(e.to_string()))?;
    let pump = std::thread::spawn(move || {
        let mut stdin = std::io::stdin();
        let mut buf = [0u8; 16 * 1024];
        loop {
            match stdin.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if up.write_all(buf.get(..n).unwrap_or(&[])).is_err() {
                        break;
                    }
                }
            }
        }
        let _ = up.shutdown(std::net::Shutdown::Write);
    });
    let mut stdout = std::io::stdout();
    let mut buf = [0u8; 16 * 1024];
    loop {
        match stream.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                if stdout.write_all(buf.get(..n).unwrap_or(&[])).and_then(|()| stdout.flush()).is_err() {
                    break;
                }
            }
        }
    }
    drop(pump);
    Ok(())
}
