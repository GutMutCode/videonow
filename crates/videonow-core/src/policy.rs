//! The delegation policy: what a client of this studio may do, enforced at the single entry point
//! before any engine runs (gap analysis P1/P9; philosophy §6.6, "the program enforces the
//! boundary of delegation").
//!
//! Two limits:
//!
//! * **Effect classes.** A policy can refuse whole classes, such as `destructive` or `fileWrite`,
//!   so an agent asked to "tidy the titles" cannot delete clips or overwrite files.
//! * **File roots.** File paths in parameters (keys such as `path`, `paths`, `destination`, and
//!   any absolute path anywhere in the parameters) must lie inside the read roots, or inside the
//!   write roots for commands that write files. Symbolic links are resolved; `..` cannot climb out.
//!
//! The CLI runs unrestricted (the person at the terminal is the one acting). The MCP server
//! confines files to its working directory unless told otherwise.

use std::path::{Component, Path, PathBuf};

use serde_json::Value;

use crate::manifest::{Effect, is_property_path_parameter};

#[derive(Clone, Debug, Default)]
pub struct Policy {
    /// Effect classes refused outright.
    pub deny: Vec<Effect>,
    /// Folders files may be read from (write roots may be read too).
    pub read_roots: Vec<PathBuf>,
    /// Folders files may be written to.
    pub write_roots: Vec<PathBuf>,
    /// No file confinement at all.
    pub unrestricted_files: bool,
}

impl Policy {
    /// Everything allowed (the CLI's and the tests' default).
    pub fn unrestricted() -> Self {
        Self { unrestricted_files: true, ..Self::default() }
    }

    /// Files confined to `roots` for both reading and writing.
    pub fn confined_to(roots: &[PathBuf]) -> Self {
        Self { read_roots: roots.to_vec(), write_roots: roots.to_vec(), ..Self::default() }
    }

    /// `Err` with the reason when `effect` or a path in `params` is outside this policy.
    pub fn check(&self, cmd: &str, effect: Effect, params: &Value) -> Result<(), String> {
        if self.deny.contains(&effect) {
            return Err(format!("{cmd} is a {} command, which this session is not allowed to run", effect_word(effect)));
        }
        if self.unrestricted_files {
            return Ok(());
        }
        let mut paths = Vec::new();
        collect_paths(cmd, None, params, &mut paths, &mut Vec::new());
        for p in paths {
            let write = effect == Effect::FileWrite;
            let roots: Vec<&PathBuf> = if write { self.write_roots.iter().collect() } else { self.read_roots.iter().chain(&self.write_roots).collect() };
            if roots.is_empty() {
                return Err(format!("{cmd}: `{p}` — this session has no {} folder", if write { "writable" } else { "readable" }));
            }
            let resolved = resolve(Path::new(&p)).ok_or_else(|| format!("{cmd}: `{p}` is not a usable path"))?;
            if !roots.iter().any(|r| resolve(r).is_some_and(|r| resolved.starts_with(&r))) {
                let list: Vec<String> = roots.iter().map(|r| r.display().to_string()).collect();
                return Err(format!("{cmd}: `{p}` is outside the folders this session may {} ({})", if write { "write" } else { "read" }, list.join(", ")));
            }
        }
        Ok(())
    }
}

fn effect_word(e: Effect) -> &'static str {
    match e {
        Effect::Read => "read",
        Effect::Edit => "edit",
        Effect::Destructive => "destructive",
        Effect::FileWrite => "file-writing",
        Effect::External => "external (network, other applications)",
    }
}

/// Parameter keys that hold file paths.
fn is_path_key(k: &str) -> bool {
    let l = k.to_ascii_lowercase();
    matches!(l.as_str(), "path" | "paths" | "file" | "files" | "dir" | "directory" | "destination" | "dest" | "output" | "folder" | "src" | "source")
        || l.ends_with("path")
        || l.ends_with("paths")
        || l.ends_with("dir")
        || l.ends_with("folder")
        || l.ends_with("file")
}

fn looks_absolute(s: &str) -> bool {
    s.starts_with('/') || s.starts_with('\\') || s.as_bytes().get(1..3) == Some(b":\\") || s.as_bytes().get(1..3) == Some(b":/") || s.starts_with('~')
}

