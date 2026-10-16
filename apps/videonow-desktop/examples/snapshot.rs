//! Render the VideoNow window offscreen and save a PNG (no window, no display needed).
//!
//! ```sh
//! cargo run -p videonow-desktop --example snapshot -- --out ui.png --size 1600x980 --demo \
//!     --script '[["workspace","motion"],["steps",20]]'
//! ```
//!
//! Script steps (JSON array of `[op, arg]`):
//! * `["workspace", "edit"|"motion"|"image"]` — click that workspace tab
//! * `["click", "label"]` — click the widget with that accessible label
//! * `["open_asset", id]` — what clicking an asset in the asset list does
//! * `["agent_panel", true]`, `["assets_panel", false]` — show or hide a side panel
//! * `["palette", "query"]` — open the command palette and type
//! * `["steps", n]` — run n frames
//! * `["shot", "path.png"]` — save a PNG now

use egui_kittest::kittest::Queryable as _;
use serde_json::Value;
use videonow_desktop::{Options, VideoNowApp, Workspace};

fn arg(args: &[String], name: &str) -> Option<String> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1).cloned())
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let out = arg(&args, "--out").unwrap_or_else(|| "videonow.png".into());
    let (w, h) = arg(&args, "--size")
        .and_then(|s| s.split_once('x').and_then(|(a, b)| Some((a.parse::<f32>().ok()?, b.parse::<f32>().ok()?))))
        .unwrap_or((1600.0, 980.0));
    let scale: f32 = arg(&args, "--scale").and_then(|s| s.parse().ok()).unwrap_or(1.0);
    let script: Vec<(String, Value)> = arg(&args, "--script").map(|s| serde_json::from_str(&s).unwrap_or_else(|e| panic!("--script: {e}"))).unwrap_or_default();
    let options = Options {
        open: arg(&args, "--open").map(std::path::PathBuf::from),
        demo: args.iter().any(|a| a == "--demo"),
        workspace: arg(&args, "--workspace").and_then(|w| Workspace::parse(&w)).unwrap_or(Workspace::Edit),
        dialogs: false,
        gpu: !args.iter().any(|a| a == "--cpu"),
        ..Default::default()
    };
    let mut harness = egui_kittest::Harness::builder()
        .with_size(egui::vec2(w, h))
        .with_pixels_per_point(scale)
        .with_max_steps(64)
        .wgpu()
        .build_eframe(move |cc| VideoNowApp::new(cc, options));
    harness.run_steps(6);
    for (op, a) in script {
        match op.as_str() {
            "workspace" => {
                let label = Workspace::parse(a.as_str().unwrap_or("")).map(Workspace::label).unwrap_or("Edit");
                harness.get_by_label(label).click();
                harness.run_steps(6);
            }
            "click" => {
                harness.get_all_by_label(a.as_str().unwrap_or("")).next().unwrap_or_else(|| panic!("no widget labelled {a}")).click();
                harness.run_steps(6);
            }
            // click the first widget whose label contains the text
            "click_contains" => {
                harness.get_all_by_label_contains(a.as_str().unwrap_or("")).next().unwrap_or_else(|| panic!("no widget with {a}")).click();
                harness.run_steps(6);
            }
            "hover_contains" => {
                harness.get_all_by_label_contains(a.as_str().unwrap_or("")).next().unwrap_or_else(|| panic!("no widget with {a}")).hover();
                harness.run_steps(6);
            }
            "open_asset" => {
                let ctx = harness.ctx.clone();
                harness.state_mut().open_asset(videonow_core::AssetId(a.as_u64().unwrap_or(0)), &ctx);
                harness.run_steps(6);
            }
            "agent_panel" => {
                harness.state_mut().show_agent = a.as_bool().unwrap_or(true);
                harness.run_steps(4);
            }
            "assets_panel" => {
                harness.state_mut().show_assets = a.as_bool().unwrap_or(true);
                harness.run_steps(4);
            }
            "palette" => {
                harness.state_mut().palette.toggle();
                harness.run_steps(3);
                harness.event(egui::Event::Text(a.as_str().unwrap_or("").to_string()));
                harness.run_steps(4);
            }
            "steps" => harness.run_steps(a.as_u64().unwrap_or(1) as usize),
            // print the accessibility tree (what `click` can find), lines containing `a`
            "dump" => {
                let tree = format!("{:#?}", harness.root());
                let filter = a.as_str().unwrap_or("");
                for line in tree.lines().filter(|l| l.contains("label") && l.contains(filter)) {
                    println!("{}", line.trim());
                }
            }
            "right_click" => {
                harness.get_by_label(a.as_str().unwrap_or("")).click_secondary();
                harness.run_steps(6);
            }
            "key" => {
                if let Some(k) = egui::Key::from_name(a.as_str().unwrap_or("")) {
                    harness.key_press(k);
                }
                harness.run_steps(4);
            }
            "type" => {
                harness.event(egui::Event::Text(a.as_str().unwrap_or("").to_string()));
                harness.run_steps(4);
            }
            // drag from [x0, y0] to [x1, y1] in points
            "drag" => {
                let v: Vec<f32> = a.as_array().map(|v| v.iter().filter_map(|x| x.as_f64().map(|x| x as f32)).collect()).unwrap_or_default();
                if let [x0, y0, x1, y1] = v[..] {
                    let (p0, p1) = (egui::pos2(x0, y0), egui::pos2(x1, y1));
                    harness.event(egui::Event::PointerMoved(p0));
                    harness.step();
                    harness.event(egui::Event::PointerButton { pos: p0, button: egui::PointerButton::Primary, pressed: true, modifiers: egui::Modifiers::NONE });
                    harness.step();
                    for i in 1..=8 {
                        harness.event(egui::Event::PointerMoved(p0 + (p1 - p0) * (i as f32 / 8.0)));
                        harness.step();
                    }
                    harness.event(egui::Event::PointerButton { pos: p1, button: egui::PointerButton::Primary, pressed: false, modifiers: egui::Modifiers::NONE });
                    harness.run_steps(6);
                }
            }
            "print" => {
                let v = harness.state_mut().run("api", "studio:inspect", serde_json::json!({}));
                println!("{}", serde_json::to_string_pretty(&v.map(|v| v["result"].clone()).unwrap_or_default()).unwrap_or_default());
            }
            "shot" => {
                let img = harness.render().unwrap_or_else(|e| panic!("render: {e}"));
                img.save(a.as_str().unwrap_or("shot.png")).unwrap_or_else(|e| panic!("save: {e}"));
            }
            other => panic!("unknown script op `{other}`"),
        }
    }
    harness.run_steps(4);
    let img = harness.render().unwrap_or_else(|e| panic!("render: {e}"));
    img.save(&out).unwrap_or_else(|e| panic!("save {out}: {e}"));
    let s = harness.state();
    println!("{out}: {}x{} · workspace {} · revision {} · {} assets", img.width(), img.height(), s.workspace.label(), s.studio.revision(), s.studio.assets().count());
}
