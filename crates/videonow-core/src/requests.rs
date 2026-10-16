//! What a client can ask of a command besides running it (gap analysis P5–P8):
//!
//! * **request ids** — a retried request (the reply was lost) returns the first answer instead of
//!   doing the work twice; the receipt says `replayed`.
//! * **expected revision** — a change made against an older revision of the project is refused
//!   (someone else, a person in an editor or another agent, changed it meanwhile), with what
//!   changed since, instead of being applied over their work.
//! * **atomic batches** — several commands that only make sense together: if one fails, the ones
//!   before it are taken back. A step can use an earlier step's result: `"$0.asset"`.
//! * **dry runs** — run, report what would change, and take it back.
//!
//! Taking back uses each engine's own history (`revert_to` on the timeline, undo on the motion
//! and image engines) and restores the asset table; the receipt says whether the documents are
//! exactly as before (`restored`).
//! File writes, external operations, project replacement, history manipulation and opaque
//! command dispatch are refused before a dry run, preview or atomic batch starts. Run those
//! operations explicitly, or in a non-atomic batch without a dry run.

use std::collections::HashSet;
use std::sync::Arc;

use serde_json::{Value, json};

use crate::assets::{Asset, AssetId};
use crate::manifest::{self, Effect};
use crate::studio::{EngineMarks, Studio};
use crate::{Result, StudioError};

/// How a command is to be run.
#[derive(Clone, Debug, Default)]
pub struct Request {
    /// A client's id for this request: the same id with the same command and parameters returns
    /// the first answer (the last 256 are remembered).
    pub request_id: Option<String>,
    /// Refuse unless the project is at this revision.
    pub expect_revision: Option<u64>,
    /// Run, report, take back. Operations outside the document checkpoint are refused first.
    pub dry_run: bool,
}

/// Remembered answers to requests with an id.
pub(crate) const REPLIES: usize = 256;

/// The project as it was before a batch or a dry run, to take it back to.
pub(crate) struct Checkpoint {
    timeline_mark: filmcraft_engine::UndoMark,
    timeline_project: Arc<filmcraft_engine::project::Project>,
    motion_undo: usize,
    motion_project: Arc<effectcraft_project::Project>,
    /// Open image documents: id, history length, document.
    images: Vec<(photocraft_doc::DocId, usize, Arc<photocraft_doc::Document>)>,
    assets: std::collections::BTreeMap<AssetId, Asset>,
    image_docs: std::collections::BTreeMap<AssetId, photocraft_doc::DocId>,
    parked: std::collections::BTreeMap<AssetId, Arc<photocraft_doc::Document>>,
    next_asset: u64,
    undo: usize,
    journal: usize,
    revision: u64,
    last: EngineMarks,
    was_dirty: bool,
}

fn fingerprint(id: &str, params: &Value) -> String {
    format!("{id} {params}")
}

fn check_rollback_command(id: &str) -> Result<()> {
    let Some((domain, command)) = id.split_once(':') else { return Ok(()) };
    let reason = if matches!(manifest::effect_of(domain, command), Some(Effect::FileWrite | Effect::External)) {
        Some("file writes and external operations")
    } else if matches!(id, "studio:project.open" | "studio:project.new") || manifest::is_host_command(domain, command) {
        Some("project or document replacement")
    } else if matches!(
        id,
        "studio:undo"
            | "timeline:edit.undo"
            | "timeline:edit.redo"
            | "motion:edit.undo"
            | "motion:edit.redo"
            | "motion:edit.history"
            | "motion:edit.history.goto"
            | "motion:edit.purgeUndo"
            | "motion:edit.purge"
            | "image:edit.undo"
            | "image:edit.redo"
            | "image:history.undo"
            | "image:history.redo"
            | "image:history.deleteState"
            | "image:history.newDocument"
            | "image:history.newSnapshot"
            | "image:history.selectSnapshot"
            | "image:history.renameSnapshot"
            | "image:history.deleteSnapshot"
            | "image:history.selectState"
            | "image:edit.purge.undo"
            | "image:edit.purge.histories"
            | "image:edit.purge.all"
    ) {
        Some("history manipulation")
    } else if matches!(
        id,
        "studio:batch"
            | "motion:engine.batch"
            | "motion:file.runScript"
            | "motion:script.run"
            | "motion:script.runTasks"
            | "motion:window.scriptPanel"
            | "motion:window.restoreScriptPanels"
            | "motion:scriptui.click"
            | "motion:scriptui.set"
            | "motion:scriptui.mouse"
            | "motion:scriptui.close"
            | "image:actions.play"
            | "image:actions.conditional"
            | "image:file.automate.batch"
            | "image:file.automate.createDroplet"
            | "image:file.automate.runDroplet"
            | "image:file.scripts.imageProcessor"
            | "image:file.scripts.scriptEventsManager"
    ) {
        Some("nested command dispatch and automation")
    } else {
        None
    };
    match reason {
        Some(reason) => Err(StudioError::BadParams {
            cmd: id.to_string(),
            msg: format!(
                "{reason} cannot be rolled back by dry runs, previews or atomic batches; run this command explicitly without a dry run, or use atomic:false. Use explicit studio:batch steps instead of nested command dispatch"
            ),
        }),
        None => Ok(()),
    }
}

