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
    /// Reaches outside the project and the file system: network downloads, other applications,
    /// printers, system settings.
    External,
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
    /// For engine commands that replace or save an engine's whole document: the studio command
    /// to use instead. The studio refuses these (see [`HOST_COMMANDS`]).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instead: Option<&'static str>,
}

/// Engine commands that create, open, revert or save an engine's whole document. In a studio
/// project these belong to the studio: run on one engine they would leave the others (and the
/// asset table) behind. `Studio::execute` refuses them and names the studio command to use; the
/// desktop application hands the same menu items to its own project handling.
pub const HOST_COMMANDS: &[(&str, &str, &str)] = &[
    ("timeline", "file.newProject", "studio:project.new"),
    ("timeline", "file.newProjectFromTemplate", "studio:project.new"),
    ("timeline", "file.openDemoProject", "studio:project.open"),
    ("timeline", "file.open", "studio:project.open"),
    ("timeline", "file.closeProject", "studio:project.new"),
    ("timeline", "file.closeAllProjects", "studio:project.new"),
    ("timeline", "file.save", "studio:project.save"),
    ("timeline", "file.saveAs", "studio:project.save"),
    ("timeline", "file.saveAll", "studio:project.save"),
    ("timeline", "file.revert", "studio:project.open"),
    ("motion", "file.newProject", "studio:project.new"),
    ("motion", "file.newFromTemplate", "studio:project.new"),
    ("motion", "file.openDemoProject", "studio:project.open"),
    ("motion", "file.open", "studio:project.open"),
    ("motion", "file.openRecent", "studio:project.open"),
    ("motion", "file.closeProject", "studio:project.new"),
    ("motion", "file.save", "studio:project.save"),
    ("motion", "file.saveAs", "studio:project.save"),
    ("motion", "file.revert", "studio:project.open"),
    ("image", "file.close", "the image workspace (project images stay in the project)"),
    ("image", "file.closeAll", "the image workspace (project images stay in the project)"),
    ("image", "file.closeOthers", "the image workspace (project images stay in the project)"),
];

/// The studio command to use instead of `domain:cmd`, when that is a [`HOST_COMMANDS`] entry.
pub fn host_command(domain: &str, cmd: &str) -> Option<&'static str> {
    HOST_COMMANDS.iter().find(|(d, c, _)| *d == domain && *c == cmd).map(|(_, _, i)| *i)
}

/// For an engine session's `host_commands` filter: is `cmd` of `domain` handled by the host?
pub fn is_host_command(domain: &str, cmd: &str) -> bool {
    host_command(domain, cmd).is_some()
}

/// The effect class of an engine command, from its id and journal flag.
///
/// The id comes first: an engine's `journal: false` means "not recorded in the history", which
/// includes commands that export files, download models or open other applications. Only when
/// the id names none of those does `journal: false` mean a read.
pub fn infer_effect(id: &str, journal: bool) -> Effect {
    let lower = id.to_ascii_lowercase();
    let words: Vec<&str> = lower.split(|c: char| c == '.' || c == '_').collect();
    let has = |w: &str| lower.contains(w);
    if ["download", "install", "reveal", "launch", "openurl", "print", "browse"].iter().any(|w| has(w)) && !has("printers") && !has("preview") {
        return Effect::External;
    }
    // `renderQueue.render`, `tts.render` write files; `renderQueue.list`, `sequence.renderBar` read
    if has("export") || lower.starts_with("file.save") || words.iter().any(|w| w.starts_with("save")) || words.last() == Some(&"render") {
        return Effect::FileWrite;
    }
    if ["delete", "clear", "remove", "purge", "discard"].iter().any(|w| has(w)) {
        return Effect::Destructive;
    }
    if journal { Effect::Edit } else { Effect::Read }
}

