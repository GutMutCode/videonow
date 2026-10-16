//! The command manifest: one list of every command the studio can run, across the three engines,
//! with what each one does to the project.
//!
//! The engines' registries describe a command by id, label, menu, shortcut, a free-form parameter
//! doc and a "journal" flag (false for pure queries). The studio adds an `effect` class so a
//! client can let reads run freely and confirm only destructive or file-writing commands, and a
//! session can be limited to some classes ([`crate::policy`]).
//!
//! * Studio commands declare their class (`basis: "declared"`).
//! * Engine commands listed in [`DECLARED`] declare theirs too: those are the ones the inference
//!   below gets wrong, found by reviewing the whole list.
//! * Every other engine command is inferred from its journal flag and id (`basis: "inferred"`).
//!   The `read` class is checked: `tests/manifest.rs` runs every read command on a populated
//!   project and fails if one changes anything.

use serde::Serialize;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Effect {
    /// Reads only; never changes the project or writes files.
    Read,
    /// Changes the project; undoable.
    Edit,
    /// Removes work (deletes, clears, replaces the project); loses data unless undone.
    Destructive,
    /// Writes files outside the project document (exports, saves, renders).
    FileWrite,
    /// Reaches outside the project: runs scripts or plug-ins, opens a browser or the file
    /// manager, prints, downloads. Refused while file access is confined.
    External,
}

impl Effect {
    pub fn word(self) -> &'static str {
        match self {
            Effect::Read => "read",
            Effect::Edit => "edit",
            Effect::Destructive => "destructive",
            Effect::FileWrite => "fileWrite",
            Effect::External => "external",
        }
    }
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
    /// Engine commands the studio refuses, and what to use instead.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reserved: Option<&'static str>,
    /// Parameters the engine reads that its parameter doc does not name (found by the engines'
    /// own tests); accepted like documented ones.
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    pub also_accepts: Vec<&'static str>,
    /// Whether unknown parameters are refused before the command runs.
    pub strict: bool,
}

/// The words of a command id: `filter.render.lensFlare` → `filter render lens flare`.
pub fn words(id: &str) -> Vec<String> {
    let mut out = Vec::new();
    for seg in id.split(['.', '_', '-']) {
        let mut cur = String::new();
        for ch in seg.chars() {
            if ch.is_ascii_uppercase() && !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
            cur.push(ch.to_ascii_lowercase());
        }
        if !cur.is_empty() {
            out.push(cur);
        }
    }
    out
}

/// Last words of a query (`export.presets.list`, `renderQueue.formats`).
const READ_LAST: &[&str] = &["list", "get", "formats", "resolve", "templates", "status", "inspect", "info", "find", "describe", "summary", "stats"];
/// Verbs that change something, in a command the engine does not journal.
const MUTATING: &[&str] = &[
    "create", "new", "add", "set", "toggle", "apply", "clear", "delete", "remove", "undo", "redo", "step", "reset", "insert", "move", "rename", "duplicate", "import", "load",
    "make", "convert", "merge", "split", "assign", "attach", "detach", "enable", "disable", "lock", "unlock", "isolate", "replace", "paste", "cut", "fill", "update", "write",
    "record", "purge", "sort", "change", "flatten", "rasterize", "discard", "lift",
];
/// Words that mean a command removes work.
const DESTRUCTIVE_WORDS: &[&str] = &["delete", "clear", "remove", "purge", "discard", "flatten", "rasterize", "lift", "reset"];

