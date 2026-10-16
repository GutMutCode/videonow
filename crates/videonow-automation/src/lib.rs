//! The studio's MCP server: five tools over every engine.
//!
//! | Tool | What it does |
//! |---|---|
//! | `command_list` | every command of every engine, with its effect class (`read`, `edit`, `destructive`, `fileWrite`, `external`) |
//! | `command_run` | run one command; the reply carries its result and its receipt |
//! | `command_batch` | run several in order; stops at the first error unless told not to |
//! | `doc_inspect` | the whole project: sequence, compositions, images, assets |
//! | `render_preview` | a PNG of the active sequence at a time (reads only) |
//!
//! The tools run against one of two backends:
//!
//! * **its own studio** (`videonow mcp`): a headless studio on a worker thread (the engines are not
//!   shared between threads); tool calls reach it through a channel, one at a time.
//! * **the desktop app** (`videonow mcp --bridge`): the studio a person has open. Tool calls go
//!   over the app's control channel (localhost, token-authenticated, see [`bridge`]) and show up in
//!   its AI panel, where the person may have to approve them.
//!
//! Both answer the same [`control::Request`]s, so a tool behaves the same either way.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

pub mod bridge;

use std::sync::mpsc;

use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ContentBlock as Content};
use rmcp::{ErrorData as McpError, ServerHandler, ServiceExt, tool, tool_handler, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};
use videonow_core::control::{self, Request};
use videonow_core::{Policy, Studio};

#[derive(Debug, thiserror::Error)]
pub enum AutomationError {
    #[error("the studio worker stopped")]
    WorkerGone,
    #[error("{0}")]
    Io(String),
}

type Job = Box<dyn FnOnce(&mut Studio) + Send>;

/// A handle to the worker thread that owns the studio.
#[derive(Clone)]
struct Worker {
    tx: mpsc::Sender<Job>,
}

impl Worker {
    fn spawn(policy: Policy, project: Option<std::path::PathBuf>) -> Self {
        let (tx, rx) = mpsc::channel::<Job>();
        std::thread::spawn(move || {
            let mut studio = Studio::with_policy(policy);
            if let Some(p) = project
                && let Err(e) = studio.execute("studio:project.open", json!({"path": p.to_string_lossy()}))
            {
                eprintln!("videonow: {e}");
            }
            while let Ok(job) = rx.recv() {
                // a panic in one command must not take the session down (never-crash rule)
                let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| job(&mut studio)));
                if r.is_err() {
                    eprintln!("videonow: a command panicked; the session keeps serving");
                }
            }
        });
        Self { tx }
    }

    async fn call<T: Send + 'static>(&self, f: impl FnOnce(&mut Studio) -> T + Send + 'static) -> Result<T, AutomationError> {
        let (rtx, rrx) = tokio::sync::oneshot::channel();
        self.tx
            .send(Box::new(move |s: &mut Studio| {
                let _ = rtx.send(f(s));
            }))
            .map_err(|_| AutomationError::WorkerGone)?;
        rrx.await.map_err(|_| AutomationError::WorkerGone)
    }
}

/// Where tool calls run.
#[derive(Clone)]
enum Backend {
    Local(Worker),
    Bridge(bridge::Client),
}

impl Backend {
    /// Run one request; `Err` carries the message for the agent.
    async fn run(&self, req: Request) -> Result<Value, String> {
        match self {
            Backend::Local(w) => match w.call(move |s| control::handle(s, &req).map_err(|e| e.to_string())).await {
                Ok(r) => r,
                Err(e) => Err(e.to_string()),
            },
            Backend::Bridge(c) => {
                let c = c.clone();
                match tokio::task::spawn_blocking(move || c.request(&req)).await {
                    Ok(r) => r,
                    Err(e) => Err(format!("bridge: {e}")),
                }
            }
        }
    }
}

#[derive(Clone)]
pub struct StudioMcp {
    backend: Backend,
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
(PhotoCraft; pass an image `asset`), `studio:` commands create assets, connect them and handle the project file \
(`studio:comp.new`, `studio:image.new`, `studio:image.toComp`, `studio:asset.place`, `studio:project.save`, `studio:project.open`). \
Every reply carries a receipt: what changed, revisions, what was checked and what was not. Look at `render_preview` before \
claiming a visual result. `studio:describe {id}` explains one command; `studio:policy` says what this session may do \
(commands outside it are refused before they run: ask the person instead).";

#[tool_router]
impl StudioMcp {
    #[tool(
        title = "List commands",
        description = "Every command of every engine: id (`domain:command`), label, parameter doc, and effect class (read, edit, destructive, fileWrite, external; `basis` says whether the class is declared or inferred).",
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
        let r = self.backend.run(Request::Execute { id: "studio:commands".into(), params: Value::Object(q) }).await;
        Ok(match r {
            Ok(v) => ok_json(&v["result"]),
            Err(e) => fail(e),
        })
    }

