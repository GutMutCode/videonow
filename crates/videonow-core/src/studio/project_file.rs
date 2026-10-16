//! `.videonow` project files: one directory holding each engine's document in that engine's own
//! format, tied together by `project.json`.
//!
//! ```text
//! Trailer.videonow/
//!   project.json          format, schema, name, the asset table, which file holds what
//!   timeline.fcproj       the timeline (FilmCraft project, with its view)
//!   motion.ecproj         every composition (EffectCraft project, with its view)
//!   images/<asset>.pcraft each image document (PhotoCraft bundle)
//! ```
//!
//! Every file is written atomically (temporary file, then rename). `project.json` is written last,
//! so a save that stops half-way leaves the previous `project.json` pointing at complete files.
//! After writing, every file is read back and decoded: the receipt says so (`checked`).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::assets::{Asset, AssetId, AssetKind};
use crate::{Result, StudioError};

use super::Studio;

pub const FORMAT: &str = "videonow.project";
pub const SCHEMA: u32 = 1;
pub const MANIFEST: &str = "project.json";
pub const TIMELINE_FILE: &str = "timeline.fcproj";
pub const MOTION_FILE: &str = "motion.ecproj";
pub const IMAGES_DIR: &str = "images";
/// Largest `project.json` read (the asset table is small; anything bigger is not ours).
const MAX_MANIFEST_BYTES: u64 = 16 << 20;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    pub format: String,
    pub schema: u32,
    #[serde(default)]
    pub generator: String,
    pub name: String,
    pub next_asset: u64,
    pub assets: Vec<Asset>,
}

fn io(path: &Path, e: impl std::fmt::Display) -> StudioError {
    StudioError::Other(format!("{}: {e}", path.display()))
}

fn image_file(id: AssetId) -> String {
    format!("{IMAGES_DIR}/{}.pcraft", id.0)
}

/// What a save wrote, for the result and the receipt.
pub struct Saved {
    pub path: PathBuf,
    pub files: Vec<(String, usize)>,
    pub removed: Vec<String>,
}

impl Studio {
    /// Write the project to the `.videonow` directory `dir` and verify every file by reading it
    /// back. Refuses a path that is a file, or a non-empty directory that is not a project.
    pub(crate) fn save_project(&mut self, dir: &Path) -> Result<Saved> {
        if dir.is_file() {
            return Err(StudioError::Other(format!("{} is a file; a project is a directory (name it like `Film.videonow`)", dir.display())));
        }
        if dir.is_dir() && !dir.join(MANIFEST).is_file() && std::fs::read_dir(dir).map_err(|e| io(dir, e))?.next().is_some() {
            return Err(StudioError::Other(format!("{} is a folder that is not a VideoNow project; nothing was written", dir.display())));
        }
        std::fs::create_dir_all(dir.join(IMAGES_DIR)).map_err(|e| io(dir, e))?;
        let mut files = Vec::new();
        let write = |rel: &str, bytes: &[u8], files: &mut Vec<(String, usize)>| -> Result<()> {
            let p = dir.join(rel);
            filmcraft_format::atomic_write(&p, bytes).map_err(|e| io(&p, e))?;
            files.push((rel.to_string(), bytes.len()));
            Ok(())
        };
        // the timeline, with what is open in it
        let view = self.timeline.project_view();
        let timeline = filmcraft_format::encode_with_view(&self.timeline.project, Some(&view), false);
        write(TIMELINE_FILE, &timeline, &mut files)?;
        // every composition
        let motion = self.motion.project.to_file_json_with_view(&self.motion.saved_view()).map_err(|e| StudioError::Motion(e.to_string()))?;
        write(MOTION_FILE, motion.as_bytes(), &mut files)?;
        // each image document
        let mut keep = Vec::new();
        for id in self.image_docs.keys() {
            let doc = self.image_doc(*id).map(|d| d.doc.clone()).ok_or(StudioError::NoAsset(id.0))?;
            let bytes = photocraft_format::save_to_bytes(&doc, &photocraft_format::SaveOptions { thumbnail: None, composite: None })
                .map_err(|e| StudioError::Image(e.to_string()))?;
            let rel = image_file(*id);
            write(&rel, &bytes, &mut files)?;
            keep.push(format!("{}.pcraft", id.0));
        }
        // the table that ties them together, last
        let manifest = Manifest {
            format: FORMAT.into(),
            schema: SCHEMA,
            generator: format!("VideoNow {}", env!("CARGO_PKG_VERSION")),
            name: self.name.clone(),
            next_asset: self.next_asset,
            assets: self.assets.values().cloned().collect(),
        };
        let text = serde_json::to_vec_pretty(&manifest).map_err(|e| StudioError::Other(e.to_string()))?;
        write(MANIFEST, &text, &mut files)?;
        // images of assets this project no longer has
        let mut removed = Vec::new();
        if let Ok(rd) = std::fs::read_dir(dir.join(IMAGES_DIR)) {
            for e in rd.flatten() {
                let n = e.file_name().to_string_lossy().into_owned();
                if n.ends_with(".pcraft") && !keep.contains(&n) && std::fs::remove_file(e.path()).is_ok() {
                    removed.push(format!("{IMAGES_DIR}/{n}"));
                }
            }
        }
        // read every file back: a save that cannot be opened is not a save
        verify(dir)?;
        self.path = Some(dir.to_path_buf());
        self.saved_revision = self.revision;
        Ok(Saved { path: dir.to_path_buf(), files, removed })
    }

