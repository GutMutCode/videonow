//! The effect classes are checked, not just declared: every `read` command runs on a populated
//! project and must leave every document, the asset table and the working folder as they were.

use std::panic::{AssertUnwindSafe, catch_unwind};

use serde_json::{Value, json};
use videonow_core::Studio;
use videonow_core::manifest::{self, Effect};

/// Everything a read must not change, as bytes: the timeline project, the motion project, each
/// image document, and the asset table.
fn state(s: &Studio) -> Vec<(String, Vec<u8>)> {
    let mut v = vec![("timeline".to_string(), filmcraft_format::encode(&s.timeline.project, false))];
    v.push(("motion".to_string(), s.motion.project.to_file_json().unwrap_or_default().into_bytes()));
    for a in s.assets() {
        v.push((format!("asset {}", a.id.0), serde_json::to_vec(a).unwrap_or_default()));
        if let Some(d) = s.image_doc(a.id) {
            let bytes = photocraft_format::save_to_bytes(&d.doc, &photocraft_format::SaveOptions { thumbnail: None, composite: None }).unwrap_or_default();
            v.push((format!("image {}", a.id.0), bytes));
        }
    }
    v
}

fn files(dir: &std::path::Path) -> Vec<String> {
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            out.push(e.file_name().to_string_lossy().into_owned());
        }
    }
    out.sort();
    out
}

#[test]
fn read_commands_change_nothing() {
    let work = std::env::temp_dir().join(format!("videonow-reads-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).unwrap();
    // the only test in this binary, so changing the working folder affects nothing else
    std::env::set_current_dir(&work).unwrap();

    let mut s = Studio::new();
    let d = videonow_core::demo::build(&mut s, |_, _| {}).unwrap();
    let mut before = state(&s);
    let mut rev = s.revision();
    let mut ran = 0;
    let mut failed_before_running = 0;
    let mut panics = Vec::new();
    let mut changed = Vec::new();
    for c in manifest::all() {
        if c.effect != Effect::Read {
            continue;
        }
        // aim each engine's commands at a real document
        let params: Value = match c.domain {
            "motion" => json!({"asset": d.title}),
            "image" => json!({"asset": d.badge}),
            _ => json!({}),
        };
        let r = catch_unwind(AssertUnwindSafe(|| s.execute(&c.id, params.clone())));
        match r {
            Err(_) => panics.push(c.id.clone()),
            Ok(Err(_)) => failed_before_running += 1,
            Ok(Ok(_)) => ran += 1,
        }
        let now = state(&s);
        let diff: Vec<&str> = before.iter().zip(&now).filter(|(a, b)| a != b).map(|(a, _)| a.0.as_str()).collect();
        if !diff.is_empty() || now.len() != before.len() || s.revision() != rev {
            changed.push(format!("{} ({})", c.id, if diff.is_empty() { "revision".to_string() } else { diff.join(", ") }));
            // start the next command from a fresh demo project
            s = Studio::new();
            videonow_core::demo::build(&mut s, |_, _| {}).unwrap();
            before = state(&s);
            rev = s.revision();
        }
    }
    eprintln!("read commands: {ran} ran, {failed_before_running} refused their parameters");
    assert!(panics.is_empty(), "read commands panicked: {panics:?}");
    assert!(changed.is_empty(), "read commands changed the project; declare their class in manifest::DECLARED:\n{}", changed.join("\n"));
    assert_eq!(files(&work), Vec::<String>::new(), "read commands wrote files");
    assert!(ran > 150, "only {ran} read commands ran");
}
