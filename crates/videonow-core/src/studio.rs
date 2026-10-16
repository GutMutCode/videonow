//! The studio: one session of each engine, the asset table, and the single command entry point.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use serde_json::{Value, json};

use effectcraft_project::ItemId as MotionItemId;
use effectcraft_render::FootageSource;
use filmcraft_media::SharedSource;

use crate::assets::{Asset, AssetId, AssetKind, parse_uri};
use crate::bridge::{CompMediaSource, Generation, ImageMediaSource, ImageSnapshot, MotionSnapshot, SharedImages, SharedMotion, StudioFootage};
use crate::manifest::{self, Effect};
use crate::policy::Policy;
use crate::receipt::{ENGINE_CHECKED, NOT_CHECKED_VISUAL, Receipt, Revisions, STRICT_CHECKED, STUDIO_CHECKED};
use crate::{Result, StudioError};

mod commands;
pub mod project_file;
pub mod ui;

pub struct Studio {
    /// FilmCraft: sequences, editing, audio, captions, export.
    pub timeline: filmcraft_engine::Session,
    /// EffectCraft: every composition of the project.
    pub motion: effectcraft_engine::Session,
    /// PhotoCraft: every image document of the project (one document per image asset).
    pub image: photocraft_engine::Session,
    /// Which document of [`Studio::image`] each image asset is.
    image_docs: BTreeMap<AssetId, photocraft_doc::DocId>,
    assets: BTreeMap<AssetId, Asset>,
    next_asset: u64,
    /// The project's name (the `.videonow` folder's, once saved).
    name: String,
    /// Where the project was last saved or opened from.
    path: Option<PathBuf>,
    saved_revision: u64,
    /// What this session may do (see [`crate::policy`]).
    policy: Policy,
    shared_motion: SharedMotion,
    shared_images: SharedImages,
    /// The asset table as the timeline's nested-source resolver sees it.
    registry: Arc<RwLock<HashMap<AssetId, Asset>>>,
    footage: Arc<dyn FootageSource>,
    /// The latest snapshot number, shared with the sources (see [`Generation`]).
    generation: Generation,
    revision: u64,
    journal: Vec<Value>,
    /// What each change touched, newest last (for `studio:undo`).
    undo: Vec<Vec<String>>,
    /// Where the engines stood after the last command (see [`Studio::absorb_ui`]).
    settled: Option<(EngineMarks, ui::JournalMarks)>,
    /// How many times the whole project was replaced (opened or started anew): a UI holding on
    /// to the engine sessions' internals rebuilds when it moves.
    epoch: u64,
}

impl Default for Studio {
    fn default() -> Self {
        Self::new()
    }
}

/// Where each engine's document stands (its revision and its snapshot pointer).
#[derive(Clone, PartialEq)]
struct EngineMarks {
    timeline: (u64, usize),
    motion: (u64, usize),
    images: BTreeMap<AssetId, (u64, usize)>,
    assets: Vec<Asset>,
}

fn read<T: Clone>(l: &RwLock<T>) -> T {
    l.read().unwrap_or_else(|e| e.into_inner()).clone()
}

pub(crate) fn bad(cmd: &str, msg: impl Into<String>) -> StudioError {
    StudioError::BadParams { cmd: cmd.to_string(), msg: msg.into() }
}

pub(crate) fn u64_of(v: &Value) -> Option<u64> {
    v.as_u64().or_else(|| v.as_str().and_then(|s| s.parse().ok()))
}

impl Studio {
    /// An empty project with no delegation limits (a local user's own session).
    pub fn new() -> Self {
        Self::with_policy(Policy::default())
    }

