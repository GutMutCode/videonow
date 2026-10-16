//! The studio: one session of each engine, the asset table, and the single command entry point.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, RwLock};

use serde_json::{Value, json};

use effectcraft_project::{Footage, FootageKind, ItemId as MotionItemId, ItemKind as MotionItemKind};
use effectcraft_render::FootageSource;
use filmcraft_media::SharedSource;
use filmcraft_time::Tick;
use photocraft_doc::{DocId, Document};

use crate::assets::{self, Asset, AssetId, AssetKind, parse_uri};
use crate::bridge::{CompMediaSource, Generation, ImageMediaSource, ImageSnapshot, MotionSnapshot, SharedImages, SharedMotion, StudioFootage};
use crate::manifest::{self, studio_command};
use crate::params_check::{self, KeyCheck};
use crate::receipt::{ENGINE_CHECKED, NOT_CHECKED_VISUAL, Receipt, Revisions, STUDIO_CHECKED};
use crate::{Result, StudioError};

pub struct Studio {
    /// FilmCraft: sequences, editing, audio, captions, export.
    pub timeline: filmcraft_engine::Session,
    /// EffectCraft: every composition of the project.
    pub motion: effectcraft_engine::Session,
    /// PhotoCraft: every image document of the project, one document per image asset.
    pub images: photocraft_engine::Session,
    /// The document that holds each image asset.
    pub(crate) image_docs: BTreeMap<AssetId, DocId>,
    /// Image assets whose document was closed in the image editor: their last state. A command
    /// or the editor reopens them (`Studio::open_image`); saving writes them as they are.
    pub(crate) parked: BTreeMap<AssetId, Arc<Document>>,
    pub(crate) assets: BTreeMap<AssetId, Asset>,
    pub(crate) next_asset: u64,
    pub(crate) shared_motion: SharedMotion,
    pub(crate) shared_images: SharedImages,
    /// The asset table as the timeline's nested-source resolver sees it.
    pub(crate) registry: Arc<RwLock<HashMap<AssetId, Asset>>>,
    pub(crate) footage: Arc<dyn FootageSource>,
    /// The latest snapshot number, shared with the sources (see [`Generation`]).
    pub(crate) generation: Generation,
    pub(crate) revision: u64,
    pub(crate) journal: Vec<Value>,
    /// What each change touched, newest last (for `studio:undo`).
    pub(crate) undo: Vec<Vec<String>>,
    /// Where the project was last saved or opened from.
    pub(crate) project_path: Option<std::path::PathBuf>,
    /// What clients of this studio may do (checked before every command).
    pub(crate) policy: crate::policy::Policy,
    /// Who is acting, for receipts and the journal (see [`Receipt::by`]).
    pub(crate) actor: String,
    /// The engines' documents as of the last command or editor sync.
    pub(crate) last: EngineMarks,
    /// How much of each engine's own journal the studio has accounted for.
    pub(crate) seen: JournalMarks,
    /// The engines' documents as of the last save, open or new project (unsaved changes = any
    /// difference).
    pub(crate) saved: EngineMarks,
}

impl Default for Studio {
    fn default() -> Self {
        Self::new()
    }
}

/// Where each engine's document stands (its revision and its snapshot pointer).
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct EngineMarks {
    timeline: (u64, usize),
    motion: (u64, usize),
    images: BTreeMap<AssetId, (u64, usize)>,
    assets: Vec<Asset>,
}

/// Lengths of the engines' own command journals.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct JournalMarks {
    pub timeline: usize,
    pub motion: usize,
    pub images: usize,
}

pub(crate) fn read<T: Clone>(l: &RwLock<T>) -> T {
    l.read().unwrap_or_else(|e| e.into_inner()).clone()
}

pub(crate) fn bad(cmd: &str, msg: impl Into<String>) -> StudioError {
    StudioError::BadParams { cmd: cmd.to_string(), msg: msg.into() }
}

pub(crate) fn u64_of(v: &Value) -> Option<u64> {
    v.as_u64().or_else(|| v.as_str().and_then(|s| s.parse().ok()))
}

impl Studio {
    /// A studio over fresh headless engine sessions.
    pub fn new() -> Self {
        Self::with_sessions(filmcraft_engine::Session::default(), effectcraft_host::session(), photocraft_engine::Session::new())
    }

