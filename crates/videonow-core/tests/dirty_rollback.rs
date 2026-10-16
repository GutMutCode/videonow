use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use videonow_core::{Request, Studio};

struct ProjectDirectory(PathBuf);

const PROJECT_FILES: [&str; 5] = ["manifest.json", "journal.json", "timeline.fcproj", "motion.ecproj", "images/2.pcraft"];
static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);

impl ProjectDirectory {
    fn new() -> Self {
        let unique = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let counter = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("videonow-dirty-rollback-{}-{unique}-{counter}.videonow", std::process::id()));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn files(&self) -> Vec<(Vec<u8>, SystemTime)> {
        PROJECT_FILES
            .into_iter()
            .map(|name| {
                let path = self.0.join(name);
                (std::fs::read(&path).unwrap(), std::fs::metadata(path).unwrap().modified().unwrap())
            })
            .collect()
    }

    fn assert_files_unchanged(&self, expected: &[(Vec<u8>, SystemTime)]) {
        let actual = self.files();
        assert_eq!(actual.len(), expected.len());
        for (name, ((actual_bytes, actual_time), (expected_bytes, expected_time))) in PROJECT_FILES.iter().zip(actual.iter().zip(expected)) {
            assert!(actual_bytes == expected_bytes, "{name}: saved contents changed");
            assert_eq!(actual_time, expected_time, "{name}: modification time changed");
        }
    }
}

impl Drop for ProjectDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn saved_project() -> (Studio, Value, ProjectDirectory) {
    let mut studio = Studio::new();
    studio.execute("studio:comp.new", json!({"name": "Title", "width": 64, "height": 64, "fps": 30, "duration": 4})).unwrap();
    let layer =
        studio.execute("motion:layer.newText", json!({"asset": 1, "text": "VIDEONOW", "size": 8, "position": [0, 20]})).unwrap()["result"]["layer"].clone();
    studio.execute("studio:image.new", json!({"name": "Image", "width": 16, "height": 16})).unwrap();
    studio.execute("timeline:file.newSequence", json!({"name": "Sequence", "width": 64, "height": 64, "fps": 30, "video": 1, "audio": 1})).unwrap();
    studio.execute("studio:asset.place", json!({"asset": 1, "track": "V1", "seconds": 0})).unwrap();
    let directory = ProjectDirectory::new();
    studio.execute("studio:project.save", json!({"path": directory.0})).unwrap();
    assert!(!studio.is_dirty());
    (studio, layer, directory)
}

fn text_step(layer: &Value) -> (String, Value) {
    ("motion:layer.setText".into(), json!({"asset": 1, "layer": layer, "text": "MY FIRST VIDEO"}))
}

fn edits(layer: &Value) -> Vec<(String, Value)> {
    vec![
        text_step(layer),
        ("image:shape.create".into(), json!({"asset": 2, "kind": "rect", "rect": [0, 0, 8, 8], "fill": "#ff0000"})),
        ("timeline:sequence.settings".into(), json!({"width": 80})),
    ]
}

fn invalid_step(layer: &Value) -> (String, Value) {
    ("motion:prop.set".into(), json!({"asset": 1, "layer": layer, "path": "missing/property", "value": 0}))
}

fn snapshot(studio: &mut Studio) -> Value {
    let mut inspect = studio.execute("studio:inspect", json!({})).unwrap()["result"].clone();
    for image in inspect["images"].as_array_mut().unwrap() {
        image.as_object_mut().unwrap().remove("revision");
    }
    json!({"inspect": inspect, "timeline": &*studio.timeline.project, "motion": &*studio.motion.project, "journal": studio.journal()})
}

#[test]
fn repeated_text_previews_keep_a_saved_project_clean_and_do_not_save() {
    let (mut studio, layer, directory) = saved_project();
    let before = snapshot(&mut studio);
    let files = directory.files();
    for _ in 0..3 {
        let (frame, restored) = studio
            .preview_with(vec![text_step(&layer)], |preview| {
                assert!(serde_json::to_string(&*preview.motion.project).unwrap().contains("MY FIRST VIDEO"));
                preview.render_rgba(2.0, 1.0).unwrap()
            })
            .unwrap();
        assert!(restored);
        assert_eq!((frame.0, frame.1), (64, 64));
        assert!(frame.3.is_empty());
        assert!(!studio.is_dirty(), "a restored text preview marked a saved project dirty");
        assert!(studio.sync_editors("ui").is_none());
        assert_eq!(snapshot(&mut studio), before);
        directory.assert_files_unchanged(&files);
    }
    let (command, params) = text_step(&layer);
    studio.execute(&command, params).unwrap();
    assert!(studio.is_dirty(), "a real edit after previews must still need saving");
    directory.assert_files_unchanged(&files);
    studio.execute("studio:project.save", json!({})).unwrap();
    assert!(!studio.is_dirty());
    assert!(directory.files() != files, "saving the real edit did not update the project files");
}

