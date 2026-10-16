//! A small MCP client: JSON-RPC 2.0 as newline-delimited JSON (the stdio transport), enough to
//! initialize, list tools and call them. Every call is recorded for the metrics.

use std::time::Instant;

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};

/// One tool call as the metrics see it.
#[derive(Clone, Debug, serde::Serialize)]
pub struct Call {
    pub tool: String,
    pub args: Value,
    /// The tool reported success (`isError` false).
    pub ok: bool,
    /// The reply's text (a JSON document for most tools, or the error message).
    pub text: String,
    pub ms: u128,
}

/// A tool's reply.
#[derive(Clone, Debug)]
pub struct Reply {
    pub ok: bool,
    pub text: String,
    /// `text` parsed as JSON, when it is JSON.
    pub json: Option<Value>,
    /// Image content blocks (render_preview).
    pub images: Vec<Vec<u8>>,
}

impl Reply {
    /// `json[key]` or Null.
    pub fn get(&self, key: &str) -> Value {
        self.json.as_ref().map(|j| j[key].clone()).unwrap_or(Value::Null)
    }
}

pub struct Client<R, W> {
    reader: BufReader<R>,
    writer: W,
    next_id: u64,
    pub calls: Vec<Call>,
}

impl<R: AsyncRead + Unpin, W: AsyncWrite + Unpin> Client<R, W> {
    pub fn new(reader: R, writer: W) -> Self {
        Self { reader: BufReader::new(reader), writer, next_id: 1, calls: Vec::new() }
    }

    async fn send(&mut self, msg: &Value) -> Result<(), String> {
        let mut line = serde_json::to_vec(msg).map_err(|e| e.to_string())?;
        line.push(b'\n');
        self.writer.write_all(&line).await.map_err(|e| format!("send: {e}"))?;
        self.writer.flush().await.map_err(|e| format!("send: {e}"))
    }

    /// Send a request and wait for its response (server notifications in between are skipped).
    pub async fn request(&mut self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})).await?;
        loop {
            let mut line = String::new();
            let n = tokio::time::timeout(std::time::Duration::from_secs(120), self.reader.read_line(&mut line))
                .await
                .map_err(|_| format!("{method}: no reply in 120 s"))?
                .map_err(|e| format!("{method}: {e}"))?;
            if n == 0 {
                return Err(format!("{method}: the server closed the connection"));
            }
            let Ok(msg) = serde_json::from_str::<Value>(line.trim()) else { continue };
            if msg["id"].as_u64() != Some(id) {
                continue;
            }
            if let Some(e) = msg.get("error") {
                return Err(format!("{method}: {}", e["message"].as_str().unwrap_or("error")));
            }
            return Ok(msg["result"].clone());
        }
    }

    pub async fn initialize(&mut self) -> Result<Value, String> {
        let r = self
            .request("initialize", json!({"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "videonow-eval", "version": env!("CARGO_PKG_VERSION")}}))
            .await?;
        self.send(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"})).await?;
        Ok(r)
    }

    pub async fn list_tools(&mut self) -> Result<Vec<Value>, String> {
        let r = self.request("tools/list", json!({})).await?;
        Ok(r["tools"].as_array().cloned().unwrap_or_default())
    }

    /// Call a tool. `Err` only when the connection failed; a tool error is `Ok` with `ok: false`.
    pub async fn call(&mut self, tool: &str, args: Value) -> Result<Reply, String> {
        let t = Instant::now();
        let r = self.request("tools/call", json!({"name": tool, "arguments": args})).await?;
        let ok = !r["isError"].as_bool().unwrap_or(false);
        let mut text = String::new();
        let mut images = Vec::new();
        for c in r["content"].as_array().into_iter().flatten() {
            match c["type"].as_str() {
                Some("text") => {
                    if !text.is_empty() {
                        text.push('\n');
                    }
                    text.push_str(c["text"].as_str().unwrap_or(""));
                }
                Some("image") => {
                    use base64::Engine as _;
                    if let Some(d) = c["data"].as_str()
                        && let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(d)
                    {
                        images.push(bytes);
                    }
                }
                _ => {}
            }
        }
        let json = serde_json::from_str::<Value>(&text).ok();
        self.calls.push(Call { tool: tool.to_string(), args: args.clone(), ok, text: text.clone(), ms: t.elapsed().as_millis() });
        Ok(Reply { ok, text, json, images })
    }

    /// `command_run` with only an id and params.
    pub async fn run(&mut self, id: &str, params: Value) -> Result<Reply, String> {
        self.call("command_run", json!({"id": id, "params": params})).await
    }
}