fn check_rollback_steps(steps: &[(String, Value)]) -> Result<()> {
    for (step, (id, _)) in steps.iter().enumerate() {
        check_rollback_command(id).map_err(|error| StudioError::BatchFailed {
            step,
            id: id.clone(),
            error: error.to_string(),
            rolled_back: "no steps were run".into(),
        })?;
    }
    Ok(())
}

impl Studio {
    pub(crate) fn checkpoint(&self) -> Checkpoint {
        Checkpoint {
            timeline_mark: self.timeline.undo_mark(),
            timeline_project: self.timeline.project.clone(),
            motion_undo: self.motion.history.undo.len(),
            motion_project: self.motion.project.clone(),
            images: self.images.documents().iter().map(|d| (d.doc.id, d.history.past_len(), d.doc.clone())).collect(),
            assets: self.assets.clone(),
            image_docs: self.image_docs.clone(),
            parked: self.parked.clone(),
            next_asset: self.next_asset,
            undo: self.undo.len(),
            journal: self.journal.len(),
            revision: self.revision,
            last: self.last.clone(),
            was_dirty: self.is_dirty(),
        }
    }

    /// Take the project back to `cp`. `keep_record`: leave the journal and revision as they are
    /// (a failed batch is history); otherwise they go back too (a dry run never happened).
    /// Returns whether every document is exactly as it was.
    pub(crate) fn rollback(&mut self, cp: Checkpoint, keep_record: bool) -> bool {
        // timeline: the engine's own revert (merged steps too; nothing left to redo)
        if !Arc::ptr_eq(&self.timeline.project, &cp.timeline_project) {
            self.timeline.revert_to(cp.timeline_mark);
        }
        // motion: undo the steps taken since, then forget them
        let mut guard = 0;
        while self.motion.history.undo.len() > cp.motion_undo && guard < 100_000 {
            if self.motion.execute("edit.undo", json!({})).is_err() {
                break;
            }
            guard += 1;
        }
        if Arc::ptr_eq(&self.motion.project, &cp.motion_project) {
            self.motion.history.redo.clear();
        }
        // images: documents opened since are closed; the others undo back
        let before: HashSet<photocraft_doc::DocId> = cp.images.iter().map(|(d, _, _)| *d).collect();
        let mut i = self.images.documents().len();
        while i > 0 {
            i -= 1;
            if self.images.documents().get(i).is_some_and(|d| !before.contains(&d.doc.id)) {
                self.images.close(i);
            }
        }
        let mut exact = Arc::ptr_eq(&self.timeline.project, &cp.timeline_project) && Arc::ptr_eq(&self.motion.project, &cp.motion_project);
        let active = self.images.active().map(|d| d.doc.id);
        for (doc, past, arc) in &cp.images {
            match self.images.documents().iter().position(|d| d.doc.id == *doc) {
                Some(idx) => {
                    if !Arc::ptr_eq(&self.images.documents()[idx].doc, arc) {
                        self.images.set_active(idx);
                        let mut guard = 0;
                        while self.images.active().is_some_and(|d| d.history.past_len() > *past) && guard < 100_000 {
                            if self.images.execute("edit.undo", json!({})).is_err() {
                                break;
                            }
                            guard += 1;
                        }
                        if let Some(d) = self.images.active_mut() {
                            d.history.clear_redo();
                        }
                    }
                    exact &= self.images.documents().get(idx).is_some_and(|d| Arc::ptr_eq(&d.doc, arc));
                }
                None => {
                    // closed by a step: bring it back as it was
                    let idx = self.images.add_document((**arc).clone(), None);
                    if let Some(new) = self.images.documents().get(idx).map(|d| d.doc.id)
                        && let Some((asset, _)) = cp.image_docs.iter().find(|(_, d)| **d == *doc)
                    {
                        let asset = *asset;
                        let mut docs = cp.image_docs.clone();
                        docs.insert(asset, new);
                        self.image_docs = docs;
                    }
                    exact = false;
                }
            }
        }
        if let Some(a) = active
            && let Some(idx) = self.images.documents().iter().position(|d| d.doc.id == a)
        {
            self.images.set_active(idx);
        }
        // the asset table
        self.assets = cp.assets;
        if self.image_docs.len() != cp.image_docs.len() || self.image_docs.iter().any(|(a, _)| !cp.image_docs.contains_key(a)) {
            self.image_docs = cp.image_docs;
        }
        self.parked = cp.parked;
        self.next_asset = cp.next_asset;
        self.undo.truncate(cp.undo);
        self.publish();
        if keep_record {
            self.last = self.marks();
            self.seen = self.journal_marks();
        } else {
            self.journal.truncate(cp.journal);
            self.revision = cp.revision;
            self.last = cp.last;
            self.seen = self.journal_marks();
            // the engines' own counters moved; what the studio compares against is the documents
            if exact {
                self.last = self.marks();
            }
        }
        if exact && !cp.was_dirty {
            self.saved = self.marks();
        }
        exact
    }

