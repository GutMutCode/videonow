//! Outcomes and metrics, as JSON and as a Markdown table.

use serde::Serialize;

use crate::client::Call;

#[derive(Clone, Debug, Serialize)]
pub struct Check {
    pub name: String,
    pub ok: bool,
    pub detail: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct Outcome {
    pub scenario: String,
    pub checks: Vec<Check>,
    /// Errors the agent fixed from the error message alone (one retry).
    pub recovered: usize,
    /// Commands the delegation policy refused.
    pub refused: usize,
    pub calls: Vec<Call>,
}

/// The numbers a run is judged by.
#[derive(Clone, Debug, Default, Serialize)]
pub struct Metrics {
    pub scenarios: usize,
    pub passed: usize,
    pub checks: usize,
    pub checks_passed: usize,
    /// Tool calls the reference agents made in all.
    pub tool_calls: usize,
    /// Tool calls that came back as errors (expected ones included: bad input, refusals).
    pub tool_errors: usize,
    pub recovered: usize,
    pub refused: usize,
    /// Median and slowest tool call, milliseconds.
    pub median_ms: u128,
    pub max_ms: u128,
}

impl Outcome {
    pub fn new(name: &str) -> Self {
        Self { scenario: name.to_string(), checks: Vec::new(), recovered: 0, refused: 0, calls: Vec::new() }
    }

    pub fn passed(&self) -> bool {
        !self.checks.is_empty() && self.checks.iter().all(|c| c.ok)
    }
}

pub fn metrics(outcomes: &[Outcome]) -> Metrics {
    let mut times: Vec<u128> = outcomes.iter().flat_map(|o| o.calls.iter().map(|c| c.ms)).collect();
    times.sort_unstable();
    Metrics {
        scenarios: outcomes.len(),
        passed: outcomes.iter().filter(|o| o.passed()).count(),
        checks: outcomes.iter().map(|o| o.checks.len()).sum(),
        checks_passed: outcomes.iter().map(|o| o.checks.iter().filter(|c| c.ok).count()).sum(),
        tool_calls: outcomes.iter().map(|o| o.calls.len()).sum(),
        tool_errors: outcomes.iter().map(|o| o.calls.iter().filter(|c| !c.ok).count()).sum(),
        recovered: outcomes.iter().map(|o| o.recovered).sum(),
        refused: outcomes.iter().map(|o| o.refused).sum(),
        median_ms: times.get(times.len() / 2).copied().unwrap_or(0),
        max_ms: times.last().copied().unwrap_or(0),
    }
}

pub fn markdown(outcomes: &[Outcome]) -> String {
    let m = metrics(outcomes);
    let mut s = String::new();
    s.push_str("# VideoNow agent evaluation\n\n");
    s.push_str(&format!(
        "{} of {} scenarios passed · {} of {} checks · {} tool calls ({} errors, {} fixed from the message alone, {} refused by policy) · median call {} ms, slowest {} ms\n\n",
        m.passed, m.scenarios, m.checks_passed, m.checks, m.tool_calls, m.tool_errors, m.recovered, m.refused, m.median_ms, m.max_ms
    ));
    s.push_str("| Scenario | Result | Checks | Calls |\n|---|---|---|---|\n");
    for o in outcomes {
        s.push_str(&format!(
            "| {} | {} | {}/{} | {} |\n",
            o.scenario,
            if o.passed() { "pass" } else { "FAIL" },
            o.checks.iter().filter(|c| c.ok).count(),
            o.checks.len(),
            o.calls.len()
        ));
    }
    for o in outcomes {
        s.push_str(&format!("\n## {}\n\n", o.scenario));
        for c in &o.checks {
            let detail = if c.detail.is_empty() { String::new() } else { format!(" — `{}`", c.detail.replace('`', "'").replace('\n', " ")) };
            s.push_str(&format!("- {} {}{}\n", if c.ok { "✔" } else { "✘" }, c.name, detail));
        }
    }
    s
}
