//! End-to-end behaviour of the studio: nesting across the three engines, receipts, strict
//! parameters and undo. Everything goes through `Studio::execute`, as a client would.

use serde_json::{Value, json};
use videonow_core::Studio;

fn ok(s: &mut Studio, id: &str, p: Value) -> Value {
    match s.execute(id, p) {
        Ok(v) => v,
        Err(e) => panic!("{id}: {e}"),
    }
}

/// A 64x64 sequence showing a 64x64 composition that holds a 64x64 image.
fn nested_project() -> (Studio, u64, u64) {
    let mut s = Studio::new();
    let img = ok(&mut s, "studio:image.new", json!({"name": "Img", "width": 64, "height": 64, "background": "white"}))["result"]["asset"].as_u64().unwrap();
    let comp = ok(&mut s, "studio:comp.new", json!({"name": "C", "width": 64, "height": 64, "fps": 30, "duration": 1}))["result"]["asset"].as_u64().unwrap();
    ok(&mut s, "studio:image.toComp", json!({"image": img, "comp": comp}));
    ok(&mut s, "timeline:file.newSequence", json!({"name": "S", "width": 64, "height": 64, "fps": 30, "video": 1, "audio": 1}));
    ok(&mut s, "studio:asset.place", json!({"asset": comp, "track": "V1", "seconds": 0}));
    (s, img, comp)
}

fn center(s: &Studio) -> [u8; 4] {
    let (w, h, px, offline) = s.render_rgba(0.5, 1.0).unwrap();
    assert!(offline.is_empty(), "offline media: {offline:?}");
    let i = ((h / 2 * w + w / 2) * 4) as usize;
    [px[i], px[i + 1], px[i + 2], px[i + 3]]
}

#[test]
fn image_shows_through_composition_on_the_timeline() {
    let (s, _, _) = nested_project();
    let c = center(&s);
    assert!(c[0] > 240 && c[1] > 240 && c[2] > 240, "expected the white image, got {c:?}");
}

/// Regression: a composition's cached frame went stale when only an image nested in it changed.
#[test]
fn editing_a_nested_image_updates_the_next_frame() {
    let (mut s, img, _) = nested_project();
    assert!(center(&s)[1] > 240);
    ok(&mut s, "image:shape.create", json!({"asset": img, "kind": "rect", "rect": [16, 16, 32, 32], "fill": "#ff0000"}));
    let c = center(&s);
    assert!(c[0] > 200 && c[1] < 60 && c[2] < 60, "expected red after the edit, got {c:?}");
}