fn collect_paths<'value>(command: &str, key: Option<&str>, value: &'value Value, out: &mut Vec<String>, location: &mut Vec<&'value str>) {
    if location.len() > 16 {
        return;
    }
    match value {
        Value::String(text) => {
            let keyed = key.is_some_and(is_path_key);
            // studio:// references name assets of this project, not files
            if !text.starts_with(crate::assets::SCHEME) && !text.is_empty() && !is_property_path_parameter(command, location) && (keyed || looks_absolute(text))
            {
                out.push(text.clone());
            }
        }
        Value::Array(items) => {
            location.push("*");
            for item in items {
                collect_paths(command, key, item, out, location);
            }
            location.pop();
        }
        Value::Object(fields) => {
            for (field, item) in fields {
                location.push(field);
                collect_paths(command, Some(field), item, out, location);
                location.pop();
            }
        }
        _ => {}
    }
}

/// An absolute, symlink-free form of `p`, also for a path that does not exist yet: the longest
/// existing ancestor is canonicalised and the rest appended. `None` if `..` remains after that.
fn resolve(p: &Path) -> Option<PathBuf> {
    let p = if p.is_absolute() { p.to_path_buf() } else { std::env::current_dir().ok()?.join(p) };
    let mut existing = p.clone();
    let mut rest: Vec<std::ffi::OsString> = Vec::new();
    loop {
        if let Ok(c) = existing.canonicalize() {
            let mut out = c;
            for part in rest.iter().rev() {
                let comp = Path::new(part).components().next();
                match comp {
                    Some(Component::Normal(_)) => out.push(part),
                    Some(Component::CurDir) => {}
                    _ => return None,
                }
            }
            return Some(out);
        }
        let name = existing.file_name()?.to_os_string();
        rest.push(name);
        if !existing.pop() {
            return None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn denied_classes_are_refused() {
        let p = Policy { deny: vec![Effect::Destructive], ..Policy::unrestricted() };
        assert!(p.check("timeline:timeline.rippleDelete", Effect::Destructive, &json!({})).is_err());
        assert!(p.check("timeline:timeline.place", Effect::Edit, &json!({})).is_ok());
    }

    #[test]
    fn paths_stay_inside_roots() {
        let root = std::env::temp_dir().join(format!("vn-policy-{}", std::process::id()));
        std::fs::create_dir_all(root.join("in")).unwrap();
        let p = Policy::confined_to(std::slice::from_ref(&root));
        let inside = root.join("in/new-file.png").to_string_lossy().into_owned();
        assert!(p.check("x", Effect::FileWrite, &json!({"path": inside})).is_ok());
        let climb = root.join("in/../../escape.png").to_string_lossy().into_owned();
        assert!(p.check("x", Effect::FileWrite, &json!({"path": climb})).is_err());
        assert!(p.check("x", Effect::Read, &json!({"paths": ["/etc/passwd"]})).is_err());
        // any absolute path, whatever its key
        assert!(p.check("x", Effect::Edit, &json!({"lut": "/etc/hosts"})).is_err());
        // asset references are not files
        assert!(p.check("x", Effect::Edit, &json!({"path": "studio://comp/1"})).is_ok());
        let none = Policy::default();
        assert!(none.check("x", Effect::FileWrite, &json!({"path": inside})).is_err());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn property_path_exceptions_are_command_and_location_specific() {
        let policy = Policy::default();
        for (command, params) in [
            ("motion:prop.set", json!({"path": "transform/opacity", "value": 40})),
            ("motion:prop.get", json!({"path": "transform/opacity"})),
            ("motion:keys.select", json!({"keys": [{"layer": 1, "path": "transform/opacity", "time": 0}]})),
            ("motion:prop.pickWhip", json!({"path": "transform/opacity", "target": {"layer": 2, "path": "transform/opacity"}})),
            ("motion:anim.applyPreset", json!({"preset": {"properties": [{"path": "transform/opacity", "value": 40}]}})),
        ] {
            assert!(policy.check(command, Effect::Edit, &params).is_ok(), "{command}: {params}");
        }
        for (command, params) in [
            ("motion:keys.rpfCameraImport", json!({"path": "camera.json"})),
            ("motion:essential.templateInfo", json!({"path": "template.ectemplate"})),
            ("motion:anim.applyPreset", json!({"path": "preset.json"})),
            ("motion:prop.set", json!({"path": "transform/opacity", "value": {"path": "private.json"}})),
            ("motion:keys.select", json!({"keys": [{"path": "transform/opacity", "file": "private.json"}]})),
            ("motion:prop.set", json!({"path": "transform/opacity", "value": "/private/file"})),
            ("motion:prop.unknown", json!({"path": "private.json"})),
        ] {
            assert!(policy.check(command, Effect::Edit, &params).is_err(), "{command}: {params}");
        }
    }
}
