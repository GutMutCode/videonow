//! Unknown-parameter checks for engine commands.
//!
//! Each engine describes a command's parameters in a free-form `params` doc. A misspelled key
//! used to be ignored by most commands, which then ran on their defaults and reported success
//! (gap analysis P2). The studio checks keys against the doc before any engine runs:
//!
//! * **motion** — EffectCraft already rejects undocumented keys itself, for every command.
//! * **timeline**, **image** — FilmCraft and PhotoCraft docs are JSON-like (`{"name":str,…}`).
//!   When a doc is closed (no top-level `…`), a key it does not name is an error that lists the
//!   accepted keys. When it is open-ended, keys are not checked, and the receipt says so.
//!
//! Some keys are accepted by many commands without being in each doc (FilmCraft's alternative
//! time spellings, PhotoCraft's history-coalescing key). Those are listed below; they were
//! found by checking every engine test call against the docs (`tests/params_corpus.rs`).

use serde_json::Value;

/// What the check did, for the receipt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyCheck {
    /// Every key was found in the command's params doc.
    Checked,
    /// The engine checks keys itself.
    ByEngine,
    /// The doc is open-ended: keys were not checked.
    Open,
}

/// Keys any FilmCraft command accepts besides its doc.
pub const TIMELINE_UNIVERSAL: &[&str] = &[
    // a time can always be given in ticks, seconds, frames or timecode (docs/agents.md)
    "seconds", "frame", "timecode", "ticks",
    // background jobs: wait for the result or return the job id at once
    "wait",
    // explicit targets instead of the selection (docs/agents.md, "named targets")
    "clip", "clips", "item", "items",
];

/// Keys any PhotoCraft command accepts besides its doc (`param_check.rs` UNIVERSAL, plus the
/// explicit `layer` target most layer commands take).
pub const IMAGE_UNIVERSAL: &[&str] = &["coalesce", "target", "layer"];

/// Commands whose params are an open map (any key is data): never key-checked.
pub const OPEN_MAP: &[(&str, &str)] = &[("image", "file.openExrParts")];

/// Keys these commands take that their params doc does not name yet, found by
/// `tests/params_corpus.rs` (engine test calls). Fixing the docs in the engines removes rows.
pub const DOC_GAPS: &[(&str, &str, &[&str])] = &[
    ("image", "edit.checkSpelling", &["end", "replace", "start", "word"]),
    ("image", "edit.deleteAndFillSelection", &["output", "sampling"]),
    ("image", "edit.fillForeground", &["color", "opacity"]),
    ("image", "edit.presets.migratePresets", &["path"]),
    ("image", "edit.transform.splitWarpCrosswise", &["style"]),
    ("image", "file.export.renderVideo", &["dir", "format"]),
    ("image", "file.export.saveForWebLegacy", &["height", "percent", "width"]),
    ("image", "file.import.variableDataSets", &["delimiter", "path"]),
    ("image", "file.import.videoFramesToLayers", &["path"]),
    ("image", "file.printOneCopy", &["dryRun", "output"]),
    ("image", "image.adjustments.colorLookup", &["fileName"]),
    ("image", "image.analysis.recordMeasurements", &["dateTime"]),
    ("image", "image.applyDataSet", &["name"]),
    ("image", "image.applyImage", &["sourceLayer"]),
    ("image", "image.trap", &["width"]),
    ("image", "image.variables.dataSets", &["dataSets"]),
    ("image", "image.variables.define", &["defs"]),
    ("image", "layer.new.artboard", &["height", "width", "x", "y"]),
    ("image", "layer.videoLayers.newVideoLayerFromFile", &["fps", "path"]),
    ("image", "paint.cloneStamp", &["rotation"]),
    ("image", "path.stroke", &["strength"]),
    ("image", "plugin.run", &["amount"]),
    ("image", "prefs.reset", &["section"]),
    ("image", "select.convertToShape", &["fill", "name", "simplifyTolerance"]),
    ("image", "shape.create", &["radius"]),
    ("image", "shape.edit", &["radius"]),
    ("image", "slice.set", &["cellText", "cellTextIsHtml", "name"]),
    ("image", "timeline.create", &["duration", "fps"]),
    ("image", "timeline.setFrame", &["frame"]),
    ("image", "timeline.setProps", &["fps", "workEnd", "workStart"]),
    ("timeline", "effects.apply", &["edge"]),
    ("timeline", "effects.toggleAnimation", &["time"]),
    ("timeline", "file.exportMedia", &["gpuRendering"]),
    ("timeline", "file.newColorMatte", &["height", "name", "width"]),
    ("timeline", "graphics.newVerticalText", &["newClip"]),
    ("timeline", "graphics.setResponsiveTime", &["introSeconds", "outroSeconds"]),
    ("timeline", "project.freeform.move", &["width"]),
    ("timeline", "timeline.rateStretch", &["deltaFrames"]),
    ("timeline", "timeline.roll", &["followLinks"]),
];

