//! Changes made in the engines' own editors.
//!
//! In the desktop application a person edits with each engine's editor (the timeline, motion and
//! image workspaces). Those editors run the engines' commands directly, so after each frame the
//! application calls [`Studio::sync_editors`], which does for them what [`Studio::execute`] does
//! for a command:
//!
//! * documents the editors created become assets (a composition made in the motion workspace, an
//!   image opened in the image workspace) and are offered to the other engines;
//! * image documents whose tab was closed are kept (an image of the project is never lost by
//!   closing its tab; opening the asset again brings it back);
//! * every engine's latest document is published to the others' renders;
//! * the change gets a receipt (`ui:<workspace>`, listing the engine commands the editor ran),
//!   joins the journal, and moves the studio revision.

use std::collections::HashSet;

use serde_json::{Value, json};

use crate::assets::{AssetId, AssetKind};
use crate::receipt::{NOT_CHECKED_VISUAL, Receipt, Revisions, UI_CHECKED};
use crate::studio::{Studio, read};

/// Engine commands that move through an engine's history rather than change the work: a studio
/// undo after them would undo the wrong step, so changes that contain them are not on the
/// studio's undo stack.
fn is_history_command(id: &str) -> bool {
    let cmd = id.split_once(':').map_or(id, |(_, c)| c);
    cmd.starts_with("edit.undo") || cmd.starts_with("edit.redo") || cmd.starts_with("history.") || cmd == "edit.stepBackward" || cmd == "edit.stepForward"
}

impl Studio {
    /// Account for what the engines' editors changed since the last command or sync. `source`
    /// names where (`edit`, `motion`, `image`, `external`). Returns the receipt, or `None` when
    /// nothing changed.
    pub fn sync_editors(&mut self, source: &str) -> Option<Value> {
        let created = self.adopt();
        self.park_closed();
        self.follow_names();
        self.publish();
        let now = self.marks();
        let changed = Self::changed(&self.last, &now);
        let commands = self.editor_commands();
        self.last = now;
        self.seen = self.journal_marks();
        if changed.is_empty() {
            return None;
        }
        let rev_before = self.revision;
        self.revision += 1;
        let by = if source == "external" { self.actor.clone() } else { "ui".to_string() };
        let mut receipt = Receipt {
            command: format!("ui:{source}"),
            domain: "ui".into(),
            by,
            commands,
            revision: Revisions { before: rev_before, after: self.revision },
            changed,
            created,
            checked: UI_CHECKED.to_vec(),
            ..Default::default()
        };
        if receipt.changed.iter().any(|c| c != "studio") {
            receipt.not_checked.extend_from_slice(NOT_CHECKED_VISUAL);
        }
        if !receipt.commands.is_empty() && !receipt.commands.iter().any(|c| is_history_command(c)) {
            self.undo.push(receipt.changed.clone());
        }
        let v = json!(receipt);
        self.journal.push(json!({"command": receipt.command, "receipt": v}));
        Some(v)
    }

    /// Engine commands run since the studio last looked, `domain:id`, oldest first.
    fn editor_commands(&self) -> Vec<String> {
        let now = self.journal_marks();
        let mut out = Vec::new();
        let mut take = |domain: &str, journal: &[(String, Value)], seen: usize, len: usize| {
            // the engines trim their journals (oldest 1000 at 10 000): never index past the end
            let from = if len >= seen { seen } else { len.saturating_sub(1) };
            for (id, _) in journal.get(from..).unwrap_or(&[]) {
                out.push(format!("{domain}:{id}"));
            }
        };
        take("timeline", &self.timeline.journal, self.seen.timeline, now.timeline);
        take("motion", &self.motion.journal, self.seen.motion, now.motion);
        take("image", &self.images.journal, self.seen.images, now.images);
        out
    }

    /// Compositions and image documents the engines hold that are not assets yet become assets,
    /// and are offered to the other engines. Returns the new asset ids.
    pub(crate) fn adopt(&mut self) -> Vec<u64> {
        let mut created = Vec::new();
        let known_comps: HashSet<u64> = self.assets.values().filter_map(|a| a.comp).collect();
        let new_comps: Vec<(u64, String)> =
            self.motion.project.items.iter().filter(|(id, it)| it.as_comp().is_some() && !known_comps.contains(&id.0)).map(|(id, it)| (id.0, it.name.clone())).collect();
        for (comp, name) in new_comps {
            created.push(self.add_asset(AssetKind::Composition, &name, Some(comp)));
        }
        let known_docs: HashSet<photocraft_doc::DocId> = self.image_docs.values().copied().collect();
        // documents opened to edit a smart object's contents belong to their parent document
        let smart: HashSet<photocraft_doc::DocId> = self.images.smart_links.iter().map(|l| l.child).collect();
        let new_docs: Vec<(photocraft_doc::DocId, String)> =
            self.images.documents().iter().filter(|d| !known_docs.contains(&d.doc.id) && !smart.contains(&d.doc.id)).map(|d| (d.doc.id, d.doc.name.clone())).collect();
        for (doc, name) in new_docs {
            let name = if name.is_empty() { format!("Image {}", self.next_asset) } else { name };
            let id = self.add_asset(AssetKind::Image, &name, None);
            self.image_docs.insert(id, doc);
            created.push(id);
        }
        if created.is_empty() {
            return Vec::new();
        }
        self.publish();
        for id in &created {
            for w in self.link_asset(*id) {
                log::warn!("{w}");
            }
        }
        created.into_iter().map(|a| a.0).collect()
    }

    /// Image assets whose document is no longer open keep their last published state.
    fn park_closed(&mut self) {
        let open: HashSet<photocraft_doc::DocId> = self.images.documents().iter().map(|d| d.doc.id).collect();
        let closed: Vec<AssetId> = self.image_docs.iter().filter(|(a, d)| !open.contains(d) && !self.parked.contains_key(a)).map(|(a, _)| *a).collect();
        if closed.is_empty() {
            return;
        }
        let snaps = read(&self.shared_images);
        for a in closed {
            if let Some(s) = snaps.get(&a) {
                self.parked.insert(a, s.doc.clone());
            }
        }
    }

    /// Asset names follow renames made in the editors (a composition renamed in the motion
    /// workspace).
    fn follow_names(&mut self) {
        let renames: Vec<(AssetId, String)> = self
            .assets
            .values()
            .filter_map(|a| match a.kind {
                AssetKind::Composition => {
                    let it = self.motion.project.items.get(&effectcraft_project::ItemId(a.comp?))?;
                    (it.name != a.name).then(|| (a.id, it.name.clone()))
                }
                AssetKind::Image => {
                    let d = self.image_index(a.id).and_then(|i| self.images.documents().get(i))?;
                    (!d.doc.name.is_empty() && d.doc.name != a.name).then(|| (a.id, d.doc.name.clone()))
                }
            })
            .collect();
        for (id, name) in renames {
            if let Some(a) = self.assets.get_mut(&id) {
                a.name = name;
            }
        }
    }
}
