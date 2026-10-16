//! Where a scenario runs: a studio on its own thread, the MCP server over an in-memory pipe, an
//! MCP client for the agent, and a simulated person who edits the same project the way the
//! desktop application's editors do (engine calls, then `sync_editors`).

use std::sync::mpsc;

use rmcp::ServiceExt as _;
use serde_json::Value;
use tokio::io::{DuplexStream, ReadHalf, WriteHalf};
use videonow_automation::{Job, StudioMcp};
use videonow_core::{Policy, Studio};

use crate::client::Client;

pub type AgentClient = Client<ReadHalf<DuplexStream>, WriteHalf<DuplexStream>>;

/// Work for the studio thread: the agent's tool calls, or a person's edit.
enum Work {
    Person(Job),
    Stop,
}

pub struct Harness {
    pub agent: AgentClient,
    person: mpsc::Sender<Work>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Harness {
    /// A fresh project; the agent's commands run under `policy`.
    pub async fn start(policy: Policy) -> Result<Self, String> {
        let (agent_tx, agent_rx) = mpsc::channel::<Job>();
        let (person_tx, person_rx) = mpsc::channel::<Work>();
        let thread = std::thread::Builder::new()
            .name("studio".into())
            .spawn(move || {
                let mut studio = Studio::new();
                loop {
                    let mut idle = true;
                    match person_rx.try_recv() {
                        Ok(Work::Person(job)) => {
                            idle = false;
                            studio.as_actor("ui", None, |s| job(s));
                        }
                        Ok(Work::Stop) | Err(mpsc::TryRecvError::Disconnected) => break,
                        Err(mpsc::TryRecvError::Empty) => {}
                    }
                    match agent_rx.recv_timeout(std::time::Duration::from_millis(if idle { 5 } else { 0 })) {
                        Ok(job) => {
                            let p = policy.clone();
                            let r = studio.as_actor("agent", Some(p), |s| std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| job(s))));
                            if r.is_err() {
                                eprintln!("videonow-eval: an agent job panicked");
                            }
                        }
                        Err(mpsc::RecvTimeoutError::Timeout) => {}
                        Err(mpsc::RecvTimeoutError::Disconnected) => {}
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        let (client_io, server_io) = tokio::io::duplex(1 << 22);
        let (sr, sw) = tokio::io::split(server_io);
        tokio::spawn(async move {
            match StudioMcp::attach(agent_tx, None).serve((sr, sw)).await {
                Ok(s) => {
                    let _ = s.waiting().await;
                }
                Err(e) => eprintln!("videonow-eval: server: {e}"),
            }
        });
        let (cr, cw) = tokio::io::split(client_io);
        let mut agent = Client::new(cr, cw);
        agent.initialize().await?;
        Ok(Self { agent, person: person_tx, thread: Some(thread) })
    }

    /// The person edits the project (in the desktop application this is a click in an editor):
    /// `edit` runs on the studio thread with `ui` as the actor; call `sync_editors` in it as the
    /// application does after each frame. Returns what `edit` returns.
    pub async fn person<T: Send + 'static>(&self, edit: impl FnOnce(&mut Studio) -> T + Send + 'static) -> Result<T, String> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.person
            .send(Work::Person(Box::new(move |s: &mut Studio| {
                let _ = tx.send(edit(s));
            })))
            .map_err(|_| "the studio thread stopped".to_string())?;
        rx.await.map_err(|_| "the studio thread dropped the edit".to_string())
    }

    /// The studio's journal, read on its thread.
    pub async fn journal(&self) -> Result<Vec<Value>, String> {
        self.person(|s| s.journal().to_vec()).await
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        let _ = self.person.send(Work::Stop);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}