/// Whether a command reaches outside the project (scripts, plug-ins, other applications,
/// printers, the network). The engines' journal flag does not tell: opening a web page changes no
/// document.
fn is_external(w: &[String], id: &str) -> bool {
    let has = |x: &str| w.iter().any(|y| y == x);
    let first = w.first().map(String::as_str).unwrap_or("");
    let last = w.last().map(String::as_str).unwrap_or("");
    if has("download") || has("finder") || id == "file.reveal" || id == "help.revealLogFile" {
        return true;
    }
    // help menus open web pages (systemInfo / systemReport only describe this machine)
    if first == "help" && w.get(1).map(String::as_str) != Some("system") {
        return true;
    }
    if (has("plugin") || has("plugins")) && matches!(last, "install" | "remove" | "reload" | "run" | "load") {
        return true;
    }
    if has("print") && !matches!(last, "page" | "printers") {
        return true;
    }
    // scripts run code; the image engine's built-in "scripts" that only edit layers do not
    let scripty = has("script") || has("scripts") || has("scriptui");
    let builtin_edit = first == "file" && w.iter().any(|x| DESTRUCTIVE_WORDS.contains(&x.as_str()));
    scripty && first != "window" && !has("settings") && !READ_LAST.contains(&last) && !builtin_edit
}

/// The effect class of an engine command, from its journal flag and the words of its id.
/// [`DECLARED`] overrides it where it is wrong.
pub fn infer_effect(id: &str, journal: bool) -> Effect {
    let w = words(id);
    let first = w.first().map(String::as_str).unwrap_or("");
    let last = w.last().map(String::as_str).unwrap_or("");
    if is_external(&w, id) {
        return Effect::External;
    }
    // pixel filters and adjustments change the document, whatever they are called ("Render ▸ Clouds")
    if matches!(first, "filter" | "image") {
        return if journal { Effect::Edit } else { Effect::Read };
    }
    if !journal && READ_LAST.contains(&last) {
        return Effect::Read;
    }
    if w.iter().any(|x| x == "export") || id.starts_with("file.save") {
        return Effect::FileWrite;
    }
    if !journal {
        // an engine may leave a change out of its journal (undo, a toggle, a panel's own list);
        // a verb that changes things makes it an edit, to be safe
        let verb = w.iter().skip(1).find(|x| MUTATING.contains(&x.as_str()));
        return match verb {
            Some(v) if DESTRUCTIVE_WORDS.contains(&v.as_str()) => Effect::Destructive,
            Some(_) => Effect::Edit,
            None => Effect::Read,
        };
    }
    if w.iter().any(|x| DESTRUCTIVE_WORDS.contains(&x.as_str())) { Effect::Destructive } else { Effect::Edit }
}