    /// An empty project whose commands are limited by `policy`.
    pub fn with_policy(policy: Policy) -> Self {
        let mut motion = effectcraft_host::session();
        let shared_images: SharedImages = Arc::default();
        let footage: Arc<dyn FootageSource> = Arc::new(StudioFootage::new(motion.footage.clone(), shared_images.clone()));
        motion.footage = footage.clone();
        let shared_motion: SharedMotion = Arc::new(RwLock::new(MotionSnapshot { project: motion.project.clone(), revision: 0 }));
        let timeline = filmcraft_engine::Session::default();
        let registry: Arc<RwLock<HashMap<AssetId, Asset>>> = Arc::default();
        let generation: Generation = Arc::default();
        {
            let (reg, m, im, f, g) = (registry.clone(), shared_motion.clone(), shared_images.clone(), footage.clone(), generation.clone());
            timeline.media.set_nested_resolver(Arc::new(move |uri: &str| -> Option<SharedSource> {
                let (kind, id) = parse_uri(uri)?;
                let a = reg.read().unwrap_or_else(|e| e.into_inner()).get(&id).cloned()?;
                match kind {
                    AssetKind::Composition => Some(Arc::new(CompMediaSource::new(&a.name, MotionItemId(a.comp?), m.clone(), g.clone(), f.clone())?)),
                    AssetKind::Image => Some(Arc::new(ImageMediaSource::new(&a.name, id, im.clone())?)),
                }
            }));
        }
        Self {
            timeline,
            motion,
            image: photocraft_engine::Session::new(),
            image_docs: BTreeMap::new(),
            assets: BTreeMap::new(),
            next_asset: 1,
            name: "Untitled".into(),
            path: None,
            saved_revision: 0,
            policy,
            shared_motion,
            shared_images,
            registry,
            footage,
            generation,
            revision: 0,
            journal: Vec::new(),
            undo: Vec::new(),
            settled: None,
            epoch: 0,
        }
    }

