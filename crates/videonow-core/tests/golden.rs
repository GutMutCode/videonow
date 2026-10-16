//! Phase 3 gate: the demo project renders the same pictures as the reviewed golden frames, the
//! same every time, the same after saving and reopening; and a frame renders within the
//! playback budget.
//!
//! The goldens are in `tests/golden/`. After a deliberate change to the picture, look at the
//! new frames and bless them: `VIDEONOW_BLESS=1 cargo test -p videonow-core --test golden`.

use std::path::{Path, PathBuf};
use std::time::Instant;

use serde_json::json;
use videonow_core::Studio;

const TIMES: [f64; 3] = [0.2, 1.0, 2.5];
const SCALE: f64 = 0.25;

fn golden_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden")
}

fn demo() -> Studio {
    let mut s = Studio::new();
    videonow_core::demo::build_quietly(&mut s).unwrap_or_else(|e| panic!("demo: {e}"));
    s
}

/// Largest per-channel difference, and how many pixels differ by more than 2.
fn compare(a: &[u8], b: &[u8]) -> (u8, usize) {
    let mut max = 0u8;
    let mut off = 0;
    for (p, q) in a.chunks_exact(4).zip(b.chunks_exact(4)) {
        let d = p.iter().zip(q).map(|(x, y)| x.abs_diff(*y)).max().unwrap_or(0);
        max = max.max(d);
        off += usize::from(d > 2);
    }
    (max, off)
}

#[test]
fn the_demo_matches_its_golden_frames() {
    let s = demo();
    let bless = std::env::var_os("VIDEONOW_BLESS").is_some();
    std::fs::create_dir_all(golden_dir()).unwrap();
    for t in TIMES {
        let (w, h, px, offline) = s.render_rgba(t, SCALE).unwrap();
        assert!(offline.is_empty(), "offline at {t}s: {offline:?}");
        let path = golden_dir().join(format!("demo-{t:.1}s.png"));
        if bless || !path.exists() {
            image::save_buffer(&path, &px, w, h, image::ColorType::Rgba8).unwrap();
            eprintln!("blessed {}", path.display());
            continue;
        }
        let g = image::open(&path).unwrap().to_rgba8();
        assert_eq!((g.width(), g.height()), (w, h), "{}: size changed", path.display());
        let (max, off) = compare(g.as_raw(), &px);
        // tolerance: rounding in float paths may move a channel by one or two levels
        assert!(off * 1000 <= (w * h) as usize, "{}: {off} pixels differ by more than 2 (max {max}); look at the frame and bless it if the change is intended", path.display());
    }
}

#[test]
fn rendering_is_deterministic_and_survives_save_and_open() {
    let mut s = demo();
    let first: Vec<Vec<u8>> = TIMES.iter().map(|t| s.render_rgba(*t, SCALE).unwrap().2).collect();
    let again: Vec<Vec<u8>> = TIMES.iter().map(|t| s.render_rgba(*t, SCALE).unwrap().2).collect();
    assert!(first == again, "two renders of the same frame differ");
    let dir = std::env::temp_dir().join(format!("videonow-golden-{}.videonow", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    s.execute("studio:project.save", json!({"path": dir.to_string_lossy()})).unwrap();
    let mut t = Studio::new();
    t.execute("studio:project.open", json!({"path": dir.to_string_lossy()})).unwrap();
    let reopened: Vec<Vec<u8>> = TIMES.iter().map(|x| t.render_rgba(*x, SCALE).unwrap().2).collect();
    assert!(first == reopened, "the reopened project renders differently");
    // the journal travelled with the project
    assert!(t.journal().iter().any(|e| e["command"] == "studio:comp.new"), "the journal was not saved with the project");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Playback budget: a half-resolution frame of the demo (1080p sequence, a composition with
/// text and a nested image) after an edit, and the same frame again from the caches.
#[test]
fn a_frame_renders_within_the_playback_budget() {
    let mut s = demo();
    s.render_rgba(1.0, 0.5).unwrap();
    // an edit invalidates the composition's frames: the next frame renders from scratch
    s.execute("motion:prop.set", json!({"asset": 2, "layer": 3, "path": "transform/position", "value": [960, 880]})).unwrap();
    let t = Instant::now();
    s.render_rgba(1.5, 0.5).unwrap();
    let cold = t.elapsed();
    let t = Instant::now();
    s.render_rgba(1.5, 0.5).unwrap();
    let warm = t.elapsed();
    eprintln!("half-resolution frame: {cold:?} after an edit, {warm:?} again");
    // generous: debug-profile builds on a two-core machine; a regression of several times fails
    let budget = std::time::Duration::from_millis(std::env::var("VIDEONOW_FRAME_BUDGET_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(4000));
    assert!(cold < budget, "a frame took {cold:?} (budget {budget:?})");
    assert!(warm < cold || warm < std::time::Duration::from_millis(50), "the second render was not faster ({warm:?} vs {cold:?})");
}
