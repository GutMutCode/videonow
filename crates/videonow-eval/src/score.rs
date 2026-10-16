//! Scoring a live agent session: any MCP client (a model in Claude Code or Claude Desktop, a
//! script) working through `videonow mcp --log FILE`. The log has every tool call, reads and
//! errors included; the saved project (optional) has the journal and the result.

use std::path::Path;

use serde_json::{Value, json};

/// Process metrics from a tool-call log, and the project's state if a project is given.
pub fn score(log: &Path, project: Option<&Path>) -> Result<Value, String> {
    let text = std::fs::read_to_string(log).map_err(|e| format!("{}: {e}", log.display()))?;
    let calls: Vec<Value> = text.lines().filter_map(|l| serde_json::from_str(l).ok()).collect();
    let id_of = |c: &Value| c["args"]["id"].as_str().map(str::to_string);
    let mut recovered = 0;
    for (i, c) in calls.iter().enumerate() {
        if c["ok"] == json!(false) && c["tool"] == "command_run" {
            let id = id_of(c);
            if calls.iter().skip(i + 1).take(3).any(|n| n["ok"] == json!(true) && n["tool"] == "command_run" && (id_of(n) == id || id.is_none())) {
                recovered += 1;
            }
        }
    }
    let errors = calls.iter().filter(|c| c["ok"] == json!(false)).count();
    let refused = calls.iter().filter(|c| c["ok"] == json!(false) && c["reply"].as_str().is_some_and(|r| r.contains("not allowed"))).count();
    let stale = calls.iter().filter(|c| c["ok"] == json!(false) && c["reply"].as_str().is_some_and(|r| r.contains("changed since revision"))).count();
    let replays = calls.iter().filter(|c| c["reply"].as_str().is_some_and(|r| r.contains("\"replayed\": true"))).count();
    let changes: Vec<usize> = calls
        .iter()
        .enumerate()
        .filter(|(_, c)| c["ok"] == json!(true) && matches!(c["tool"].as_str(), Some("command_run" | "command_batch")) && c["reply"].as_str().is_some_and(|r| r.contains("\"changed\": [\n")))
        .map(|(i, _)| i)
        .collect();
    let last_change = changes.last().copied();
    let looked_after = last_change.is_some_and(|i| calls.iter().skip(i + 1).any(|c| c["tool"] == "render_preview" && c["ok"] == json!(true)));
    let ms: Vec<u64> = calls.iter().filter_map(|c| c["ms"].as_u64()).collect();
    let mut out = json!({
        "calls": calls.len(),
        "errors": errors,
        "recoveredWithinThreeCalls": recovered,
        "refusedByPolicy": refused,
        "refusedAsStale": stale,
        "replayedRequests": replays,
        "changes": changes.len(),
        "lookedAtTheResult": looked_after,
        "totalMs": ms.iter().sum::<u64>(),
        "tools": {
            "command_list": calls.iter().filter(|c| c["tool"] == "command_list").count(),
            "command_run": calls.iter().filter(|c| c["tool"] == "command_run").count(),
            "command_batch": calls.iter().filter(|c| c["tool"] == "command_batch").count(),
            "doc_inspect": calls.iter().filter(|c| c["tool"] == "doc_inspect").count(),
            "render_preview": calls.iter().filter(|c| c["tool"] == "render_preview").count(),
        },
    });
    if let Some(dir) = project {
        let mut s = videonow_core::Studio::new();
        let opened = s.execute("studio:project.open", json!({"path": dir.to_string_lossy()})).map_err(|e| e.to_string())?;
        let inspect = s.execute("studio:inspect", json!({})).map_err(|e| e.to_string())?;
        let journal = s.journal();
        out["project"] = json!({
            "assets": opened["result"]["assets"],
            "inspect": inspect["result"],
            "changesByAgent": journal.iter().filter(|e| e["receipt"]["by"] == "agent").count(),
            "changesByPerson": journal.iter().filter(|e| e["receipt"]["by"] == "ui").count(),
        });
    }
    Ok(out)
}
