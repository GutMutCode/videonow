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
    s.execute_with(
        "image:shape.create",
        json!({"asset": img, "kind": "rect", "rect": [0, 0, 8, 8], "fill": "#ff0000"}),
        &Request { dry_run: true, ..Default::default() },
    )
    .unwrap();
    s.execute_with("studio:image.new", json!({"name": "J"}), &Request { dry_run: true, ..Default::default() }).unwrap();
    assert_eq!((s.revision(), s.images.documents().len(), s.assets().count()), (rev, docs, 2));
}

fn request_test_dir(name: &str) -> std::path::PathBuf {
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let path = std::env::temp_dir().join(format!("videonow-requests-{name}-{}-{stamp}", std::process::id()));
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn request_state(studio: &mut Studio) -> Value {
    let inspect = ok(studio, "studio:inspect", json!({}))["result"].clone();
    json!({"inspect": inspect, "journal": studio.journal()})
}

fn assert_rollback_rejection(studio: &mut Studio, id: &str, params: Value) {
    let before = request_state(studio);
    let error = studio.execute_with(id, params.clone(), &Request { dry_run: true, ..Default::default() }).unwrap_err().to_string();
    assert!(error.contains("cannot be rolled back"), "{id}: {error}");
    assert_eq!(request_state(studio), before, "dry run changed state: {id}");

    let steps = vec![("studio:comp.new".into(), json!({"name": "Must not be created", "width": 16, "height": 16})), (id.into(), params)];
    for (atomic, dry_run) in [(true, false), (true, true), (false, true)] {
        let error = studio.batch(steps.clone(), atomic, &Request { dry_run, ..Default::default() }).unwrap_err().to_string();
        assert!(error.contains("step 1") && error.contains("no steps were run"), "{id}: {error}");
        assert_eq!(request_state(studio), before, "batch changed state: {id}, atomic={atomic}, dry_run={dry_run}");
    }

    let mut looked = false;
    let error = studio.preview_with(steps, |_| looked = true).unwrap_err().to_string();
    assert!(error.contains("step 1") && error.contains("no steps were run"), "{id}: {error}");
    assert!(!looked, "preview callback ran: {id}");
    assert_eq!(request_state(studio), before, "preview changed state: {id}");
}

#[test]
fn speculative_saves_and_renders_never_create_or_overwrite_files() {
    let root = request_test_dir("files");
    let project = root.join("project.videonow");
    let mut studio = Studio::new();
    ok(&mut studio, "studio:comp.new", json!({"width": 16, "height": 16}));

    assert_rollback_rejection(&mut studio, "studio:project.save", json!({"path": project}));
    assert!(!project.exists());
    assert!(studio.project_path().is_none());
    assert!(studio.is_dirty());

    ok(&mut studio, "studio:project.save", json!({"path": project}));
    let files = ["manifest.json", "journal.json", "timeline.fcproj", "motion.ecproj"];
    let saved: Vec<_> = files.iter().map(|name| std::fs::read(project.join(name)).unwrap()).collect();
    assert!(!studio.is_dirty());
    ok(&mut studio, "studio:comp.new", json!({"name": "Unsaved", "width": 16, "height": 16}));
    assert!(studio.is_dirty());
    assert_rollback_rejection(&mut studio, "studio:project.save", json!({"path": project}));
    assert_rollback_rejection(&mut studio, "studio:project.save", json!({}));
    assert_eq!(studio.project_path(), Some(project.as_path()));
    assert!(studio.is_dirty());
    for (name, bytes) in files.iter().zip(saved) {
        assert_eq!(std::fs::read(project.join(name)).unwrap(), bytes, "{name} was overwritten");
    }

    let frame = root.join("frame.png");
    assert_rollback_rejection(&mut studio, "studio:render.frame", json!({"path": frame}));
    assert!(!frame.exists());
    std::fs::write(&frame, b"existing output").unwrap();
    assert_rollback_rejection(&mut studio, "studio:render.frame", json!({"path": frame}));
    assert_eq!(std::fs::read(&frame).unwrap(), b"existing output");
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn speculative_file_writers_are_rejected_before_engine_execution() {
    let root = request_test_dir("engine-file-writers");
    let collected = root.join("collected");
    let fill = root.join("fill");
    let mut studio = Studio::new();
    let asset = ok(&mut studio, "studio:comp.new", json!({"width": 16, "height": 16}))["result"]["asset"].clone();
    assert_rollback_rejection(&mut studio, "motion:file.collectFiles", json!({"folder": collected}));
    assert!(!collected.exists());

    let layer = ok(&mut studio, "motion:layer.newSolid", json!({"asset": asset, "color": "#ffffff"}))["result"]["layer"].clone();
    for (id, params) in [
        ("motion:contentFill.generate", json!({"asset": asset, "layer": layer, "outputDir": fill, "wait": true})),
        ("motion:layer.newContentAwareFill", json!({"asset": asset, "layer": layer, "outputDir": fill, "wait": true})),
        ("motion:essential.importTemplate", json!({"path": root.join("input.ectemplate")})),
        ("motion:file.importLottie", json!({"path": root.join("input.json")})),
    ] {
        assert_rollback_rejection(&mut studio, id, params);
    }
    assert!(!fill.exists());
    assert_eq!(std::fs::read_dir(&root).unwrap().count(), 0);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn speculative_operations_reject_nested_dispatch_before_any_step_runs() {
    let root = request_test_dir("nested");
    let project = root.join("nested.videonow");
    let mut studio = Studio::new();
    ok(&mut studio, "studio:comp.new", json!({"width": 16, "height": 16}));
    assert_rollback_rejection(&mut studio, "studio:batch", json!({"atomic": false, "steps": [{"id": "studio:project.save", "params": {"path": project}}]}));
    assert_rollback_rejection(
        &mut studio,
        "motion:engine.batch",
        json!({"asset": 1, "atomic": false, "steps": [{"command": "file.saveAs", "params": {"path": project}}]}),
    );
    for id in [
        "motion:file.runScript",
        "motion:script.run",
        "motion:script.runTasks",
        "motion:window.scriptPanel",
        "motion:window.restoreScriptPanels",
        "motion:scriptui.click",
        "motion:scriptui.set",
        "motion:scriptui.mouse",
        "motion:scriptui.close",
        "image:actions.play",
        "image:actions.conditional",
        "image:file.automate.batch",
        "image:file.automate.createDroplet",
        "image:file.automate.runDroplet",
        "image:file.scripts.imageProcessor",
        "image:file.scripts.scriptEventsManager",
    ] {
        let (domain, command) = id.split_once(':').unwrap();
        assert!(videonow_core::manifest::effect_of(domain, command).is_some(), "missing command: {id}");
        assert_rollback_rejection(&mut studio, id, json!({}));
    }
    assert!(!project.exists());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn speculative_operations_reject_project_replacement_and_history_changes() {
    let mut studio = Studio::new();
    ok(&mut studio, "studio:comp.new", json!({"width": 16, "height": 16}));
    for id in [
        "studio:project.new",
        "studio:project.open",
        "studio:undo",
        "timeline:edit.undo",
        "timeline:edit.redo",
        "motion:edit.undo",
        "motion:edit.redo",
        "motion:edit.history",
        "motion:edit.history.goto",
        "motion:edit.purgeUndo",
        "motion:edit.purge",
        "image:edit.undo",
        "image:edit.redo",
        "image:history.undo",
        "image:history.redo",
        "image:history.deleteState",
        "image:history.newDocument",
        "image:history.newSnapshot",
        "image:history.selectSnapshot",
        "image:history.renameSnapshot",
        "image:history.deleteSnapshot",
        "image:history.selectState",
        "image:edit.purge.undo",
        "image:edit.purge.histories",
        "image:edit.purge.all",
        "motion:file.installScript",
    ] {
        let (domain, command) = id.split_once(':').unwrap();
        assert!(videonow_core::manifest::effect_of(domain, command).is_some(), "missing command: {id}");
        assert_rollback_rejection(&mut studio, id, json!({}));
    }
}

#[test]
fn read_only_history_and_script_queries_remain_available_in_dry_runs() {
    let mut studio = Studio::new();
    let before = request_state(&mut studio);
    for id in ["timeline:history.list", "motion:edit.history.list", "motion:file.scripts.list"] {
        let result = studio.execute_with(id, json!({}), &Request { dry_run: true, ..Default::default() }).unwrap();
        assert_eq!(result["receipt"]["dryRun"], true);
        assert_eq!(result["receipt"]["restored"], true);
        assert_eq!(request_state(&mut studio), before);
    }
}

#[test]
fn non_atomic_batches_allow_explicit_saves() {
    let root = request_test_dir("non-atomic");
    let project = root.join("project.videonow");
    let mut studio = Studio::new();
    let result = ok(
        &mut studio,
        "studio:batch",
        json!({"atomic": false, "steps": [
            {"id": "studio:comp.new", "params": {"width": 16, "height": 16}},
            {"id": "studio:project.save", "params": {"path": project}}
        ]}),
    );
    assert_eq!(result["completed"], 2);
    assert!(project.join("manifest.json").is_file());
    assert_eq!(studio.project_path(), Some(project.as_path()));
    assert!(!studio.is_dirty());
    std::fs::remove_dir_all(root).unwrap();
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
    let (shown, exact) =
        s.preview_with(vec![("motion:prop.set".into(), json!({"asset": 1, "layer": layer, "path": "transform/opacity", "value": 0}))], |s| center(s)).unwrap();
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
