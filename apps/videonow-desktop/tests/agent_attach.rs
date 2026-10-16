//! An agent attaches to the running window over its localhost port and edits the project the
//! person sees: its commands run between frames, under the Agent panel's permissions, and its
//! receipts say `by: agent`.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use egui_kittest::Harness;
use serde_json::{Value, json};
use videonow_desktop::{Options, VideoNowApp};

/// A blocking MCP client over the agent port: the hello line, then JSON-RPC lines.
struct Agent {
    w: TcpStream,
    r: BufReader<TcpStream>,
    id: u64,
}

impl Agent {
    fn connect(addr: std::net::SocketAddr, token: &str) -> Agent {
        let mut w = TcpStream::connect(addr).unwrap();
        w.set_read_timeout(Some(Duration::from_secs(60))).unwrap();
        writeln!(w, "{} {token}", videonow_automation::HELLO).unwrap();
        let r = BufReader::new(w.try_clone().unwrap());
        let mut a = Agent { w, r, id: 0 };
        a.request("initialize", json!({"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "test", "version": "0"}}));
        writeln!(a.w, "{}", json!({"jsonrpc": "2.0", "method": "notifications/initialized"})).unwrap();
        a
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
        self.id += 1;
        writeln!(self.w, "{}", json!({"jsonrpc": "2.0", "id": self.id, "method": method, "params": params})).unwrap();
        loop {
            let mut line = String::new();
            assert!(self.r.read_line(&mut line).unwrap() > 0, "the window closed the connection");
            let v: Value = serde_json::from_str(line.trim()).unwrap();
            if v["id"] == json!(self.id) {
                return v["result"].clone();
            }
        }
    }

    /// command_run → (ok, the reply's JSON or its error text)
    fn run(&mut self, id: &str, params: Value) -> (bool, Value) {
        let r = self.request("tools/call", json!({"name": "command_run", "arguments": {"id": id, "params": params}}));
        let text = r["content"][0]["text"].as_str().unwrap_or("").to_string();
        let ok = !r["isError"].as_bool().unwrap_or(false);
        (ok, serde_json::from_str(&text).unwrap_or(Value::String(text)))
    }
}

#[test]
fn an_agent_edits_the_project_open_in_the_window() {
    let options = Options { dialogs: false, gpu: false, ..Default::default() };
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).with_max_steps(64).wgpu().build_eframe(move |cc| VideoNowApp::new(cc, options));
    h.run_steps(6);
    let ctx = h.ctx.clone();
    let (addr, token) = h.state_mut().start_agent(0, &ctx).expect("the agent port should open");

    // the agent works on its own thread; the window keeps drawing frames on this one
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut a = Agent::connect(addr, &token);
        let made = a.run("studio:comp.new", json!({"name": "From the agent", "width": 640, "height": 360}));
        let refused = a.run("studio:project.new", json!({}));
        let wrong_token = TcpStream::connect(addr).and_then(|mut s| {
            writeln!(s, "{} not-the-token", videonow_automation::HELLO)?;
            s.set_read_timeout(Some(Duration::from_secs(5)))?;
            let mut line = String::new();
            BufReader::new(s).read_line(&mut line).map(|n| n == 0)
        });
        let _ = tx.send((made, refused, wrong_token.unwrap_or(false)));
    });
    let t = Instant::now();
    let (made, refused, closed) = loop {
        h.step();
        if let Ok(r) = rx.try_recv() {
            break r;
        }
        assert!(t.elapsed() < Duration::from_secs(120), "the agent got no answer");
        std::thread::sleep(Duration::from_millis(5));
    };
    h.run_steps(4);

    assert!(made.0, "comp.new failed: {}", made.1);
    assert_eq!(made.1["receipt"]["by"], "agent");
    // what the person sees: the asset, and the agent's receipt in the activity log
    let s = &h.state().studio;
    assert!(s.assets().any(|a| a.name == "From the agent"));
    assert!(s.journal().iter().any(|e| e["command"] == "studio:comp.new" && e["receipt"]["by"] == "agent"));
    // the Agent panel's default permissions: nothing destructive
    assert!(!refused.0 && refused.1.as_str().is_some_and(|t| t.contains("destructive")), "{}", refused.1);
    // a connection without the token is closed at once
    assert!(closed, "a connection with a wrong token was served");
}
