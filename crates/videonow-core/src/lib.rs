//! studio-core: one video production project built from three engines.
//!
//! * **timeline** — FilmCraft: sequences, editing, audio, captions, export.
//! * **motion** — EffectCraft: compositions, keyframes, effects, tracking.
//! * **image** — PhotoCraft: layered image documents, painting, retouching.
//!
//! A [`Studio`] owns one session of each engine and the [`assets`] table that ties them together.
//! Compositions and image documents are nested into a sequence (and images into compositions)
//! through the engines' own source interfaces (see [`bridge`]), so each engine keeps rendering its
//! part and the result is one picture.
//!
//! Every command, whatever engine runs it, goes through [`Studio::execute`] and comes back with a
//! [`receipt`]: what changed, which revisions moved, what was checked and what was not. That is
//! the contract AI agents and the UI build on (docs/design.md, "Commands and AI").

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod assets;
pub mod bridge;
pub mod control;
pub mod demo;
pub mod manifest;
pub mod params;
mod params_extra;
pub mod policy;
pub mod receipt;
mod studio;

pub use assets::{Asset, AssetId, AssetKind};
pub use manifest::Effect;
pub use policy::Policy;
pub use receipt::Receipt;
pub use studio::Studio;
pub use studio::project_file;
/// For applications that host the engines' own UIs on the studio's sessions.
pub use studio::ui as host;

#[derive(Debug, thiserror::Error)]
pub enum StudioError {
    #[error("unknown command `{0}` (commands are `timeline:…`, `motion:…`, `image:…` or `studio:…`; see `studio:commands`)")]
    UnknownCommand(String),
    #[error("{cmd}: {msg}")]
    BadParams { cmd: String, msg: String },
    #[error("no asset {0}")]
    NoAsset(u64),
    #[error("asset {0} is not {1}")]
    WrongKind(u64, &'static str),
    #[error("timeline: {0}")]
    Timeline(String),
    #[error("motion: {0}")]
    Motion(String),
    #[error("image: {0}")]
    Image(String),
    /// Refused by the session's delegation policy (see [`policy`]); nothing ran.
    #[error("policy: {0}")]
    Policy(String),
    /// An engine command the studio does not run, and what to use instead; nothing ran.
    #[error("`{0}` is not available in VideoNow: it would cut one engine's document loose from the project. Use `{1}` instead")]
    Reserved(String, &'static str),
    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, StudioError>;