/// Engine commands whose class the inference gets wrong: `(domain:id, class)`. Found by reviewing
/// the whole inferred list; `tests/manifest.rs` checks every `read` command changes nothing.
pub const DECLARED: &[(&str, Effect)] = &[
    // they write render, preview or media files
    ("timeline:sequence.renderEffectsInToOut", Effect::FileWrite),
    ("timeline:sequence.renderInToOut", Effect::FileWrite),
    ("timeline:sequence.renderSelection", Effect::FileWrite),
    ("timeline:sequence.renderAudio", Effect::FileWrite),
    ("timeline:sequence.renderBar", Effect::FileWrite),
    ("timeline:sequence.deleteRenderFiles", Effect::FileWrite),
    ("timeline:sequence.deleteRenderFilesInToOut", Effect::FileWrite),
    ("timeline:media.createProxies", Effect::FileWrite),
    ("timeline:clip.extractAudio", Effect::FileWrite),
    ("timeline:tts.render", Effect::FileWrite),
    ("timeline:export.queue.start", Effect::FileWrite),
    ("timeline:export.quick", Effect::FileWrite),
    ("motion:renderQueue.render", Effect::FileWrite),
    ("motion:render.preRender", Effect::FileWrite),
    ("motion:render.saveCurrentPreview", Effect::FileWrite),
    ("motion:file.collectFiles", Effect::FileWrite),
    ("motion:file.saveCopy", Effect::FileWrite),
    ("motion:file.saveCopyAsXml", Effect::FileWrite),
    ("motion:templates.saveAs", Effect::FileWrite),
    ("image:file.saveACopy", Effect::FileWrite),
    ("motion:file.createProxy", Effect::FileWrite),
    // they cut material out of a sequence or a composition
    ("timeline:sequence.extract", Effect::Destructive),
    ("timeline:transcript.extract", Effect::Destructive),
    ("timeline:edit.rippleDelete", Effect::Destructive),
    ("motion:edit.extractWorkArea", Effect::Destructive),
    // they merge layers into one, losing what kept them apart
    ("image:layer.mergeDown", Effect::Destructive),
    ("image:layer.mergeVisible", Effect::Destructive),
    ("image:layer.mergeGroup", Effect::Destructive),
    ("image:layer.mergeLayers", Effect::Destructive),
    ("image:channel.merge", Effect::Destructive),
    ("image:channel.mergeSpot", Effect::Destructive),
    // not journaled by the engine, but they change the project
    ("timeline:media.toggleProxies", Effect::Edit),
    // export queue bookkeeping and presets: project or preference edits, not files
    ("timeline:export.queue.add", Effect::Edit),
    ("timeline:export.queue.stop", Effect::Edit),
    ("timeline:export.queue.cancel", Effect::Edit),
    ("timeline:export.queue.retry", Effect::Edit),
    ("timeline:export.queue.move", Effect::Edit),
    ("timeline:export.queue.remove", Effect::Edit),
    ("timeline:export.queue.clear", Effect::Edit),
    ("timeline:export.presets.save", Effect::Edit),
    ("timeline:export.presets.delete", Effect::Edit),
    ("timeline:export.presets.favorite", Effect::Edit),
    ("timeline:export.presets.import", Effect::Edit),
    ("image:file.export.presets.save", Effect::Edit),
    ("image:file.export.presets.delete", Effect::Edit),
];

/// Engine commands the studio never runs: they replace or close one engine's document, which
/// would cut it loose from the project's other engines. `(domain:id, what to use instead)`.
pub const RESERVED: &[(&str, &str)] = &[
    ("timeline:file.open", "studio:project.open"),
    ("timeline:file.newProject", "studio:project.new"),
    ("timeline:file.revert", "studio:project.open"),
    ("timeline:file.recover", "studio:project.open"),
    ("timeline:file.openDemoProject", "studio:project.open"),
    ("timeline:file.newProjectFromTemplate", "studio:project.new"),
    ("timeline:file.close", "studio:project.new"),
    ("timeline:file.closeProject", "studio:project.new"),
    ("timeline:file.closeAllProjects", "studio:project.new"),
    ("timeline:file.closeAllOtherProjects", "studio:project.new"),
    ("timeline:file.save", "studio:project.save"),
    ("timeline:file.saveAs", "studio:project.save"),
    ("timeline:file.saveAll", "studio:project.save"),
    ("motion:file.open", "studio:project.open"),
    ("motion:file.newProject", "studio:project.new"),
    ("motion:file.revert", "studio:project.open"),
    ("motion:file.openDemoProject", "studio:project.open"),
    ("motion:file.openRecent", "studio:project.open"),
    ("motion:file.newFromTemplate", "studio:project.new"),
    ("motion:file.close", "studio:project.new"),
    ("motion:file.closeProject", "studio:project.new"),
    ("motion:app.quit", "quitting the VideoNow app"),
    ("motion:file.save", "studio:project.save"),
    ("motion:file.saveAs", "studio:project.save"),
    ("motion:file.incrementAndSave", "studio:project.save"),
    ("image:file.openAs", "studio:image.open"),
    ("image:file.openExrParts", "studio:image.open"),
    ("image:file.new", "studio:image.new"),
    ("image:file.newFromClipboard", "studio:image.new"),
    ("image:file.close", "studio:project.new"),
    ("image:file.closeAll", "studio:project.new"),
    ("image:file.closeOthers", "studio:project.new"),
    ("image:file.revert", "studio:project.open"),
];

