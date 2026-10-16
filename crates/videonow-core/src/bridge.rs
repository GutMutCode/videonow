//! Bridges between the engines: each engine shows the others' work through the source interface
//! it already has.
//!
//! * The timeline (FilmCraft) reads media through `filmcraft_media::MediaSource`. A composition
//!   placed on a sequence is a [`CompMediaSource`]; an image document is an [`ImageMediaSource`].
//! * The motion engine (EffectCraft) reads footage through `effectcraft_render::FootageSource`.
//!   [`StudioFootage`] wraps its media pool so an image document used inside a composition renders
//!   from the image engine (PhotoCraft) instead of a file.
//!
//! The engines run on the studio's thread; renders may run anywhere (the timeline renders on
//! background threads during playback and export). So each engine publishes an immutable
//! snapshot of its document after every command ([`SharedMotion`], [`SharedImages`]), and the
//! sources render from the latest snapshot. Every snapshot carries a revision, which keys the
//! sources' frame caches: an edit is visible on the next frame without any explicit flush.

use std::collections::HashMap;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, RwLock};

use filmcraft_color::ColorInfo;
use filmcraft_frame::{AudioBuffer, VideoFrame};
use filmcraft_media::{FrameRequest, MediaError, MediaInfo, MediaKind, MediaSource, VideoStreamInfo};
use filmcraft_time::{FrameRate, Tick};

use effectcraft_project::{Footage, ItemId as MotionItemId, Project as MotionProject};
use effectcraft_render::{FloatCurve, FootageSource, Image as MotionImage, RenderOpts, Renderer};

use crate::assets::{AssetId, AssetKind, parse_uri};

/// The motion engine's project as of its last command.
#[derive(Clone)]
pub struct MotionSnapshot {
    pub project: Arc<MotionProject>,
    pub revision: u64,
}

/// The image engine's documents as of their last commands, by asset.
#[derive(Clone)]
pub struct ImageSnapshot {
    pub doc: Arc<photocraft_doc::Document>,
    pub revision: u64,
}

pub type SharedMotion = Arc<RwLock<MotionSnapshot>>;
pub type SharedImages = Arc<RwLock<HashMap<AssetId, ImageSnapshot>>>;
/// Moves whenever any engine publishes a new snapshot. A composition's frame depends on the
/// motion project *and* on every image nested in it, so its cache keys on this, not on the motion
/// revision alone.
pub type Generation = Arc<std::sync::atomic::AtomicU64>;

fn read<T: Clone>(l: &RwLock<T>) -> T {
    l.read().unwrap_or_else(|e| e.into_inner()).clone()
}

/// An image document flattened to straight-alpha 8-bit RGBA (the document's own RGB encoding;
/// documents are sRGB by default).
pub fn image_rgba8(doc: &photocraft_doc::Document) -> (u32, u32, Vec<u8>) {
    let img = photocraft_compose::flatten(doc).to_rgba8();
    (img.width, img.height, img.pixels)
}

/// An image document as the motion engine's footage pixels: premultiplied float, sRGB-encoded
/// values (what `effectcraft_raster::Image` holds for 8-bit footage).
pub fn image_for_motion(doc: &photocraft_doc::Document) -> MotionImage {
    let buf = photocraft_compose::flatten(doc);
    let (w, h) = (buf.rect.width(), buf.rect.height());
    let data = buf
        .px
        .iter()
        .map(|p| {
            let a = p[3].clamp(0.0, 1.0);
            [p[0].clamp(0.0, 1.0) * a, p[1].clamp(0.0, 1.0) * a, p[2].clamp(0.0, 1.0) * a, a]
        })
        .collect();
    MotionImage { width: w, height: h, data }
}

pub(crate) fn film_rate(r: effectcraft_time::FrameRate) -> FrameRate {
    if r.num > 0 && r.den > 0 { FrameRate::new(r.num, r.den) } else { FrameRate::FPS_30 }
}

fn video_info(name: &str, kind: MediaKind, codec: &str, w: u32, h: u32, rate: FrameRate, duration: Tick) -> MediaInfo {
    MediaInfo {
        name: name.to_string(),
        kind,
        duration,
        video: Some(VideoStreamInfo {
            width: w.max(1),
            height: h.max(1),
            frame_rate: rate,
            par: (1, 1),
            codec: codec.to_string(),
            pixel_format: "RGBA 8-bit".into(),
            color: ColorInfo::SRGB_FULL,
            has_alpha: true,
            bitrate: None,
            hdr: None,
        }),
        audio_streams: vec![],
        container: "Studio".into(),
        start_timecode: None,
        file_size: None,
    }
}