    /// Moves each time the project is replaced (`studio:project.open`, `studio:project.new`).
    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn is_dirty(&self) -> bool {
        self.revision != self.saved_revision
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn path(&self) -> Option<&std::path::Path> {
        self.path.as_deref()
    }

    pub fn policy(&self) -> &Policy {
        &self.policy
    }

    pub fn assets(&self) -> impl Iterator<Item = &Asset> {
        self.assets.values()
    }

    pub fn asset(&self, id: AssetId) -> Option<&Asset> {
        self.assets.get(&id)
    }

    /// The image engine's document of an image asset.
    pub fn image_doc(&self, id: AssetId) -> Option<&photocraft_engine::DocState> {
        let doc = self.image_docs.get(&id)?;
        self.image.documents().iter().find(|d| d.doc.id == *doc)
    }

    /// The image asset whose document is `doc` (the image engine's active one, say).
    pub fn image_asset_of(&self, doc: photocraft_doc::DocId) -> Option<AssetId> {
        self.image_docs.iter().find(|(_, d)| **d == doc).map(|(a, _)| *a)
    }

    /// Make the image asset's document the image engine's active one.
    pub(crate) fn activate_image(&mut self, id: AssetId) -> Result<()> {
        let doc = *self.image_docs.get(&id).ok_or(StudioError::NoAsset(id.0))?;
        let i = self.image.documents().iter().position(|d| d.doc.id == doc).ok_or_else(|| StudioError::Image(format!("image asset {} has no open document", id.0)))?;
        self.image.set_active(i);
        Ok(())
    }

    /// Register the image engine's active document as a new image asset.
    pub(crate) fn adopt_active_image(&mut self, name: &str) -> Result<AssetId> {
        let doc = self.image.active().map(|d| d.doc.id).ok_or_else(|| StudioError::Image("no image document was created".into()))?;
        let id = self.add_asset(AssetKind::Image, name, None);
        self.image_docs.insert(id, doc);
        Ok(id)
    }

    /// The journal: every command that changed the project, with its receipt.
    pub fn journal(&self) -> &[Value] {
        &self.journal
    }

    /// Run any command: `timeline:<id>`, `motion:<id>`, `image:<id>` or `studio:<id>`.
    /// Returns `{"result": <the command's own result>, "receipt": <Receipt>}`.
    ///
    /// `motion:` commands take an optional `"asset"` (a composition asset) and run on that
    /// composition; `image:` commands need `"asset"` (an image asset).
    ///
    /// Before anything runs: the command must exist and not be reserved, the session's policy
    /// must allow its effect class (and, when file access is confined, its paths), and its
    /// parameters must be ones it knows. A refusal changes nothing.
    pub fn execute(&mut self, id: &str, params: Value) -> Result<Value> {
        let params = if params.is_null() { json!({}) } else { params };
        if !params.is_object() {
            return Err(bad(id, "params must be a JSON object"));
        }
        let (domain, cmd) = id.split_once(':').ok_or_else(|| StudioError::UnknownCommand(id.to_string()))?;
        let mut receipt = Receipt { command: id.to_string(), domain: domain.to_string(), ..Default::default() };
        let original = params.clone();
        // what the command is, and whether this session may run it
        let (effect, doc) = match domain {
            "studio" => {
                let c = manifest::studio_command(cmd).ok_or_else(|| StudioError::UnknownCommand(id.to_string()))?;
                (c.effect, c.params)
            }
            "timeline" => {
                let c = filmcraft_engine::find_command(cmd).ok_or_else(|| StudioError::UnknownCommand(id.to_string()))?;
                (manifest::engine_effect(id, c.journal).0, c.params)
            }
            "motion" => {
                let c = effectcraft_engine::find_command(cmd).ok_or_else(|| StudioError::UnknownCommand(id.to_string()))?;
                (manifest::engine_effect(id, c.journal).0, c.params)
            }
            "image" => {
                let c = photocraft_engine::commands::find(cmd).ok_or_else(|| StudioError::UnknownCommand(id.to_string()))?;
                (manifest::engine_effect(id, c.journal).0, c.params)
            }
            _ => return Err(StudioError::UnknownCommand(id.to_string())),
        };
        if let Some(alt) = manifest::reserved(id) {
            return Err(StudioError::Reserved(id.to_string(), alt));
        }
        self.policy.check_effect(id, effect)?;
        let mut params = params;
        if domain != "studio" {
            self.policy.check_confinable(id, effect)?;
            receipt.notes = self.policy.confine_params(id, &mut params, effect == Effect::FileWrite)?.notes;
        }
        let before = self.marks();
        let rev_before = self.revision;
        let r = match domain {
            "timeline" => {
                crate::params::check(domain, id, doc, &params)?;
                self.timeline.execute(cmd, params).map_err(|e| StudioError::Timeline(e.to_string()))
            }
            "motion" => {
                let p = self.target_comp(id, params, &mut receipt)?;
                self.motion.execute(cmd, p).map_err(|e| StudioError::Motion(e.to_string()))
            }
            "image" => {
                let (asset, p) = self.take_asset(id, params, "asset", AssetKind::Image)?;
                crate::params::check(domain, id, doc, &p)?;
                receipt.asset = Some(asset.0);
                self.activate_image(asset)?;
                self.image.execute(cmd, p).map_err(|e| StudioError::Image(e.to_string()))
            }
            _ => self.studio(cmd, params, &mut receipt),
        };
        // publish even after a failure: a command that failed half-way may have changed state
        self.publish();
        let after = self.marks();
        receipt.changed = Self::changed(&before, &after);
        if !receipt.changed.is_empty() && !matches!(cmd, "project.open" | "project.new") {
            self.revision += 1;
        }
        receipt.revision = Revisions { before: rev_before, after: self.revision };
        let result = match r {
            Ok(v) => v,
            Err(e) => {
                self.settle();
                return Err(e);
            }
        };
        receipt.checked = if domain == "studio" { STUDIO_CHECKED.to_vec() } else { ENGINE_CHECKED.to_vec() };
        let extra = std::mem::take(&mut receipt.extra_checked);
        receipt.checked.extend(extra);
        if domain != "studio" && crate::params::is_strict(domain, id, doc) {
            receipt.checked.push(STRICT_CHECKED);
        }
        if receipt.changed.iter().any(|c| c != "studio") && !matches!(cmd, "project.open" | "project.new") {
            receipt.not_checked = NOT_CHECKED_VISUAL.to_vec();
        }
        if matches!(cmd, "project.open" | "project.new") && domain == "studio" {
            // a new history starts with the project
            self.journal.clear();
            self.undo.clear();
        }
        if !receipt.changed.is_empty() && !(domain == "studio" && matches!(cmd, "undo" | "project.open" | "project.new")) {
            self.undo.push(receipt.changed.clone());
        }
        if !receipt.changed.is_empty() || effect == Effect::FileWrite {
            self.journal.push(json!({"command": id, "params": original, "receipt": receipt}));
        }
        self.settle();
        Ok(json!({"result": result, "receipt": receipt}))
    }

    fn marks(&self) -> EngineMarks {
        EngineMarks {
            timeline: (self.timeline.revision, Arc::as_ptr(&self.timeline.project) as usize),
            motion: (self.motion.revision, Arc::as_ptr(&self.motion.project) as usize),
            images: self.image_docs.keys().map(|id| (*id, self.image_doc(*id).map(|d| (d.revision, Arc::as_ptr(&d.doc) as usize)).unwrap_or((0, 0)))).collect(),
            assets: self.assets.values().cloned().collect(),
        }
    }

    fn changed(a: &EngineMarks, b: &EngineMarks) -> Vec<String> {
        let mut v = Vec::new();
        if a.timeline != b.timeline {
            v.push("timeline".to_string());
        }
        if a.motion != b.motion {
            v.push("motion".to_string());
        }
        for (id, m) in &b.images {
            if a.images.get(id) != Some(m) {
                v.push(format!("image:{}", id.0));
            }
        }
        for id in a.images.keys() {
            if !b.images.contains_key(id) {
                v.push(format!("image:{}", id.0));
            }
        }
        if a.assets != b.assets {
            v.push("studio".to_string());
        }
        v
    }

    /// Make every engine's latest document visible to the renders of the others, and keep the
    /// timeline's picture of each nested item (size, rate, duration) in step with its document.
    pub(crate) fn publish(&mut self) {
        let mut seq = self.generation.load(std::sync::atomic::Ordering::Acquire);
        {
            let cur = read(&self.shared_motion);
            if !Arc::ptr_eq(&cur.project, &self.motion.project) {
                seq += 1;
                *self.shared_motion.write().unwrap_or_else(|e| e.into_inner()) = MotionSnapshot { project: self.motion.project.clone(), revision: seq };
            }
        }
        {
            let mut w = self.shared_images.write().unwrap_or_else(|e| e.into_inner());
            w.retain(|id, _| self.image_docs.contains_key(id));
            for id in self.image_docs.keys() {
                let Some(d) = self.image_doc(*id) else { continue };
                if w.get(id).is_none_or(|x| !Arc::ptr_eq(&x.doc, &d.doc)) {
                    seq += 1;
                    w.insert(*id, ImageSnapshot { doc: d.doc.clone(), revision: seq });
                }
            }
        }
        // after the snapshots: a render that sees the new generation also sees the new snapshots
        self.generation.store(seq, std::sync::atomic::Ordering::Release);
        *self.registry.write().unwrap_or_else(|e| e.into_inner()) = self.assets.iter().map(|(k, v)| (*k, v.clone())).collect();
        self.sync_nested_media();
    }

    /// A composition resized, re-timed or lengthened (or an image resized) after it was put on
    /// the timeline: update the timeline item's media description to match, as a relink would.
    /// Not an undo step of the timeline; undoing the change in its own engine syncs it back.
    fn sync_nested_media(&mut self) {
        let mut updates = Vec::new();
        for a in self.assets.values() {
            let Some(item) = a.timeline_item.map(filmcraft_engine::project::ItemId) else { continue };
            let (w, h, rate, dur) = match a.kind {
                AssetKind::Composition => {
                    let Some(c) = a.comp.and_then(|c| self.motion.project.comp(MotionItemId(c))) else { continue };
                    (c.width, c.height, crate::bridge::film_rate(c.frame_rate), Some(filmcraft_time::Tick(c.duration.0)))
                }
                AssetKind::Image => {
                    let Some(d) = self.image_doc(a.id) else { continue };
                    let b = d.doc.bounds();
                    (b.width(), b.height(), filmcraft_time::FrameRate::FPS_30, None)
                }
            };
            let Some(m) = self.timeline.project.item(item).and_then(|i| i.as_media()) else { continue };
            let Some(v) = m.info.video.as_ref() else { continue };
            if v.width != w.max(1) || v.height != h.max(1) || (dur.is_some() && (v.frame_rate != rate || Some(m.info.duration) != dur)) {
                updates.push((item, w.max(1), h.max(1), rate, dur));
            }
        }
        if updates.is_empty() {
            return;
        }
        let p = Arc::make_mut(&mut self.timeline.project);
        for (item, w, h, rate, dur) in updates {
            if let Some(m) = p.item_mut(item).and_then(|i| match &mut i.kind {
                filmcraft_engine::project::ItemKind::Media(m) => Some(m),
                _ => None,
            }) {
                if let Some(v) = m.info.video.as_mut() {
                    v.width = w;
                    v.height = h;
                    if dur.is_some() {
                        v.frame_rate = rate;
                    }
                }
                if let Some(d) = dur {
                    m.info.duration = d;
                }
            }
            // the cached source describes the old document; the next frame opens a new one
            self.timeline.media.remove(item);
        }
        self.timeline.revision += 1;
    }

    fn add_asset(&mut self, kind: AssetKind, name: &str, comp: Option<u64>) -> AssetId {
        let id = AssetId(self.next_asset);
        self.next_asset += 1;
        self.assets.insert(id, Asset { id, kind, name: name.to_string(), comp, timeline_item: None, motion_footage: None });
        id
    }

    fn asset_of(&self, cmd: &str, v: Option<&Value>, key: &str, kind: AssetKind) -> Result<Asset> {
        let id = v.and_then(u64_of).ok_or_else(|| bad(cmd, format!("`{key}` must be an asset id (see studio:assets.list)")))?;
        let a = self.assets.get(&AssetId(id)).ok_or(StudioError::NoAsset(id))?;
        if a.kind != kind {
            return Err(StudioError::WrongKind(id, if kind == AssetKind::Image { "an image" } else { "a composition" }));
        }
        Ok(a.clone())
    }

    /// Remove `key` from the params and resolve it to an asset of `kind`.
    fn take_asset(&self, cmd: &str, mut p: Value, key: &str, kind: AssetKind) -> Result<(AssetId, Value)> {
        let v = p.as_object_mut().and_then(|o| o.remove(key));
        let a = self.asset_of(cmd, v.as_ref(), key, kind)?;
        Ok((a.id, p))
    }

    /// `motion:` commands: an `asset` param makes that composition the active one, which is
    /// what the motion engine's commands act on when they take no `comp` of their own.
    fn target_comp(&mut self, cmd: &str, mut p: Value, receipt: &mut Receipt) -> Result<Value> {
        let Some(v) = p.as_object_mut().and_then(|o| o.remove("asset")) else { return Ok(p) };
        let a = self.asset_of(cmd, Some(&v), "asset", AssetKind::Composition)?;
        let comp = a.comp.ok_or_else(|| StudioError::Other(format!("asset {} has no composition", a.id.0)))?;
        self.motion.open_comp(MotionItemId(comp));
        receipt.asset = Some(a.id.0);
        Ok(p)
    }

    /// The active sequence at `seconds` as straight 8-bit RGBA (`scale` 1 = full size), with the
    /// names of offline media the frame drew a slate for. Reads only.
    pub fn render_rgba(&self, seconds: f64, scale: f64) -> Result<(u32, u32, Vec<u8>, Vec<String>)> {
        if !(scale.is_finite() && scale > 0.0 && scale <= 1.0) || !seconds.is_finite() || seconds < 0.0 {
            return Err(bad("render", "scale must be in (0, 1] and seconds non-negative"));
        }
        let (img, offline) = self
            .timeline
            .try_render_program_with_offline_at(scale as f32, filmcraft_time::Tick::from_seconds_f64(seconds))
            .map_err(|e| StudioError::Timeline(e.to_string()))?;
        let (w, h) = (u32::try_from(img.w).unwrap_or(0), u32::try_from(img.h).unwrap_or(0));
        Ok((w, h, img.to_rgba8(), offline.into_iter().map(|o| o.name).collect()))
    }
}
