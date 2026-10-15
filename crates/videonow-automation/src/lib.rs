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
//! The studio lives on one worker thread (the engines are not shared between threads); tool calls
//! reach it through a channel, one at a time, in the order they arrive.

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

type Job = Box<dyn FnOnce(&mut Studio) + Send>;

/// A handle to the worker thread that owns the studio.
#[derive(Clone)]
struct Worker {
    tx: mpsc::Sender<Job>,
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
