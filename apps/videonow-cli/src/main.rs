//! `videonow` — the headless studio.
//!
//! ```text
//! videonow demo [OUT_DIR]        build a small project across all three engines and render frames
//! videonow run SCRIPT.jsonl      run {"id": "...", "params": {...}} lines; print one result per line
//! videonow commands [FILTER]     list commands (all engines) with their effect class
//! videonow mcp                   serve the Model Context Protocol on stdio
//! ```

use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use videonow_core::Studio;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = match args.first().map(String::as_str) {
        Some("demo") => demo(Path::new(args.get(1).map(String::as_str).unwrap_or("videonow-demo"))),
        Some("run") => match args.get(1) {
            Some(p) => run(Path::new(p)),
            None => usage("run needs a script path"),
        },
        Some("commands") => commands(args.get(1).map(String::as_str)),
        Some("mcp") => match videonow_automation::serve_stdio() {
            Ok(()) => 0,
            Err(e) => {
                eprintln!("videonow mcp: {e}");
                1
            }
        },
        Some("help" | "--help" | "-h") | None => {
            print!("{}", HELP);
            0
        }
        Some(other) => usage(&format!("unknown subcommand `{other}`")),
    };
    std::process::exit(code);
}

const HELP: &str = "videonow — one video production project across a timeline, compositions and images

USAGE:
  videonow demo [OUT_DIR]       build a demo project and render frames into OUT_DIR (default videonow-demo)
  videonow run SCRIPT.jsonl     run {\"id\": \"domain:command\", \"params\": {...}} lines, one JSON result per line
  videonow commands [FILTER]    list commands of every engine with their effect class
  videonow mcp                  serve MCP on stdio

Command ids are `timeline:…` (FilmCraft), `motion:…` (EffectCraft), `image:…` (PhotoCraft) or `studio:…`.
Exit status: 0 success, 1 a command failed, 2 usage error.
";

fn usage(msg: &str) -> i32 {
    eprintln!("videonow: {msg}\n\n{HELP}");
    2
}

fn commands(filter: Option<&str>) -> i32 {
    let mut s = Studio::new();
    let p = filter.map(|f| json!({"filter": f})).unwrap_or(json!({}));
    match s.execute("studio:commands", p) {
        Ok(v) => {
            for c in v["result"].as_array().into_iter().flatten() {
                println!("{:<48} {:<12} {}", c["id"].as_str().unwrap_or(""), c["effect"].as_str().unwrap_or(""), c["label"].as_str().unwrap_or(""));
            }
            0
        }
        Err(e) => {
            eprintln!("{e}");
            1
        }
    }
}

fn run(path: &Path) -> i32 {
    let f = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) => return usage(&format!("{}: {e}", path.display())),
    };
    let mut s = Studio::new();
    let mut failed = false;
    let out = std::io::stdout();
    for (n, line) in std::io::BufReader::new(f).lines().enumerate() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() || line.trim_start().starts_with("//") {
            continue;
        }
        let r = serde_json::from_str::<Value>(&line).map_err(|e| format!("line {}: {e}", n + 1)).and_then(|v| {
            let id = v["id"].as_str().ok_or_else(|| format!("line {}: missing \"id\"", n + 1))?.to_string();
            s.execute(&id, v.get("params").cloned().unwrap_or(Value::Null)).map_err(|e| format!("line {}: {e}", n + 1))
        });
        let mut o = out.lock();
        match r {
            Ok(v) => {
                let _ = writeln!(o, "{v}");
            }
            Err(e) => {
                failed = true;
                let _ = writeln!(o, "{}", json!({"error": e}));
            }
        }
    }
    i32::from(failed)
}

