//! Delegation: what this session may do, enforced by the studio before any engine runs.
//!
//! An account (or a local user) can run every command; a task usually should not. The policy
//! says what was delegated to the agent driving this session:
//!
//! * **Effect classes** (`--allow read,edit`): commands outside them are refused before they run,
//!   with the class named so the agent can ask the person instead.
//! * **File roots** (`--read-root DIR`, `--write-root DIR`): with any root given, file paths in
//!   parameters must resolve inside them (symbolic links and `..` resolved; relative paths are
//!   taken from the first root and rewritten to absolute ones, reported in the receipt). Reading is
//!   allowed under read and write roots, writing only under write roots. While confined,
//!   `external` commands (scripts, plug-ins, other applications, printers, downloads) are refused,
//!   and so are file-writing commands that name no path: where they would write cannot be checked.
//!
//! The policy only narrows: no parameter, document or command result can widen it.

use std::path::{Component, Path, PathBuf};

use serde::Serialize;
use serde_json::Value;

use crate::manifest::Effect;
use crate::{Result, StudioError};

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Policy {
    pub read_roots: Vec<PathBuf>,
    pub write_roots: Vec<PathBuf>,
    /// The effect classes that may run; `None` = every class.
    pub allow: Option<Vec<Effect>>,
}

/// Parameter keys that hold file or folder paths (any engine).
const PATH_KEYS: &[&str] = &[
    "path", "paths", "file", "files", "dir", "folder", "directory", "destination", "out", "output", "outputPath", "mapPath", "lutPath", "proxyPath",
    "importKys", "fontPath", "sourcePath", "targetPath", "outDir", "dataDir",
];

/// Engine commands that write to paths stored in the project or the engine's settings, not ones
/// their parameters give, refused while confined: `(local id, why)`.
const STORED_PATHS: &[(&str, &str)] = &[
    ("file.save", "writes to the engine's own file"),
    ("file.saveAll", "writes to the engine's own file"),
    ("file.incrementAndSave", "writes to the engine's own file"),
    ("renderQueue.render", "writes to the output paths stored in the render queue"),
    ("export.queue.start", "writes to the output paths stored in the export queue"),
];

/// What [`Policy::confine_params`] did to a command's parameters.
#[derive(Debug, Default)]
pub struct Confined {
    /// One receipt note per path rewritten to an absolute one.
    pub notes: Vec<String>,
    /// How many file paths the parameters hold (all checked).
    pub paths: usize,
}

impl Policy {
    pub fn confined(&self) -> bool {
        !self.read_roots.is_empty() || !self.write_roots.is_empty()
    }

    /// Parse `read,edit,destructive,fileWrite` (or `all`).
    pub fn parse_allow(s: &str) -> std::result::Result<Option<Vec<Effect>>, String> {
        if s.trim().eq_ignore_ascii_case("all") {
            return Ok(None);
        }
        let mut v = Vec::new();
        for part in s.split(',').map(str::trim).filter(|p| !p.is_empty()) {
            v.push(match part.to_ascii_lowercase().as_str() {
                "read" => Effect::Read,
                "edit" => Effect::Edit,
                "destructive" => Effect::Destructive,
                "filewrite" | "file-write" | "write" => Effect::FileWrite,
                "external" => Effect::External,
                other => return Err(format!("unknown effect class `{other}` (read, edit, destructive, fileWrite, external, all)")),
            });
        }
        if !v.contains(&Effect::Read) {
            // looking is always allowed: a session that cannot read cannot check anything
            v.push(Effect::Read);
        }
        Ok(Some(v))
    }

    /// Refuse a command whose effect class was not delegated.
    pub fn check_effect(&self, cmd: &str, effect: Effect) -> Result<()> {
        match &self.allow {
            Some(a) if !a.contains(&effect) => Err(StudioError::Policy(format!(
                "`{cmd}` is a {} command, which this session was not given (allowed: {}); ask the person to run it or to widen the session",
                effect.word(),
                a.iter().map(|e| e.word()).collect::<Vec<_>>().join(", ")
            ))),
            _ => Ok(()),
        }
    }

    /// Refuse an engine command that cannot be confined to the session's folders.
    pub fn check_confinable(&self, cmd: &str, effect: Effect) -> Result<()> {
        if !self.confined() {
            return Ok(());
        }
        if effect == Effect::External {
            return Err(StudioError::Policy(format!(
                "`{cmd}` reaches outside the project (scripts, plug-ins, other applications, printers or downloads), which is not allowed while file access is confined to the session's folders"
            )));
        }
        let local = cmd.split_once(':').map_or(cmd, |(_, c)| c);
        match STORED_PATHS.iter().find(|(id, _)| *id == local) {
            Some((_, why)) => Err(StudioError::Policy(format!("`{cmd}` {why}, which cannot be checked while file access is confined to the session's folders"))),
            None => Ok(()),
        }
    }

