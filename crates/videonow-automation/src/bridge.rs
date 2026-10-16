//! The control channel between the desktop app and agents: JSON lines over a localhost TCP
//! socket, each request carrying a token only the person's user account can read.
//!
//! The app listens on `127.0.0.1` (an ephemeral port by default) and writes where it listens and
//! the token to `control.json` in its data folder (readable by the user only). `videonow mcp
//! --bridge` reads that file, connects, and forwards every MCP tool call.
//!
//! ```text
//! → {"token": "…", "request": {"op": "execute", "id": "studio:inspect", "params": {}}}
//! ← {"ok": true, "result": {…}}
//! ```
//!
//! A wrong token is answered with `{"ok": false, "error": "unauthorized"}` and the connection is
//! closed. The app decides what runs: it may hold a request until the person approves it.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError, mpsc};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use videonow_core::control::Request;

/// Where a running app listens, as written to `control.json`.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Endpoint {
    pub port: u16,
    pub token: String,
    pub pid: u32,
}

/// The app's data folder: `VIDEONOW_DATA_DIR`, else the platform's per-user application data
/// folder.
pub fn data_dir() -> Option<PathBuf> {
    if let Some(d) = std::env::var_os("VIDEONOW_DATA_DIR") {
        return Some(PathBuf::from(d));
    }
    let home = std::env::var_os("HOME").map(PathBuf::from);
    if cfg!(target_os = "macos") {
        return home.map(|h| h.join("Library/Application Support/VideoNow"));
    }
    if cfg!(target_os = "windows") {
        return std::env::var_os("APPDATA").map(|a| PathBuf::from(a).join("VideoNow"));
    }
    std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).or_else(|| home.map(|h| h.join(".local/share"))).map(|d| d.join("videonow"))
}

/// `control.json` in the data folder.
pub fn endpoint_file() -> Option<PathBuf> {
    data_dir().map(|d| d.join("control.json"))
}

pub fn read_endpoint(path: &Path) -> Result<Endpoint, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e} (is the VideoNow app running?)", path.display()))?;
    serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))
}

/// Write `control.json`, readable by this user only.
pub fn write_endpoint(path: &Path, e: &Endpoint) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let text = serde_json::to_string_pretty(e).map_err(std::io::Error::other)?;
    let tmp = path.with_extension("json.tmp");
    {
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut f = opts.open(&tmp)?;
        f.write_all(text.as_bytes())?;
    }
    std::fs::rename(&tmp, path)
}

