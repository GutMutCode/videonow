//! videonow-eval: can an AI agent get video work done with VideoNow, and does the program keep
//! it honest and safe while it does?
//!
//! The AI-first design this project follows judges software by what happens when an agent uses
//! it: the first run on an unknown project, a change of requirements, bad input, an interrupted
//! connection, a person editing at the same time, the limits of what was delegated. Each of those
//! is a scenario here, run by a reference agent over MCP against a real studio, with checks on
//! both sides: did the agent get the work done, and did the program give it what it needed
//! (errors that say how to fix them, receipts, previews, refusals, no double work, no clobbering).
//!
//! The reference agents use only what any MCP client sees: tool descriptions, replies, error
//! messages. A model-driven agent can be scored on the same scenarios from the project journal
//! it leaves (`docs/eval.md`).

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

pub mod client;
pub mod harness;
pub mod report;
pub mod scenarios;
pub mod score;

/// Run `names` (all scenarios when empty), one after another.
pub async fn run(names: &[String]) -> Vec<report::Outcome> {
    let mut out = Vec::new();
    for n in scenarios::ALL.iter().filter(|n| names.is_empty() || names.iter().any(|x| x == *n)) {
        out.push(scenarios::run(n).await);
    }
    out
}
