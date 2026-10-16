//! `videonow-eval [SCENARIO…] [--json FILE] [--md FILE]` — run the agent scenarios and report.
//! `videonow-eval score LOG [PROJECT]` — score a live agent session from `videonow mcp --log`.

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let flag = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
    if args.iter().any(|a| a == "-h" || a == "--help") {
        println!("videonow-eval [SCENARIO…] [--json FILE] [--md FILE]\nvideonow-eval score LOG [PROJECT.videonow]\n\nscenarios: {}", videonow_eval::scenarios::ALL.join(", "));
        return;
    }
    if args.first().map(String::as_str) == Some("score") {
        let Some(log) = args.get(1) else {
            eprintln!("videonow-eval score LOG [PROJECT.videonow]");
            std::process::exit(2);
        };
        match videonow_eval::score::score(std::path::Path::new(log), args.get(2).map(std::path::Path::new)) {
            Ok(v) => println!("{}", serde_json::to_string_pretty(&v).unwrap_or_default()),
            Err(e) => {
                eprintln!("videonow-eval: {e}");
                std::process::exit(1);
            }
        }
        return;
    }
    let mut skip = false;
    let names: Vec<String> = args
        .iter()
        .filter(|a| {
            if skip {
                skip = false;
                return false;
            }
            if a.starts_with("--") {
                skip = true;
                return false;
            }
            true
        })
        .cloned()
        .collect();
    let outcomes = videonow_eval::run(&names).await;
    let md = videonow_eval::report::markdown(&outcomes);
    print!("{md}");
    if let Some(p) = flag("--md")
        && let Err(e) = std::fs::write(&p, &md)
    {
        eprintln!("{p}: {e}");
    }
    if let Some(p) = flag("--json") {
        let v = serde_json::json!({"metrics": videonow_eval::report::metrics(&outcomes), "scenarios": outcomes});
        if let Err(e) = std::fs::write(&p, serde_json::to_string_pretty(&v).unwrap_or_default()) {
            eprintln!("{p}: {e}");
        }
    }
    let failed = outcomes.iter().filter(|o| !o.passed()).count();
    std::process::exit(i32::from(failed > 0));
}