/// A random token (128 bits, hex).
pub fn new_token() -> String {
    let mut b = [0u8; 16];
    if getrandom::fill(&mut b).is_err() {
        // no OS randomness: still unique per run, and the socket is localhost-only
        let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
        b = (t ^ u128::from(std::process::id()) << 64).to_le_bytes();
    }
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn same(a: &str, b: &str) -> bool {
    // constant time in the token's length
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// One request that reached the app; answer it with [`Incoming::answer`].
pub struct Incoming {
    /// The request, or why it could not be read (answered with that error).
    pub request: Result<Request, String>,
    reply: mpsc::Sender<Value>,
}

impl Incoming {
    pub fn answer(self, result: Result<Value, String>) {
        let line = match result {
            Ok(v) => json!({"ok": true, "result": v}),
            Err(e) => json!({"ok": false, "error": e}),
        };
        let _ = self.reply.send(line);
    }
}

/// Listen on `127.0.0.1:port` (0 = any free port). Requests with the right token arrive on the
/// returned channel; `wake` is called after each one (to repaint the window that answers them).
pub fn listen(port: u16, token: String, wake: impl Fn() + Send + Sync + 'static) -> std::io::Result<(u16, mpsc::Receiver<Incoming>)> {
    let listener = TcpListener::bind(("127.0.0.1", port))?;
    let port = listener.local_addr()?.port();
    let (tx, rx) = mpsc::channel();
    let wake = Arc::new(wake);
    std::thread::Builder::new().name("videonow-control".into()).spawn(move || {
        for stream in listener.incoming().flatten() {
            let (tx, token, wake) = (tx.clone(), token.clone(), wake.clone());
            let _ = std::thread::Builder::new().name("videonow-control-conn".into()).spawn(move || serve(stream, &token, &tx, &*wake));
        }
    })?;
    Ok((port, rx))
}

fn serve(stream: TcpStream, token: &str, tx: &mpsc::Sender<Incoming>, wake: &(dyn Fn() + Send + Sync)) {
    let Ok(read) = stream.try_clone() else { return };
    let mut out = stream;
    let mut lines = BufReader::new(read);
    let mut line = String::new();
    loop {
        line.clear();
        // a line is at most 64 MiB (images in parameters are paths, not pixels)
        match (&mut lines).take(64 << 20).read_line(&mut line) {
            Ok(0) | Err(_) => return,
            Ok(_) => {}
        }
        if line.trim().is_empty() {
            continue;
        }
        let msg: Value = match serde_json::from_str(line.trim()) {
            Ok(v) => v,
            Err(e) => {
                let _ = writeln!(out, "{}", json!({"ok": false, "error": format!("not JSON: {e}")}));
                continue;
            }
        };
        if !msg.get("token").and_then(Value::as_str).is_some_and(|t| same(t, token)) {
            let _ = writeln!(out, "{}", json!({"ok": false, "error": "unauthorized"}));
            return;
        }
        let request = msg.get("request").ok_or_else(|| "missing `request`".to_string()).and_then(|r| Request::parse(r).map_err(|e| e.to_string()));
        let (rtx, rrx) = mpsc::channel();
        if tx.send(Incoming { request, reply: rtx }).is_err() {
            let _ = writeln!(out, "{}", json!({"ok": false, "error": "the app is closing"}));
            return;
        }
        wake();
        let answer = rrx.recv().unwrap_or_else(|_| json!({"ok": false, "error": "the app dropped the request"}));
        if writeln!(out, "{answer}").is_err() {
            return;
        }
    }
}

/// A connection to a running app.
#[derive(Clone)]
pub struct Client {
    endpoint: Endpoint,
    conn: Arc<Mutex<Option<(BufReader<TcpStream>, TcpStream)>>>,
}

impl Client {
    pub fn new(endpoint: Endpoint) -> Client {
        Client { endpoint, conn: Arc::new(Mutex::new(None)) }
    }

    /// Connect now, to report a missing app before the first tool call.
    pub fn check(&self) -> Result<(), String> {
        self.request(&Request::Execute { id: "studio:policy".into(), params: json!({}) }).map(|_| ())
    }

    fn open(&self) -> Result<(BufReader<TcpStream>, TcpStream), String> {
        let s = TcpStream::connect(("127.0.0.1", self.endpoint.port)).map_err(|e| format!("cannot reach the VideoNow app on port {}: {e} (is it running?)", self.endpoint.port))?;
        // a request may wait for the person to approve it
        let _ = s.set_read_timeout(Some(Duration::from_secs(900)));
        let r = s.try_clone().map_err(|e| e.to_string())?;
        Ok((BufReader::new(r), s))
    }

    /// Send one request and wait for its answer: `Ok(result)` or the app's error.
    pub fn request(&self, req: &Request) -> Result<Value, String> {
        let mut guard = self.conn.lock().unwrap_or_else(PoisonError::into_inner);
        let line = json!({"token": self.endpoint.token, "request": req.to_value()}).to_string();
        // one retry on a fresh connection (the app may have restarted the socket)
        for attempt in 0..2 {
            if guard.is_none() {
                *guard = Some(self.open()?);
            }
            let Some((reader, writer)) = guard.as_mut() else { continue };
            let mut answer = String::new();
            let sent = writeln!(writer, "{line}").and_then(|()| writer.flush()).is_ok();
            if sent && reader.read_line(&mut answer).is_ok_and(|n| n > 0) {
                let v: Value = serde_json::from_str(answer.trim()).map_err(|e| format!("bad answer from the app: {e}"))?;
                return if v["ok"] == true { Ok(v["result"].clone()) } else { Err(v["error"].as_str().unwrap_or("the app refused the request").to_string()) };
            }
            *guard = None;
            if attempt == 1 {
                break;
            }
        }
        Err("the connection to the VideoNow app closed".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_need_the_token() {
        let (port, rx) = listen(0, "secret".into(), || {}).unwrap();
        std::thread::spawn(move || {
            while let Ok(i) = rx.recv() {
                let r = i.request.clone().map(|r| json!({"echo": r.summary()}));
                i.answer(r);
            }
        });
        let good = Client::new(Endpoint { port, token: "secret".into(), pid: 0 });
        let v = good.request(&Request::Execute { id: "studio:inspect".into(), params: json!({}) }).unwrap();
        assert_eq!(v["echo"], "studio:inspect {}");
        let bad = Client::new(Endpoint { port, token: "guess".into(), pid: 0 });
        assert_eq!(bad.request(&Request::Preview { seconds: 0.0, max_side: 16 }).unwrap_err(), "unauthorized");
    }

    #[test]
    fn endpoint_file_round_trips() {
        let p = std::env::temp_dir().join(format!("videonow-endpoint-{}/control.json", std::process::id()));
        let e = Endpoint { port: 1234, token: new_token(), pid: 7 };
        write_endpoint(&p, &e).unwrap();
        assert_eq!(read_endpoint(&p).unwrap(), e);
        assert_eq!(e.token.len(), 32);
    }
}
