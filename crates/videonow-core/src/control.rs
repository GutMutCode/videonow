//! Requests from outside the process: the MCP server, the desktop app's control channel and the
//! bridge between them all speak these JSON requests, so a tool call means the same thing whether
//! the studio runs headless or in the window a person is looking at.
//!
//! ```json
//! {"op": "execute", "id": "studio:comp.new", "params": {"name": "Title"}}
//! {"op": "batch", "steps": [{"id": "…", "params": {}}], "stopOnError": true}
//! {"op": "preview", "seconds": 1.5, "maxSide": 960}
//! ```
//!
//! `execute` answers `{result, receipt}`; `batch` answers `{completed, failed, results}`;
//! `preview` answers `{png (base64), width, height, offline}`.

use base64::Engine as _;
use serde_json::{Value, json};

use crate::manifest::{self, Effect};
use crate::{Result, Studio, StudioError};

/// One request, parsed.
#[derive(Clone, Debug)]
pub enum Request {
    Execute { id: String, params: Value },
    Batch { steps: Vec<(String, Value)>, stop_on_error: bool },
    Preview { seconds: f64, max_side: u32 },
}

impl Request {
    pub fn parse(v: &Value) -> Result<Request> {
        let bad = |msg: &str| StudioError::BadParams { cmd: "request".into(), msg: msg.into() };
        let o = v.as_object().ok_or_else(|| bad("a request is a JSON object with `op`"))?;
        let allowed: &[&str] = match o.get("op").and_then(Value::as_str) {
            Some("execute") => &["op", "id", "params"],
            Some("batch") => &["op", "steps", "stopOnError"],
            Some("preview") => &["op", "seconds", "maxSide"],
            Some(other) => return Err(bad(&format!("unknown op `{other}` (execute, batch, preview)"))),
            None => return Err(bad("missing `op` (execute, batch, preview)")),
        };
        if let Some(k) = o.keys().find(|k| !allowed.contains(&k.as_str())) {
            return Err(bad(&format!("unknown key `{k}` (expected: {})", allowed.join(", "))));
        }
        let step = |s: &Value| -> Result<(String, Value)> {
            let id = s.get("id").and_then(Value::as_str).ok_or_else(|| bad("each step needs an `id`"))?;
            Ok((id.to_string(), s.get("params").cloned().unwrap_or(Value::Null)))
        };
        Ok(match o.get("op").and_then(Value::as_str) {
            Some("execute") => {
                let (id, params) = step(v)?;
                Request::Execute { id, params }
            }
            Some("batch") => {
                let steps = o.get("steps").and_then(Value::as_array).ok_or_else(|| bad("`steps` must be an array"))?;
                if steps.len() > 10_000 {
                    return Err(bad("at most 10000 steps in one batch"));
                }
                Request::Batch { steps: steps.iter().map(step).collect::<Result<_>>()?, stop_on_error: o.get("stopOnError").and_then(Value::as_bool).unwrap_or(true) }
            }
            _ => Request::Preview {
                seconds: o.get("seconds").and_then(Value::as_f64).filter(|s| s.is_finite()).unwrap_or(0.0),
                max_side: o.get("maxSide").and_then(Value::as_u64).map_or(960, |m| m.clamp(16, 4096) as u32),
            },
        })
    }

    /// The request as JSON, the form [`Request::parse`] reads.
    pub fn to_value(&self) -> Value {
        match self {
            Request::Execute { id, params } => json!({"op": "execute", "id": id, "params": params}),
            Request::Batch { steps, stop_on_error } => {
                json!({"op": "batch", "steps": steps.iter().map(|(id, p)| json!({"id": id, "params": p})).collect::<Vec<_>>(), "stopOnError": stop_on_error})
            }
            Request::Preview { seconds, max_side } => json!({"op": "preview", "seconds": seconds, "maxSide": max_side}),
        }
    }

    /// The most consequential effect class this request may have: what a person approving it
    /// needs to know. A batch is as strong as its strongest step.
    pub fn effect(&self) -> Effect {
        let of = |id: &str| manifest::find(id).map_or(Effect::Edit, |c| c.effect);
        match self {
            Request::Execute { id, .. } => of(id),
            Request::Batch { steps, .. } => steps.iter().map(|(id, _)| of(id)).max_by_key(|e| rank(*e)).unwrap_or(Effect::Read),
            Request::Preview { .. } => Effect::Read,
        }
    }

