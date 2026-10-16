//! Strict parameters for engine commands: a key a command does not know is an error before the
//! command runs, not a silently ignored typo that runs it on its defaults.
//!
//! * `motion:` — EffectCraft already refuses unknown keys itself, for every command.
//! * `timeline:` and `image:` — the studio checks each call against the command's parameter doc
//!   (`"key":…` entries), plus the keys in [`EXTRA`]: keys the engine reads that its doc leaves
//!   out, found by running the engines' own test suites with the parameter audit on
//!   (`VIDEONOW_PARAM_AUDIT`, see `docs/params-audit.md`). A doc that ends in `…` is open-ended
//!   (the command forwards keys elsewhere) and is not checked.

use serde_json::Value;

use crate::{Result, StudioError};

/// Keys every PhotoCraft command accepts (history coalescing, the Channels panel target).
const IMAGE_UNIVERSAL: &[&str] = &["coalesce", "target"];
/// Keys every FilmCraft command accepts.
const TIMELINE_UNIVERSAL: &[&str] = &[];

/// `(domain:id, keys its doc leaves out)`, generated from the engines' tests.
pub use crate::params_extra::EXTRA;

pub fn also_accepts(full_id: &str) -> Vec<&'static str> {
    EXTRA.iter().find(|(id, _)| *id == full_id).map(|(_, k)| k.to_vec()).unwrap_or_default()
}

/// The keys a parameter doc names (at any depth), or `None` when the doc is open-ended.
pub fn documented_keys(doc: &str) -> Option<Vec<&str>> {
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
        // `…` / `...` straight inside the top-level braces: the list is not complete
        if depth == 1 && doc.get(i..).is_some_and(|r| r.starts_with('…') || r.starts_with("...")) && doc.get(..i).is_some_and(|l| l.trim_start().starts_with('{')) {
            return None;
        }
    }
    if !doc.trim_start().starts_with('{') {
        // not a JSON-ish doc at all (free text): nothing can be checked
        return None;
    }
    let mut keys = Vec::new();
    for (n, &(a, b)) in quoted.iter().enumerate() {
        let Some(word) = doc.get(a..b) else { continue };
        if word.is_empty() || !word.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_') {
            continue;
        }
        // a key is followed by `:`, or by `,"other"` pairs that end in one (`"all","pixels":bool`)
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
    Some(keys)
}

/// Whether unknown keys of this command are refused (by the studio or by the engine itself).
pub fn is_strict(domain: &str, _full_id: &str, doc: &str) -> bool {
    match domain {
        "motion" | "studio" => true,
        "timeline" | "image" => documented_keys(doc).is_some(),
        _ => false,
    }
}

/// Refuse a key the command does not know.
pub fn check(domain: &str, full_id: &str, doc: &str, params: &Value) -> Result<()> {
    let universal = match domain {
        "timeline" => TIMELINE_UNIVERSAL,
        "image" => IMAGE_UNIVERSAL,
        _ => return Ok(()),
    };
    let Some(map) = params.as_object() else { return Ok(()) };
    let Some(keys) = documented_keys(doc) else { return Ok(()) };
    let extra = also_accepts(full_id);
    for k in map.keys() {
        let k = k.as_str();
        if keys.contains(&k) || universal.contains(&k) || extra.contains(&k) {
            continue;
        }
        let mut accepted: Vec<&str> = keys.iter().copied().chain(extra.iter().copied()).collect();
        accepted.dedup();
        let expected = if accepted.is_empty() { "it takes no parameters".to_string() } else { format!("expected: {}", accepted.join(", ")) };
        let hint = suggest(k, &accepted).map(|s| format!("; did you mean `{s}`?")).unwrap_or_default();
        return Err(StudioError::BadParams { cmd: full_id.to_string(), msg: format!("unknown parameter `{k}` ({expected}){hint}; nothing ran") });
    }
    Ok(())
}

/// The accepted key closest to a typo (edit distance ≤ 2), if any.
fn suggest<'a>(k: &str, accepted: &[&'a str]) -> Option<&'a str> {
    accepted.iter().map(|a| (lev(&k.to_ascii_lowercase(), &a.to_ascii_lowercase()), *a)).filter(|(d, _)| *d <= 2).min_by_key(|(d, _)| *d).map(|(_, a)| a)
}

fn lev(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut cur = vec![i + 1];
        for (j, cb) in b.iter().enumerate() {
            let sub = prev.get(j).copied().unwrap_or(0) + usize::from(ca != *cb);
            let del = prev.get(j + 1).copied().unwrap_or(0) + 1;
            let ins = cur.get(j).copied().unwrap_or(0) + 1;
            cur.push(sub.min(del).min(ins));
        }
        prev = cur;
    }
    prev.last().copied().unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn documented_keys_reads_keys_and_stops_at_open_docs() {
        assert_eq!(documented_keys(r##"{"layer":id?} (shows them instead)"##), Some(vec!["layer"]));
        assert_eq!(documented_keys(r##"{"locks":{"all","pixels":bool}?,"name":str?}"##), Some(vec!["locks", "all", "pixels", "name"]));
        assert_eq!(documented_keys("{}"), Some(vec![]));
        assert_eq!(documented_keys(r##"{"layer":id?, …params of that kind}"##), None);
        assert_eq!(documented_keys("layer and time"), None);
    }

    #[test]
    fn unknown_keys_are_refused_with_a_suggestion() {
        let doc = r#"{"name":str,"width":u32=1920}"#;
        assert!(check("timeline", "timeline:x", doc, &json!({"name": "a", "width": 3})).is_ok());
        let e = check("timeline", "timeline:x", doc, &json!({"nmae": "a"})).unwrap_err().to_string();
        assert!(e.contains("unknown parameter `nmae`") && e.contains("did you mean `name`"), "{e}");
        assert!(check("image", "image:x", "{}", &json!({"coalesce": "k"})).is_ok(), "universal keys");
        assert!(check("motion", "motion:x", doc, &json!({"anything": 1})).is_ok(), "the motion engine checks itself");
        assert!(check("timeline", "timeline:x", r#"{"a":1, …}"#, &json!({"b": 1})).is_ok(), "open docs are not checked");
    }
}