    /// The path a command may use for `value`: unchanged when not confined; otherwise resolved
    /// against the roots and checked. `write` = the command writes there.
    pub fn resolve(&self, cmd: &str, value: &str, write: bool) -> Result<PathBuf> {
        let p = expand_home(value);
        if !self.confined() {
            return Ok(p);
        }
        let roots: Vec<&PathBuf> = if write { self.write_roots.iter().collect() } else { self.read_roots.iter().chain(&self.write_roots).collect() };
        let Some(first) = roots.first() else {
            return Err(StudioError::Policy(format!(
                "`{cmd}` would {} `{value}`, but this session has no {} folder",
                if write { "write" } else { "read" },
                if write { "writable" } else { "readable" }
            )));
        };
        let abs = if p.is_absolute() { p } else { first.join(p) };
        let real = normalize(&abs);
        for r in &roots {
            let root = normalize(r);
            if real.starts_with(&root) {
                return Ok(real);
            }
        }
        Err(StudioError::Policy(format!(
            "`{cmd}`: `{value}` is outside the folders this session may {} ({})",
            if write { "write" } else { "read" },
            roots.iter().map(|r| r.display().to_string()).collect::<Vec<_>>().join(", ")
        )))
    }

    /// Check (and rewrite to absolute) every path in an engine command's parameters. A
    /// file-writing command (`write`) must name where it writes: otherwise the engine would pick a
    /// place the roots were never checked against.
    pub fn confine_params(&self, cmd: &str, params: &mut Value, write: bool) -> Result<Confined> {
        let mut c = Confined::default();
        if self.confined() {
            walk(self, cmd, params, None, write, &mut c)?;
            if write && c.paths == 0 {
                return Err(StudioError::Policy(format!(
                    "`{cmd}` writes files but its parameters name no path, so where it writes cannot be checked; give `path` inside {}",
                    self.write_roots.iter().map(|r| r.display().to_string()).collect::<Vec<_>>().join(", ")
                )));
            }
        }
        Ok(c)
    }
}

fn walk(p: &Policy, cmd: &str, v: &mut Value, key: Option<&str>, write: bool, c: &mut Confined) -> Result<()> {
    match v {
        Value::Object(o) => {
            for (k, x) in o.iter_mut() {
                walk(p, cmd, x, Some(k.as_str()), write, c)?;
            }
        }
        Value::Array(a) => {
            for x in a.iter_mut() {
                walk(p, cmd, x, key, write, c)?;
            }
        }
        Value::String(s) => {
            if is_file_param(cmd, key, s) {
                c.paths += 1;
                let r = p.resolve(cmd, s, write)?;
                let r = r.to_string_lossy().into_owned();
                if r != *s {
                    c.notes.push(format!("`{}` resolved to {r}", key.unwrap_or("path")));
                    *s = r;
                }
            }
        }
        _ => {}
    }
    Ok(())
}

/// Whether a string parameter names a file or folder. Absolute paths always count. Under a path
/// key (`path`, `file`, …) a relative value counts when the command works with files, or when it
/// has a file extension, but never for property and shape commands, where `path` is a property or
/// a vector path (`transform/opacity`, `effects/#1/blurriness`).
fn is_file_param(cmd: &str, key: Option<&str>, value: &str) -> bool {
    if looks_absolute(value) {
        return true;
    }
    let is_path_key = key.is_some_and(|k| PATH_KEYS.iter().any(|pk| pk.eq_ignore_ascii_case(k)));
    if !is_path_key || value.is_empty() {
        return false;
    }
    let local = cmd.split_once(':').map_or(cmd, |(_, c)| c).to_ascii_lowercase();
    const PROPERTY: &[&str] = &["prop.", "keys.", "expression", "mask.", "shape.", "path.", "vector.", "layer.setmask", "text."];
    if PROPERTY.iter().any(|p| local.starts_with(p) || local.contains(&format!(".{p}"))) {
        return false;
    }
    const FILE_WORDS: &[&str] = &["file", "import", "export", "save", "open", "load", "relink", "replace", "proxy", "render", "script", "lut", "preset", "font", "folder", "media", "frame", "batch", "place", "link"];
    if FILE_WORDS.iter().any(|w| local.contains(w)) {
        return true;
    }
    has_extension(value)
}

fn has_extension(s: &str) -> bool {
    Path::new(s).extension().is_some_and(|e| (1..=5).contains(&e.len()) && e.to_string_lossy().chars().all(|c| c.is_ascii_alphanumeric()))
}

fn looks_absolute(s: &str) -> bool {
    s.starts_with('/') || s.starts_with("~/") || s.starts_with('\\') || (s.len() > 2 && s.as_bytes().get(1) == Some(&b':') && matches!(s.as_bytes().get(2), Some(b'\\' | b'/')))
}

fn expand_home(s: &str) -> PathBuf {
    if let Some(rest) = s.strip_prefix("~/")
        && let Some(home) = std::env::var_os("HOME")
    {
        return PathBuf::from(home).join(rest);
    }
    PathBuf::from(s)
}

