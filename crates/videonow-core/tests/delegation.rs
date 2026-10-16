//! What a session may do is enforced before anything runs: effect classes, file folders, reserved
//! engine commands and unknown parameters. A refusal changes nothing.

use std::path::PathBuf;

use serde_json::{Value, json};
use videonow_core::{Policy, Studio};

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("videonow-del-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn ok(s: &mut Studio, id: &str, p: Value) -> Value {
    match s.execute(id, p) {
        Ok(v) => v,
        Err(e) => panic!("{id}: {e}"),
    }
}

#[test]
fn effect_classes_outside_the_session_are_refused() {
    let mut s = Studio::with_policy(Policy { allow: Policy::parse_allow("read").unwrap(), ..Default::default() });
    let e = s.execute("studio:comp.new", json!({})).unwrap_err().to_string();
    assert!(e.starts_with("policy:") && e.contains("edit"), "{e}");
    assert_eq!(s.revision(), 0);
    assert_eq!(s.assets().count(), 0);
    // reading is always allowed
    ok(&mut s, "studio:inspect", json!({}));
    let p = ok(&mut s, "studio:policy", json!({}));
    assert_eq!(p["result"]["allowedEffects"], json!(["read"]));
}

#[test]
fn destructive_commands_need_their_class() {
    let mut s = Studio::with_policy(Policy { allow: Policy::parse_allow("read,edit").unwrap(), ..Default::default() });
    ok(&mut s, "studio:comp.new", json!({}));
    let e = s.execute("studio:project.new", json!({"discard": true})).unwrap_err().to_string();
    assert!(e.contains("destructive"), "{e}");
    assert_eq!(s.assets().count(), 1, "nothing was discarded");
}

#[test]
fn confined_sessions_write_only_inside_their_folders() {
    let root = tmp("confined");
    let mut s = Studio::with_policy(Policy { write_roots: vec![root.clone()], ..Default::default() });
    videonow_core::demo::build(&mut s, |_, _| {}).unwrap();
    // relative paths land in the folder, and the receipt says where
    let r = ok(&mut s, "studio:render.frame", json!({"seconds": 1, "path": "out/f.png", "scale": 0.1}));
    assert!(root.join("out/f.png").is_file());
    assert!(r["result"]["path"].as_str().unwrap().starts_with(&*videonow_core::policy::normalize(&root).to_string_lossy()));
    // anything outside is refused before it runs
    let outside = tmp("outside").join("f.png");
    let e = s.execute("studio:render.frame", json!({"path": outside.to_string_lossy()})).unwrap_err().to_string();
    assert!(e.starts_with("policy:") && e.contains("outside"), "{e}");
    assert!(!outside.exists());
    let e = s.execute("studio:render.frame", json!({"path": "../escape.png"})).unwrap_err().to_string();
    assert!(e.starts_with("policy:"), "{e}");
    // engine commands too: an export to a path outside the folder
    let e = s.execute("timeline:file.exportFrame", json!({"path": outside.to_string_lossy()})).unwrap_err().to_string();
    assert!(e.starts_with("policy:"), "{e}");
    // and the project file
    ok(&mut s, "studio:project.save", json!({"path": "Film.videonow"}));
    assert!(root.join("Film.videonow/project.json").is_file());
}

#[test]
fn confined_sessions_refuse_commands_that_cannot_be_confined() {
    let root = tmp("unconfinable");
    let mut s = Studio::with_policy(Policy { write_roots: vec![root], ..Default::default() });
    let e = s.execute("motion:file.runScript", json!({"path": "x.jsx"})).unwrap_err().to_string();
    assert!(e.contains("reaches outside the project"), "{e}");
    // a file-writing command that does not say where it writes
    let e = s.execute("timeline:export.quick", json!({})).unwrap_err().to_string();
    assert!(e.starts_with("policy:") && e.contains("name no path"), "{e}");
}

#[test]
fn reserved_engine_commands_point_to_the_studio_command() {
    let mut s = Studio::new();
    for id in ["timeline:file.open", "motion:file.newProject", "image:file.close"] {
        if videonow_core::manifest::find(id).is_none() {
            continue;
        }
        let e = s.execute(id, json!({"path": "x"})).unwrap_err().to_string();
        assert!(e.contains("not available in VideoNow") && e.contains("studio:project."), "{id}: {e}");
    }
}

#[test]
fn unknown_engine_parameters_are_refused_with_a_suggestion() {
    let mut s = Studio::new();
    let e = s.execute("timeline:file.newSequence", json!({"name": "S", "widht": 640})).unwrap_err().to_string();
    assert!(e.contains("unknown parameter `widht`") && e.contains("did you mean `width`"), "{e}");
    assert!(s.timeline.active_sequence().is_none(), "nothing ran");
    let r = ok(&mut s, "timeline:file.newSequence", json!({"name": "S", "width": 640, "height": 360}));
    assert!(r["receipt"]["checked"].to_string().contains("unknown parameters rejected"));
}

#[test]
fn describe_says_whether_a_command_may_run_here() {
    let mut s = Studio::with_policy(Policy { allow: Policy::parse_allow("read").unwrap(), ..Default::default() });
    let d = ok(&mut s, "studio:describe", json!({"id": "studio:comp.new"}))["result"].clone();
    assert_eq!(d["allowedHere"], false);
    assert!(d["refusedBecause"].as_str().unwrap().contains("edit"));
    let d = ok(&mut s, "studio:describe", json!({"id": "timeline:file.newSequence"}))["result"].clone();
    assert!(d["documentedKeys"].as_array().unwrap().iter().any(|k| k == "width"));
}