pub fn reserved(full_id: &str) -> Option<&'static str> {
    RESERVED.iter().find(|(id, _)| *id == full_id).map(|(_, alt)| *alt)
}

/// The class of an engine command and where it came from.
pub fn engine_effect(full_id: &str, journal: bool) -> (Effect, &'static str) {
    match DECLARED.iter().find(|(id, _)| *id == full_id) {
        Some((_, e)) => (*e, "declared"),
        None => {
            let local = full_id.split_once(':').map_or(full_id, |(_, c)| c);
            (infer_effect(local, journal), "inferred")
        }
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
        params: r#"{"filter":str?,"domain":"timeline|motion|image|studio"?,"effect":"read|edit|destructive|fileWrite|external"?}"#,
        keys: &["filter", "domain", "effect"],
        summary: "List every command of every engine with its effect class.",
    },
    StudioCommand { id: "describe", effect: Effect::Read, params: r#"{"id":"domain:command"}"#, keys: &["id"], summary: "One command in full: parameters, effect class, accepted keys, whether it is refused here and why." },
    StudioCommand { id: "inspect", effect: Effect::Read, params: "{}", keys: &[], summary: "The whole project: name, path, unsaved changes, assets, the active sequence, compositions, images, the session's policy." },
    StudioCommand { id: "assets.list", effect: Effect::Read, params: "{}", keys: &[], summary: "Compositions and image documents with their ids in each engine." },
    StudioCommand { id: "journal", effect: Effect::Read, params: r#"{"since":revision?}"#, keys: &["since"], summary: "Every command that changed the project, in order, with its receipt (`since`: only later ones)." },
    StudioCommand { id: "policy", effect: Effect::Read, params: "{}", keys: &[], summary: "What this session may do: allowed effect classes and file folders." },
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
        id: "image.open",
        effect: Effect::Edit,
        params: r#"{"path":"photo.png","name":str?}"#,
        keys: &["path", "name"],
        summary: "Open an image file (PNG, JPEG, PSD, …) as a new image document. Returns its asset id.",
    },
    StudioCommand {
        id: "asset.open",
        effect: Effect::Read,
        params: r#"{"asset":id}"#,
        keys: &["asset"],
        summary: "Show an asset in its engine: a composition becomes the motion engine's active composition, an image the image engine's active document. Returns the workspace (`motion` or `image`) that edits it.",
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
    StudioCommand {
        id: "project.save",
        effect: Effect::FileWrite,
        params: r#"{"path":"Film.videonow"?}"#,
        keys: &["path"],
        summary: "Save the whole project (timeline, compositions, images) to a .videonow folder; without `path`, where it was last saved. Every file is read back after writing.",
    },
    StudioCommand {
        id: "project.open",
        effect: Effect::Destructive,
        params: r#"{"path":"Film.videonow","discard":bool=false}"#,
        keys: &["path", "discard"],
        summary: "Open a .videonow project, replacing this one. Refused with unsaved changes unless `discard` is true.",
    },
    StudioCommand {
        id: "project.new",
        effect: Effect::Destructive,
        params: r#"{"name":str?,"discard":bool=false}"#,
        keys: &["name", "discard"],
        summary: "Start an empty project, replacing this one. Refused with unsaved changes unless `discard` is true.",
    },
];

pub fn studio_command(id: &str) -> Option<&'static StudioCommand> {
    STUDIO_COMMANDS.iter().find(|c| c.id == id)
}

fn engine_info(domain: &'static str, id: &str, label: &str, params: &str, journal: bool) -> CommandInfo {
    let full = format!("{domain}:{id}");
    let (effect, basis) = engine_effect(&full, journal);
    let extra = crate::params::also_accepts(&full);
    let strict = crate::params::is_strict(domain, &full, params);
    CommandInfo { reserved: reserved(&full), id: full, domain, label: label.to_string(), params: params.to_string(), effect, basis, summary: None, also_accepts: extra, strict }
}

/// Every command, studio first, then each engine's registry.
pub fn all() -> Vec<CommandInfo> {
    let mut v: Vec<CommandInfo> = STUDIO_COMMANDS
        .iter()
        .map(|c| CommandInfo {
            id: format!("studio:{}", c.id),
            domain: "studio",
            label: c.id.to_string(),
            params: c.params.to_string(),
            effect: c.effect,
            basis: "declared",
            summary: Some(c.summary),
            reserved: None,
            also_accepts: Vec::new(),
            strict: true,
        })
        .collect();
    for c in filmcraft_engine::command_specs() {
        v.push(engine_info("timeline", c.id, c.label, c.params, c.journal));
    }
    for c in effectcraft_engine::command_specs() {
        v.push(engine_info("motion", c.id, c.label, c.params, c.journal));
    }
    for c in photocraft_engine::command_specs() {
        v.push(engine_info("image", c.id, c.label, c.params, c.journal));
    }
    v
}

/// One command's manifest entry (`domain:id`).
pub fn find(full_id: &str) -> Option<CommandInfo> {
    let (domain, id) = full_id.split_once(':')?;
    match domain {
        "studio" => all().into_iter().find(|c| c.id == full_id),
        "timeline" => filmcraft_engine::find_command(id).map(|c| engine_info("timeline", c.id, c.label, c.params, c.journal)),
        "motion" => effectcraft_engine::find_command(id).map(|c| engine_info("motion", c.id, c.label, c.params, c.journal)),
        "image" => photocraft_engine::commands::find(id).map(|c| engine_info("image", c.id, c.label, c.params, c.journal)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inference_classes() {
        assert_eq!(infer_effect("sequence.inspect", false), Effect::Read);
        assert_eq!(infer_effect("file.exportMedia", false), Effect::FileWrite, "an export writes a file even when it is not journaled");
        assert_eq!(infer_effect("timeline.rippleDelete", true), Effect::Destructive);
        assert_eq!(infer_effect("timeline.place", true), Effect::Edit);
        assert_eq!(infer_effect("filter.render.lensFlare", true), Effect::Edit, "a pixel filter, not a render to file");
        assert_eq!(infer_effect("gradient.presets.select", true), Effect::Edit, "`presets` is not `reset`");
        assert_eq!(infer_effect("export.presets.list", false), Effect::Read);
        assert_eq!(infer_effect("filter.distort.ripple", true), Effect::Edit);
        assert_eq!(infer_effect("help.website", true), Effect::External);
        assert_eq!(infer_effect("help.systemInfo", false), Effect::Read);
        assert_eq!(infer_effect("file.runScript", true), Effect::External);
        assert_eq!(infer_effect("file.scripts.flattenAllMasks", true), Effect::Destructive, "a built-in edit, not a script");
        assert_eq!(infer_effect("plugin.install", false), Effect::External, "not journaled, but not a read either");
        assert_eq!(infer_effect("layer.revealInFinder", true), Effect::External);
        assert_eq!(infer_effect("layer.layerMask.revealAll", true), Effect::Edit);
        assert_eq!(infer_effect("history.undo", false), Effect::Edit, "not journaled, but it changes the document");
        assert_eq!(infer_effect("history.list", false), Effect::Read);
        assert_eq!(infer_effect("view.colorSamplers.clear", false), Effect::Destructive);
    }

    #[test]
    fn words_split_ids() {
        assert_eq!(words("filter.render.lensFlare"), ["filter", "render", "lens", "flare"]);
        assert_eq!(words("export.presets.list"), ["export", "presets", "list"]);
    }

    #[test]
    fn declared_and_reserved_ids_exist() {
        for (id, _) in DECLARED.iter() {
            assert!(find(id).is_some(), "{id} is declared but no engine has it");
        }
        for (id, _) in RESERVED.iter() {
            assert!(find(id).is_some(), "{id} is reserved but no engine has it");
        }
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
