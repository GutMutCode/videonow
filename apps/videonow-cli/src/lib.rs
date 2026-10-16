//! The `videonow` command line: run commands, scripts and the demo; serve MCP. The desktop app
//! (`videonow`) runs these subcommands too; `videonow-cli` is the same without the window, for
//! servers and CI.

use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use videonow_core::{Policy, Studio};

pub const HELP: &str = "videonow — one video production project across a timeline, compositions and images

USAGE:
  videonow [PROJECT.videonow]            open the desktop app (not in videonow-cli)
  videonow demo [OUT_DIR]                build the demo project, render frames and save it as OUT_DIR/demo.videonow
  videonow run [OPTIONS] SCRIPT.jsonl    run {\"id\": \"domain:command\", \"params\": {...}} lines, one JSON reply per line
  videonow exec [OPTIONS] ID [PARAMS]    run one command (PARAMS is a JSON object) and print its reply
  videonow commands [FILTER]             list commands of every engine with their effect class
  videonow describe ID                   one command in full
  videonow mcp [OPTIONS]                 serve MCP on stdio, over its own project or (--bridge) the open app

OPTIONS:
  --project DIR        open this .videonow project first
  --save [DIR]         save the project when done (to DIR, or where it was opened from)
  --read-root DIR      confine file reads to DIR (repeatable; write roots are readable too)
  --write-root DIR     confine file writes to DIR (repeatable)
  --allow CLASSES      effect classes this session may run: read,edit,destructive,fileWrite,external or all (default all)
  --bridge [FILE]      mcp, exec: work on the project open in the desktop app (FILE: its control.json,
                       default the one in the app's data folder); the app's AI panel shows and may
                       ask the person to approve each request

Command ids are `timeline:…` (FilmCraft), `motion:…` (EffectCraft), `image:…` (PhotoCraft) or `studio:…`.
Every reply is {\"result\": …, \"receipt\": …}; the receipt says what changed and what was and was not checked.
Exit status: 0 success, 1 a command failed, 2 usage error.
";

#[derive(Default)]
struct Opts {
    project: Option<PathBuf>,
    bridge: Option<Option<PathBuf>>,
    save: Option<Option<PathBuf>>,
    policy: Policy,
    rest: Vec<String>,
}

fn parse(args: &[String]) -> Result<Opts, String> {
    let mut o = Opts::default();
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        let next = |i: usize| args.get(i + 1).cloned().ok_or_else(|| format!("{a} needs a value"));
        match a {
            "--project" => {
                o.project = Some(PathBuf::from(next(i)?));
                i += 1;
            }
            "--save" => {
                // an optional folder: the next argument, unless it is another option or the script
                match args.get(i + 1).filter(|v| !v.starts_with("--") && !v.ends_with(".jsonl") && i + 2 < args.len()) {
                    Some(v) => {
                        o.save = Some(Some(PathBuf::from(v)));
                        i += 1;
                    }
                    None => o.save = Some(None),
                }
            }
            "--bridge" => match args.get(i + 1).filter(|v| v.ends_with(".json")) {
                Some(v) => {
                    o.bridge = Some(Some(PathBuf::from(v)));
                    i += 1;
                }
                None => o.bridge = Some(None),
            },
            "--read-root" => {
                o.policy.read_roots.push(PathBuf::from(next(i)?));
                i += 1;
            }
            "--write-root" => {
                o.policy.write_roots.push(PathBuf::from(next(i)?));
                i += 1;
            }
            "--allow" => {
                o.policy.allow = Policy::parse_allow(&next(i)?)?;
                i += 1;
            }
            _ if a.starts_with("--") => return Err(format!("unknown option `{a}`")),
            _ => o.rest.push(a.to_string()),
        }
        i += 1;
    }
    for r in o.policy.read_roots.iter().chain(&o.policy.write_roots) {
        if !r.is_dir() {
            return Err(format!("{} is not a folder", r.display()));
        }
    }
    Ok(o)
}

/// The subcommands; anything else is for the desktop app.
pub const SUBCOMMANDS: &[&str] = &["demo", "run", "exec", "commands", "describe", "mcp", "help", "--help", "-h"];

/// Run a subcommand (`args` without the program name); returns the exit status.
pub fn run_cli(args: &[String]) -> i32 {
    let Some(sub) = args.first().cloned() else {
        print!("{HELP}");
        return 0;
    };
    let opts = match parse(&args[1..]) {
        Ok(o) => o,
        Err(e) => return usage(&e),
    };
    if opts.bridge.is_some() && !matches!(sub.as_str(), "mcp" | "exec") {
        return usage("--bridge works with mcp and exec");
    }
    match sub.as_str() {
        "demo" => demo(Path::new(opts.rest.first().map(String::as_str).unwrap_or("videonow-demo"))),
        "run" => match opts.rest.first() {
            Some(p) => with_studio(&opts, |s| run(s, Path::new(p))),
            None => usage("run needs a script path"),
        },
        "exec" => match opts.rest.first() {
            Some(id) => {
                let params = match opts.rest.get(1).map(|p| serde_json::from_str::<Value>(p)) {
                    None => Ok(Value::Null),
                    Some(Ok(v)) => Ok(v),
                    Some(Err(e)) => Err(e),
                };
                match (params, &opts.bridge) {
                    (Ok(p), Some(file)) => match bridge_client(file.as_deref()) {
                        Ok(c) => match c.request(&videonow_core::control::Request::Execute { id: id.clone(), params: p }) {
                            Ok(v) => {
                                println!("{}", serde_json::to_string_pretty(&v).unwrap_or_default());
                                0
                            }
                            Err(e) => {
                                eprintln!("videonow: {e}");
                                1
                            }
                        },
                        Err(e) => {
                            eprintln!("videonow: {e}");
                            1
                        }
                    },
                    (Ok(p), None) => with_studio(&opts, |s| exec(s, id, p)),
                    (Err(e), _) => usage(&format!("PARAMS is not JSON: {e}")),
                }
            }
            None => usage("exec needs a command id"),
        },
        "commands" => commands(opts.rest.first().map(String::as_str)),
        "describe" => match opts.rest.first() {
            Some(id) => exec(&mut Studio::new(), "studio:describe", json!({"id": id})),
            None => usage("describe needs a command id"),
        },
        "mcp" => {
            let served = match &opts.bridge {
                Some(file) => match bridge_client(file.as_deref()) {
                    Ok(c) => videonow_automation::serve_bridge(c).map_err(|e| e.to_string()),
                    Err(e) => Err(e),
                },
                None => videonow_automation::serve_stdio(opts.policy.clone(), opts.project.clone()).map_err(|e| e.to_string()),
            };
            match served {
                Ok(()) => 0,
                Err(e) => {
                    eprintln!("videonow mcp: {e}");
                    1
                }
            }
        }
        "help" | "--help" | "-h" => {
            print!("{HELP}");
            0
        }
        other => usage(&format!("unknown subcommand `{other}`")),
    }
}

/// A connection to the running desktop app, from its `control.json`.
fn bridge_client(file: Option<&Path>) -> Result<videonow_automation::bridge::Client, String> {
    let path = match file {
        Some(f) => f.to_path_buf(),
        None => videonow_automation::bridge::endpoint_file().ok_or("no data folder to find the app's control.json in; pass --bridge FILE")?,
    };
    let c = videonow_automation::bridge::Client::new(videonow_automation::bridge::read_endpoint(&path)?);
    c.check()?;
    Ok(c)
}

fn usage(msg: &str) -> i32 {
    eprintln!("videonow: {msg}\n\n{HELP}");
    2
}

/// A studio with the options' policy, the project opened, `f` run, the project saved.
fn with_studio(o: &Opts, f: impl FnOnce(&mut Studio) -> i32) -> i32 {
    let mut s = Studio::with_policy(o.policy.clone());
    if let Some(p) = &o.project
        && let Err(e) = s.execute("studio:project.open", json!({"path": p.to_string_lossy()}))
    {
        eprintln!("videonow: {e}");
        return 1;
    }
    let code = f(&mut s);
    if let Some(dir) = &o.save {
        let p = match dir {
            Some(d) => json!({"path": d.to_string_lossy()}),
            None => json!({}),
        };
        match s.execute("studio:project.save", p) {
            Ok(r) => eprintln!("saved {}", r["result"]["path"].as_str().unwrap_or("")),
            Err(e) => {
                eprintln!("videonow: {e}");
                return 1;
            }
        }
    }
    code
}

fn exec(s: &mut Studio, id: &str, params: Value) -> i32 {
    match s.execute(id, params) {
        Ok(v) => {
            println!("{}", serde_json::to_string_pretty(&v).unwrap_or_default());
            0
        }
        Err(e) => {
            println!("{}", json!({"error": e.to_string()}));
            1
        }
    }
}

fn commands(filter: Option<&str>) -> i32 {
    let mut s = Studio::new();
    let p = filter.map(|f| json!({"filter": f})).unwrap_or(json!({}));
    match s.execute("studio:commands", p) {
        Ok(v) => {
            for c in v["result"].as_array().into_iter().flatten() {
                let flag = if c["reserved"].is_string() { " (reserved)" } else { "" };
                println!("{:<48} {:<12} {}{flag}", c["id"].as_str().unwrap_or(""), c["effect"].as_str().unwrap_or(""), c["label"].as_str().unwrap_or(""));
            }
            0
        }
        Err(e) => {
            eprintln!("{e}");
            1
        }
    }
}

fn run(s: &mut Studio, path: &Path) -> i32 {
    let f = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) => return usage(&format!("{}: {e}", path.display())),
    };
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

fn demo(out: &Path) -> i32 {
    if let Err(e) = std::fs::create_dir_all(out) {
        eprintln!("{}: {e}", out.display());
        return 1;
    }
    let mut s = Studio::new();
    let print = |id: &str, r: &Value| {
        let rc = &r["receipt"];
        println!("{:<28} rev {}→{}  changed {:<34} {}", id, rc["revision"]["before"], rc["revision"]["after"], rc["changed"].to_string(), compact(&r["result"]));
    };
    let r = (|| -> Result<(), String> {
        let d = videonow_core::demo::build(&mut s, print).map_err(|e| e.to_string())?;
        let mut step = |id: &str, p: Value| -> Result<Value, String> {
            let r = s.execute(id, p).map_err(|e| format!("{id}: {e}"))?;
            print(id, &r);
            Ok(r["result"].clone())
        };
        let png = |name: &str| out.join(name).to_string_lossy().into_owned();
        for (t, name) in [(0.2, "frame-0.2s.png"), (1.0, "frame-1.0s.png"), (2.5, "frame-2.5s.png")] {
            step("studio:render.frame", json!({"seconds": t, "path": png(name), "scale": 0.5}))?;
        }
        // edit the badge in the image engine: the next frame shows it through comp and timeline
        step("image:shape.create", json!({"asset": d.badge, "kind": "rect", "rect": [240, 280, 160, 80], "fill": "#e4572e"}))?;
        step("studio:render.frame", json!({"seconds": 2.5, "path": png("frame-2.5s-after-badge-edit.png"), "scale": 0.5}))?;
        step("studio:project.save", json!({"path": out.join("demo.videonow").to_string_lossy()}))?;
        let inspect = step("studio:inspect", json!({}))?;
        let journal = step("studio:journal", json!({}))?;
        let write = |name: &str, v: &Value| std::fs::write(out.join(name), serde_json::to_string_pretty(v).unwrap_or_default()).map_err(|e| e.to_string());
        write("inspect.json", &inspect)?;
        write("journal.json", &journal)?;
        Ok(())
    })();
    match r {
        Ok(()) => {
            println!("\ndemo written to {} (project: {})", out.display(), out.join("demo.videonow").display());
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
