//! Phase 3: request ids, expected revisions, atomic batches, dry runs, previews, float frames,
//! composition audio.

use serde_json::{Value, json};
use videonow_core::{Request, Studio};

fn ok(s: &mut Studio, id: &str, p: Value) -> Value {
    match s.execute(id, p) {
        Ok(v) => v,
        Err(e) => panic!("{id}: {e}"),
    }
}

fn comps(s: &mut Studio) -> usize {
    ok(s, "studio:inspect", json!({}))["result"]["compositions"].as_array().map_or(0, Vec::len)
}

#[test]
fn a_retried_request_runs_once() {
    let mut s = Studio::new();
    let req = Request { request_id: Some("r-1".into()), ..Default::default() };
    let a = s.execute_with("studio:comp.new", json!({"name": "Once"}), &req).unwrap();
    let b = s.execute_with("studio:comp.new", json!({"name": "Once"}), &req).unwrap();
    assert_eq!(comps(&mut s), 1);
    assert_eq!(a["result"]["asset"], b["result"]["asset"]);
    assert_eq!(b["receipt"]["replayed"], json!(true));
    // the same id for something else is refused, not replayed
    let e = s.execute_with("studio:comp.new", json!({"name": "Other"}), &req).unwrap_err().to_string();
    assert!(e.contains("r-1") && e.contains("new id"), "{e}");
}

#[test]
fn a_change_against_an_old_revision_is_refused() {
    let mut s = Studio::new();
    ok(&mut s, "studio:comp.new", json!({"name": "C", "width": 64, "height": 64}));
    let seen = s.revision();
    // someone else changes the project
    s.set_actor("ui");
    ok(&mut s, "motion:layer.newSolid", json!({"asset": 1, "color": "#336699"}));
    s.set_actor("agent");
    let e = s
        .execute_with("motion:layer.newSolid", json!({"asset": 1, "color": "#ff0000"}), &Request { expect_revision: Some(seen), ..Default::default() })
        .unwrap_err()
        .to_string();
    assert!(e.contains(&format!("since revision {seen}")) && e.contains("motion:layer.newSolid by ui"), "{e}");
    let now = s.revision();
    s.execute_with("motion:layer.newSolid", json!({"asset": 1, "color": "#ff0000"}), &Request { expect_revision: Some(now), ..Default::default() }).unwrap();
}

#[test]
fn a_failing_atomic_batch_leaves_nothing() {
    let mut s = Studio::new();
    ok(&mut s, "timeline:file.newSequence", json!({"name": "S", "width": 64, "height": 64, "fps": 30, "video": 1, "audio": 1}));
    let before = ok(&mut s, "studio:inspect", json!({}))["result"].clone();
    let rev = s.revision();
    let journal = s.journal().len();
    let e = s
        .execute(
            "studio:batch",
            json!({"steps": [
                {"id": "studio:comp.new", "params": {"name": "L3", "width": 64, "height": 16}},
                {"id": "motion:layer.newText", "params": {"asset": "$0.asset", "text": "Name"}},
                {"id": "studio:image.new", "params": {"name": "Pic", "width": 8, "height": 8}},
                {"id": "studio:asset.place", "params": {"asset": "$0.asset", "track": "V9"}}
            ]}),
        )
        .unwrap_err()
        .to_string();
    assert!(e.contains("step 3") && e.contains("taken back"), "{e}");
    let after = ok(&mut s, "studio:inspect", json!({}))["result"].clone();
    assert_eq!(before["compositions"], after["compositions"]);
    assert_eq!(before["images"], after["images"]);
    assert_eq!(before["sequence"], after["sequence"]);
    assert_eq!(s.revision(), rev, "a rolled-back batch moved the revision");
    assert_eq!(s.journal().len(), journal);
    assert_eq!(s.assets().count(), 0);
    assert_eq!(s.images.documents().len(), 0);

    // the same steps, fixed, go in whole; later steps use earlier results
    let r = ok(
        &mut s,
        "studio:batch",
        json!({"steps": [
            {"id": "studio:comp.new", "params": {"name": "L3", "width": 64, "height": 16}},
            {"id": "motion:layer.newText", "params": {"asset": "$0.asset", "text": "Name"}},
            {"id": "studio:asset.place", "params": {"asset": "$0.asset", "track": "V1"}}
        ]}),
    );
    assert_eq!(r["completed"], 3);
    assert_eq!(comps(&mut s), 1);
}

