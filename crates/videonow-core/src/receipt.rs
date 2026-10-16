//! Receipts: every command's answer to "what did that do?".
//!
//! A command's own result says what the command reports (an id, a count). The receipt says what
//! the studio can vouch for: which engine documents changed and by how many revisions, which
//! asset it targeted, what was checked before and after it ran, and what was not checked. An
//! agent reads `checked` / `not_checked` instead of assuming that "ok" means "the picture is
//! right" (design principle 6.4 of the AI-software philosophy this project follows).

use serde::Serialize;

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Revisions {
    pub before: u64,
    pub after: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Receipt {
    /// The full command id, `domain:id`.
    pub command: String,
    pub domain: String,
    /// The studio-wide revision: moves by one for every command that changed anything.
    pub revision: Revisions,
    /// Engine documents that changed: `timeline`, `motion`, `image:<asset>`, `studio`.
    pub changed: Vec<String>,
    /// The asset the command targeted, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub asset: Option<u64>,
    /// Assets the command created.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub created: Vec<u64>,
    pub warnings: Vec<String>,
    /// What the studio did to the request before running it (paths resolved inside the
    /// session's folders, …).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
    /// What was verified (by the studio or the engine) around this command.
    pub checked: Vec<&'static str>,
    /// What nobody verified: look before relying on it.
    pub not_checked: Vec<&'static str>,
    /// Checks a command made itself, added to `checked` when the receipt is finished.
    #[serde(skip)]
    pub extra_checked: Vec<&'static str>,
}

impl Receipt {
    /// Record a check the command itself made (e.g. a saved file read back).
    pub fn checked_extra(&mut self, what: &'static str) {
        self.extra_checked.push(what);
    }
}

/// What every engine command gets checked for.
pub const ENGINE_CHECKED: &[&str] = &["command exists", "delegation policy", "engine preconditions (enabled state)", "engine parameter parsing"];
/// Engine commands whose unknown parameters are refused (by the studio or the engine).
pub const STRICT_CHECKED: &str = "unknown parameters rejected";
/// What the studio's own commands check.
pub const STUDIO_CHECKED: &[&str] = &["command exists", "delegation policy", "unknown parameters rejected", "asset ids resolved"];
/// What a command never verifies on its own.
pub const NOT_CHECKED_VISUAL: &[&str] = &["the rendered picture (render a frame to look at it)"];
