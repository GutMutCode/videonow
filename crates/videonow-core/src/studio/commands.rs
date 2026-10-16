//! The studio's own commands (`studio:…`): creating and connecting assets, the project file,
//! rendering, undo, and the read-only views (commands, describe, inspect, journal, policy).

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::{Value, json};

use effectcraft_project::{Footage, FootageKind, ItemId as MotionItemId, ItemKind as MotionItemKind};
use filmcraft_media::SharedSource;

use super::{Studio, bad, u64_of};
use crate::assets::{self, AssetId, AssetKind};
use crate::bridge::{CompMediaSource, ImageMediaSource};
use crate::manifest::{self, Effect, studio_command};
use crate::receipt::Receipt;
use crate::{Result, StudioError};

impl Studio {
    pub(super) fn studio(&mut self, cmd: &str, p: Value, receipt: &mut Receipt) -> Result<Value> {
        let full = format!("studio:{cmd}");
        let spec = studio_command(cmd).ok_or_else(|| StudioError::UnknownCommand(full.clone()))?;
        if let Some(o) = p.as_object() {
            for k in o.keys() {
                if !spec.keys.contains(&k.as_str()) {
                    let expected = if spec.keys.is_empty() { "it takes no parameters".to_string() } else { format!("expected: {}", spec.keys.join(", ")) };
                    return Err(bad(&full, format!("unknown parameter `{k}` ({expected}); nothing ran")));
                }
            }
        }
        let f64_p = |k: &str| p.get(k).and_then(Value::as_f64);
        let u64_p = |k: &str| p.get(k).and_then(Value::as_u64);
        let str_p = |k: &str| p.get(k).and_then(Value::as_str);
        let bool_p = |k: &str| p.get(k).and_then(Value::as_bool);
        match cmd {
            "commands" => {
                let filter = str_p("filter").map(str::to_ascii_lowercase);
                let domain = str_p("domain");
                let effect = str_p("effect");
                let list: Vec<_> = manifest::all()
                    .into_iter()
                    .filter(|c| domain.is_none_or(|d| c.domain == d))
                    .filter(|c| effect.is_none_or(|e| c.effect.word().eq_ignore_ascii_case(e)))
                    .filter(|c| filter.as_ref().is_none_or(|f| c.id.to_ascii_lowercase().contains(f) || c.label.to_ascii_lowercase().contains(f)))
                    .collect();
                Ok(serde_json::to_value(list).unwrap_or(Value::Null))
            }
            "describe" => {
                let id = str_p("id").ok_or_else(|| bad(&full, "need `id` (`domain:command`)"))?;
                let c = manifest::find(id).ok_or_else(|| StudioError::UnknownCommand(id.to_string()))?;
                let keys = crate::params::documented_keys(&c.params).map(|k| k.into_iter().map(str::to_string).collect::<Vec<_>>());
                let allowed = self.policy.check_effect(id, c.effect).and_then(|()| if c.domain == "studio" { Ok(()) } else { self.policy.check_confinable(id, c.effect) });
                let mut v = serde_json::to_value(&c).unwrap_or(Value::Null);
                v["documentedKeys"] = json!(keys);
                v["allowedHere"] = json!(allowed.is_ok() && c.reserved.is_none());
                if let Err(e) = allowed {
                    v["refusedBecause"] = json!(e.to_string());
                }
                Ok(v)
            }
            "assets.list" => Ok(serde_json::to_value(self.assets.values().collect::<Vec<_>>()).unwrap_or(Value::Null)),
            "journal" => {
                let since = u64_p("since").unwrap_or(0);
                Ok(Value::Array(self.journal.iter().filter(|e| e["receipt"]["revision"]["after"].as_u64().unwrap_or(0) > since || since == 0).cloned().collect()))
            }
            "policy" => Ok(self.policy_json()),
            "inspect" => Ok(self.inspect()),
            "comp.new" => {
                let n = self.assets.len() + 1;
                let name = str_p("name").map(str::to_string).unwrap_or_else(|| format!("Composition {n}"));
                let (w, h) = (u64_p("width").unwrap_or(1920), u64_p("height").unwrap_or(1080));
                let (fps, dur) = (f64_p("fps").unwrap_or(30.0), f64_p("duration").unwrap_or(5.0));
                if !(1..=16384).contains(&w) || !(1..=16384).contains(&h) {
                    return Err(bad(&full, "width and height must be 1..16384"));
                }
                if !(fps.is_finite() && fps > 0.0 && fps <= 240.0) || !(dur.is_finite() && dur > 0.0 && dur <= 86_400.0) {
                    return Err(bad(&full, "fps must be in (0, 240] and duration in (0, 86400] seconds"));
                }
                let r = self
                    .motion
                    .execute("comp.new", json!({"name": name, "width": w, "height": h, "frameRate": fps, "duration": dur, "open": true}))
                    .map_err(|e| StudioError::Motion(e.to_string()))?;
                let comp = r.get("comp").and_then(u64_of).ok_or_else(|| StudioError::Other(format!("comp.new returned no composition id: {r}")))?;
                let id = self.add_asset(AssetKind::Composition, &name, Some(comp));
                receipt.created.push(id.0);
                receipt.asset = Some(id.0);
                Ok(json!({"asset": id.0, "comp": comp, "name": name}))
            }
            "image.new" => {
                let n = self.assets.len() + 1;
                let name = str_p("name").map(str::to_string).unwrap_or_else(|| format!("Image {n}"));
                let (w, h) = (u64_p("width").unwrap_or(1920), u64_p("height").unwrap_or(1080));
                let background = str_p("background").unwrap_or("transparent");
                self.image.execute("file.new", json!({"width": w, "height": h, "background": background, "name": name})).map_err(|e| StudioError::Image(e.to_string()))?;
                let id = self.adopt_active_image(&name)?;
                receipt.created.push(id.0);
                receipt.asset = Some(id.0);
                Ok(json!({"asset": id.0, "name": name, "width": w, "height": h}))
            }
            "image.open" => {
                let path = self.policy.resolve(&full, str_p("path").ok_or_else(|| bad(&full, "`path` must be an image file"))?, false)?;
                let stem = path.file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "Image".into());
                let name = str_p("name").map(str::to_string).unwrap_or(stem);
                let io = |e: &dyn std::fmt::Display| StudioError::Image(format!("{}: {e}", path.display()));
                let mut doc = if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("pcraft")) {
                    photocraft_format::load_path(&path).map_err(|e| io(&e))?
                } else {
                    let len = std::fs::metadata(&path).map_err(|e| io(&e))?.len();
                    if len > 4 << 30 {
                        return Err(io(&"larger than 4 GiB"));
                    }
                    let bytes = std::fs::read(&path).map_err(|e| io(&e))?;
                    let r = photocraft_io::import(&path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(), &bytes).map_err(|e| io(&e))?;
                    receipt.warnings.extend(r.warnings);
                    r.document
                };
                doc.name = name.clone();
                let i = self.image.add_document(doc, None);
                self.image.set_active(i);
                let id = self.adopt_active_image(&name)?;
                let (w, h) = self.image.active().map(|d| (d.doc.bounds().width(), d.doc.bounds().height())).unwrap_or((0, 0));
                receipt.created.push(id.0);
                receipt.asset = Some(id.0);
                Ok(json!({"asset": id.0, "name": name, "width": w, "height": h, "from": path.to_string_lossy()}))
            }
            "asset.open" => {
                let id = p.get("asset").and_then(u64_of).ok_or_else(|| bad(&full, "`asset` must be an asset id"))?;
                let a = self.assets.get(&AssetId(id)).cloned().ok_or(StudioError::NoAsset(id))?;
                let workspace = match a.kind {
                    AssetKind::Composition => {
                        let comp = a.comp.ok_or_else(|| StudioError::Other(format!("asset {id} has no composition")))?;
                        self.motion.execute("comp.open", json!({"comp": comp})).map_err(|e| StudioError::Motion(e.to_string()))?;
                        "motion"
                    }
                    AssetKind::Image => {
                        self.activate_image(AssetId(id))?;
                        "image"
                    }
                };
                receipt.asset = Some(id);
                Ok(json!({"asset": id, "name": a.name, "workspace": workspace}))
            }
            "asset.place" => {
                let id = p.get("asset").and_then(u64_of).ok_or_else(|| bad(&full, "`asset` must be an asset id"))?;
                let a = self.assets.get(&AssetId(id)).cloned().ok_or(StudioError::NoAsset(id))?;
                receipt.asset = Some(id);
                let item = match a.timeline_item.filter(|i| self.timeline.project.item(filmcraft_engine::project::ItemId(*i)).is_some()) {
                    Some(i) => i,
                    None => {
                        self.publish();
                        let src: SharedSource = match a.kind {
                            AssetKind::Composition => {
                                let comp = a.comp.ok_or_else(|| StudioError::Other(format!("asset {id} has no composition")))?;
                                Arc::new(
                                    CompMediaSource::new(&a.name, MotionItemId(comp), self.shared_motion.clone(), self.generation.clone(), self.footage.clone())
                                        .ok_or_else(|| StudioError::Other(format!("composition {comp} not found")))?,
                                )
                            }
                            AssetKind::Image => Arc::new(
                                ImageMediaSource::new(&a.name, a.id, self.shared_images.clone()).ok_or_else(|| StudioError::Other(format!("image {id} has no document")))?,
                            ),
                        };
                        let uri = assets::uri(a.kind, a.id);
                        let it = filmcraft_engine::commands::import_nested(&mut self.timeline, &uri, &a.name, src, None).map_err(|e| StudioError::Timeline(e.to_string()))?;
                        if let Some(x) = self.assets.get_mut(&a.id) {
                            x.timeline_item = Some(it.0);
                        }
                        it.0
                    }
                };
                let mut tp = json!({"item": item, "seconds": f64_p("seconds").unwrap_or(0.0)});
                if let Some(t) = p.get("track") {
                    tp["track"] = t.clone();
                }
                let placed = self.timeline.execute("timeline.place", tp).map_err(|e| StudioError::Timeline(e.to_string()))?;
                Ok(json!({"item": item, "placed": placed}))
            }
            "image.toComp" => {
                let img = self.asset_of(&full, p.get("image"), "image", AssetKind::Image)?;
                let comp_a = self.asset_of(&full, p.get("comp"), "comp", AssetKind::Composition)?;
                receipt.asset = Some(comp_a.id.0);
                let comp = MotionItemId(comp_a.comp.ok_or_else(|| StudioError::Other(format!("asset {} has no composition", comp_a.id.0)))?);
                let footage_item = match img.motion_footage.filter(|f| self.motion.project.item(MotionItemId(*f)).is_some()) {
                    Some(f) => f,
                    None => {
                        let (w, h) = self
                            .image_doc(img.id)
                            .map(|d| (d.doc.bounds().width(), d.doc.bounds().height()))
                            .ok_or(StudioError::NoAsset(img.id.0))?;
                        let c = self.motion.project.comp(comp).ok_or_else(|| StudioError::Other(format!("composition {} not found", comp.0)))?;
                        let f = Footage {
                            path: assets::uri(AssetKind::Image, img.id),
                            kind: FootageKind::Still,
                            width: w,
                            height: h,
                            pixel_aspect: 1.0,
                            frame_rate: c.frame_rate,
                            duration: c.duration,
                            has_video: true,
                            ..Default::default()
                        };
                        let name = img.name.clone();
                        let fid = self
                            .motion
                            .edit(&format!("Add {name}"), None, |pr, _| Ok(pr.add_item(&name, effectcraft_color::Label::Aqua, None, MotionItemKind::Footage(f))))
                            .map_err(|e| StudioError::Motion(e.to_string()))?;
                        if let Some(x) = self.assets.get_mut(&img.id) {
                            x.motion_footage = Some(fid.0);
                        }
                        fid.0
                    }
                };
                let layer = self
                    .motion
                    .execute("layer.addItem", json!({"comp": comp.0, "item": footage_item, "time": f64_p("time").unwrap_or(0.0)}))
                    .map_err(|e| StudioError::Motion(e.to_string()))?;
                Ok(json!({"footage": footage_item, "layer": layer}))
            }
            "render.frame" => {
                let sec = f64_p("seconds").unwrap_or(0.0);
                let scale = f64_p("scale").unwrap_or(1.0);
                let path = self.policy.resolve(&full, str_p("path").unwrap_or("frame.png"), true)?;
                let (w, h, rgba, offline) = self.render_rgba(sec, scale)?;
                if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
                    std::fs::create_dir_all(dir).map_err(|e| StudioError::Other(format!("{}: {e}", dir.display())))?;
                }
                image::save_buffer(&path, &rgba, w, h, image::ColorType::Rgba8).map_err(|e| StudioError::Other(format!("{}: {e}", path.display())))?;
                for o in &offline {
                    receipt.warnings.push(format!("{o} is offline; the frame shows its slate"));
                }
                Ok(json!({"path": path.to_string_lossy(), "width": w, "height": h, "seconds": sec, "offline": offline}))
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
                        if self.activate_image(AssetId(id)).is_ok() {
                            self.image.execute("edit.undo", json!({})).map_err(|e| StudioError::Image(e.to_string()))?;
                        }
                    } else {
                        // the asset table itself is not under undo
                        receipt.warnings.push("the asset table (which assets exist and where they are placed) is not undone".into());
                        continue;
                    }
                    undone.push(d.clone());
                }
                Ok(json!({"undone": undone}))
            }
            "project.save" => {
                let target = match str_p("path") {
                    Some(s) => self.policy.resolve(&full, s, true)?,
                    None => self.path.clone().ok_or_else(|| bad(&full, "the project has not been saved yet: give a `path` (a folder like `Film.videonow`)"))?,
                };
                let target = if self.policy.confined() && str_p("path").is_none() { self.policy.resolve(&full, &target.to_string_lossy(), true)? } else { target };
                if str_p("path").is_some()
                    && let Some(stem) = target.file_stem().map(|s| s.to_string_lossy().into_owned())
                {
                    self.name = stem;
                }
                let saved = self.save_project(&target)?;
                receipt.checked_extra("saved files read back (timeline and motion decoded, image bundles' manifests read)");
                for r in &saved.removed {
                    receipt.notes.push(format!("removed {r} (its asset is no longer in the project)"));
                }
                Ok(json!({
                    "path": saved.path.to_string_lossy(),
                    "files": saved.files.iter().map(|(f, n)| json!({"file": f, "bytes": n})).collect::<Vec<_>>(),
                    "removed": saved.removed,
                }))
            }
            "project.open" => {
                let path = self.policy.resolve(&full, str_p("path").ok_or_else(|| bad(&full, "need `path`"))?, false)?;
                self.refuse_unsaved(&full, bool_p("discard").unwrap_or(false))?;
                let r = self.open_project(&path)?;
                receipt.checked_extra("project files decoded; every composition the asset table names exists");
                Ok(r)
            }
            "project.new" => {
                self.refuse_unsaved(&full, bool_p("discard").unwrap_or(false))?;
                let mut next = Studio::with_policy(self.policy.clone());
                next.name = str_p("name").unwrap_or("Untitled").to_string();
                next.revision = self.revision + 1;
                next.saved_revision = next.revision;
                next.epoch = self.epoch + 1;
                *self = next;
                Ok(json!({"name": self.name}))
            }
            _ => Err(StudioError::UnknownCommand(full)),
        }
    }

    fn refuse_unsaved(&self, cmd: &str, discard: bool) -> Result<()> {
        if self.is_dirty() && !discard {
            return Err(StudioError::Other(format!(
                "{cmd}: the project has unsaved changes (revision {} of {}, saved at {}); save it first (studio:project.save) or pass `discard: true`; nothing changed",
                self.revision, self.name, self.saved_revision
            )));
        }
        Ok(())
    }

    pub(crate) fn policy_json(&self) -> Value {
        let mut v = serde_json::to_value(&self.policy).unwrap_or(Value::Null);
        v["confined"] = json!(self.policy.confined());
        v["allowedEffects"] = json!(match &self.policy.allow {
            None => vec!["read", "edit", "destructive", "fileWrite"],
            Some(a) => a.iter().map(|e| e.word()).collect(),
        });
        v
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
                    "onTimeline": a.timeline_item,
                })
            })
            .collect();
        let images: Vec<Value> = self
            .assets
            .values()
            .filter(|a| a.kind == AssetKind::Image)
            .map(|a| {
                let d = self.image_doc(a.id);
                json!({
                    "asset": a.id.0, "name": a.name,
                    "width": d.map(|d| d.doc.bounds().width()), "height": d.map(|d| d.doc.bounds().height()),
                    "layers": d.map(|d| d.doc.layer_count()),
                    "onTimeline": a.timeline_item, "inComps": a.motion_footage.is_some(),
                })
            })
            .collect();
        let seq_id = self.timeline.state.active_sequence;
        let seq_name = seq_id.and_then(|id| self.timeline.project.item(id)).map(|i| i.name.clone());
        json!({
            "project": {
                "name": self.name,
                "path": self.path.as_ref().map(|p| p.to_string_lossy().into_owned()),
                "unsavedChanges": self.is_dirty(),
                "revision": self.revision,
                "savedRevision": self.saved_revision,
            },
            "revision": self.revision,
            "compositions": comps,
            "images": images,
            "sequence": self.timeline.active_sequence().map(|q| json!({
                "item": seq_id.map(|i| i.0), "name": seq_name, "width": q.settings.width, "height": q.settings.height,
                "fps": q.settings.frame_rate.num as f64 / q.settings.frame_rate.den.max(1) as f64,
                "videoTracks": q.video_tracks.len(), "audioTracks": q.audio_tracks.len(),
                "clips": q.video_tracks.iter().chain(&q.audio_tracks).map(|t| t.items.len()).sum::<usize>(),
                "duration": q.duration().0 as f64 / filmcraft_time::TICKS_PER_SECOND as f64,
            })),
            "policy": self.policy_json(),
        })
    }

    /// Render the active sequence at `seconds` into `path` (helper for tools and tests).
    pub fn render_to(&self, seconds: f64, scale: f64, path: &std::path::Path) -> Result<PathBuf> {
        let (w, h, rgba, _) = self.render_rgba(seconds, scale)?;
        image::save_buffer(path, &rgba, w, h, image::ColorType::Rgba8).map_err(|e| StudioError::Other(format!("{}: {e}", path.display())))?;
        Ok(path.to_path_buf())
    }

    /// Effect class of a command, for clients deciding whether to ask the person first.
    pub fn effect_of(&self, id: &str) -> Option<Effect> {
        manifest::find(id).map(|c| c.effect)
    }
}
