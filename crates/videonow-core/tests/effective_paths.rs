use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use videonow_core::{Policy, Studio, StudioError};

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new(name: &str) -> Self {
        let unique = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let path = std::env::temp_dir().join(format!("videonow-effective-paths-{name}-{}-{unique}", std::process::id()));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn snapshot(studio: &mut Studio) -> Value {
    let inspect = studio.execute("studio:inspect", json!({})).unwrap();
    json!({
        "inspect": inspect,
        "timeline": &*studio.timeline.project,
        "timelinePath": studio.timeline.path,
        "timelineRevision": studio.timeline.revision,
        "motion": &*studio.motion.project,
        "motionPath": studio.motion.path,
        "motionRevision": studio.motion.revision,
        "journal": studio.journal(),
    })
}

fn project_files(path: &Path) -> BTreeMap<String, (Vec<u8>, SystemTime)> {
    ["manifest.json", "timeline.fcproj", "motion.ecproj", "journal.json"]
        .into_iter()
        .map(|name| {
            let file = path.join(name);
            (name.to_string(), (std::fs::read(&file).unwrap(), std::fs::metadata(file).unwrap().modified().unwrap()))
        })
        .collect()
}

fn add_composition(studio: &mut Studio) {
    studio.execute("studio:comp.new", json!({"name": "Changed", "width": 16, "height": 16})).unwrap();
}

#[test]
fn default_render_path_obeys_policy_without_changing_process_cwd() {
    const CHILD_FLAG: &str = "VIDEONOW_EFFECTIVE_PATHS_RENDER_CHILD";
    if std::env::var_os(CHILD_FLAG).is_none() {
        let directory = TestDirectory::new("render");
        let output = Command::new(std::env::current_exe().unwrap())
            .arg("--exact")
            .arg("default_render_path_obeys_policy_without_changing_process_cwd")
            .arg("--nocapture")
            .env(CHILD_FLAG, "1")
            .current_dir(&directory.0)
            .output()
            .unwrap();
        assert!(output.status.success(), "{}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
        return;
    }

    let directory = std::env::current_dir().unwrap();
    let allowed = directory.join("allowed");
    std::fs::create_dir(&allowed).unwrap();
    let frame = directory.join("frame.png");
    let mut studio = Studio::new();
    studio.execute("timeline:file.newSequence", json!({"name": "S", "width": 16, "height": 16, "fps": 30, "video": 1, "audio": 1})).unwrap();
    studio.set_policy(Policy::confined_to(&[allowed]));
    let before = snapshot(&mut studio);

    for params in [json!({}), Value::Null, json!({"path": null})] {
        let error = studio.execute("studio:render.frame", params).unwrap_err();
        assert!(matches!(error, StudioError::Denied(_)), "{error}");
        assert!(error.to_string().contains("frame.png"), "{error}");
        assert!(!frame.exists());
        assert_eq!(snapshot(&mut studio), before);
    }

    std::fs::write(&frame, b"existing frame").unwrap();
    let modified = std::fs::metadata(&frame).unwrap().modified().unwrap();
    assert!(matches!(studio.execute("studio:render.frame", json!({})), Err(StudioError::Denied(_))));
    assert_eq!(std::fs::read(&frame).unwrap(), b"existing frame");
    assert_eq!(std::fs::metadata(&frame).unwrap().modified().unwrap(), modified);
    assert_eq!(snapshot(&mut studio), before);

    studio.set_policy(Policy::confined_to(&[directory]));
    for params in [json!({}), Value::Null, json!({"path": null})] {
        let result = studio.execute("studio:render.frame", params).unwrap();
        assert_eq!(result["result"]["path"], "frame.png");
        assert_eq!(image::image_dimensions(&frame).unwrap(), (16, 16));
        assert_eq!(snapshot(&mut studio), before);
    }
}

#[test]
fn fallback_save_rechecks_the_current_policy_before_any_write() {
    let directory = TestDirectory::new("denied-save");
    let project = directory.0.join("original.videonow");
    let allowed = directory.0.join("allowed");
    std::fs::create_dir(&allowed).unwrap();
    let mut studio = Studio::new();
    studio.execute("studio:project.save", json!({"path": project.to_str().unwrap()})).unwrap();
    add_composition(&mut studio);
    studio.set_policy(Policy::confined_to(&[allowed]));
    let before = snapshot(&mut studio);
    let files = project_files(&project);

    for params in [json!({}), Value::Null, json!({"path": null})] {
        let error = studio.execute("studio:project.save", params).unwrap_err();
        assert!(matches!(error, StudioError::Denied(_)), "{error}");
        assert!(error.to_string().contains("original.videonow"), "{error}");
        assert_eq!(snapshot(&mut studio), before);
        assert_eq!(project_files(&project), files);
        assert_eq!(studio.project_path(), Some(project.as_path()));
        assert!(studio.is_dirty());
    }
}

#[test]
fn fallback_save_still_works_inside_the_current_policy() {
    let directory = TestDirectory::new("allowed-save");
    let project = directory.0.join("project.videonow");
    let mut studio = Studio::new();
    studio.set_policy(Policy::confined_to(std::slice::from_ref(&directory.0)));
    studio.execute("studio:project.save", json!({"path": project.to_str().unwrap()})).unwrap();

    for params in [json!({}), Value::Null, json!({"path": null})] {
        let previous_motion = std::fs::read(project.join("motion.ecproj")).unwrap();
        add_composition(&mut studio);
        assert!(studio.is_dirty());
        let result = studio.execute("studio:project.save", params).unwrap();
        assert_eq!(result["result"]["path"], project.to_str().unwrap());
        assert_eq!(studio.project_path(), Some(project.as_path()));
        assert!(!studio.is_dirty());
        assert_ne!(std::fs::read(project.join("motion.ecproj")).unwrap(), previous_motion);
    }
}

#[test]
fn project_extension_is_applied_before_checking_the_output_folder() {
    let directory = TestDirectory::new("extension");
    let requested = directory.0.join("allowed");
    std::fs::create_dir(&requested).unwrap();
    let mut studio = Studio::new();
    studio.set_policy(Policy::confined_to(std::slice::from_ref(&requested)));
    let before = snapshot(&mut studio);
    let error = studio.execute("studio:project.save", json!({"path": requested.to_str().unwrap()})).unwrap_err();
    assert!(matches!(error, StudioError::Denied(_)), "{error}");
    assert!(!requested.with_extension("videonow").exists());
    assert!(std::fs::read_dir(&requested).unwrap().next().is_none());
    assert_eq!(snapshot(&mut studio), before);
}

#[test]
fn output_defaults_preserve_parameter_validation() {
    let directory = TestDirectory::new("params");
    let mut studio = Studio::new();
    let before = snapshot(&mut studio);
    for command in ["studio:render.frame", "studio:project.save"] {
        for params in [json!([]), json!(false), json!("path")] {
            let error = studio.execute(command, params).unwrap_err().to_string();
            assert!(error.contains("params must be a JSON object"), "{error}");
        }
        let error = studio.execute(command, json!({"path": directory.0.join("output.png").to_str().unwrap(), "typo": true})).unwrap_err().to_string();
        assert!(error.contains("unknown parameter `typo`"), "{error}");
    }
    let error = studio.execute("studio:project.save", Value::Null).unwrap_err().to_string();
    assert!(error.contains("never saved"), "{error}");
    assert!(std::fs::read_dir(&directory.0).unwrap().next().is_none());
    assert_eq!(snapshot(&mut studio), before);
}
