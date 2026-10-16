//! Studio: the parameter audit. With `VIDEONOW_PARAM_AUDIT=<file>` set, every command that runs without an error appends
//! `id<TAB>key,key,…` to that file, so a test run lists the keys each command is really given
//! (VideoNow compares them with the commands' parameter docs). Off, it costs one cached check.

use std::io::Write;
use std::sync::{Mutex, OnceLock};

static AUDIT: OnceLock<Option<Mutex<std::fs::File>>> = OnceLock::new();

pub(crate) fn record(id: &str, params: &serde_json::Value) {
    let sink = AUDIT.get_or_init(|| {
        let path = std::env::var_os("VIDEONOW_PARAM_AUDIT")?;
        std::fs::OpenOptions::new().create(true).append(true).open(path).ok().map(Mutex::new)
    });
    let Some(sink) = sink else { return };
    let mut keys: Vec<&str> = params.as_object().map(|o| o.keys().map(String::as_str).collect()).unwrap_or_default();
    keys.sort_unstable();
    let mut f = sink.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let _ = writeln!(f, "{id}\t{}", keys.join(","));
}