    /// A summary of the journal after `revision`: who changed what.
    fn since(&self, revision: u64) -> String {
        let mut parts: Vec<String> = self
            .journal
            .iter()
            .filter(|e| e["receipt"]["revision"]["after"].as_u64().is_some_and(|r| r > revision))
            .map(|e| {
                let by = e["receipt"]["by"].as_str().unwrap_or("api");
                let cmd = e["command"].as_str().unwrap_or("?");
                let changed: Vec<&str> = e["receipt"]["changed"].as_array().map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
                format!("{cmd} by {by} (changed {})", changed.join(", "))
            })
            .collect();
        if parts.len() > 8 {
            let n = parts.len() - 8;
            parts.truncate(8);
            parts.push(format!("and {n} more"));
        }
        if parts.is_empty() { "changes not in the journal".into() } else { parts.join("; ") }
    }

    fn check_request(&self, id: &str, params: &Value, req: &Request) -> Result<Option<Value>> {
        if let Some(rid) = &req.request_id {
            let fp = fingerprint(id, params);
            if let Some((_, f, reply)) = self.replies.iter().find(|(r, _, _)| r == rid) {
                if *f != fp {
                    return Err(StudioError::BadParams {
                        cmd: id.to_string(),
                        msg: format!("request id `{rid}` was already used for another command or other parameters; use a new id for a new request"),
                    });
                }
                let mut v = reply.clone();
                v["receipt"]["replayed"] = json!(true);
                return Ok(Some(v));
            }
        }
        if let Some(expected) = req.expect_revision
            && expected != self.revision
        {
            return Err(StudioError::Stale { expected, actual: self.revision, since: self.since(expected) });
        }
        Ok(None)
    }

    fn remember(&mut self, id: &str, params: &Value, req: &Request, reply: &Value) {
        if let Some(rid) = &req.request_id {
            self.replies.push_back((rid.clone(), fingerprint(id, params), reply.clone()));
            while self.replies.len() > REPLIES {
                self.replies.pop_front();
            }
        }
    }

