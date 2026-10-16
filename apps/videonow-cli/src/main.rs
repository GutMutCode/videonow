//! `videonow` — the headless studio.
//!
//! ```text
//! videonow demo [OUT_DIR]        build a small project across all three engines and render frames
//! videonow run SCRIPT.jsonl      run {"id": "...", "params": {...}} lines; print one result per line
//! videonow commands [FILTER]     list commands (all engines) with their effect class
//! videonow mcp                   serve the Model Context Protocol on stdio
//! videonow mcp --attach [TARGET]  connect stdio to the open desktop application's agent port
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
        Some("mcp") if args.get(1).map(String::as_str) == Some("--attach") => {
            let target = args.get(2).map(String::as_str).filter(|a| !a.starts_with("--"));
            let token = args.iter().position(|a| a == "--token").and_then(|i| args.get(i + 1)).map(String::as_str);
            match videonow_automation::proxy_stdio(target, token) {
                Ok(()) => 0,
                Err(e) => {
                    eprintln!("videonow mcp --attach: {e}");
                    1
                }
            }
        }
        Some("mcp") => match mcp_policy(&args[1..]) {
            Err(e) => usage(&e),
            Ok((policy, log)) => match videonow_automation::serve_stdio(policy, log.as_deref()) {
                Ok(()) => 0,
                Err(e) => {
                    eprintln!("videonow mcp: {e}");
                    1
                }
            },
        },
        Some("help" | "--help" | "-h") | None => {
            print!("{}", HELP);
            0
        }
        Some(other) => usage(&format!("unknown subcommand `{other}`")),
    };
    std::process::exit(code);
}

/// `mcp` options: files are confined to the working directory unless `--root` names other
/// folders or `--allow-anywhere` lifts the confinement; `--deny CLASS` refuses an effect class.
fn mcp_policy(args: &[String]) -> Result<(videonow_core::Policy, Option<PathBuf>), String> {
    use videonow_core::manifest::Effect;
    let mut roots = Vec::new();
    let (mut anywhere, mut deny) = (false, Vec::new());
    let mut log = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--root" => roots.push(PathBuf::from(it.next().ok_or("--root needs a folder")?)),
            "--allow-anywhere" => anywhere = true,
            "--log" => log = Some(PathBuf::from(it.next().ok_or("--log needs a file")?)),
            "--deny" => deny.push(match it.next().map(String::as_str) {
                Some("destructive") => Effect::Destructive,
                Some("fileWrite") => Effect::FileWrite,
                Some("external") => Effect::External,
                Some("edit") => Effect::Edit,
                other => return Err(format!("--deny takes destructive, fileWrite, external or edit, not {other:?}")),
            }),
            other => return Err(format!("unknown mcp option `{other}`")),
        }
    }
    let mut p = if anywhere {
        videonow_core::Policy::unrestricted()
    } else {
        if roots.is_empty() {
            roots.push(std::env::current_dir().map_err(|e| format!("working directory: {e}"))?);
        }
        videonow_core::Policy::confined_to(&roots)
    };
    p.deny = deny;
    Ok((p, log))
}

const HELP: &str = "videonow — one video production project across a timeline, compositions and images

USAGE:
  videonow demo [OUT_DIR]       build a demo project and render frames into OUT_DIR (default videonow-demo)
  videonow run SCRIPT.jsonl     run {\"id\": \"domain:command\", \"params\": {...}} lines, one JSON result per line
                              (optional per line: requestId, expectRevision, dryRun)
  videonow commands [FILTER]    list commands of every engine with their effect class
  videonow mcp [--root DIR]... [--allow-anywhere] [--deny CLASS]... [--log FILE]
                              serve MCP on stdio; files stay inside DIR (default: the working
                              directory); CLASS is destructive, fileWrite, external or edit;
                              --log appends every tool call to FILE (JSON lines)
  videonow mcp --attach [FILE | HOST:PORT --token T]
                              serve MCP on stdio for the project open in VideoNow (the desktop
                              application, with agent access turned on in its Agent panel)

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
            let req = videonow_core::Request {
                request_id: v["requestId"].as_str().map(str::to_string),
                expect_revision: v["expectRevision"].as_u64(),
                dry_run: v["dryRun"].as_bool().unwrap_or(false),
            };
            s.execute_with(&id, v.get("params").cloned().unwrap_or(Value::Null), &req).map_err(|e| format!("line {}: {e}", n + 1))
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
        // 1–3. the demo project: badge → title composition → sequence
        let demo = videonow_core::demo::build(&mut s, step)?;
        let badge = demo.badge;
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