/// Engine commands whose effect the id and journal flag get wrong, found by running every `read`
/// command on a populated project (`tests/studio.rs`, `read_commands_change_nothing`): they are
/// not journaled but change the document (view state stored in it, a document timeline).
pub const EFFECT_OVERRIDES: &[(&str, &str, Effect)] = &[
    ("timeline", "media.toggleProxies", Effect::Edit),
    ("image", "select.isolateLayers", Effect::Edit),
    ("image", "timeline.create", Effect::Edit),
    ("image", "timeline.nextFrame", Effect::Edit),
    ("image", "timeline.previousFrame", Effect::Edit),
    ("image", "timeline.setProps", Effect::Edit),
];

/// An engine command's effect: a declared override, else inferred.
pub fn engine_effect(domain: &str, id: &str, journal: bool) -> (Effect, &'static str) {
    match EFFECT_OVERRIDES.iter().find(|(d, c, _)| *d == domain && *c == id) {
        Some((_, _, e)) => (*e, "declared"),
        None => (infer_effect(id, journal), "inferred"),
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
    StudioCommand {
        id: "project.save",
        effect: Effect::FileWrite,
        params: r#"{"path":str? (a .videonow folder; default: where it was last saved)}"#,
        keys: &["path"],
        summary: "Save the whole project (timeline, compositions, images, assets) as a .videonow folder.",
    },
    StudioCommand {
        id: "project.open",
        effect: Effect::Destructive,
        params: r#"{"path":str (a .videonow folder)}"#,
        keys: &["path"],
        summary: "Replace the open project with a saved one (unsaved changes are lost; undo history starts over).",
    },
    StudioCommand { id: "project.new", effect: Effect::Destructive, params: "{}", keys: &[], summary: "Start an empty project (unsaved changes are lost)." },
    StudioCommand { id: "undo", effect: Effect::Edit, params: "{}", keys: &[], summary: "Undo the last change, in whichever engine made it." },
    StudioCommand {
        id: "batch",
        effect: Effect::Edit,
        params: r#"{"steps":[{"id":str,"params":{…}}],"atomic":bool=true}"#,
        keys: &["steps", "atomic"],
        summary: "Run several commands as one change: if a step fails the earlier ones are taken back (atomic). A string \"$0.asset\" in a step is replaced by step 0's result.asset. Each step is checked like a command of its own.",
    },
];

/// The effect class of `domain:cmd`, if the command exists.
pub fn effect_of(domain: &str, cmd: &str) -> Option<Effect> {
    match domain {
        "studio" => studio_command(cmd).map(|c| c.effect),
        "timeline" => filmcraft_engine::find_command(cmd).map(|c| engine_effect(domain, c.id, c.journal).0),
        "motion" => effectcraft_engine::find_command(cmd).map(|c| engine_effect(domain, c.id, c.journal).0),
        "image" => photocraft_engine::commands::find(cmd).map(|c| engine_effect(domain, c.id, c.journal).0),
        _ => None,
    }
}

pub fn studio_command(id: &str) -> Option<&'static StudioCommand> {
    STUDIO_COMMANDS.iter().find(|c| c.id == id)
}

/// Edit distance between two short strings.
fn distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut cur = Vec::with_capacity(b.len() + 1);
        cur.push(i + 1);
        for (j, cb) in b.iter().enumerate() {
            let sub = prev.get(j).copied().unwrap_or(0) + usize::from(ca != *cb);
            let del = prev.get(j + 1).copied().unwrap_or(0) + 1;
            let ins = cur.get(j).copied().unwrap_or(0) + 1;
            cur.push(sub.min(del).min(ins));
        }
        prev = cur;
    }
    prev.last().copied().unwrap_or(0)
}

