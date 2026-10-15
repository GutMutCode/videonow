//! The command manifest: one list of every command the studio can run, across the three engines,
//! with what each one does to the project.
//!
//! The engines' registries describe a command by id, label, menu, shortcut, a free-form parameter
//! doc and a "journal" flag (false for pure queries). The studio adds an `effect` class so a
//! client can let reads run freely and confirm only destructive or file-writing commands. For
//! engine commands the class is inferred from the journal flag and the id (`basis: "inferred"`);
//! the studio's own commands declare theirs (`basis: "declared"`). Replacing inference with
//! declarations in the engine registries is roadmap phase 1.

use serde::Serialize;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Effect {
    /// Reads only; never changes the project.
    Read,
    /// Changes the project; undoable.
    Edit,
    /// Removes work (deletes, clears); undoable inside the session but loses data if saved.
    Destructive,
    /// Writes files outside the project (exports, saves).
    FileWrite,
}

#[derive(Clone, Debug, Serialize)]
pub struct CommandInfo {
    pub id: String,
    pub domain: &'static str,
    pub label: String,
    pub params: String,
    pub effect: Effect,
    pub basis: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<&'static str>,
}

/// The effect class of an engine command, from its journal flag and id.
pub fn infer_effect(id: &str, journal: bool) -> Effect {
    if !journal {
        return Effect::Read;
    }
    let lower = id.to_ascii_lowercase();
    if lower.contains("export") || lower.starts_with("file.save") || (lower.contains("render") && lower.contains("queue")) {
        Effect::FileWrite
    } else if ["delete", "clear", "remove", "purge", "discard"].iter().any(|w| lower.contains(w)) {
        Effect::Destructive
    } else {
        Effect::Edit
    }
}

/// A studio command: id (without `studio:`), effect, parameter doc, one-line summary.
pub struct StudioCommand {
    pub id: &'static str,
    pub effect: Effect,
    pub params: &'static str,
    /// Every key `params` names: anything else is rejected.
    pub keys: &'static [&'static str],
    pub summary: &'static str,
}

pub const STUDIO_COMMANDS: &[StudioCommand] = &[
    StudioCommand {
        id: "commands",
        effect: Effect::Read,
        params: r#"{"filter":str?,"domain":"timeline|motion|image|studio"?}"#,
        keys: &["filter", "domain"],
        summary: "List every command of every engine with its effect class.",
    },
    StudioCommand { id: "inspect", effect: Effect::Read, params: "{}", keys: &[], summary: "The whole project: assets, the active sequence, compositions, images." },
    StudioCommand { id: "assets.list", effect: Effect::Read, params: "{}", keys: &[], summary: "Compositions and image documents with their ids in each engine." },
    StudioCommand { id: "journal", effect: Effect::Read, params: "{}", keys: &[], summary: "Every command that changed the project, in order, with its receipt." },
    StudioCommand {
        id: "comp.new",
        effect: Effect::Edit,
        params: r#"{"name":str?,"width":u32=1920,"height":u32=1080,"fps":f64=30,"duration":seconds=5}"#,
        keys: &["name", "width", "height", "fps", "duration"],
        summary: "Create a composition (motion engine). Returns its asset id; pass `asset` to `motion:` commands to target it.",
    },
    StudioCommand {
        id: "image.new",
        effect: Effect::Edit,
        params: r#"{"name":str?,"width":u32=1920,"height":u32=1080,"background":"white|black|transparent|#rrggbb"="transparent"}"#,
        keys: &["name", "width", "height", "background"],
        summary: "Create an image document (image engine). Returns its asset id; pass `asset` to `image:` commands to target it.",
    },
    StudioCommand {
        id: "asset.place",
        effect: Effect::Edit,
        params: r#"{"asset":id,"track":"V1"?,"seconds":f64=0}"#,
        keys: &["asset", "track", "seconds"],
        summary: "Put a composition or image on the active sequence; it stays live (edits show on the next frame).",
    },
    StudioCommand {
        id: "image.toComp",
        effect: Effect::Edit,
        params: r#"{"image":id,"comp":id,"time":seconds=0}"#,
        keys: &["image", "comp", "time"],
        summary: "Use an image document as a layer of a composition; it stays live.",
    },
    StudioCommand {
        id: "render.frame",
        effect: Effect::FileWrite,
        params: r#"{"seconds":f64=0,"path":str="frame.png","scale":0..1=1}"#,
        keys: &["seconds", "path", "scale"],
        summary: "Render the active sequence at a time to a PNG; reports offline media it drew a slate for.",
    },
    StudioCommand { id: "undo", effect: Effect::Edit, params: "{}", keys: &[], summary: "Undo the last change, in whichever engine made it." },
];

pub fn studio_command(id: &str) -> Option<&'static StudioCommand> {
    STUDIO_COMMANDS.iter().find(|c| c.id == id)
}

/// Every command, studio first, then each engine's registry.
pub fn all() -> Vec<CommandInfo> {
    let mut v: Vec<CommandInfo> = STUDIO_COMMANDS
        .iter()
        .map(|c| CommandInfo { id: format!("studio:{}", c.id), domain: "studio", label: c.id.to_string(), params: c.params.to_string(), effect: c.effect, basis: "declared", summary: Some(c.summary) })
        .collect();
    for c in filmcraft_engine::command_specs() {
        v.push(CommandInfo { id: format!("timeline:{}", c.id), domain: "timeline", label: c.label.to_string(), params: c.params.to_string(), effect: infer_effect(c.id, c.journal), basis: "inferred", summary: None });
    }
    for c in effectcraft_engine::command_specs() {
        v.push(CommandInfo { id: format!("motion:{}", c.id), domain: "motion", label: c.label.to_string(), params: c.params.to_string(), effect: infer_effect(c.id, c.journal), basis: "inferred", summary: None });
    }
    for c in photocraft_engine::command_specs() {
        v.push(CommandInfo { id: format!("image:{}", c.id), domain: "image", label: c.label.to_string(), params: c.params.to_string(), effect: infer_effect(c.id, c.journal), basis: "inferred", summary: None });
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inference_classes() {
        assert_eq!(infer_effect("sequence.inspect", false), Effect::Read);
        assert_eq!(infer_effect("file.exportMedia", true), Effect::FileWrite);
        assert_eq!(infer_effect("timeline.rippleDelete", true), Effect::Destructive);
        assert_eq!(infer_effect("timeline.place", true), Effect::Edit);
    }

    #[test]
    fn studio_params_docs_name_their_keys() {
        for c in STUDIO_COMMANDS {
            for k in c.keys {
                assert!(c.params.contains(&format!("\"{k}\"")), "{}: `{k}` missing from its params doc", c.id);
            }
        }
    }
}