#[test]
fn a_dry_run_reports_and_keeps_nothing() {
    let mut s = Studio::new();
    ok(&mut s, "studio:comp.new", json!({"name": "C", "width": 64, "height": 64}));
    let before = ok(&mut s, "studio:inspect", json!({}))["result"].clone();
    let rev = s.revision();
    let r = s.execute_with("motion:layer.newSolid", json!({"asset": 1, "color": "#ff0000"}), &Request { dry_run: true, ..Default::default() }).unwrap();
    assert_eq!(r["receipt"]["dryRun"], json!(true));
    assert_eq!(r["receipt"]["restored"], json!(true));
    assert_eq!(r["receipt"]["changed"], json!(["motion"]));
    let after = ok(&mut s, "studio:inspect", json!({}))["result"].clone();
    assert_eq!(before["compositions"], after["compositions"]);
    assert_eq!(s.revision(), rev);
    // a dry run of an image edit, and of a new image
    let img = ok(&mut s, "studio:image.new", json!({"name": "I", "width": 16, "height": 16}))["result"]["asset"].clone();
    let rev = s.revision();
    let docs = s.images.documents().len();
    s.execute_with("image:shape.create", json!({"asset": img, "kind": "rect", "rect": [0, 0, 8, 8], "fill": "#ff0000"}), &Request { dry_run: true, ..Default::default() }).unwrap();
    s.execute_with("studio:image.new", json!({"name": "J"}), &Request { dry_run: true, ..Default::default() }).unwrap();
    assert_eq!((s.revision(), s.images.documents().len(), s.assets().count()), (rev, docs, 2));
}

/// A 64×64 sequence with a 64×64 composition holding a solid; returns the solid's layer.
fn solid_project(color: &str) -> (Studio, Value) {
    let mut s = Studio::new();
    ok(&mut s, "studio:comp.new", json!({"name": "C", "width": 64, "height": 64, "fps": 30, "duration": 1}));
    let layer = ok(&mut s, "motion:layer.newSolid", json!({"asset": 1, "color": color}))["result"]["layer"].clone();
    ok(&mut s, "timeline:file.newSequence", json!({"name": "S", "width": 64, "height": 64, "fps": 30, "video": 1, "audio": 1}));
    ok(&mut s, "studio:asset.place", json!({"asset": 1, "track": "V1", "seconds": 0}));
    (s, layer)
}

fn center(s: &Studio) -> [u8; 4] {
    let (w, h, px, _) = s.render_rgba(0.5, 1.0).unwrap();
    let i = ((h / 2 * w + w / 2) * 4) as usize;
    [px[i], px[i + 1], px[i + 2], px[i + 3]]
}

#[test]
fn a_preview_shows_a_change_and_keeps_nothing() {
    let (mut s, layer) = solid_project("#ff8000");
    let before = center(&s);
    let (shown, exact) = s
        .preview_with(vec![("motion:prop.set".into(), json!({"asset": 1, "layer": layer, "path": "transform/opacity", "value": 0}))], |s| center(s))
        .unwrap();
    assert!(exact);
    assert!(shown[0] < 20, "the preview should show the solid gone: {shown:?}");
    assert_eq!(center(&s), before, "the preview kept its change");
}

#[test]
fn composition_frames_reach_the_timeline_in_float_without_drift() {
    // 8-bit colours survive the float path exactly (sRGB → linear → sRGB)
    for (hex, rgb) in [("#ff8000", [255, 128, 0]), ("#336699", [0x33, 0x66, 0x99]), ("#010203", [1, 2, 3])] {
        let (s, _) = solid_project(hex);
        let c = center(&s);
        for k in 0..3 {
            assert!((i32::from(c[k]) - rgb[k]).abs() <= 1, "{hex}: got {c:?}");
        }
    }
}

#[test]
fn composition_audio_joins_the_timeline() {
    let mut s = Studio::new();
    ok(&mut s, "studio:comp.new", json!({"name": "Beep", "width": 32, "height": 32, "fps": 30, "duration": 2}));
    ok(&mut s, "motion:layer.newSolid", json!({"asset": 1, "color": "#000000"}));
    ok(&mut s, "motion:effect.apply", json!({"asset": 1, "effect": "ec.audio.tone"}));
    ok(&mut s, "motion:layer.setSwitch", json!({"asset": 1, "switch": "audio", "value": true}));
    let (frames, peak) = s.asset_audio(videonow_core::AssetId(1), 0.5, 4800).unwrap();
    assert_eq!(frames, 4800);
    assert!(peak > 0.01, "the tone composition is silent on the timeline (peak {peak})");
}