/// Small frame cache keyed by (revision, frame, scale bucket).
#[derive(Default)]
struct Frames {
    entries: Vec<((u64, i64, u16), Arc<VideoFrame>)>,
}

impl Frames {
    const CAP: usize = 48;
    fn get(&self, k: (u64, i64, u16)) -> Option<Arc<VideoFrame>> {
        self.entries.iter().find(|(key, _)| *key == k).map(|(_, f)| f.clone())
    }
    fn put(&mut self, k: (u64, i64, u16), f: Arc<VideoFrame>) {
        // a new revision makes every older frame stale
        self.entries.retain(|(key, _)| key.0 == k.0);
        if self.entries.len() >= Self::CAP {
            self.entries.remove(0);
        }
        self.entries.push((k, f));
    }
}

/// A composition as timeline media: each frame is rendered by the motion engine.
pub struct CompMediaSource {
    comp: MotionItemId,
    motion: SharedMotion,
    generation: Generation,
    footage: Arc<dyn FootageSource>,
    info: MediaInfo,
    cache: Mutex<Frames>,
}

impl CompMediaSource {
    /// `None` when the snapshot has no such composition.
    pub fn new(name: &str, comp: MotionItemId, motion: SharedMotion, generation: Generation, footage: Arc<dyn FootageSource>) -> Option<Self> {
        let snap = read(&motion);
        let c = snap.project.comp(comp)?;
        let info = video_info(name, MediaKind::Movie, "Composition", c.width, c.height, film_rate(c.frame_rate), Tick(c.duration.0));
        Some(Self { comp, motion, generation, footage, info, cache: Mutex::new(Frames::default()) })
    }
}

impl MediaSource for CompMediaSource {
    fn info(&self) -> &MediaInfo {
        &self.info
    }

    fn video_frame(&self, req: FrameRequest) -> Result<Arc<VideoFrame>, MediaError> {
        // read the generation first: a snapshot published after this read gets a newer key
        let generation = self.generation.load(std::sync::atomic::Ordering::Acquire);
        let snap = read(&self.motion);
        let c = snap.project.comp(self.comp).ok_or_else(|| MediaError::Offline(format!("{}: the composition was deleted", self.info.name)))?;
        let rate = film_rate(c.frame_rate);
        let frame = rate.frame_at(req.time.max(Tick::ZERO));
        let scale = req.scale.clamp(1.0 / 16.0, 1.0);
        let bucket = (scale * 64.0).round() as u16;
        let key = (generation, frame, bucket);
        if let Some(f) = self.cache.lock().unwrap_or_else(|e| e.into_inner()).get(key) {
            return Ok(f);
        }
        let t = rate.tick_of(frame);
        let mut r = Renderer::new(&snap.project, self.footage.as_ref(), RenderOpts { scale: f64::from(bucket) / 64.0, ..Default::default() });
        r.expr = Some(&effectcraft_expr::Expressions);
        let img = r.comp_frame(self.comp, effectcraft_time::Tick(t.0));
        let f = Arc::new(VideoFrame::rgba8(img.width, img.height, img.to_rgba8()).with_pts(t));
        self.cache.lock().unwrap_or_else(|e| e.into_inner()).put(key, f.clone());
        Ok(f)
    }

    fn audio(&self, _start: i64, _frames: usize, _sample_rate: u32) -> Result<AudioBuffer, MediaError> {
        // composition audio joins the timeline mix in a later phase (docs: render pipeline)
        Err(MediaError::NoStream("audio"))
    }
}

/// An image document as timeline media: a still rendered by the image engine.
pub struct ImageMediaSource {
    asset: AssetId,
    images: SharedImages,
    info: MediaInfo,
    cache: Mutex<Frames>,
}

impl ImageMediaSource {
    /// `None` when the snapshot has no such image. Stills report 5 s, the timeline's default.
    pub fn new(name: &str, asset: AssetId, images: SharedImages) -> Option<Self> {
        let doc = read(&images).get(&asset)?.doc.clone();
        let b = doc.bounds();
        let info = video_info(name, MediaKind::Still, "PhotoCraft document", b.width(), b.height(), FrameRate::FPS_30, Tick::from_seconds_f64(5.0));
        Some(Self { asset, images, info, cache: Mutex::new(Frames::default()) })
    }
}

impl MediaSource for ImageMediaSource {
    fn info(&self) -> &MediaInfo {
        &self.info
    }

