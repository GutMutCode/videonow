//! VideoNow, the desktop application: one window, one project, three editors.
//!
//! * **Edit** — the timeline editor (FilmCraft's): sequences, editing, audio, captions, export.
//! * **Motion** — the composition editor (EffectCraft's): layers, keyframes, effects, tracking.
//! * **Image** — the image editor (PhotoCraft's): layers, painting, retouching.
//!
//! The [`Studio`](videonow_core::Studio) owns the engine sessions. Each frame the active editor
//! borrows its session ([`lend`]), draws and edits it with its own UI, and gives it back; the
//! studio then accounts for what changed ([`Studio::sync_editors`](videonow_core::Studio)), so a
//! change made by hand gets a receipt like one made by a command. Compositions and images are
//! assets of the one project: placed on a sequence or inside a composition, they stay live.
//!
//! Around the editors the shell adds what spans them: the asset list, the command palette (every
//! command of every engine), and the Agent panel (agent access over MCP, what the agent may do,
//! and the activity log of receipts, human and agent alike).

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod agent;
mod brand;
mod dialogs;
pub mod lend;
mod palette;
mod panels;
mod photo_services;
pub mod shell;

pub use shell::{Options, VideoNowApp, Workspace};

/// The window options the application starts with.
pub fn native_options() -> eframe::NativeOptions {
    eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("VideoNow")
            .with_app_id("ai.videonow.VideoNow")
            .with_inner_size([1600.0, 980.0])
            .with_min_inner_size([960.0, 600.0])
            .with_drag_and_drop(true)
            .with_icon(brand::app_icon()),
        ..Default::default()
    }
}