#[test]
fn dry_runs_in_each_engine_preserve_saved_state() {
    let (mut studio, layer, directory) = saved_project();
    let before = snapshot(&mut studio);
    let files = directory.files();
    let image = studio.images.documents()[0].doc.clone();
    for (command, params) in edits(&layer) {
        let reply = studio.execute_with(&command, params, &Request { dry_run: true, ..Default::default() }).unwrap();
        assert_eq!(reply["receipt"]["restored"], true);
        assert!(!studio.is_dirty(), "{command} dry run marked a saved project dirty");
        assert_eq!(snapshot(&mut studio), before);
        assert!(Arc::ptr_eq(&studio.images.documents()[0].doc, &image));
        directory.assert_files_unchanged(&files);
    }
}

#[test]
fn batch_dry_runs_restore_clean_state_across_all_engines() {
    let (mut studio, layer, directory) = saved_project();
    let before = snapshot(&mut studio);
    let files = directory.files();
    let image = studio.images.documents()[0].doc.clone();
    for atomic in [true, false] {
        let reply = studio.batch(edits(&layer), atomic, &Request { dry_run: true, ..Default::default() }).unwrap();
        assert_eq!(reply["receipt"]["restored"], true);
        assert!(!studio.is_dirty());
        assert_eq!(snapshot(&mut studio), before);
        assert!(Arc::ptr_eq(&studio.images.documents()[0].doc, &image));
        directory.assert_files_unchanged(&files);
    }
}

#[test]
fn failed_atomic_batches_restore_clean_state_across_all_engines() {
    let (mut studio, layer, directory) = saved_project();
    let before = snapshot(&mut studio);
    let files = directory.files();
    let image = studio.images.documents()[0].doc.clone();
    let mut steps = edits(&layer);
    steps.push(invalid_step(&layer));
    let error = studio.batch(steps, true, &Request::default()).unwrap_err().to_string();
    assert!(error.contains("step 3") && error.contains("taken back"), "{error}");
    assert!(!studio.is_dirty());
    assert_eq!(snapshot(&mut studio), before);
    assert!(Arc::ptr_eq(&studio.images.documents()[0].doc, &image));
    directory.assert_files_unchanged(&files);
}

#[test]
fn preview_step_and_render_errors_restore_clean_state() {
    let (mut studio, layer, directory) = saved_project();
    let before = snapshot(&mut studio);
    let files = directory.files();
    let mut called = false;
    let error = studio.preview_with(vec![text_step(&layer), invalid_step(&layer)], |_| called = true).unwrap_err().to_string();
    assert!(error.contains("step 1"), "{error}");
    assert!(!called);
    assert!(!studio.is_dirty());
    assert_eq!(snapshot(&mut studio), before);
    let (rendered, restored) = studio.preview_with(vec![text_step(&layer)], |_| Err::<(), _>("render failed")).unwrap();
    assert_eq!(rendered, Err("render failed"));
    assert!(restored);
    assert!(!studio.is_dirty());
    assert_eq!(snapshot(&mut studio), before);
    directory.assert_files_unchanged(&files);
}

#[test]
fn rollback_never_clears_preexisting_unsaved_changes() {
    let (mut studio, layer, directory) = saved_project();
    studio.execute("motion:layer.setText", json!({"asset": 1, "layer": layer, "text": "Unsaved title"})).unwrap();
    assert!(studio.is_dirty());
    let before = snapshot(&mut studio);
    let files = directory.files();
    let (command, params) = text_step(&layer);
    studio.execute_with(&command, params, &Request { dry_run: true, ..Default::default() }).unwrap();
    assert!(studio.is_dirty());
    assert_eq!(snapshot(&mut studio), before);
    studio.batch(edits(&layer), true, &Request { dry_run: true, ..Default::default() }).unwrap();
    assert!(studio.is_dirty());
    assert_eq!(snapshot(&mut studio), before);
    let (_, restored) = studio.preview_with(vec![text_step(&layer)], |_| ()).unwrap();
    assert!(restored);
    assert!(studio.is_dirty());
    assert_eq!(snapshot(&mut studio), before);
    let failed = vec![text_step(&layer), invalid_step(&layer)];
    assert!(studio.batch(failed.clone(), true, &Request::default()).is_err());
    assert!(studio.preview_with(failed, |_| ()).is_err());
    assert!(studio.is_dirty());
    assert_eq!(snapshot(&mut studio), before);
    directory.assert_files_unchanged(&files);
}

#[test]
fn read_only_preview_preserves_clean_state_and_successful_atomic_edits_do_not() {
    let (mut studio, layer, directory) = saved_project();
    let before = snapshot(&mut studio);
    let files = directory.files();
    let (_, restored) = studio.preview_with(vec![("studio:inspect".into(), json!({}))], |_| ()).unwrap();
    assert!(restored);
    assert!(!studio.is_dirty());
    assert_eq!(snapshot(&mut studio), before);
    studio.batch(vec![text_step(&layer)], true, &Request::default()).unwrap();
    assert!(studio.is_dirty());
    directory.assert_files_unchanged(&files);
}

#[test]
fn incomplete_preview_rollback_never_marks_a_changed_project_clean() {
    let (mut studio, _, directory) = saved_project();
    let files = directory.files();
    let (_, restored) = studio
        .preview_with(Vec::new(), |preview| {
            Arc::make_mut(&mut preview.motion.project).items.values_mut().next().unwrap().name = "Unjournaled change".into();
        })
        .unwrap();
    assert!(!restored);
    assert!(studio.is_dirty());
    directory.assert_files_unchanged(&files);
}