/// The keys a params doc names (at any depth), or `None` when the doc is open-ended or not a
/// braced list. Same reading as PhotoCraft's own `param_check::documented_keys`.
pub fn documented_keys(doc: &str) -> Option<Vec<&str>> {
    if !doc.trim_start().starts_with('{') {
        return None;
    }
    let bytes = doc.as_bytes();
    let (mut depth, mut in_str, mut start) = (0usize, false, 0usize);
    let mut quoted: Vec<(usize, usize)> = Vec::new();
    for (i, &b) in bytes.iter().enumerate() {
        if in_str {
            if b == b'"' {
                in_str = false;
                quoted.push((start, i));
            }
            continue;
        }
        match b {
            b'"' => {
                in_str = true;
                start = i + 1;
            }
            b'{' | b'[' => depth += 1,
            b'}' | b']' => depth = depth.saturating_sub(1),
            _ => {}
        }
        if depth == 1 && doc.get(i..).is_some_and(|r| r.starts_with('…') || r.starts_with("...")) {
            return None;
        }
    }
    let mut keys = Vec::new();
    for (n, &(a, b)) in quoted.iter().enumerate() {
        let Some(word) = doc.get(a..b) else { continue };
        if word.is_empty() || !word.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_') {
            continue;
        }
        let mut end = b + 1;
        let mut k = n;
        loop {
            let rest = doc.get(end..).unwrap_or("");
            if rest.starts_with(':') {
                keys.push(word);
                break;
            }
            let Some(after) = rest.strip_prefix(',') else { break };
            let Some(&(na, nb)) = quoted.get(k + 1) else { break };
            if na != end + 2 || after.as_bytes().first() != Some(&b'"') {
                break;
            }
            end = nb + 1;
            k += 1;
        }
    }
    if keys.is_empty() && doc.contains('"') {
        // quoted words but none reads as a key: not a doc this parser understands
        return None;
    }
    Some(keys)
}

/// Check `params` of `domain:cmd` against its doc. `Err` names the unknown key and the accepted ones.
pub fn check(domain: &str, cmd: &str, doc: &str, params: &Value) -> Result<KeyCheck, String> {
    let universal: &[&str] = match domain {
        "motion" => return Ok(KeyCheck::ByEngine),
        "timeline" => TIMELINE_UNIVERSAL,
        "image" => IMAGE_UNIVERSAL,
        _ => return Ok(KeyCheck::Open),
    };
    if OPEN_MAP.iter().any(|(d, c)| *d == domain && *c == cmd) {
        return Ok(KeyCheck::Open);
    }
    let Some(keys) = documented_keys(doc) else { return Ok(KeyCheck::Open) };
    let Some(map) = params.as_object() else { return Ok(KeyCheck::Checked) };
    let gaps: &[&str] = DOC_GAPS.iter().find(|(d, c, _)| *d == domain && *c == cmd).map(|(_, _, k)| *k).unwrap_or(&[]);
    for k in map.keys() {
        if !keys.contains(&k.as_str()) && !universal.contains(&k.as_str()) && !gaps.contains(&k.as_str()) {
            let mut accepted: Vec<&str> = keys.clone();
            accepted.sort_unstable();
            accepted.dedup();
            let list = if accepted.is_empty() { "none".to_string() } else { accepted.join(", ") };
            return Err(format!("unknown parameter `{k}` (expected: {list})"));
        }
    }
    Ok(KeyCheck::Checked)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn closed_docs_name_their_keys() {
        let doc = r#"{"name":str,"width":u32=1920,"fromItems":[itemId]?}"#;
        assert_eq!(documented_keys(doc), Some(vec!["name", "width", "fromItems"]));
        assert_eq!(check("timeline", "x", doc, &json!({"name": "A", "seconds": 2})), Ok(KeyCheck::Checked));
        let e = check("timeline", "x", doc, &json!({"nmae": "A"})).unwrap_err();
        assert!(e.contains("`nmae`") && e.contains("fromItems, name, width"), "{e}");
    }

    #[test]
    fn open_or_unbraced_docs_are_not_checked() {
        assert_eq!(documented_keys(r#"{"a":1,…}"#), None);
        assert_eq!(documented_keys("{} (reloads the saved file)"), Some(vec![]));
        assert_eq!(check("image", "x", "free text", &json!({"x": 1})), Ok(KeyCheck::Open));
        assert_eq!(check("motion", "x", "{text?}", &json!({"anything": 1})), Ok(KeyCheck::ByEngine));
    }

    #[test]
    fn universal_keys_pass() {
        assert_eq!(check("image", "x", r#"{"amount":0..100}"#, &json!({"amount": 5, "coalesce": "drag"})), Ok(KeyCheck::Checked));
    }
}
