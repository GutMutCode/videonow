//! Calibrates the studio's unknown-parameter check against real usage: every
//! `execute("<id>", json!({...}))` call in the FilmCraft and PhotoCraft engine sources (their
//! tests are known-good calls) must pass `params_check::check`. A failure here means a params doc
//! is missing a key the command really takes: add it to the doc or to the universal list.

use std::path::{Path, PathBuf};

use videonow_core::params_check::{KeyCheck, check};

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            rust_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

/// (command id, top-level keys, the call expects an error)
fn calls(src: &str) -> Vec<(String, Vec<String>, bool)> {
    let mut out = Vec::new();
    let b = src.as_bytes();
    let mut at = 0;
    while let Some(i) = src[at..].find("execute(") {
        let start = at + i + "execute(".len();
        at = start;
        let rest = src[start..].trim_start();
        let Some(rest) = rest.strip_prefix('"') else { continue };
        let Some(q) = rest.find('"') else { continue };
        let id = &rest[..q];
        if !id.contains('.') || id.contains(' ') {
            continue;
        }
        let after = rest[q + 1..].trim_start();
        let Some(after) = after.strip_prefix(',') else { continue };
        let after = after.trim_start();
        let Some(obj) = after.strip_prefix("json!(") else { continue };
        if !obj.starts_with('{') {
            continue;
        }
        // match braces of the object literal
        let off = src.len() - obj.len();
        let (mut depth, mut k, mut in_str, mut esc) = (0i32, off, false, false);
        while k < b.len() {
            let c = b[k];
            if in_str {
                if esc {
                    esc = false;
                } else if c == b'\\' {
                    esc = true;
                } else if c == b'"' {
                    in_str = false;
                }
            } else if c == b'"' {
                in_str = true;
            } else if c == b'{' {
                depth += 1;
            } else if c == b'}' {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            k += 1;
        }
        let body = &src[off..k.min(src.len())];
        // top-level quoted keys: "key" followed by ':' at depth 1
        let mut keys = Vec::new();
        let bb = body.as_bytes();
        let (mut d, mut j) = (0i32, 0usize);
        while j < bb.len() {
            match bb[j] {
                b'{' | b'[' | b'(' => d += 1,
                b'}' | b']' | b')' => d -= 1,
                b'"' => {
                    let s = j + 1;
                    let mut e = s;
                    while e < bb.len() && bb[e] != b'"' {
                        if bb[e] == b'\\' {
                            e += 1;
                        }
                        e += 1;
                    }
                    if d == 1 && body[(e + 1).min(body.len())..].trim_start().starts_with(':') {
                        keys.push(body[s..e.min(body.len())].to_string());
                    }
                    j = e;
                }
                _ => {}
            }
            j += 1;
        }
        let tail_end = src[k..].find(';').map(|x| k + x).unwrap_or(src.len());
        let tail = &src[k..tail_end.min(k + 400)];
        let expects_err = tail.contains("unwrap_err") || tail.contains("is_err") || tail.contains("expect_err");
        out.push((id.to_string(), keys, expects_err));
    }
    out
}

#[test]
fn engine_test_calls_pass_the_studio_parameter_check() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../engines");
    let mut failures: Vec<String> = Vec::new();
    let (mut seen, mut checked, mut keys_checked) = (0usize, 0usize, 0usize);
    let mut sample: Vec<String> = Vec::new();
    for (domain, dir) in [("timeline", "film/crates"), ("image", "photo/crates")] {
        let mut files = Vec::new();
        rust_files(&root.join(dir), &mut files);
        for f in files {
            let Ok(src) = std::fs::read_to_string(&f) else { continue };
            for (id, mut keys, expects_err) in calls(&src) {
                // engine tests that pass a `junk` key check that it is ignored; the studio rejects it
                keys.retain(|k| k != "junk");
                let doc = match domain {
                    "timeline" => filmcraft_engine::find_command(&id).map(|c| c.params),
                    _ => photocraft_engine::commands::find(&id).map(|c| c.params),
                };
                let Some(doc) = doc else { continue };
                seen += 1;
                if expects_err {
                    continue;
                }
                // one key at a time, so every missing key is reported, not only the first
                for k in &keys {
                    let one = serde_json::json!({ k.as_str(): null });
                    if let Err(e) = check(domain, &id, doc, &one) {
                        failures.push(format!("{domain}:{id}: {e}  [{}]", f.strip_prefix(&root).unwrap_or(&f).display()));
                    }
                }
                let obj: serde_json::Map<String, serde_json::Value> = keys.iter().map(|k| (k.clone(), serde_json::Value::Null)).collect();
                match check(domain, &id, doc, &serde_json::Value::Object(obj)) {
                    Ok(KeyCheck::Checked) => {
                        checked += 1;
                        keys_checked += keys.len();
                        if sample.len() < 3 && keys.len() > 2 {
                            sample.push(format!("{domain}:{id} {keys:?}"));
                        }
                    }
                    Ok(_) => {}
                    Err(_) => {}
                }
            }
        }
    }
    failures.sort();
    failures.dedup();
    eprintln!("{seen} engine calls found, {checked} checked against closed docs ({keys_checked} keys), {} failures", failures.len());
    eprintln!("sample: {sample:?}");
    for f in failures.iter().take(400) {
        eprintln!("  {f}");
    }
    assert!(seen > 1000, "the scanner found only {seen} calls");
    assert!(keys_checked > 5000, "only {keys_checked} keys checked: the scanner is not reading keys");
    assert!(failures.is_empty(), "{} engine calls would be rejected", failures.len());
}

#[test]
fn the_check_catches_a_known_misspelling() {
    let doc = photocraft_engine::commands::find("layer.setProps").map(|c| c.params).unwrap_or("");
    let e = check("image", "layer.setProps", doc, &serde_json::json!({"opactiy": 0.5}));
    assert!(e.is_err(), "layer.setProps accepted `opactiy`: doc = {doc}");
    let doc = filmcraft_engine::find_command("file.newSequence").map(|c| c.params).unwrap_or("");
    assert!(check("timeline", "file.newSequence", doc, &serde_json::json!({"widht": 10})).is_err());
}
