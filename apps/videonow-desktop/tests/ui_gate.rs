//! Phase 2 gate: a person builds the demo project through the window alone — clicks, keys and
//! drags on the shell and the three editors, no commands typed — and the result is one live
//! project: an image inside a composition on a sequence.
//!
//! Runs offscreen (egui_kittest with wgpu; on a machine without a GPU, Mesa's lavapipe works).

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable as _;
use serde_json::{Value, json};
use videonow_desktop::{Options, VideoNowApp, Workspace};

fn app() -> Harness<'static, VideoNowApp> {
    let options = Options { dialogs: false, gpu: false, ..Default::default() };
    let mut h = Harness::builder().with_size(egui::vec2(1600.0, 980.0)).with_pixels_per_point(1.0).with_max_steps(64).wgpu().build_eframe(move |cc| VideoNowApp::new(cc, options));
    h.run_steps(8);
    h
}

/// Click the first widget with this accessible label (the shell's tabs come before the
/// editors' menus of the same name).
fn click(h: &mut Harness<'static, VideoNowApp>, label: &str) {
    h.get_all_by_label(label).next().unwrap_or_else(|| panic!("no widget labelled {label:?}")).click();
    h.run_steps(6);
}

/// Click the first widget whose label contains `part` (menu items carry their shortcut).
fn click_containing(h: &mut Harness<'static, VideoNowApp>, part: &str) {
    h.get_all_by_label_contains(part).next().unwrap_or_else(|| panic!("no widget with {part:?}")).click();
    h.run_steps(6);
}

fn hover_containing(h: &mut Harness<'static, VideoNowApp>, part: &str) {
    h.get_all_by_label_contains(part).next().unwrap_or_else(|| panic!("no widget with {part:?}")).hover();
    h.run_steps(6);
}

fn inspect(h: &mut Harness<'static, VideoNowApp>) -> Value {
    h.state_mut().run("api", "studio:inspect", json!({})).map(|v| v["result"].clone()).unwrap_or_default()
}

fn drag(h: &mut Harness<'static, VideoNowApp>, from: egui::Pos2, to: egui::Pos2) {
    h.event(egui::Event::PointerMoved(from));
    h.step();
    h.event(egui::Event::PointerButton { pos: from, button: egui::PointerButton::Primary, pressed: true, modifiers: egui::Modifiers::NONE });
    h.step();
    for i in 1..=10 {
        h.event(egui::Event::PointerMoved(from + (to - from) * (i as f32 / 10.0)));
        h.step();
    }
    h.event(egui::Event::PointerButton { pos: to, button: egui::PointerButton::Primary, pressed: false, modifiers: egui::Modifiers::NONE });
    h.run_steps(8);
}

#[test]
fn the_demo_project_can_be_built_by_hand() {
    let mut h = app();

    // 1. an image: the asset list's "+ Image" opens it in the image editor
    click(&mut h, "+ Image");
    assert_eq!(h.state().workspace, Workspace::Image);
    let img = inspect(&mut h)["images"][0]["asset"].as_u64().expect("an image asset");

    // paint on it with the image editor: white foreground, the Rectangle tool dragged across
    // the canvas
    click(&mut h, "Swap colours (X)");
    click(&mut h, "Rectangle Tool  (U)");
    drag(&mut h, egui::pos2(450.0, 350.0), egui::pos2(700.0, 600.0));
    let layers = inspect(&mut h)["images"][0]["layers"].as_u64().unwrap_or(0);
    assert!(layers >= 2, "drawing made no shape layer ({layers} layers)");

    // 2. a composition: "+ Composition" opens it in the motion editor
    click(&mut h, "+ Composition");
    assert_eq!(h.state().workspace, Workspace::Motion);
    let comp = inspect(&mut h)["compositions"][0]["asset"].as_u64().expect("a composition asset");

    // a text layer from the motion editor's own menu: Layer ▸ New ▸ Text, then type
    click(&mut h, "Layer");
    hover_containing(&mut h, " New ⏵");
    click_containing(&mut h, " Text Ctrl");
    h.event(egui::Event::Text("VIDEONOW".into()));
    h.run_steps(6);
    h.key_press(egui::Key::Escape);
    h.run_steps(6);
    assert_eq!(inspect(&mut h)["compositions"][0]["layers"].as_u64(), Some(1), "no text layer");

    // the image goes into the composition from the asset list's context menu
    h.get_by_label("◩  Image 1").click_secondary();
    h.run_steps(6);
    click(&mut h, "Add to the open composition");
    let layers = inspect(&mut h)["compositions"][0]["layers"].as_u64().unwrap_or(0);
    assert!(layers >= 2, "the composition should hold the text and the image, has {layers} layers");

    // 3. a sequence, from the timeline editor's own menu: File ▸ New ▸ Sequence…, OK
    click(&mut h, "Edit");
    assert_eq!(h.state().workspace, Workspace::Edit);
    click(&mut h, "File");
    hover_containing(&mut h, "New ⏵");
    click_containing(&mut h, "Sequence… Ctrl+N");
    h.run_steps(4);
    click(&mut h, "OK");
    assert!(inspect(&mut h)["sequence"].is_object(), "no sequence after File ▸ New ▸ Sequence…");

    // the composition goes on it from the asset list
    h.get_by_label("▶  Composition 1").click_secondary();
    h.run_steps(6);
    click(&mut h, "Place on the timeline at the playhead");
    let i = inspect(&mut h);
    assert!(i["sequence"]["clips"].as_u64().unwrap_or(0) >= 1, "nothing on the sequence: {i}");
    assert_eq!(i["compositions"][0]["onTimeline"].is_u64(), true);

    // one live project: the sequence frame shows the composition, which shows the image
    let s = &h.state().studio;
    let (w, hgt, px, offline) = s.render_rgba(0.5, 0.25).expect("render");
    assert!(offline.is_empty(), "offline media: {offline:?}");
    let lit = px.chunks_exact(4).filter(|p| p[0] > 40 || p[1] > 40 || p[2] > 40).count();
    assert!(lit > (w * hgt / 400) as usize, "the frame is almost empty ({lit} lit pixels of {})", w * hgt);

    // every change was made by hand, and each has a receipt
    let journal = h.state().studio.journal().to_vec();
    assert!(!journal.is_empty());
    for e in &journal {
        let by = e["receipt"]["by"].as_str().unwrap_or("");
        assert_eq!(by, "ui", "a change not made through the window: {e}");
    }
    let commands: Vec<String> = journal.iter().filter_map(|e| e["command"].as_str().map(str::to_string)).collect();
    assert!(commands.iter().any(|c| c == "ui:image"), "{commands:?}");
    assert!(commands.iter().any(|c| c == "ui:motion"), "{commands:?}");
    assert!(commands.iter().any(|c| c == "ui:edit"), "{commands:?}");
    let _ = (img, comp);

    // what the person sees at the end (target/tmp/ui-gate.png), with the activity log open
    h.state_mut().show_agent = true;
    h.run_steps(6);
    if let Ok(shot) = h.render() {
        let _ = shot.save(std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("ui-gate.png"));
    }
}
