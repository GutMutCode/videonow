//! Agent access: an MCP server on a localhost port, serving the project open in this window.
//!
//! Turned on from the Agent panel (or `--agent`). An MCP client starts `videonow mcp --attach`,
//! which reads the connection file the application writes (address and a random token) and
//! connects. Tool calls become jobs the shell runs between frames, with the agent's own policy
//! (what it may do, which folders it may touch) and `agent` as the actor in every receipt.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};

use videonow_automation::{AgentEvent, Job};
use videonow_core::Policy;
use videonow_core::manifest::Effect;

/// What the person allows the agent.
#[derive(Clone, Debug, PartialEq)]
pub struct Permissions {
    pub destructive: bool,
    pub file_writes: bool,
    pub external: bool,
    /// Files only inside the project folder (or, before the first save, the home folder).
    pub project_folder_only: bool,
}

impl Default for Permissions {
    fn default() -> Self {
        Self { destructive: false, file_writes: true, external: false, project_folder_only: true }
    }
}

impl Permissions {
    pub fn policy(&self, project: Option<&std::path::Path>) -> Policy {
        let mut p = if self.project_folder_only {
            let root = project
                .and_then(|p| p.parent().map(std::path::Path::to_path_buf))
                .or_else(|| std::env::var_os("HOME").map(std::path::PathBuf::from))
                .unwrap_or_else(|| std::path::PathBuf::from("."));
            Policy::confined_to(&[root])
        } else {
            Policy::unrestricted()
        };
        if !self.destructive {
            p.deny.push(Effect::Destructive);
        }
        if !self.file_writes {
            p.deny.push(Effect::FileWrite);
        }
        if !self.external {
            p.deny.push(Effect::External);
        }
        p
    }
}

pub struct Agent {
    pub permissions: Permissions,
    tx: mpsc::Sender<Job>,
    rx: mpsc::Receiver<Job>,
    stop: Option<Arc<AtomicBool>>,
    pub addr: Option<SocketAddr>,
    pub connections: Arc<Mutex<Vec<SocketAddr>>>,
    pub last_event: Arc<Mutex<String>>,
    pub connection_file: Option<std::path::PathBuf>,
    pub error: Option<String>,
    /// Jobs run so far.
    pub served: u64,
}

impl Agent {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            permissions: Permissions::default(),
            tx,
            rx,
            stop: None,
            addr: None,
            connections: Arc::default(),
            last_event: Arc::default(),
            connection_file: None,
            error: None,
            served: 0,
        }
    }

    pub fn is_on(&self) -> bool {
        self.stop.is_some()
    }

    /// Start listening on `port` (0: any free port) of 127.0.0.1.
    pub fn start(&mut self, port: u16, ctx: &egui::Context) {
        if self.is_on() {
            return;
        }
        let stop = Arc::new(AtomicBool::new(false));
        let token = videonow_automation::new_token();
        let wake_ctx = ctx.clone();
        let wake: videonow_automation::Wake = Arc::new(move || wake_ctx.request_repaint());
        let (conns, last, ev_ctx) = (self.connections.clone(), self.last_event.clone(), ctx.clone());
        let events = move |e: AgentEvent| {
            let text = match &e {
                AgentEvent::Listening(a) => format!("listening on {a}"),
                AgentEvent::Connected(a) => {
                    if let Ok(mut c) = conns.lock() {
                        c.push(*a);
                    }
                    "an agent connected".to_string()
                }
                AgentEvent::Disconnected(a) => {
                    if let Ok(mut c) = conns.lock() {
                        c.retain(|x| x != a);
                    }
                    "an agent disconnected".to_string()
                }
                AgentEvent::Refused(a, why) => format!("refused a connection from {a}: {why}"),
                AgentEvent::Failed(why) => format!("agent connection failed: {why}"),
            };
            log::info!("agent: {text}");
            if let Ok(mut l) = last.lock() {
                *l = text;
            }
            ev_ctx.request_repaint();
        };
        let addr = SocketAddr::from(([127, 0, 0, 1], port));
        match videonow_automation::serve_tcp(addr, token.clone(), self.tx.clone(), Some(wake), events, stop.clone()) {
            Ok(bound) => {
                self.addr = Some(bound);
                self.stop = Some(stop);
                self.error = None;
                match videonow_automation::write_connection(bound, &token) {
                    Ok(p) => self.connection_file = Some(p),
                    Err(e) => self.error = Some(format!("listening on {bound}, but the connection file could not be written: {e}")),
                }
            }
            Err(e) => self.error = Some(e.to_string()),
        }
    }

    pub fn stop(&mut self) {
        if let Some(s) = self.stop.take() {
            s.store(true, Ordering::Relaxed);
        }
        if let Some(a) = self.addr.take() {
            videonow_automation::remove_connection(a);
        }
        self.connection_file = None;
        if let Ok(mut c) = self.connections.lock() {
            c.clear();
        }
    }

    /// Jobs waiting to run, at most `max`.
    pub fn take_jobs(&mut self, max: usize) -> Vec<Job> {
        let mut v = Vec::new();
        while v.len() < max {
            match self.rx.try_recv() {
                Ok(j) => v.push(j),
                Err(_) => break,
            }
        }
        self.served += v.len() as u64;
        v
    }

    pub fn connected(&self) -> usize {
        self.connections.lock().map(|c| c.len()).unwrap_or(0)
    }
}

impl Drop for Agent {
    fn drop(&mut self) {
        self.stop();
    }
}
