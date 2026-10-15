//! Saving and opening a project: a `.videonow` directory holding each engine's document in its
//! own format, tied together by `manifest.json`.
//!
//! ```text
//! my-film.videonow/
//!   manifest.json        format, schema, assets (ids in each engine), which file holds what
//!   timeline.fcproj      the timeline engine's project (FilmCraft JSON)
//!   motion.ecproj        every composition (EffectCraft JSON)
//!   images/<asset>.pcraft  each image document (PhotoCraft native bundle)
//! ```
//!
//! The engines keep their own schema versions and migrations; the manifest adds only what ties
//! them together. Engine files are written first and the manifest last, atomically, so a save
//! that fails part-way never leaves a manifest pointing at files that were not written.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::assets::{Asset, AssetId, AssetKind};
use crate::studio::Studio;
use crate::{Result, StudioError};

pub const FORMAT: &str = "videonow";
/// Bumped when the manifest changes shape; readers refuse a newer schema than they know.
pub const SCHEMA: u32 = 1;
pub const MANIFEST: &str = "manifest.json";
pub const TIMELINE_FILE: &str = "timeline.fcproj";
pub const MOTION_FILE: &str = "motion.ecproj";
pub const IMAGES_DIR: &str = "images";

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    pub format: String,
    pub schema: u32,
    pub saved_by: String,
    pub next_asset: u64,
    pub assets: Vec<Asset>,
    pub timeline: String,
    pub motion: String,
    /// Asset id → image file, relative to the project directory.
    pub images: std::collections::BTreeMap<u64, String>,
}

fn io(path: &Path, e: impl std::fmt::Display) -> StudioError {
    StudioError::Other(format!("{}: {e}", path.display()))
}

/// The project directory for `path`: `.videonow` is added when it has no extension.
pub fn project_dir(path: &str) -> PathBuf {
    let p = PathBuf::from(path);
    if p.extension().is_none() { p.with_extension("videonow") } else { p }
}

impl Studio {
    /// Write the project to `dir` (created if needed). An existing directory must be empty or
    /// already hold a VideoNow project.
    pub fn save_project(&mut self, dir: &Path) -> Result<Value> {
        if dir.exists() {
            let is_project = dir.join(MANIFEST).is_file();
            let empty = std::fs::read_dir(dir).map_err(|e| io(dir, e))?.next().is_none();
            if !is_project && !empty {
                return Err(StudioError::Other(format!("{}: the folder is not empty and is not a VideoNow project; choose another path", dir.display())));
            }
        }
        std::fs::create_dir_all(dir.join(IMAGES_DIR)).map_err(|e| io(dir, e))?;
        let s = |p: &Path| p.to_string_lossy().into_owned();

        let timeline = dir.join(TIMELINE_FILE);
        self.timeline.execute("file.saveAs", json!({"path": s(&timeline)})).map_err(|e| StudioError::Timeline(e.to_string()))?;
        let motion = dir.join(MOTION_FILE);
        self.motion.execute("file.saveAs", json!({"path": s(&motion)})).map_err(|e| StudioError::Motion(e.to_string()))?;

        let mut images = std::collections::BTreeMap::new();
        for (id, session) in &self.images {
            let doc = session.active().map(|d| d.doc.clone()).ok_or(StudioError::NoAsset(id.0))?;
            let rel = format!("{IMAGES_DIR}/{}.pcraft", id.0);
            let bytes = photocraft_format::save_to_bytes(&doc, &photocraft_format::SaveOptions::default()).map_err(|e| StudioError::Image(e.to_string()))?;
            let path = dir.join(&rel);
            photocraft_format::atomic_write(&path, &bytes).map_err(|e| io(&path, e))?;
            images.insert(id.0, rel);
        }
        // image files of assets that no longer exist are left out of the manifest; remove them
        if let Ok(rd) = std::fs::read_dir(dir.join(IMAGES_DIR)) {
            for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                let keep = images.values().any(|r| r.ends_with(&format!("/{name}")));
                if !keep && name.ends_with(".pcraft") {
                    let _ = std::fs::remove_file(e.path());
                }
            }
        }

        let manifest = Manifest {
            format: FORMAT.into(),
            schema: SCHEMA,
            saved_by: format!("videonow {}", env!("CARGO_PKG_VERSION")),
            next_asset: self.next_asset,
            assets: self.assets.values().cloned().collect(),
            timeline: TIMELINE_FILE.into(),
            motion: MOTION_FILE.into(),
            images,
        };
        let text = serde_json::to_vec_pretty(&manifest).map_err(|e| StudioError::Other(e.to_string()))?;
        let mpath = dir.join(MANIFEST);
        photocraft_format::atomic_write(&mpath, &text).map_err(|e| io(&mpath, e))?;
        self.project_path = Some(dir.to_path_buf());
        Ok(json!({"path": s(dir), "assets": manifest.assets.len(), "images": manifest.images.len()}))
    }

    /// Replace the studio's project with the one in `dir`. The studio revision keeps counting;
    /// undo history does not survive (the engines start fresh histories).
    pub fn open_project(&mut self, dir: &Path) -> Result<Value> {
        let mpath = dir.join(MANIFEST);
        let text = std::fs::read(&mpath).map_err(|e| io(&mpath, e))?;
        let m: Manifest = serde_json::from_slice(&text).map_err(|e| io(&mpath, format!("not a VideoNow manifest: {e}")))?;
        if m.format != FORMAT {
            return Err(io(&mpath, format!("format `{}` is not `{FORMAT}`", m.format)));
        }
        if m.schema > SCHEMA {
            return Err(io(&mpath, format!("written by a newer VideoNow (schema {} > {SCHEMA}); update to open it", m.schema)));
        }
        let inside = |rel: &str| -> Result<PathBuf> {
            // project files stay inside the project directory
            let p = Path::new(rel);
            if p.is_absolute() || p.components().any(|c| matches!(c, std::path::Component::ParentDir)) {
                return Err(io(&mpath, format!("`{rel}` points outside the project")));
            }
            Ok(dir.join(p))
        };
        let s = |p: &Path| p.to_string_lossy().into_owned();

        // build the new studio fully before replacing this one: a failed open changes nothing
        let mut next = Studio::new();
        next.revision = self.revision;
        next.next_asset = m.next_asset;
        for a in &m.assets {
            next.assets.insert(a.id, a.clone());
        }
        let motion = inside(&m.motion)?;
        next.motion.execute("file.open", json!({"path": s(&motion)})).map_err(|e| StudioError::Motion(e.to_string()))?;
        for (id, rel) in &m.images {
            let path = inside(rel)?;
            let doc = photocraft_format::load_path(&path).map_err(|e| io(&path, e))?;
            let mut session = photocraft_engine::Session::new();
            session.add_document(doc, None);
            next.images.insert(AssetId(*id), session);
        }
        for a in next.assets.values() {
            if a.kind == AssetKind::Image && !next.images.contains_key(&a.id) {
                return Err(io(&mpath, format!("image asset {} has no file", a.id.0)));
            }
        }
        // the nested sources must know every asset before the timeline asks for them
        next.publish();
        let timeline = inside(&m.timeline)?;
        next.timeline.execute("file.open", json!({"path": s(&timeline)})).map_err(|e| StudioError::Timeline(e.to_string()))?;
        next.project_path = Some(dir.to_path_buf());
        next.journal = std::mem::take(&mut self.journal);
        let summary = json!({"path": s(dir), "assets": next.assets.len(), "images": next.images.len()});
        *self = next;
        Ok(summary)
    }
}