    /// Replace this studio's project with the one in `dir`. Builds the new project completely
    /// before touching the current one: a project that fails to open leaves everything as it was.
    pub(crate) fn open_project(&mut self, dir: &Path) -> Result<Value> {
        let m = read_manifest(dir)?;
        let mut next = Studio::with_policy(self.policy.clone());
        next.name = m.name.clone();
        next.next_asset = m.next_asset.max(m.assets.iter().map(|a| a.id.0 + 1).max().unwrap_or(1));
        // images first, then the asset table: the timeline resolves its nested items through them
        for a in &m.assets {
            if a.kind == AssetKind::Image {
                let p = dir.join(image_file(a.id));
                let doc = photocraft_format::load_path(&p).map_err(|e| io(&p, e))?;
                let i = next.image.add_document(doc, None);
                let id = next.image.documents().get(i).map(|d| d.doc.id).ok_or_else(|| io(&p, "the image did not open"))?;
                next.image_docs.insert(a.id, id);
            }
            next.assets.insert(a.id, a.clone());
        }
        let motion = dir.join(MOTION_FILE);
        next.motion
            .execute("file.open", json!({"path": motion.to_string_lossy()}))
            .map_err(|e| StudioError::Motion(format!("{}: {e}", motion.display())))?;
        for a in m.assets.iter().filter(|a| a.kind == AssetKind::Composition) {
            if a.comp.is_none_or(|c| next.motion.project.comp(effectcraft_project::ItemId(c)).is_none()) {
                return Err(StudioError::Other(format!("{}: asset {} names a composition the motion file does not have", dir.display(), a.id.0)));
            }
        }
        next.publish();
        let timeline = dir.join(TIMELINE_FILE);
        let opened = next
            .timeline
            .execute("file.open", json!({"path": timeline.to_string_lossy()}))
            .map_err(|e| StudioError::Timeline(format!("{}: {e}", timeline.display())))?;
        // the timeline's file belongs to the project folder, not to a standalone FilmCraft project
        next.timeline.path = None;
        next.path = Some(dir.to_path_buf());
        // the opened project counts as a change of everything; history starts here
        next.revision = self.revision + 1;
        next.saved_revision = next.revision;
        next.publish();
        next.epoch = self.epoch + 1;
        *self = next;
        Ok(json!({
            "path": dir.to_string_lossy(),
            "name": m.name,
            "assets": m.assets.len(),
            "missingMedia": opened.get("missingMedia").cloned().unwrap_or(Value::Null),
        }))
    }
}

pub fn read_manifest(dir: &Path) -> Result<Manifest> {
    let p = dir.join(MANIFEST);
    let len = std::fs::metadata(&p).map_err(|e| io(&p, format!("{e} (is this a .videonow project folder?)")))?.len();
    if len > MAX_MANIFEST_BYTES {
        return Err(io(&p, format!("{len} bytes is too large for a project manifest")));
    }
    let bytes = std::fs::read(&p).map_err(|e| io(&p, e))?;
    let m: Manifest = serde_json::from_slice(&bytes).map_err(|e| io(&p, e))?;
    if m.format != FORMAT {
        return Err(io(&p, format!("not a VideoNow project (format `{}`)", m.format)));
    }
    if m.schema > SCHEMA {
        return Err(io(&p, format!("written by a newer VideoNow (schema {}, this build reads up to {SCHEMA})", m.schema)));
    }
    let mut seen = std::collections::BTreeSet::new();
    for a in &m.assets {
        if !seen.insert(a.id) {
            return Err(io(&p, format!("asset {} is listed twice", a.id.0)));
        }
    }
    Ok(m)
}

/// Read every file of a saved project back and decode it.
pub fn verify(dir: &Path) -> Result<()> {
    let m = read_manifest(dir)?;
    let t = dir.join(TIMELINE_FILE);
    let bytes = std::fs::read(&t).map_err(|e| io(&t, e))?;
    filmcraft_format::decode(&bytes).map_err(|e| io(&t, e))?;
    let mo = dir.join(MOTION_FILE);
    let text = std::fs::read_to_string(&mo).map_err(|e| io(&mo, e))?;
    effectcraft_project::Project::from_json(&text).map_err(|e| io(&mo, e))?;
    for a in m.assets.iter().filter(|a| a.kind == AssetKind::Image) {
        let p = dir.join(image_file(a.id));
        let bytes = std::fs::read(&p).map_err(|e| io(&p, e))?;
        photocraft_format::read_manifest(&bytes).map_err(|e| io(&p, e))?;
    }
    Ok(())
}