/// The real path, with every symbolic link resolved and `.` / `..` applied in order: a `..`
/// after a link goes up from where the link points, as the file system would. Components that do
/// not exist yet are kept as written.
pub fn normalize(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::Prefix(_) | Component::RootDir => out.push(c.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                if let Ok(real) = std::fs::canonicalize(&out) {
                    out = real;
                }
                out.pop();
            }
            Component::Normal(n) => {
                out.push(n);
                if std::fs::symlink_metadata(&out).is_ok_and(|m| m.file_type().is_symlink())
                    && let Ok(real) = std::fs::canonicalize(&out)
                {
                    out = real;
                }
            }
        }
    }
    // the existing part, canonical (resolves links in the root itself, e.g. /tmp → /private/tmp)
    let mut existing = out.clone();
    let mut rest = Vec::new();
    while std::fs::symlink_metadata(&existing).is_err() {
        let Some(n) = existing.file_name().map(|n| n.to_os_string()) else { break };
        rest.push(n);
        if !existing.pop() {
            break;
        }
    }
    let mut real = std::fs::canonicalize(&existing).unwrap_or(existing);
    for n in rest.iter().rev() {
        real.push(n);
    }
    real
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("videonow-policy-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn unconfined_passes_paths_through() {
        let p = Policy::default();
        assert_eq!(p.resolve("x", "a/b.png", true).unwrap(), PathBuf::from("a/b.png"));
    }

    #[test]
    fn confined_resolves_relative_and_refuses_escapes() {
        let root = tmp("roots");
        let p = Policy { write_roots: vec![root.clone()], ..Default::default() };
        let ok = p.resolve("x", "out/frame.png", true).unwrap();
        assert!(ok.starts_with(normalize(&root)), "{ok:?}");
        assert!(p.resolve("x", "../escape.png", true).is_err());
        assert!(p.resolve("x", "/etc/passwd", false).is_err());
        assert!(p.resolve("x", "out/../../escape.png", true).is_err());
    }

    #[test]
    fn symlinks_cannot_escape() {
        let root = tmp("links");
        let outside = tmp("outside");
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&outside, root.join("link")).unwrap();
            let p = Policy { write_roots: vec![root.clone()], ..Default::default() };
            assert!(p.resolve("x", "link/file.png", true).is_err());
        }
    }

    #[test]
    fn reads_allowed_under_write_roots_but_not_writes_under_read_roots() {
        let r = tmp("read");
        let w = tmp("write");
        let p = Policy { read_roots: vec![r.clone()], write_roots: vec![w.clone()], ..Default::default() };
        assert!(p.resolve("x", &w.join("a").to_string_lossy(), false).is_ok());
        assert!(p.resolve("x", &r.join("a").to_string_lossy(), true).is_err());
    }

    #[test]
    fn effect_classes() {
        let p = Policy { allow: Policy::parse_allow("edit").unwrap(), ..Default::default() };
        assert!(p.check_effect("a", Effect::Read).is_ok(), "read is always allowed");
        assert!(p.check_effect("a", Effect::Edit).is_ok());
        assert!(p.check_effect("a", Effect::Destructive).is_err());
        assert!(p.check_effect("a", Effect::FileWrite).is_err());
        assert!(Policy::parse_allow("read,nope").is_err());
    }

    #[test]
    fn property_paths_are_not_files() {
        let root = tmp("props");
        let p = Policy { write_roots: vec![root], ..Default::default() };
        let mut v = serde_json::json!({"layer": 2, "path": "transform/opacity", "time": 0});
        assert!(p.confine_params("motion:prop.addKey", &mut v, false).unwrap().notes.is_empty());
        assert_eq!(v["path"], "transform/opacity");
        let mut v = serde_json::json!({"path": "clip.mp4"});
        assert_eq!(p.confine_params("timeline:file.import", &mut v, false).unwrap().notes.len(), 1);
    }

    #[test]
    fn params_are_walked_and_rewritten() {
        let root = tmp("params");
        let p = Policy { write_roots: vec![root.clone()], ..Default::default() };
        let mut v = serde_json::json!({"path": "a.mp4", "nested": {"files": ["b.png"]}, "name": "/not/a/key/but/absolute"});
        assert!(p.confine_params("x", &mut v, true).is_err(), "an absolute path outside the roots anywhere is refused");
        let mut v = serde_json::json!({"path": "a.mp4", "nested": {"files": ["b.png"]}, "name": "Title"});
        let c = p.confine_params("x", &mut v, true).unwrap();
        assert_eq!((c.notes.len(), c.paths), (2, 2));
        assert!(v["path"].as_str().unwrap().starts_with(&*normalize(&root).to_string_lossy()));
        assert_eq!(v["name"], "Title");
    }

    #[test]
    fn confined_writes_must_name_a_path() {
        let root = tmp("unnamed");
        let p = Policy { write_roots: vec![root], ..Default::default() };
        let e = p.confine_params("timeline:export.quick", &mut serde_json::json!({"preset": "H.264"}), true).unwrap_err().to_string();
        assert!(e.contains("name no path"), "{e}");
        assert!(p.check_confinable("motion:help.website", Effect::External).is_err());
        assert!(p.check_confinable("timeline:export.queue.start", Effect::FileWrite).is_err());
        assert!(p.check_confinable("image:layer.layerMask.revealAll", Effect::Edit).is_ok(), "not the file manager");
    }
}