    fn video_frame(&self, _req: FrameRequest) -> Result<Arc<VideoFrame>, MediaError> {
        let snap = read(&self.images).get(&self.asset).cloned().ok_or_else(|| MediaError::Offline(format!("{}: the image was deleted", self.info.name)))?;
        let key = (snap.revision, 0, 64);
        if let Some(f) = self.cache.lock().unwrap_or_else(|e| e.into_inner()).get(key) {
            return Ok(f);
        }
        let (w, h, px) = image_rgba8(&snap.doc);
        let f = Arc::new(VideoFrame::rgba8(w, h, px));
        self.cache.lock().unwrap_or_else(|e| e.into_inner()).put(key, f.clone());
        Ok(f)
    }

    fn audio(&self, _start: i64, _frames: usize, _sample_rate: u32) -> Result<AudioBuffer, MediaError> {
        Err(MediaError::NoStream("audio"))
    }
}

/// The motion engine's footage source: `studio://image/<id>` footage renders from the image
/// engine; everything else goes to the wrapped media pool unchanged.
pub struct StudioFootage {
    inner: Arc<dyn FootageSource>,
    images: SharedImages,
    cache: Mutex<HashMap<AssetId, (u64, Arc<MotionImage>)>>,
}

impl StudioFootage {
    pub fn new(inner: Arc<dyn FootageSource>, images: SharedImages) -> Self {
        Self { inner, images, cache: Mutex::new(HashMap::new()) }
    }

    fn image(&self, footage: &Footage) -> Option<Option<Arc<MotionImage>>> {
        let (AssetKind::Image, asset) = parse_uri(&footage.path)? else { return Some(None) };
        let Some(snap) = read(&self.images).get(&asset).cloned() else { return Some(None) };
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((rev, img)) = cache.get(&asset)
            && *rev == snap.revision
        {
            return Some(Some(img.clone()));
        }
        let img = Arc::new(image_for_motion(&snap.doc));
        cache.insert(asset, (snap.revision, img.clone()));
        Some(Some(img))
    }
}

impl FootageSource for StudioFootage {
    fn frame(&self, item: MotionItemId, footage: &Footage, t: effectcraft_time::Tick) -> Option<Arc<MotionImage>> {
        match self.image(footage) {
            Some(img) => img,
            None => self.inner.frame(item, footage, t),
        }
    }
    fn frame_at_size(&self, item: MotionItemId, footage: &Footage, t: effectcraft_time::Tick, width: u32, height: u32) -> Option<Arc<MotionImage>> {
        match self.image(footage) {
            // the full frame is allowed here; the renderer resamples it
            Some(img) => img,
            None => self.inner.frame_at_size(item, footage, t, width, height),
        }
    }
    fn frame_with(&self, item: MotionItemId, footage: &Footage, t: effectcraft_time::Tick, size: Option<(u32, u32)>, float: FloatCurve) -> Option<Arc<MotionImage>> {
        match self.image(footage) {
            Some(img) => img,
            None => self.inner.frame_with(item, footage, t, size, float),
        }
    }
    fn audio(&self, item: MotionItemId, footage: &Footage, t: effectcraft_time::Tick, frames: usize, rate: u32) -> Option<Vec<f32>> {
        self.inner.audio(item, footage, t, frames, rate)
    }
    fn aux(&self, item: MotionItemId, footage: &Footage, t: effectcraft_time::Tick) -> Option<Arc<effectcraft_raster::AuxChannels>> {
        self.inner.aux(item, footage, t)
    }
    fn model(&self, item: MotionItemId, footage: &Footage) -> Option<Arc<effectcraft_model::Model>> {
        self.inner.model(item, footage)
    }
    fn set_cache_budget(&self, bytes: usize) {
        self.inner.set_cache_budget(bytes)
    }
    fn purge(&self) {
        self.cache.lock().unwrap_or_else(|e| e.into_inner()).clear();
        self.inner.purge()
    }
    fn forget(&self, path: &str) {
        self.inner.forget(path)
    }
    fn set_conform_folder(&self, folder: Option<std::path::PathBuf>) {
        self.inner.set_conform_folder(folder)
    }
    fn cache_budget(&self) -> Option<usize> {
        self.inner.cache_budget()
    }
    fn vector_frame(&self, item: MotionItemId, footage: &Footage, scale: f64) -> Option<Arc<MotionImage>> {
        self.inner.vector_frame(item, footage, scale)
    }
    fn cancellable(&self, cancel: &Arc<AtomicBool>, f: &mut dyn FnMut()) {
        self.inner.cancellable(cancel, f)
    }
    fn called_off(&self) -> bool {
        self.inner.called_off()
    }
    fn problem(&self, footage: &Footage) -> Option<String> {
        if parse_uri(&footage.path).is_some() { None } else { self.inner.problem(footage) }
    }
}
