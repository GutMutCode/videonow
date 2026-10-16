//! Lending the studio's engine sessions to the editors.
//!
//! Each engine's editor (`FilmcraftApp`, `EffectcraftApp`, `PhotocraftApp`) was written to own
//! its session. In VideoNow the studio owns them, so for each call into an editor the session is
//! swapped into the editor and back out afterwards. Between frames the sessions are in the
//! studio, where commands (the palette, an agent) run on them.
//!
//! While lent, a session's `host_commands` filter is set: the editors' menu items that would
//! save or open only their own engine's document are queued for the shell, which saves or opens
//! the whole project instead.

use filmcraft_ui_egui::FilmcraftApp;
use effectcraft_ui_egui::EffectcraftApp;
use photocraft_ui_egui::PhotocraftApp;
use videonow_core::Studio;

/// The timeline editor's menu items the shell handles (see `videonow_core::manifest::HOST_COMMANDS`).
pub fn film_host(id: &str) -> bool {
    videonow_core::manifest::is_host_command("timeline", id)
}

/// The motion editor's menu items the shell handles.
pub fn motion_host(id: &str) -> bool {
    videonow_core::manifest::is_host_command("motion", id)
}

/// The image editor's menu items the shell handles: Save saves the project (Save As still writes
/// the image to a file of its own). Closing a tab is the editor's: the image stays in the project.
pub fn photo_host(id: &str) -> bool {
    matches!(id, "file.save")
}

/// What a lent call returned, or that it panicked (the editors catch their own panics; this is
/// the last guard, so a panic never leaves a session stranded in an editor).
pub type Lent<R> = Result<R, String>;

fn panic_message(e: Box<dyn std::any::Any + Send>) -> String {
    e.downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| e.downcast_ref::<String>().cloned()).unwrap_or_else(|| "unknown error".into())
}

pub fn film<R>(studio: &mut Studio, app: &mut FilmcraftApp, f: impl FnOnce(&mut FilmcraftApp) -> R) -> Lent<R> {
    std::mem::swap(&mut studio.timeline, &mut app.session);
    app.session.host_commands = Some(film_host);
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(app)));
    app.session.host_commands = None;
    std::mem::swap(&mut studio.timeline, &mut app.session);
    r.map_err(panic_message)
}

pub fn motion<R>(studio: &mut Studio, app: &mut EffectcraftApp, f: impl FnOnce(&mut EffectcraftApp) -> R) -> Lent<R> {
    std::mem::swap(&mut studio.motion, &mut app.session);
    app.session.host_commands = Some(motion_host);
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(app)));
    app.session.host_commands = None;
    std::mem::swap(&mut studio.motion, &mut app.session);
    r.map_err(panic_message)
}

pub fn photo<R>(studio: &mut Studio, app: &mut PhotocraftApp, f: impl FnOnce(&mut PhotocraftApp) -> R) -> Lent<R> {
    std::mem::swap(&mut studio.images, &mut app.session);
    app.session.host_commands = Some(photo_host);
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(app)));
    app.session.host_commands = None;
    std::mem::swap(&mut studio.images, &mut app.session);
    r.map_err(panic_message)
}

/// Requests the editors queued for the shell since the last call: `(domain, id, params)`.
pub fn take_host_requests(studio: &mut Studio) -> Vec<(&'static str, String, serde_json::Value)> {
    let mut out = Vec::new();
    out.extend(studio.timeline.host_requests.drain(..).map(|(i, p)| ("timeline", i, p)));
    out.extend(studio.motion.host_requests.drain(..).map(|(i, p)| ("motion", i, p)));
    out.extend(studio.images.host_requests.drain(..).map(|(i, p)| ("image", i, p)));
    out
}