    /// A studio over engine sessions the caller configured (the desktop application's, with its
    /// preferences, decoders and services). Anything already open in them becomes assets on the
    /// first [`Studio::sync_editors`].
    pub fn with_sessions(timeline: filmcraft_engine::Session, mut motion: effectcraft_engine::Session, images: photocraft_engine::Session) -> Self {
        let shared_images: SharedImages = Arc::default();
        let footage: Arc<dyn FootageSource> = Arc::new(StudioFootage::new(motion.footage.clone(), shared_images.clone()));
        motion.footage = footage.clone();
        let shared_motion: SharedMotion = Arc::new(RwLock::new(MotionSnapshot { project: motion.project.clone(), revision: 0 }));
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
        let mut s = Self {
            timeline,
            motion,
            images,
            image_docs: BTreeMap::new(),
            parked: BTreeMap::new(),
            assets: BTreeMap::new(),
            next_asset: 1,
            shared_motion,
            shared_images,
            registry,
            footage,
            generation,
            revision: 0,
            journal: Vec::new(),
            undo: Vec::new(),
            project_path: None,
            policy: crate::policy::Policy::unrestricted(),
            actor: "api".into(),
            last: EngineMarks::default(),
            seen: JournalMarks::default(),
            saved: EngineMarks::default(),
        };
        s.last = s.marks();
        s.seen = s.journal_marks();
        s.saved = s.last.clone();
        s
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn assets(&self) -> impl Iterator<Item = &Asset> {
        self.assets.values()
    }

    pub fn asset_info(&self, id: AssetId) -> Option<&Asset> {
        self.assets.get(&id)
    }

    /// The image document of an image asset: open in the image engine, or as it was when its
    /// editor tab was closed.
    pub fn image_document(&self, id: AssetId) -> Option<Arc<Document>> {
        match self.image_index(id) {
            Some(i) => self.images.documents().get(i).map(|d| d.doc.clone()),
            None => self.parked.get(&id).cloned(),
        }
    }

    /// The image asset a document of the image engine holds, if any.
    pub fn asset_of_document(&self, doc: DocId) -> Option<AssetId> {
        self.image_docs.iter().find(|(_, d)| **d == doc).map(|(a, _)| *a)
    }

    /// The composition asset of a motion-engine composition item, if any.
    pub fn asset_of_comp(&self, comp: u64) -> Option<AssetId> {
        self.assets.values().find(|a| a.kind == AssetKind::Composition && a.comp == Some(comp)).map(|a| a.id)
    }

    /// The asset a timeline project item shows (a nested composition or image), if any.
    pub fn asset_of_timeline_item(&self, item: u64) -> Option<AssetId> {
        let it = self.timeline.project.item(filmcraft_engine::project::ItemId(item))?;
        match &it.as_media()?.media {
            filmcraft_engine::project::MediaRef::Nested { uri } => parse_uri(uri).map(|(_, id)| id),
            _ => None,
        }
    }

    /// Limit what clients of this studio may do (see [`crate::policy`]).
    pub fn set_policy(&mut self, policy: crate::policy::Policy) {
        self.policy = policy;
    }

    pub fn policy(&self) -> &crate::policy::Policy {
        &self.policy
    }

    /// Run `f` with another policy and actor (an agent's commands in the desktop application),
    /// then put the studio's own back.
    pub fn as_actor<T>(&mut self, actor: &str, policy: Option<crate::policy::Policy>, f: impl FnOnce(&mut Self) -> T) -> T {
        let saved_actor = std::mem::replace(&mut self.actor, actor.to_string());
        let saved_policy = policy.map(|p| std::mem::replace(&mut self.policy, p));
        let r = f(self);
        self.actor = saved_actor;
        if let Some(p) = saved_policy {
            self.policy = p;
        }
        r
    }

    /// Who receipts and journal entries name as acting (default `api`).
    pub fn set_actor(&mut self, actor: &str) {
        self.actor = actor.to_string();
    }

    /// Where the project was last saved or opened from.
    pub fn project_path(&self) -> Option<&std::path::Path> {
        self.project_path.as_deref()
    }

    /// Has anything changed since the project was last saved, opened or started?
    pub fn is_dirty(&self) -> bool {
        self.marks() != self.saved
    }

    /// Every change so far, oldest first (`studio:journal`).
    pub fn journal(&self) -> &[Value] {
        &self.journal
    }

    /// Run any command: `timeline:<id>`, `motion:<id>`, `image:<id>` or `studio:<id>`.
    /// Returns `{"result": <the command's own result>, "receipt": <Receipt>}`.
    ///
    /// `motion:` commands take an optional `"asset"` (a composition asset) and run on that
    /// composition; `image:` commands need `"asset"` (an image asset).
    pub fn execute(&mut self, id: &str, params: Value) -> Result<Value> {
        // changes made outside commands (an editor, direct engine calls) are accounted for first,
        // so this command's receipt names only what it did
        if self.marks() != self.last {
            self.sync_editors("external");
        }
        let params = if params.is_null() { json!({}) } else { params };
        if !params.is_object() {
            return Err(bad(id, "params must be a JSON object"));
        }
        let (domain, cmd) = id.split_once(':').ok_or_else(|| StudioError::UnknownCommand(id.to_string()))?;
        let mut receipt = Receipt { command: id.to_string(), domain: domain.to_string(), by: self.actor.clone(), ..Default::default() };
        let before = self.last.clone();
        let rev_before = self.revision;
        let original = params.clone();
        let mut key_check: Option<KeyCheck> = None;
        let effect = manifest::effect_of(domain, cmd).ok_or_else(|| StudioError::UnknownCommand(id.to_string()))?;
        if let Some(instead) = manifest::host_command(domain, cmd) {
            return Err(bad(id, format!("in a VideoNow project use {instead}: this command acts on one engine's document only")));
        }
        self.policy.check(id, effect, &params).map_err(StudioError::Denied)?;
        let assets_before = self.assets.len();
        let r = match domain {
            "timeline" => {
                let spec = filmcraft_engine::find_command(cmd).ok_or_else(|| StudioError::UnknownCommand(id.to_string()))?;
                key_check = Some(params_check::check(domain, cmd, spec.params, &params).map_err(|m| bad(id, m))?);
                self.timeline.execute(cmd, params).map_err(|e| StudioError::Timeline(e.to_string()))
            }
            "motion" => {
                let spec = effectcraft_engine::find_command(cmd).ok_or_else(|| StudioError::UnknownCommand(id.to_string()))?;
                let p = self.target_comp(id, params, &mut receipt)?;
                // the engine rejects unknown keys itself when the command's doc lists them
                let closed = effectcraft_engine::commands::accepted_params(spec.params).is_some();
                key_check = Some(if closed { KeyCheck::ByEngine } else { KeyCheck::Open });
                self.motion.execute_checked(cmd, p).map_err(|e| StudioError::Motion(e.to_string()))
            }
            "image" => {
                let spec = photocraft_engine::commands::find(cmd).ok_or_else(|| StudioError::UnknownCommand(id.to_string()))?;
                let (asset, p) = self.take_asset(id, params, "asset", AssetKind::Image)?;
                key_check = Some(params_check::check(domain, cmd, spec.params, &p).map_err(|m| bad(id, m))?);
                receipt.asset = Some(asset.0);
                let i = self.open_image(asset)?;
                self.images.set_active(i);
                self.images.execute(cmd, p).map_err(|e| StudioError::Image(e.to_string()))
            }
            "studio" => self.studio(cmd, params, &mut receipt),
            _ => Err(StudioError::UnknownCommand(id.to_string())),
        };
        // documents a command opened (an import as a new document, a new composition) become
        // assets like the ones an editor makes
        if !(domain == "studio" && matches!(cmd, "project.open" | "project.new")) {
            receipt.created.extend(self.adopt());
            receipt.created.dedup();
        }
        // publish even after a failure: a command that failed half-way may have changed state
        self.publish();
        let after = self.marks();
        receipt.changed = Self::changed(&before, &after);
        if domain == "studio" && cmd == "project.save" {
            // saving rewrites the engines' file metadata (path, saved time), not the work itself
            receipt.changed.clear();
        }
        if self.assets.len() != assets_before && !receipt.changed.iter().any(|c| c == "studio") {
            receipt.changed.push("studio".into());
        }
        if !receipt.changed.is_empty() {
            self.revision += 1;
        }
        self.last = after;
        self.seen = self.journal_marks();
        if domain == "studio" && matches!(cmd, "project.save" | "project.open" | "project.new") && r.is_ok() {
            self.saved = self.last.clone();
        }
        receipt.revision = Revisions { before: rev_before, after: self.revision };
        let result = r?;
        if domain == "studio" {
            receipt.checked = STUDIO_CHECKED.to_vec();
            receipt.checked.push("delegation policy (effect class, file folders)");
        } else {
            receipt.checked = ENGINE_CHECKED.to_vec();
            receipt.checked.push("delegation policy (effect class, file folders)");
            match key_check {
                Some(KeyCheck::Checked) => receipt.checked.push("unknown parameters rejected (params doc)"),
                Some(KeyCheck::ByEngine) => receipt.checked.push("unknown parameters rejected (by the engine)"),
                _ => receipt.not_checked.push("parameter names (the command's params doc is open-ended)"),
            }
        }
        if receipt.changed.iter().any(|c| c != "studio") {
            receipt.not_checked.extend_from_slice(NOT_CHECKED_VISUAL);
        }
        // undo replays engine histories; replacing the whole project is not undoable
        let undoable = !(domain == "studio" && matches!(cmd, "undo" | "project.open" | "project.new" | "project.save"));
        if !receipt.changed.is_empty() && undoable {
            self.undo.push(receipt.changed.clone());
        }
        if !receipt.changed.is_empty() {
            self.journal.push(json!({"command": id, "params": original, "receipt": receipt}));
        }
        Ok(json!({"result": result, "receipt": receipt}))
    }

    pub(crate) fn marks(&self) -> EngineMarks {
        EngineMarks {
            timeline: (self.timeline.revision, Arc::as_ptr(&self.timeline.project) as usize),
            motion: (self.motion.revision, Arc::as_ptr(&self.motion.project) as usize),
            images: self
                .assets
                .values()
                .filter(|a| a.kind == AssetKind::Image)
                .map(|a| {
                    let m = match self.image_index(a.id).and_then(|i| self.images.documents().get(i)) {
                        Some(d) => (d.revision, Arc::as_ptr(&d.doc) as usize),
                        None => (0, self.parked.get(&a.id).map_or(0, |d| Arc::as_ptr(d) as usize)),
                    };
                    (a.id, m)
                })
                .collect(),
            assets: self.assets.values().cloned().collect(),
        }
    }

    pub(crate) fn journal_marks(&self) -> JournalMarks {
        JournalMarks { timeline: self.timeline.journal.len(), motion: self.motion.journal.len(), images: self.images.journal.len() }
    }

    pub(crate) fn changed(a: &EngineMarks, b: &EngineMarks) -> Vec<String> {
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
        if a.assets != b.assets {
            v.push("studio".to_string());
        }
        v
    }

    /// Make every engine's latest document visible to the renders of the others.
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
            let docs: Vec<(AssetId, Arc<Document>)> = self.assets.values().filter(|a| a.kind == AssetKind::Image).filter_map(|a| self.image_document(a.id).map(|d| (a.id, d))).collect();
            let mut w = self.shared_images.write().unwrap_or_else(|e| e.into_inner());
            w.retain(|id, _| self.assets.contains_key(id));
            for (id, doc) in docs {
                if w.get(&id).is_none_or(|x| !Arc::ptr_eq(&x.doc, &doc)) {
                    seq += 1;
                    w.insert(id, ImageSnapshot { doc, revision: seq });
                }
            }
        }
        // after the snapshots: a render that sees the new generation also sees the new snapshots
        self.generation.store(seq, std::sync::atomic::Ordering::Release);
        *self.registry.write().unwrap_or_else(|e| e.into_inner()) = self.assets.iter().map(|(k, v)| (*k, v.clone())).collect();
    }

    pub(crate) fn add_asset(&mut self, kind: AssetKind, name: &str, comp: Option<u64>) -> AssetId {
        let id = AssetId(self.next_asset);
        self.next_asset += 1;
        self.assets.insert(id, Asset { id, kind, name: name.to_string(), comp, timeline_item: None, motion_footage: None });
        id
    }

    /// Index of an image asset's document among the image engine's open documents.
    pub(crate) fn image_index(&self, id: AssetId) -> Option<usize> {
        let doc = self.image_docs.get(&id)?;
        self.images.documents().iter().position(|d| d.doc.id == *doc)
    }

    /// The open document of an image asset, reopening it from its last state if its editor tab
    /// was closed. Returns its index in the image engine.
    pub fn open_image(&mut self, id: AssetId) -> Result<usize> {
        if let Some(i) = self.image_index(id) {
            return Ok(i);
        }
        match self.assets.get(&id) {
            Some(a) if a.kind == AssetKind::Image => {}
            Some(_) => return Err(StudioError::WrongKind(id.0, "an image")),
            None => return Err(StudioError::NoAsset(id.0)),
        }
        let doc = self.parked.remove(&id).or_else(|| read(&self.shared_images).get(&id).map(|s| s.doc.clone())).ok_or(StudioError::NoAsset(id.0))?;
        let i = self.images.add_document((*doc).clone(), None);
        let new_id = self.images.documents().get(i).map(|d| d.doc.id).ok_or(StudioError::NoAsset(id.0))?;
        self.image_docs.insert(id, new_id);
        Ok(i)
    }

    fn asset(&self, cmd: &str, v: Option<&Value>, key: &str, kind: AssetKind) -> Result<Asset> {
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
        let a = self.asset(cmd, v.as_ref(), key, kind)?;
        Ok((a.id, p))
    }

    /// `motion:` commands: an `asset` param makes that composition the active one, which is
    /// what the motion engine's commands act on when they take no `comp` of their own.
    fn target_comp(&mut self, cmd: &str, mut p: Value, receipt: &mut Receipt) -> Result<Value> {
        let Some(v) = p.as_object_mut().and_then(|o| o.remove("asset")) else { return Ok(p) };
        let a = self.asset(cmd, Some(&v), "asset", AssetKind::Composition)?;
        let comp = a.comp.ok_or_else(|| StudioError::Other(format!("asset {} has no composition", a.id.0)))?;
        self.motion.open_comp(MotionItemId(comp));
        receipt.asset = Some(a.id.0);
        Ok(p)
    }

    /// The timeline project item that shows asset `id`, importing it (as a live nested source)
    /// if there is none yet or the one there was deleted.
    pub(crate) fn ensure_timeline_item(&mut self, id: AssetId) -> Result<u64> {
        let a = self.assets.get(&id).cloned().ok_or(StudioError::NoAsset(id.0))?;
        if let Some(i) = a.timeline_item
            && self.timeline.project.item(filmcraft_engine::project::ItemId(i)).is_some()
        {
            return Ok(i);
        }
        self.publish();
        let src: SharedSource = match a.kind {
            AssetKind::Composition => {
                let comp = a.comp.ok_or_else(|| StudioError::Other(format!("asset {} has no composition", id.0)))?;
                Arc::new(
                    CompMediaSource::new(&a.name, MotionItemId(comp), self.shared_motion.clone(), self.generation.clone(), self.footage.clone())
                        .ok_or_else(|| StudioError::Other(format!("composition {comp} not found")))?,
                )
            }
            AssetKind::Image => Arc::new(ImageMediaSource::new(&a.name, a.id, self.shared_images.clone()).ok_or_else(|| StudioError::Other(format!("image {} has no document", id.0)))?),
        };
        let uri = assets::uri(a.kind, a.id);
        let it = filmcraft_engine::commands::import_nested(&mut self.timeline, &uri, &a.name, src, None).map_err(|e| StudioError::Timeline(e.to_string()))?;
        if let Some(x) = self.assets.get_mut(&id) {
            x.timeline_item = Some(it.0);
        }
        Ok(it.0)
    }

    /// The motion project's footage item that shows image asset `id` inside compositions,
    /// adding it if there is none yet or the one there was deleted.
    pub(crate) fn ensure_motion_footage(&mut self, id: AssetId) -> Result<u64> {
        let a = self.assets.get(&id).cloned().ok_or(StudioError::NoAsset(id.0))?;
        if a.kind != AssetKind::Image {
            return Err(StudioError::WrongKind(id.0, "an image"));
        }
        if let Some(f) = a.motion_footage
            && self.motion.project.items.contains_key(&MotionItemId(f))
        {
            return Ok(f);
        }
        let doc = self.image_document(id).ok_or(StudioError::NoAsset(id.0))?;
        let (w, h) = (doc.bounds().width(), doc.bounds().height());
        let f = Footage {
            path: assets::uri(AssetKind::Image, id),
            kind: FootageKind::Still,
            width: w,
            height: h,
            pixel_aspect: 1.0,
            frame_rate: effectcraft_time::FrameRate::new(30, 1),
            duration: effectcraft_time::Tick::from_seconds_f64(10.0),
            has_video: true,
            ..Default::default()
        };
        let name = a.name.clone();
        let fid = self
            .motion
            .edit(&format!("Add {name}"), None, |pr, _| Ok(pr.add_item(&name, effectcraft_color::Label::Aqua, None, MotionItemKind::Footage(f))))
            .map_err(|e| StudioError::Motion(e.to_string()))?;
        if let Some(x) = self.assets.get_mut(&id) {
            x.motion_footage = Some(fid.0);
        }
        Ok(fid.0)
    }

    /// Make a new asset available to the other engines: every asset in the timeline's project
    /// panel, every image in the motion project. Failures are warnings, not errors: the asset
    /// exists either way and `studio:asset.place` / `studio:image.toComp` retry.
    pub(crate) fn link_asset(&mut self, id: AssetId) -> Vec<String> {
        let mut warnings = Vec::new();
        if let Err(e) = self.ensure_timeline_item(id) {
            warnings.push(format!("asset {}: not in the timeline project yet ({e})", id.0));
        }
        if self.assets.get(&id).is_some_and(|a| a.kind == AssetKind::Image)
            && let Err(e) = self.ensure_motion_footage(id)
        {
            warnings.push(format!("asset {}: not in the motion project yet ({e})", id.0));
        }
        warnings
    }

    fn studio(&mut self, cmd: &str, p: Value, receipt: &mut Receipt) -> Result<Value> {
        let full = format!("studio:{cmd}");
        let spec = studio_command(cmd).ok_or_else(|| StudioError::UnknownCommand(full.clone()))?;
        if let Some(o) = p.as_object() {
            for k in o.keys() {
                if !spec.keys.contains(&k.as_str()) {
                    return Err(bad(&full, format!("unknown parameter `{k}` (expected: {})", if spec.keys.is_empty() { "none".to_string() } else { spec.keys.join(", ") })));
                }
            }
        }
        let f64_p = |k: &str| p.get(k).and_then(Value::as_f64);
        let u64_p = |k: &str| p.get(k).and_then(Value::as_u64);
        let str_p = |k: &str| p.get(k).and_then(Value::as_str);
        match cmd {
            "commands" => {
                let filter = str_p("filter").map(str::to_ascii_lowercase);
                let domain = str_p("domain");
                let list: Vec<_> = manifest::all()
                    .into_iter()
                    .filter(|c| domain.is_none_or(|d| c.domain == d))
                    .filter(|c| filter.as_ref().is_none_or(|f| c.id.to_ascii_lowercase().contains(f) || c.label.to_ascii_lowercase().contains(f)))
                    .collect();
                Ok(serde_json::to_value(list).unwrap_or(Value::Null))
            }
            "assets.list" => Ok(serde_json::to_value(self.assets.values().collect::<Vec<_>>()).unwrap_or(Value::Null)),
            "journal" => Ok(Value::Array(self.journal.clone())),
            "inspect" => Ok(self.inspect()),
            "comp.new" => {
                let n = self.assets.len() + 1;
                let name = str_p("name").map(str::to_string).unwrap_or_else(|| format!("Composition {n}"));
                let (w, h) = (u64_p("width").unwrap_or(1920), u64_p("height").unwrap_or(1080));
                let (fps, dur) = (f64_p("fps").unwrap_or(30.0), f64_p("duration").unwrap_or(5.0));
                if !(1..=16384).contains(&w) || !(1..=16384).contains(&h) {
                    return Err(bad(&full, "width and height must be 1..16384"));
                }
                if !(fps.is_finite() && fps > 0.0 && fps <= 240.0) || !(dur.is_finite() && dur > 0.0) {
                    return Err(bad(&full, "fps must be in (0, 240] and duration positive"));
                }
                let r = self
                    .motion
                    .execute("comp.new", json!({"name": name, "width": w, "height": h, "frameRate": fps, "duration": dur, "open": true}))
                    .map_err(|e| StudioError::Motion(e.to_string()))?;
                let comp = r.get("comp").and_then(u64_of).ok_or_else(|| StudioError::Other(format!("comp.new returned no composition id: {r}")))?;
                let id = self.add_asset(AssetKind::Composition, &name, Some(comp));
                self.publish();
                receipt.warnings.extend(self.link_asset(id));
                receipt.created.push(id.0);
                receipt.asset = Some(id.0);
                Ok(json!({"asset": id.0, "comp": comp, "name": name}))
            }
            "image.new" => {
                let n = self.assets.len() + 1;
                let name = str_p("name").map(str::to_string).unwrap_or_else(|| format!("Image {n}"));
                let (w, h) = (u64_p("width").unwrap_or(1920), u64_p("height").unwrap_or(1080));
                if !(1..=30000).contains(&w) || !(1..=30000).contains(&h) {
                    return Err(bad(&full, "width and height must be 1..30000"));
                }
                let background = str_p("background").unwrap_or("transparent");
                let count = self.images.documents().len();
                self.images.execute("file.new", json!({"width": w, "height": h, "background": background, "name": name})).map_err(|e| StudioError::Image(e.to_string()))?;
                let doc = match self.images.documents().get(count) {
                    Some(d) if self.images.documents().len() == count + 1 => d.doc.id,
                    _ => return Err(StudioError::Other("image engine did not open a new document".into())),
                };
                let id = self.add_asset(AssetKind::Image, &name, None);
                self.image_docs.insert(id, doc);
                self.publish();
                receipt.warnings.extend(self.link_asset(id));
                receipt.created.push(id.0);
                receipt.asset = Some(id.0);
                Ok(json!({"asset": id.0, "name": name, "width": w, "height": h}))
            }
            "asset.place" => {
                let v = p.get("asset");
                let id = v.and_then(u64_of).ok_or_else(|| bad(&full, "`asset` must be an asset id"))?;
                if !self.assets.contains_key(&AssetId(id)) {
                    return Err(StudioError::NoAsset(id));
                }
                receipt.asset = Some(id);
                let item = self.ensure_timeline_item(AssetId(id))?;
                let mut tp = json!({"item": item, "seconds": f64_p("seconds").unwrap_or(0.0)});
                if let Some(t) = p.get("track") {
                    tp["track"] = t.clone();
                }
                let placed = self.timeline.execute("timeline.place", tp).map_err(|e| StudioError::Timeline(e.to_string()))?;
                Ok(json!({"item": item, "placed": placed}))
            }
            "image.toComp" => {
                let img = self.asset(&full, p.get("image"), "image", AssetKind::Image)?;
                let comp_a = self.asset(&full, p.get("comp"), "comp", AssetKind::Composition)?;
                receipt.asset = Some(comp_a.id.0);
                let comp = MotionItemId(comp_a.comp.ok_or_else(|| StudioError::Other(format!("asset {} has no composition", comp_a.id.0)))?);
                if self.motion.project.comp(comp).is_none() {
                    return Err(StudioError::Other(format!("composition {} not found", comp.0)));
                }
                let footage_item = self.ensure_motion_footage(img.id)?;
                let layer = self
                    .motion
                    .execute("layer.addItem", json!({"comp": comp.0, "item": footage_item, "time": f64_p("time").unwrap_or(0.0)}))
                    .map_err(|e| StudioError::Motion(e.to_string()))?;
                Ok(json!({"footage": footage_item, "layer": layer}))
            }
            "render.frame" => {
                let sec = f64_p("seconds").unwrap_or(0.0);
                let scale = f64_p("scale").unwrap_or(1.0);
                if !(scale.is_finite() && scale > 0.0 && scale <= 1.0) || !sec.is_finite() || sec < 0.0 {
                    return Err(bad(&full, "scale must be in (0, 1] and seconds non-negative"));
                }
                let path = str_p("path").unwrap_or("frame.png").to_string();
                let (img, offline) = self.timeline.try_render_program_with_offline_at(scale as f32, Tick::from_seconds_f64(sec)).map_err(|e| StudioError::Timeline(e.to_string()))?;
                let (w, h) = (u32::try_from(img.w).unwrap_or(0), u32::try_from(img.h).unwrap_or(0));
                image::save_buffer(&path, &img.to_rgba8(), w, h, image::ColorType::Rgba8).map_err(|e| StudioError::Other(format!("{path}: {e}")))?;
                for o in &offline {
                    receipt.warnings.push(format!("{} is offline ({}); the frame shows its slate", o.name, o.status));
                }
                Ok(json!({"path": path, "width": w, "height": h, "seconds": sec, "offline": offline}))
            }
            "project.save" => {
                let path = match str_p("path") {
                    Some(p) => crate::project::project_dir(p),
                    None => self.project_path.clone().ok_or_else(|| bad(&full, "`path` is needed for a project that was never saved"))?,
                };
                self.save_project(&path)
            }
            "project.open" => {
                let path = str_p("path").ok_or_else(|| bad(&full, "`path` must be a .videonow folder"))?;
                let r = self.open_project(std::path::Path::new(path))?;
                receipt.warnings.push("undo history starts over after opening a project".into());
                Ok(r)
            }
            "project.new" => {
                self.new_project()?;
                Ok(json!({"new": true}))
            }
            "undo" => {
                let Some(last) = self.undo.pop() else { return Err(StudioError::Other("nothing to undo".into())) };
                let mut undone = Vec::new();
                for d in &last {
                    if d == "timeline" {
                        self.timeline.execute("edit.undo", json!({})).map_err(|e| StudioError::Timeline(e.to_string()))?;
                    } else if d == "motion" {
                        self.motion.execute("edit.undo", json!({})).map_err(|e| StudioError::Motion(e.to_string()))?;
                    } else if let Some(id) = d.strip_prefix("image:").and_then(|s| s.parse::<u64>().ok()) {
                        let i = self.open_image(AssetId(id))?;
                        self.images.set_active(i);
                        self.images.execute("edit.undo", json!({})).map_err(|e| StudioError::Image(e.to_string()))?;
                    } else {
                        // the asset table itself is not under undo
                        receipt.warnings.push("asset table changes are not undone".into());
                        continue;
                    }
                    undone.push(d.clone());
                }
                Ok(json!({"undone": undone}))
            }
            _ => Err(StudioError::UnknownCommand(full)),
        }
    }

    /// The active sequence at `seconds` as straight 8-bit RGBA (`scale` 1 = full size), with the
    /// names of offline media the frame drew a slate for. Reads only.
    pub fn render_rgba(&self, seconds: f64, scale: f64) -> Result<(u32, u32, Vec<u8>, Vec<String>)> {
        if !(scale.is_finite() && scale > 0.0 && scale <= 1.0) || !seconds.is_finite() || seconds < 0.0 {
            return Err(bad("render", "scale must be in (0, 1] and seconds non-negative"));
        }
        let (img, offline) = self.timeline.try_render_program_with_offline_at(scale as f32, Tick::from_seconds_f64(seconds)).map_err(|e| StudioError::Timeline(e.to_string()))?;
        let (w, h) = (u32::try_from(img.w).unwrap_or(0), u32::try_from(img.h).unwrap_or(0));
        Ok((w, h, img.to_rgba8(), offline.into_iter().map(|o| o.name).collect()))
    }

    fn inspect(&self) -> Value {
        let comps: Vec<Value> = self
            .assets
            .values()
            .filter(|a| a.kind == AssetKind::Composition)
            .map(|a| {
                let c = a.comp.and_then(|c| self.motion.project.comp(MotionItemId(c)));
                json!({
                    "asset": a.id.0, "name": a.name, "comp": a.comp,
                    "width": c.map(|c| c.width), "height": c.map(|c| c.height),
                    "fps": c.map(|c| c.frame_rate.num as f64 / c.frame_rate.den.max(1) as f64),
                    "duration": c.map(|c| c.duration.0 as f64 / filmcraft_time::TICKS_PER_SECOND as f64),
                    "layers": c.map(|c| c.layers.len()),
                    "missing": c.is_none(),
                    "onTimeline": a.timeline_item,
                })
            })
            .collect();
        let images: Vec<Value> = self
            .assets
            .values()
            .filter(|a| a.kind == AssetKind::Image)
            .map(|a| {
                let d = self.image_index(a.id).and_then(|i| self.images.documents().get(i));
                let doc = self.image_document(a.id);
                json!({
                    "asset": a.id.0, "name": a.name,
                    "width": doc.as_ref().map(|d| d.bounds().width()), "height": doc.as_ref().map(|d| d.bounds().height()),
                    "layers": doc.as_ref().map(|d| d.layers.len()),
                    "revision": d.map(|d| d.revision),
                    "open": d.is_some(),
                    "onTimeline": a.timeline_item, "inComps": a.motion_footage.is_some(),
                })
            })
            .collect();
        let seq_id = self.timeline.state.active_sequence;
        let seq_name = seq_id.and_then(|id| self.timeline.project.item(id)).map(|i| i.name.clone());
        json!({
            "revision": self.revision,
            "unsaved": self.is_dirty(),
            "path": self.project_path.as_ref().map(|p| p.to_string_lossy()),
            "compositions": comps,
            "images": images,
            "sequence": self.timeline.active_sequence().map(|q| json!({
                "item": seq_id.map(|i| i.0), "name": seq_name, "width": q.settings.width, "height": q.settings.height,
                "fps": q.settings.frame_rate.num as f64 / q.settings.frame_rate.den.max(1) as f64,
                "videoTracks": q.video_tracks.len(), "audioTracks": q.audio_tracks.len(),
                "clips": q.video_tracks.iter().chain(&q.audio_tracks).map(|t| t.items.len()).sum::<usize>(),
            })),
        })
    }
}
