//! `videonow-desktop` — the VideoNow application window.
//!
//! ```text
//! videonow-desktop [PROJECT.videonow] [--demo] [--agent] [--agent-port N] [--workspace edit|motion|image]
//! ```

#![cfg_attr(all(target_os = "windows", not(debug_assertions)), windows_subsystem = "windows")]

use videonow_desktop::{Options, VideoNowApp, Workspace};

const USAGE: &str = "videonow-desktop — VideoNow: one video project, edited on a timeline, in compositions and in images

USAGE:
  videonow-desktop [PROJECT.videonow] [OPTIONS]

OPTIONS:
  --demo                 start with the demo project (a badge image in an animated title on a sequence)
  --agent                turn agent access on (an MCP client connects with `videonow mcp --attach`)
  --agent-port N         the agent port on 127.0.0.1 (default: any free port)
  --workspace W          start in edit, motion or image
  --cpu                  draw the editors without the GPU
  -h, --help             this text
";

/// Log lines on stderr: `level target: message` (engine crates log a lot at debug; Info is the
/// default).
struct StderrLog;
static STDERR_LOG: StderrLog = StderrLog;

impl log::Log for StderrLog {
    fn enabled(&self, m: &log::Metadata) -> bool {
        m.level() <= log::max_level() && !(m.level() > log::Level::Warn && (m.target().starts_with("wgpu") || m.target().starts_with("naga")))
    }
    fn log(&self, r: &log::Record) {
        if self.enabled(r.metadata()) {
            eprintln!("{} {}: {}", r.level(), r.target(), r.args());
        }
    }
    fn flush(&self) {}
}

fn parse(args: &[String]) -> Result<Options, String> {
    let mut o = Options::default();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--demo" => o.demo = true,
            "--agent" => o.agent = true,
            "--agent-port" => o.agent_port = it.next().and_then(|p| p.parse().ok()).ok_or("--agent-port needs a port number")?,
            "--workspace" => o.workspace = it.next().and_then(|w| Workspace::parse(w)).ok_or("--workspace takes edit, motion or image")?,
            "--cpu" => o.gpu = false,
            "-h" | "--help" => return Err(String::new()),
            p if !p.starts_with('-') => o.open = Some(std::path::PathBuf::from(p)),
            other => return Err(format!("unknown option `{other}`")),
        }
    }
    Ok(o)
}

fn main() -> eframe::Result {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let options = match parse(&args) {
        Ok(o) => o,
        Err(e) if e.is_empty() => {
            print!("{USAGE}");
            return Ok(());
        }
        Err(e) => {
            eprintln!("videonow-desktop: {e}\n\n{USAGE}");
            std::process::exit(2);
        }
    };
    let level = match std::env::var("RUST_LOG").ok().as_deref() {
        Some("trace") => log::LevelFilter::Trace,
        Some("debug") => log::LevelFilter::Debug,
        Some("warn") => log::LevelFilter::Warn,
        Some("error") => log::LevelFilter::Error,
        _ => log::LevelFilter::Info,
    };
    if log::set_logger(&STDERR_LOG).is_ok() {
        log::set_max_level(level);
    }
    eframe::run_native("VideoNow", videonow_desktop::native_options(), Box::new(move |cc| Ok(Box::new(VideoNowApp::new(cc, options)))))
}