    /// [`Studio::execute`] with a [`Request`]: replay, revision check, dry run.
    pub fn execute_with(&mut self, id: &str, params: Value, req: &Request) -> Result<Value> {
        if id == "studio:batch" {
            return self.batch_command(params, req);
        }
        if req.dry_run {
            check_rollback_command(id)?;
        }
        if let Some(replay) = self.check_request(id, &params, req)? {
            return Ok(replay);
        }
        let original = params.clone();
        if req.dry_run {
            if self.marks() != self.last {
                self.sync_editors("external");
            }
            let cp = self.checkpoint();
            let r = self.execute(id, params);
            let exact = self.rollback(cp, false);
            let mut v = r?;
            v["receipt"]["dryRun"] = json!(true);
            v["receipt"]["restored"] = json!(exact);
            v["receipt"]["revision"]["after"] = v["receipt"]["revision"]["before"].clone();
            return Ok(v);
        }
        let v = self.execute(id, params)?;
        self.remember(id, &original, req, &v);
        Ok(v)
    }

    /// `studio:batch`: `{"steps": [{"id", "params"}], "atomic": true}`.
    pub(crate) fn batch_command(&mut self, params: Value, req: &Request) -> Result<Value> {
        let bad = |m: &str| StudioError::BadParams { cmd: "studio:batch".into(), msg: m.into() };
        let obj = params.as_object().ok_or_else(|| bad("params must be an object"))?;
        for k in obj.keys() {
            if !matches!(k.as_str(), "steps" | "atomic") {
                return Err(bad(&format!("unknown parameter `{k}` (expected: atomic, steps)")));
            }
        }
        let steps: Vec<(String, Value)> = obj
            .get("steps")
            .and_then(Value::as_array)
            .ok_or_else(|| bad("`steps` must be a list of {\"id\": \"domain:command\", \"params\": {…}}"))?
            .iter()
            .map(|s| Ok((s["id"].as_str().ok_or_else(|| bad("every step needs an `id`"))?.to_string(), s.get("params").cloned().unwrap_or(json!({})))))
            .collect::<Result<_>>()?;
        let atomic = obj.get("atomic").and_then(Value::as_bool).unwrap_or(true);
        self.batch(steps, atomic, req)
    }

    /// Run `steps` in order. A string parameter `"$N"` or `"$N.path.to.value"` is replaced by
    /// step N's result (or part of it). `atomic`: when a step fails, the ones before it are taken
    /// back. Atomic batches and dry runs preflight every step for operations outside the
    /// document checkpoint. Returns `{"completed", "results": [{id, result, receipt}]}`.
    pub fn batch(&mut self, steps: Vec<(String, Value)>, atomic: bool, req: &Request) -> Result<Value> {
        if atomic || req.dry_run {
            check_rollback_steps(&steps)?;
        }
        let fp_params = json!(steps.iter().map(|(i, p)| json!([i, p])).collect::<Vec<_>>());
        if let Some(replay) = self.check_request("studio:batch", &fp_params, req)? {
            return Ok(replay);
        }
        if steps.is_empty() {
            return Err(StudioError::BadParams { cmd: "studio:batch".into(), msg: "`steps` is empty".into() });
        }
        if self.marks() != self.last {
            self.sync_editors("external");
        }
        let rev0 = self.revision;
        let cp = (atomic || req.dry_run).then(|| self.checkpoint());
        let mut results: Vec<Value> = Vec::new();
        let mut failure = None;
        for (n, (id, params)) in steps.iter().enumerate() {
            if id == "studio:batch" {
                failure = Some((n, id.clone(), "a batch cannot contain a batch".to_string()));
                break;
            }
            let p = match substitute(params, &results) {
                Ok(p) => p,
                Err(e) => {
                    failure = Some((n, id.clone(), e));
                    break;
                }
            };
            match self.execute(id, p) {
                Ok(v) => results.push(json!({"id": id, "result": v["result"], "receipt": v["receipt"]})),
                Err(e) => {
                    failure = Some((n, id.clone(), e.to_string()));
                    break;
                }
            }
        }
        if let Some((step, id, error)) = failure {
            let rolled_back = match cp {
                Some(cp) => {
                    let exact = self.rollback(cp, false);
                    if exact {
                        "the steps before it were taken back".to_string()
                    } else {
                        "the steps before it were taken back, but not every document is exactly as before".to_string()
                    }
                }
                None => format!("the {step} steps before it stay applied (not atomic)"),
            };
            return Err(StudioError::BatchFailed { step, id, error, rolled_back });
        }
        let changed: Vec<Value> = {
            let mut c: Vec<Value> = results.iter().flat_map(|r| r["receipt"]["changed"].as_array().cloned().unwrap_or_default()).collect();
            c.dedup();
            let mut seen = Vec::new();
            c.retain(|x| {
                let keep = !seen.contains(x);
                seen.push(x.clone());
                keep
            });
            c
        };
        let mut v = json!({
            "completed": results.len(),
            "results": results,
            "receipt": {"command": "studio:batch", "by": self.actor, "revision": {"before": rev0, "after": self.revision}, "changed": changed, "atomic": atomic},
        });
        if req.dry_run {
            if let Some(cp) = cp {
                let exact = self.rollback(cp, false);
                v["receipt"]["dryRun"] = json!(true);
                v["receipt"]["restored"] = json!(exact);
                v["receipt"]["revision"]["after"] = json!(rev0);
            }
            return Ok(v);
        }
        self.remember("studio:batch", &fp_params, req, &v);
        Ok(v)
    }