    #[tool(
        title = "Run a command",
        description = "Run one command: {\"id\":\"studio:comp.new\",\"params\":{\"name\":\"Title\"}}. The reply is {result, receipt}; the receipt lists what changed, the revisions, what was checked and what was not.",
        annotations(read_only_hint = false, destructive_hint = true, idempotent_hint = false, open_world_hint = false)
    )]
    async fn command_run(&self, Parameters(p): Parameters<RunParams>) -> Result<CallToolResult, McpError> {
        let r = self.backend.run(Request::Execute { id: p.id, params: p.params.unwrap_or(Value::Null) }).await;
        Ok(match r {
            Ok(v) => ok_json(&v),
            Err(e) => fail(e),
        })
    }

    #[tool(
        title = "Run several commands",
        description = "Run commands in order: {steps: [{id, params?}], stop_on_error?: true}. Returns {completed, failed, results}. Not a transaction: steps that ran stay applied.",
        annotations(read_only_hint = false, destructive_hint = true, idempotent_hint = false, open_world_hint = false)
    )]
    async fn command_batch(&self, Parameters(p): Parameters<BatchParams>) -> Result<CallToolResult, McpError> {
        let steps = p.steps.into_iter().map(|st| (st.id, st.params.unwrap_or(Value::Null))).collect();
        let r = self.backend.run(Request::Batch { steps, stop_on_error: p.stop_on_error.unwrap_or(true) }).await;
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
        let r = self.backend.run(Request::Execute { id: "studio:inspect".into(), params: json!({}) }).await;
        Ok(match r {
            Ok(v) => ok_json(&v["result"]),
            Err(e) => fail(e),
        })
    }

    #[tool(
        title = "Render a preview",
        description = "A PNG of the active sequence at `seconds` (compositions and images on it render live). Reads only; names any offline media drawn as a slate.",
        annotations(read_only_hint = true, destructive_hint = false, idempotent_hint = true, open_world_hint = false)
    )]
    async fn render_preview(&self, Parameters(p): Parameters<PreviewParams>) -> Result<CallToolResult, McpError> {
        let r = self.backend.run(Request::Preview { seconds: p.seconds.unwrap_or(0.0), max_side: p.max_side.unwrap_or(960).clamp(16, 4096) }).await;
        Ok(match r {
            Ok(v) => {
                let (w, h, secs) = (v["width"].as_u64().unwrap_or(0), v["height"].as_u64().unwrap_or(0), v["seconds"].as_f64().unwrap_or(0.0));
                let offline: Vec<&str> = v["offline"].as_array().map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
                let note = if offline.is_empty() { format!("{w}x{h} at {secs}s") } else { format!("{w}x{h} at {secs}s; offline (slate): {}", offline.join(", ")) };
                CallToolResult::success(vec![Content::image(v["png"].as_str().unwrap_or_default().to_string(), "image/png"), Content::text(note)])
            }
            Err(e) => fail(e),
        })
    }
}

impl StudioMcp {
    /// A server over a new studio limited by `policy`, with `project` opened first.
    pub fn new(policy: Policy, project: Option<std::path::PathBuf>) -> Self {
        Self { backend: Backend::Local(Worker::spawn(policy, project)), tool_router: Self::tool_router() }
    }

    /// A server that forwards every tool call to a running desktop app.
    pub fn bridged(client: bridge::Client) -> Self {
        Self { backend: Backend::Bridge(client), tool_router: Self::tool_router() }
    }
}

impl Default for StudioMcp {
    fn default() -> Self {
        Self::new(Policy::default(), None)
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for StudioMcp {
    fn get_info(&self) -> rmcp::model::ServerConfig {
        rmcp::model::ServerConfig::new(rmcp::model::ServerCapabilities::builder().enable_tools().build())
            .with_server_info(rmcp::model::Implementation::new("videonow", env!("CARGO_PKG_VERSION")))
            .with_instructions(INSTRUCTIONS.to_string())
    }
}

/// Serve MCP on stdin/stdout until the client disconnects, over a headless studio.
pub fn serve_stdio(policy: Policy, project: Option<std::path::PathBuf>) -> Result<(), AutomationError> {
    serve(StudioMcp::new(policy, project))
}

/// Serve MCP on stdin/stdout, forwarding to the desktop app.
pub fn serve_bridge(client: bridge::Client) -> Result<(), AutomationError> {
    serve(StudioMcp::bridged(client))
}

fn serve(server: StudioMcp) -> Result<(), AutomationError> {
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().map_err(|e| AutomationError::Io(e.to_string()))?;
    rt.block_on(async {
        let service = server.serve((tokio::io::stdin(), tokio::io::stdout())).await.map_err(|e| AutomationError::Io(e.to_string()))?;
        service.waiting().await.map_err(|e| AutomationError::Io(e.to_string()))?;
        Ok(())
    })
}