/// The command id closest to `id` (a misspelling, or the right command in the wrong domain),
/// when one is close enough to suggest.
pub fn closest_command(id: &str) -> Option<String> {
    static IDS: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
    let ids = IDS.get_or_init(|| all().into_iter().map(|c| c.id).collect());
    let lower = id.to_ascii_lowercase();
    let cmd = lower.split_once(':').map_or(lower.as_str(), |(_, c)| c);
    // the same command in another domain (`timeline:comp.new` → `studio:comp.new`)
    if let Some(other) = ids.iter().find(|i| i.split_once(':').is_some_and(|(_, c)| c.eq_ignore_ascii_case(cmd))) {
        return Some(other.clone());
    }
    // the right name in another group of the same domain (`timeline:timeline.rippleDelete` →
    // `timeline:edit.rippleDelete`)
    let domain = lower.split_once(':').map(|(d, _)| d);
    let last = cmd.rsplit('.').next().unwrap_or(cmd);
    if last.len() >= 4 {
        let same: Vec<&String> = ids
            .iter()
            .filter(|i| i.split_once(':').is_some_and(|(d, c)| Some(d) == domain && c.rsplit('.').next().is_some_and(|l| l.eq_ignore_ascii_case(last))))
            .collect();
        if let [one] = same.as_slice() {
            return Some((*one).clone());
        }
    }
    let (d, best) = ids.iter().map(|i| (distance(&i.to_ascii_lowercase(), &lower), i)).min_by_key(|(d, _)| *d)?;
    (d <= 3.max(lower.len() / 6)).then(|| best.clone())
}

/// Every command, studio first, then each engine's registry.
pub fn all() -> Vec<CommandInfo> {
    let mut v: Vec<CommandInfo> = STUDIO_COMMANDS
        .iter()
        .map(|c| CommandInfo { id: format!("studio:{}", c.id), domain: "studio", label: c.id.to_string(), params: c.params.to_string(), effect: c.effect, basis: "declared", summary: Some(c.summary), instead: None })
        .collect();
    for c in filmcraft_engine::command_specs() {
        let (effect, basis) = engine_effect("timeline", c.id, c.journal);
        v.push(CommandInfo { id: format!("timeline:{}", c.id), domain: "timeline", label: c.label.to_string(), params: c.params.to_string(), effect, basis, summary: None, instead: host_command("timeline", c.id) });
    }
    for c in effectcraft_engine::command_specs() {
        let (effect, basis) = engine_effect("motion", c.id, c.journal);
        v.push(CommandInfo { id: format!("motion:{}", c.id), domain: "motion", label: c.label.to_string(), params: c.params.to_string(), effect, basis, summary: None, instead: host_command("motion", c.id) });
    }
    for c in photocraft_engine::command_specs() {
        let (effect, basis) = engine_effect("image", c.id, c.journal);
        v.push(CommandInfo { id: format!("image:{}", c.id), domain: "image", label: c.label.to_string(), params: c.params.to_string(), effect, basis, summary: None, instead: host_command("image", c.id) });
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
        // not journaled, but not reads
        assert_eq!(infer_effect("lut.export", false), Effect::FileWrite);
        assert_eq!(infer_effect("actions.save", false), Effect::FileWrite);
        assert_eq!(infer_effect("tts.downloadVoices", false), Effect::External);
        assert_eq!(infer_effect("file.reveal", false), Effect::External);
        assert_eq!(infer_effect("file.export.presets.delete", false), Effect::FileWrite);
        assert_eq!(infer_effect("file.printers", false), Effect::Read);
        assert_eq!(infer_effect("renderQueue.list", false), Effect::Read);
        assert_eq!(infer_effect("renderQueue.render", true), Effect::FileWrite);
        assert_eq!(infer_effect("sequence.renderBar", false), Effect::Read);
    }

    #[test]
    fn misspelled_commands_get_a_suggestion() {
        assert_eq!(closest_command("timeline:timeline.plcae").as_deref(), Some("timeline:timeline.place"));
        assert_eq!(closest_command("timeline:comp.new").as_deref(), Some("studio:comp.new"));
        assert_eq!(closest_command("timeline:timeline.rippleDelete").as_deref(), Some("timeline:edit.rippleDelete"));
        assert_eq!(closest_command("zzz:qqqqqqqqqqqqqqqqqq"), None);
    }

    #[test]
    fn host_commands_exist_in_their_engines() {
        for (d, c, instead) in HOST_COMMANDS {
            assert!(effect_of(d, c).is_some(), "{d}:{c} is not an engine command");
            if let Some(s) = instead.strip_prefix("studio:") {
                assert!(studio_command(s).is_some(), "{instead} is not a studio command");
            }
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