/// Builds: a badge (image engine) → animated title composition with the badge in it (motion
/// engine) → a sequence with a colour matte and the composition on top (timeline). Then renders
/// frames, edits the badge, and renders again to show the edit flows through every level.
fn demo(out: &Path) -> i32 {
    if let Err(e) = std::fs::create_dir_all(out) {
        eprintln!("{}: {e}", out.display());
        return 1;
    }
    let mut s = Studio::new();
    let png = |name: &str| -> String { out.join(name).to_string_lossy().into_owned() };
    let step = |s: &mut Studio, id: &str, p: Value| -> Result<Value, String> {
        let r = s.execute(id, p.clone()).map_err(|e| format!("{id}: {e}"))?;
        let rc = &r["receipt"];
        println!(
            "{:<28} rev {}→{}  changed {:<34} {}",
            id,
            rc["revision"]["before"],
            rc["revision"]["after"],
            rc["changed"].to_string(),
            compact(&r["result"])
        );
        Ok(r["result"].clone())
    };
    let r = (|| -> Result<(), String> {
        // 1. image engine: a badge
        let badge = step(&mut s, "studio:image.new", json!({"name": "Badge", "width": 640, "height": 640, "background": "transparent"}))?["asset"].clone();
        step(&mut s, "image:shape.create", json!({"asset": badge, "kind": "ellipse", "rect": [20, 20, 600, 600], "fill": "#f2b134"}))?;
        step(&mut s, "image:shape.create", json!({"asset": badge, "kind": "ellipse", "rect": [120, 120, 400, 400], "fill": "#1d2b53"}))?;
        // 2. motion engine: an animated title with the badge in it
        let title = step(&mut s, "studio:comp.new", json!({"name": "Title", "width": 1920, "height": 1080, "fps": 30, "duration": 4}))?["asset"].clone();
        let text = step(&mut s, "motion:layer.newText", json!({"asset": title, "text": "STUDIO", "size": 220, "fill": "#ffffff", "position": [960, 900]}))?["layer"].clone();
        step(&mut s, "motion:prop.addKey", json!({"asset": title, "layer": text, "path": "transform/opacity", "time": 0.0, "value": 0}))?;
        step(&mut s, "motion:prop.addKey", json!({"asset": title, "layer": text, "path": "transform/opacity", "time": 1.0, "value": 100}))?;
        let placed = step(&mut s, "studio:image.toComp", json!({"image": badge, "comp": title, "time": 0}))?;
        let badge_layer = placed["layer"]["layer"].clone();
        step(&mut s, "motion:prop.set", json!({"asset": title, "layer": badge_layer, "path": "transform/position", "value": [960, 430]}))?;
        step(&mut s, "motion:prop.addKey", json!({"asset": title, "layer": badge_layer, "path": "transform/scale", "time": 0.0, "value": [0, 0]}))?;
        step(&mut s, "motion:prop.addKey", json!({"asset": title, "layer": badge_layer, "path": "transform/scale", "time": 1.2, "value": [100, 100]}))?;
        // 3. timeline: a sequence with a matte, the title composition on top
        step(&mut s, "timeline:file.newSequence", json!({"name": "Main", "width": 1920, "height": 1080, "fps": 30, "video": 2, "audio": 1}))?;
        let matte = step(&mut s, "timeline:file.newColorMatte", json!({"color": "#20304a", "seconds": 4}))?;
        let matte_item = matte.get("item").cloned().unwrap_or(matte.clone());
        step(&mut s, "timeline:timeline.place", json!({"item": matte_item, "track": "V1", "seconds": 0}))?;
        step(&mut s, "studio:asset.place", json!({"asset": title, "track": "V2", "seconds": 0}))?;
        // 4. render
        for (t, name) in [(0.2, "frame-0.2s.png"), (1.0, "frame-1.0s.png"), (2.5, "frame-2.5s.png")] {
            step(&mut s, "studio:render.frame", json!({"seconds": t, "path": png(name), "scale": 0.5}))?;
        }
        // 5. edit the badge in the image engine: the next frame shows it through comp and timeline
        step(&mut s, "image:shape.create", json!({"asset": badge, "kind": "rect", "rect": [240, 280, 160, 80], "fill": "#e4572e"}))?;
        step(&mut s, "studio:render.frame", json!({"seconds": 2.5, "path": png("frame-2.5s-after-badge-edit.png"), "scale": 0.5}))?;
        // 6. save the whole project, open it again, and check the picture is the same
        let project = out.join("demo.videonow").to_string_lossy().into_owned();
        step(&mut s, "studio:project.save", json!({"path": project}))?;
        let before = s.render_rgba(2.5, 0.5).map_err(|e| e.to_string())?;
        step(&mut s, "studio:project.open", json!({"path": project}))?;
        let after = s.render_rgba(2.5, 0.5).map_err(|e| e.to_string())?;
        if before.2 != after.2 {
            return Err("the reopened project renders differently".into());
        }
        println!("{:<28} reopened project renders the same frame", "check");
        let inspect = step(&mut s, "studio:inspect", json!({}))?;
        let journal = s.execute("studio:journal", json!({})).map_err(|e| e.to_string())?;
        let write = |name: &str, v: &Value| std::fs::write(out.join(name), serde_json::to_string_pretty(v).unwrap_or_default()).map_err(|e| e.to_string());
        write("inspect.json", &inspect)?;
        write("journal.json", &journal["result"])?;
        Ok(())
    })();
    match r {
        Ok(()) => {
            println!("\ndemo written to {}", PathBuf::from(out).display());
            0
        }
        Err(e) => {
            eprintln!("demo failed: {e}");
            1
        }
    }
}

fn compact(v: &Value) -> String {
    let s = v.to_string();
    if s.chars().count() > 90 { format!("{}…", s.chars().take(90).collect::<String>()) } else { s }
}