#[test]
fn receipts_say_what_changed() {
    let mut s = Studio::new();
    let r = ok(&mut s, "studio:comp.new", json!({"name": "C", "width": 32, "height": 32}));
    let rc = &r["receipt"];
    assert_eq!(rc["revision"]["before"], 0);
    assert_eq!(rc["revision"]["after"], 1);
    let changed: Vec<&str> = rc["changed"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
    assert!(changed.contains(&"motion") && changed.contains(&"studio"), "{changed:?}");
    assert_eq!(rc["created"], json!([1]));

    // a read changes nothing and does not move the revision
    let r = ok(&mut s, "studio:inspect", json!({}));
    assert_eq!(r["receipt"]["changed"], json!([]));
    assert_eq!(r["receipt"]["revision"]["after"], 1);
    assert_eq!(r["receipt"]["not_checked"], json!([]));

    // an edit that changes the picture says the picture was not checked
    let r = ok(&mut s, "motion:layer.newSolid", json!({"asset": 1, "color": "#336699"}));
    assert_eq!(r["receipt"]["changed"], json!(["motion"]));
    assert!(!r["receipt"]["not_checked"].as_array().unwrap().is_empty());
}

#[test]
fn studio_commands_reject_unknown_parameters() {
    let mut s = Studio::new();
    let e = s.execute("studio:comp.new", json!({"nmae": "Typo"})).unwrap_err().to_string();
    assert!(e.contains("unknown parameter `nmae`") && e.contains("name"), "{e}");
    // nothing was created
    assert_eq!(s.revision(), 0);
    assert_eq!(s.assets().count(), 0);
}

#[test]
fn unknown_commands_and_domains_are_errors() {
    let mut s = Studio::new();
    assert!(s.execute("timeline:no.such.command", json!({})).is_err());
    assert!(s.execute("sound:play", json!({})).is_err());
    assert!(s.execute("no-domain", json!({})).is_err());
    assert!(s.execute("studio:inspect", json!([1, 2])).is_err());
}

#[test]
fn image_commands_need_an_image_asset() {
    let mut s = Studio::new();
    let comp = ok(&mut s, "studio:comp.new", json!({"width": 32, "height": 32}))["result"]["asset"].clone();
    let e = s.execute("image:shape.create", json!({"asset": comp, "kind": "rect", "rect": [0, 0, 4, 4]})).unwrap_err().to_string();
    assert!(e.contains("is not an image"), "{e}");
    let e = s.execute("image:shape.create", json!({"kind": "rect", "rect": [0, 0, 4, 4]})).unwrap_err().to_string();
    assert!(e.contains("asset"), "{e}");
}

#[test]
fn undo_goes_to_the_engine_that_changed() {
    let mut s = Studio::new();
    ok(&mut s, "studio:comp.new", json!({"name": "C", "width": 32, "height": 32}));
    ok(&mut s, "motion:layer.newSolid", json!({"asset": 1, "color": "#336699"}));
    let layers = |s: &mut Studio| ok(s, "studio:inspect", json!({}))["result"]["compositions"][0]["layers"].as_u64().unwrap();
    assert_eq!(layers(&mut s), 1);
    let r = ok(&mut s, "studio:undo", json!({}));
    assert_eq!(r["result"]["undone"], json!(["motion"]));
    assert_eq!(layers(&mut s), 0);
}

#[test]
fn every_command_has_an_effect_class() {
    let mut s = Studio::new();
    let list = ok(&mut s, "studio:commands", json!({}))["result"].as_array().unwrap().clone();
    assert!(list.len() > 1000, "only {} commands", list.len());
    for d in ["timeline", "motion", "image", "studio"] {
        assert!(list.iter().any(|c| c["domain"] == d), "no {d} commands");
    }
    assert!(list.iter().all(|c| c["effect"].is_string()));
}

fn tmpdir(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("videonow-test-{name}-{}.videonow", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}

/// Phase 1 gate: a saved project opens to the same picture, and stays live after opening.
#[test]
fn save_and_open_round_trip_renders_the_same() {
    let (mut s, img, _) = nested_project();
    ok(&mut s, "image:shape.create", json!({"asset": img, "kind": "ellipse", "rect": [8, 8, 48, 48], "fill": "#2266cc"}));
    let before = s.render_rgba(0.5, 1.0).unwrap();
    let dir = tmpdir("roundtrip");
    let r = ok(&mut s, "studio:project.save", json!({"path": dir.to_string_lossy()}));
    assert_eq!(r["receipt"]["command"], "studio:project.save");
    for f in ["manifest.json", "timeline.fcproj", "motion.ecproj", "images/1.pcraft"] {
        assert!(dir.join(f).is_file(), "{f} missing");
    }

    let mut t = Studio::new();
    let r = ok(&mut t, "studio:project.open", json!({"path": dir.to_string_lossy()}));
    assert_eq!(r["result"]["assets"], 2);
    let after = t.render_rgba(0.5, 1.0).unwrap();
    assert_eq!((before.0, before.1), (after.0, after.1));
    assert!(after.3.is_empty(), "offline after open: {:?}", after.3);
    assert!(before.2 == after.2, "the reopened project renders differently");

    // still live: an image edit after opening reaches the timeline
    ok(&mut t, "image:shape.create", json!({"asset": img, "kind": "rect", "rect": [24, 24, 16, 16], "fill": "#ff0000"}));
    let c = center(&t);
    assert!(c[0] > 200 && c[1] < 60, "{c:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn open_refuses_what_is_not_a_project() {
    let mut s = Studio::new();
    let dir = tmpdir("notproject");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("manifest.json"), br#"{"format":"other","schema":1}"#).unwrap();
    assert!(s.execute("studio:project.open", json!({"path": dir.to_string_lossy()})).is_err());
    // and saving into a folder with other files is refused
    std::fs::write(dir.join("notes.txt"), b"x").unwrap();
    std::fs::remove_file(dir.join("manifest.json")).unwrap();
    let e = s.execute("studio:project.save", json!({"path": dir.to_string_lossy()})).unwrap_err().to_string();
    assert!(e.contains("not empty"), "{e}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Every command classed `read` runs on a populated project without changing it and without
/// writing files (gap analysis P4: "check a read class by running it").
#[test]
fn read_commands_change_nothing() {
    let (mut s, img, comp) = nested_project();
    let list = ok(&mut s, "studio:commands", json!({}))["result"].as_array().unwrap().clone();
    let cwd = tmpdir("reads");
    std::fs::create_dir_all(&cwd).unwrap();
    std::env::set_current_dir(&cwd).unwrap();
    let mut changed = Vec::new();
    let mut ran = 0;
    for c in list.iter().filter(|c| c["effect"] == "read" && c["domain"] != "studio") {
        let id = c["id"].as_str().unwrap();
        let p = match c["domain"].as_str() {
            Some("image") => json!({"asset": img}),
            Some("motion") => json!({"asset": comp}),
            _ => json!({}),
        };
        let Ok(r) = s.execute(id, p) else { continue }; // a read may need parameters; failing is fine
        ran += 1;
        if r["receipt"]["changed"] != json!([]) {
            changed.push(format!("{id} changed {}", r["receipt"]["changed"]));
        }
    }
    let written: Vec<_> = std::fs::read_dir(&cwd).unwrap().flatten().map(|e| e.file_name()).collect();
    assert!(ran > 100, "only {ran} read commands ran");
    assert!(changed.is_empty(), "read commands that changed the project:\n{}", changed.join("\n"));
    assert!(written.is_empty(), "read commands wrote files: {written:?}");
}

#[test]
fn the_policy_is_enforced_and_survives_opening_a_project() {
    use videonow_core::Policy;
    use videonow_core::manifest::Effect;
    let (mut s, _, _) = nested_project();
    let root = tmpdir("policy");
    std::fs::create_dir_all(&root).unwrap();
    let saved = root.join("p.videonow");
    ok(&mut s, "studio:project.save", json!({"path": saved.to_string_lossy()}));

    s.set_policy(Policy { deny: vec![Effect::Destructive], ..Policy::confined_to(std::slice::from_ref(&root)) });
    // inside the root: allowed
    ok(&mut s, "studio:render.frame", json!({"path": root.join("in.png").to_string_lossy(), "scale": 0.5}));
    // outside: refused before anything runs
    let e = s.execute("studio:render.frame", json!({"path": "/tmp/../tmp/outside.png"})).unwrap_err().to_string();
    assert!(e.contains("not allowed") && e.contains("outside"), "{e}");
    // a denied class
    let e = s.execute("studio:project.new", json!({})).unwrap_err().to_string();
    assert!(e.contains("destructive"), "{e}");

    // opening is destructive too; allow it, then check the confinement still holds afterwards
    s.set_policy(Policy::confined_to(std::slice::from_ref(&root)));
    ok(&mut s, "studio:project.open", json!({"path": saved.to_string_lossy()}));
    assert!(s.execute("studio:render.frame", json!({"path": "/tmp/outside.png"})).is_err(), "opening a project lifted the policy");
    let _ = std::fs::remove_dir_all(&root);
}
