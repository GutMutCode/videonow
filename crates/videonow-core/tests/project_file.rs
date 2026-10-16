//! `.videonow` project files: what is saved opens to the same picture, nested links survive, and
//! a project that cannot be opened changes nothing.

use std::path::PathBuf;

use serde_json::{Value, json};
use videonow_core::Studio;

fn ok(s: &mut Studio, id: &str, p: Value) -> Value {
    match s.execute(id, p) {
        Ok(v) => v,
        Err(e) => panic!("{id}: {e}"),
    }
}

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("videonow-pf-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn frames(s: &Studio) -> Vec<Vec<u8>> {
    [0.2, 1.0, 2.5].iter().map(|t| s.render_rgba(*t, 0.25).unwrap().2).collect()
}

fn demo() -> (Studio, videonow_core::demo::Demo) {
    let mut s = Studio::new();
    let d = videonow_core::demo::build(&mut s, |_, _| {}).unwrap();
    (s, d)
}

#[test]
fn saved_project_opens_to_the_same_frames() {
    let (mut a, _) = demo();
    let before = frames(&a);
    let dir = tmp("roundtrip").join("Demo.videonow");
    let r = ok(&mut a, "studio:project.save", json!({"path": dir.to_string_lossy()}));
    assert!(r["receipt"]["checked"].to_string().contains("read back"), "{}", r["receipt"]);
    assert!(!a.is_dirty());
    for f in ["project.json", "timeline.fcproj", "motion.ecproj", "images/1.pcraft"] {
        assert!(dir.join(f).is_file(), "{f} missing");
    }

    let mut b = Studio::new();
    let r = ok(&mut b, "studio:project.open", json!({"path": dir.to_string_lossy()}));
    assert_eq!(r["result"]["assets"], 2);
    assert_eq!(b.name(), "Demo");
    assert!(!b.is_dirty());
    assert_eq!(a.assets().cloned().collect::<Vec<_>>(), b.assets().cloned().collect::<Vec<_>>());
    let after = frames(&b);
    for (i, (x, y)) in before.iter().zip(&after).enumerate() {
        assert!(x == y, "frame {i} differs after save and open");
    }
}

#[test]
fn nested_links_survive_reopening() {
    let (mut a, d) = demo();
    let dir = tmp("links").join("Links.videonow");
    ok(&mut a, "studio:project.save", json!({"path": dir.to_string_lossy()}));
    let mut b = Studio::new();
    ok(&mut b, "studio:project.open", json!({"path": dir.to_string_lossy()}));
    let before = b.render_rgba(2.5, 0.25).unwrap().2;
    // an edit in the reopened image engine shows through the reopened composition and timeline
    ok(&mut b, "image:shape.create", json!({"asset": d.badge, "kind": "rect", "rect": [0, 0, 640, 640], "fill": "#ff0000"}));
    assert!(b.is_dirty());
    let after = b.render_rgba(2.5, 0.25).unwrap().2;
    assert_ne!(before, after, "the edit did not reach the timeline after reopening");
    // and the timeline still reports nothing offline
    let (_, _, _, offline) = b.render_rgba(2.5, 0.25).unwrap();
    assert!(offline.is_empty(), "{offline:?}");
}

#[test]
fn saving_again_writes_in_place_and_drops_removed_images() {
    let (mut a, _) = demo();
    let dir = tmp("resave").join("P.videonow");
    ok(&mut a, "studio:project.save", json!({"path": dir.to_string_lossy()}));
    std::fs::write(dir.join("images/99.pcraft"), b"stale").unwrap();
    ok(&mut a, "motion:layer.newSolid", json!({"asset": 2, "color": "#000000"}));
    let r = ok(&mut a, "studio:project.save", json!({}));
    assert_eq!(r["result"]["path"].as_str().unwrap(), dir.to_string_lossy());
    assert!(!dir.join("images/99.pcraft").exists(), "an image no asset uses was left behind");
}

#[test]
fn opening_refuses_unsaved_changes_unless_discarded() {
    let (mut a, _) = demo();
    let dir = tmp("dirty").join("D.videonow");
    ok(&mut a, "studio:project.save", json!({"path": dir.to_string_lossy()}));
    ok(&mut a, "studio:comp.new", json!({"name": "Unsaved"}));
    let e = a.execute("studio:project.open", json!({"path": dir.to_string_lossy()})).unwrap_err().to_string();
    assert!(e.contains("unsaved changes"), "{e}");
    assert_eq!(a.assets().count(), 3, "nothing changed");
    ok(&mut a, "studio:project.open", json!({"path": dir.to_string_lossy(), "discard": true}));
    assert_eq!(a.assets().count(), 2);
}

#[test]
fn a_damaged_project_changes_nothing() {
    let (mut a, _) = demo();
    let dir = tmp("damaged").join("X.videonow");
    ok(&mut a, "studio:project.save", json!({"path": dir.to_string_lossy()}));
    std::fs::write(dir.join("motion.ecproj"), b"{ not json").unwrap();
    let mut b = Studio::new();
    ok(&mut b, "studio:comp.new", json!({"name": "Mine"}));
    let rev = b.revision();
    let e = b.execute("studio:project.open", json!({"path": dir.to_string_lossy(), "discard": true})).unwrap_err().to_string();
    assert!(e.contains("motion"), "{e}");
    assert_eq!(b.revision(), rev);
    assert_eq!(b.assets().map(|a| a.name.clone()).collect::<Vec<_>>(), vec!["Mine".to_string()]);
}

#[test]
fn saving_refuses_to_overwrite_a_folder_that_is_not_a_project() {
    let (mut a, _) = demo();
    let dir = tmp("foreign");
    std::fs::write(dir.join("notes.txt"), b"mine").unwrap();
    let e = a.execute("studio:project.save", json!({"path": dir.to_string_lossy()})).unwrap_err().to_string();
    assert!(e.contains("not a VideoNow project"), "{e}");
    assert_eq!(std::fs::read(dir.join("notes.txt")).unwrap(), b"mine");
    assert!(!dir.join("project.json").exists());
}

#[test]
fn new_project_starts_empty() {
    let (mut a, _) = demo();
    let e = a.execute("studio:project.new", json!({})).unwrap_err().to_string();
    assert!(e.contains("unsaved changes"), "{e}");
    ok(&mut a, "studio:project.new", json!({"name": "Next", "discard": true}));
    assert_eq!(a.assets().count(), 0);
    assert_eq!(a.name(), "Next");
    // the journal starts again, with how this project began
    assert_eq!(a.journal().len(), 1);
    assert_eq!(a.journal()[0]["command"], "studio:project.new");
}

#[test]
fn a_composition_changed_after_placing_updates_its_timeline_item() {
    let mut s = Studio::new();
    ok(&mut s, "studio:comp.new", json!({"name": "C", "width": 64, "height": 64, "fps": 30, "duration": 2}));
    ok(&mut s, "timeline:file.newSequence", json!({"name": "S", "width": 64, "height": 64, "fps": 30}));
    ok(&mut s, "studio:asset.place", json!({"asset": 1, "track": "V1"}));
    let item = s.asset(videonow_core::AssetId(1)).unwrap().timeline_item.unwrap();
    let dur = |s: &Studio| s.timeline.project.item(filmcraft_engine::project::ItemId(item)).unwrap().as_media().unwrap().info.duration;
    let before = dur(&s);
    ok(&mut s, "motion:comp.settings", json!({"asset": 1, "duration": 6}));
    assert!(dur(&s) > before, "the timeline still thinks the composition is {before:?} long");
}