    /// One line for a person: `studio:comp.new {"name":"Title"}`.
    pub fn summary(&self) -> String {
        let short = |p: &Value| {
            let s = if p.is_null() { String::new() } else { p.to_string() };
            if s.chars().count() > 120 { format!(" {}…", s.chars().take(120).collect::<String>()) } else if s.is_empty() { s } else { format!(" {s}") }
        };
        match self {
            Request::Execute { id, params } => format!("{id}{}", short(params)),
            Request::Batch { steps, .. } => format!("batch of {} ({})", steps.len(), steps.iter().take(3).map(|(id, _)| id.as_str()).collect::<Vec<_>>().join(", ")),
            Request::Preview { seconds, .. } => format!("preview at {seconds}s"),
        }
    }
}

/// How much a class can lose: read < edit < fileWrite < destructive < external.
pub fn rank(e: Effect) -> u8 {
    match e {
        Effect::Read => 0,
        Effect::Edit => 1,
        Effect::FileWrite => 2,
        Effect::Destructive => 3,
        Effect::External => 4,
    }
}

/// Run a request on the studio.
pub fn handle(s: &mut Studio, req: &Request) -> Result<Value> {
    match req {
        Request::Execute { id, params } => s.execute(id, params.clone()),
        Request::Batch { steps, stop_on_error } => {
            let (mut completed, mut failed, mut results) = (0u32, 0u32, Vec::new());
            for (id, params) in steps {
                match s.execute(id, params.clone()) {
                    Ok(v) => {
                        completed += 1;
                        results.push(json!({"ok": true, "id": id, "result": v["result"], "receipt": v["receipt"]}));
                    }
                    Err(e) => {
                        failed += 1;
                        results.push(json!({"ok": false, "id": id, "error": e.to_string()}));
                        if *stop_on_error {
                            break;
                        }
                    }
                }
            }
            Ok(json!({"completed": completed, "failed": failed, "results": results}))
        }
        Request::Preview { seconds, max_side } => {
            let full = s.timeline.active_sequence().map(|q| q.settings.width.max(q.settings.height)).unwrap_or(1920).max(1);
            let scale = (f64::from(*max_side) / f64::from(full)).min(1.0);
            let (w, h, rgba, offline) = s.render_rgba(*seconds, scale)?;
            let mut png = Vec::new();
            image::write_buffer_with_format(&mut std::io::Cursor::new(&mut png), &rgba, w, h, image::ExtendedColorType::Rgba8, image::ImageFormat::Png)
                .map_err(|e| StudioError::Other(format!("preview: {e}")))?;
            Ok(json!({"png": base64::engine::general_purpose::STANDARD.encode(&png), "width": w, "height": h, "seconds": seconds, "offline": offline}))
        }
    }
}

/// Parse and run; errors become `{"ok": false, "error": …}` so a client always gets one line back.
pub fn handle_value(s: &mut Studio, v: &Value) -> Value {
    match Request::parse(v).and_then(|r| handle(s, &r)) {
        Ok(result) => json!({"ok": true, "result": result}),
        Err(e) => json!({"ok": false, "error": e.to_string()}),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_parse_strictly() {
        assert!(Request::parse(&json!({"op": "execute", "id": "studio:inspect"})).is_ok());
        assert!(Request::parse(&json!({"op": "execute", "id": "studio:inspect", "parms": {}})).is_err(), "typo in a key");
        assert!(Request::parse(&json!({"op": "nope"})).is_err());
        assert!(Request::parse(&json!({"op": "batch", "steps": [{"params": {}}]})).is_err(), "a step without an id");
    }

    #[test]
    fn requests_round_trip() {
        for v in [json!({"op": "execute", "id": "a:b", "params": {"x": 1}}), json!({"op": "batch", "steps": [{"id": "a:b", "params": null}], "stopOnError": false}), json!({"op": "preview", "seconds": 1.5, "maxSide": 300})] {
            assert_eq!(Request::parse(&v).unwrap().to_value(), v);
        }
    }

    #[test]
    fn effect_of_a_batch_is_its_strongest_step() {
        let r = Request::parse(&json!({"op": "batch", "steps": [{"id": "studio:inspect"}, {"id": "studio:project.new"}, {"id": "studio:comp.new"}]})).unwrap();
        assert_eq!(r.effect(), Effect::Destructive);
        let r = Request::parse(&json!({"op": "preview"})).unwrap();
        assert_eq!(r.effect(), Effect::Read);
    }

    #[test]
    fn handle_value_always_answers() {
        let mut s = Studio::new();
        let v = handle_value(&mut s, &json!({"op": "execute", "id": "studio:comp.new", "params": {"name": "A"}}));
        assert_eq!(v["ok"], true, "{v}");
        let v = handle_value(&mut s, &json!({"op": "execute", "id": "nope:x"}));
        assert_eq!(v["ok"], false);
        assert!(v["error"].as_str().unwrap().contains("unknown command"));
    }
}
