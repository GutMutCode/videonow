//! `videonow-cli`: the command line without the desktop app (see the library).

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    std::process::exit(videonow_cli::run_cli(&args));
}