    /// Run `steps`, call `look` on the result, take everything back: what a change would look
    /// like (a preview render) without keeping it. Operations outside the document checkpoint
    /// are refused before any step runs or `look` is called.
    pub fn preview_with<T>(&mut self, steps: Vec<(String, Value)>, look: impl FnOnce(&mut Studio) -> T) -> Result<(T, bool)> {
        check_rollback_steps(&steps)?;
        if self.marks() != self.last {
            self.sync_editors("external");
        }
        let cp = self.checkpoint();
        let mut results: Vec<Value> = Vec::new();
        for (n, (id, params)) in steps.iter().enumerate() {
            let p = match substitute(params, &results) {
                Ok(p) => p,
                Err(e) => {
                    self.rollback(cp, false);
                    return Err(StudioError::BatchFailed { step: n, id: id.clone(), error: e, rolled_back: "nothing was kept".into() });
                }
            };
            match self.execute(id, p) {
                Ok(v) => results.push(json!({"result": v["result"]})),
                Err(e) => {
                    self.rollback(cp, false);
                    return Err(StudioError::BatchFailed { step: n, id: id.clone(), error: e.to_string(), rolled_back: "nothing was kept".into() });
                }
            }
        }
        let t = look(self);
        let exact = self.rollback(cp, false);
        Ok((t, exact))
    }
}

/// Replace `"$N"` / `"$N.a.b.0"` strings in `params` with parts of earlier steps' results.
pub(crate) fn substitute(params: &Value, results: &[Value]) -> std::result::Result<Value, String> {
    match params {
        Value::String(s) => {
            let Some(rest) = s.strip_prefix('$') else { return Ok(params.clone()) };
            let (n, path) = rest.split_once('.').unwrap_or((rest, ""));
            let Ok(n) = n.parse::<usize>() else { return Ok(params.clone()) };
            let mut v = results.get(n).map(|r| &r["result"]).ok_or_else(|| format!("`{s}` refers to step {n}, which has not run (steps count from 0)"))?;
            for part in path.split('.').filter(|p| !p.is_empty()) {
                v = match part.parse::<usize>() {
                    Ok(i) if v.is_array() => &v[i],
                    _ => &v[part],
                };
                if v.is_null() {
                    return Err(format!(
                        "`{s}`: step {n}'s result has no `{part}` (it was {})",
                        results.get(n).map(|r| r["result"].to_string()).unwrap_or_default()
                    ));
                }
            }
            Ok(v.clone())
        }
        Value::Array(a) => a.iter().map(|x| substitute(x, results)).collect::<std::result::Result<Vec<_>, _>>().map(Value::Array),
        Value::Object(o) => {
            let mut m = serde_json::Map::new();
            for (k, x) in o {
                m.insert(k.clone(), substitute(x, results)?);
            }
            Ok(Value::Object(m))
        }
        _ => Ok(params.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn references_to_earlier_results() {
        let results = vec![json!({"result": {"asset": 3, "layers": [7, 8]}})];
        assert_eq!(substitute(&json!({"asset": "$0.asset", "l": "$0.layers.1", "s": "$x"}), &results).ok(), Some(json!({"asset": 3, "l": 8, "s": "$x"})));
        assert!(substitute(&json!("$1.asset"), &results).is_err());
        assert!(substitute(&json!("$0.nothing"), &results).is_err());
    }
}
