//! Edits a person makes in an engine's own interface. The desktop app hosts each engine's UI on
//! the studio's session of that engine; those edits do not go through [`Studio::execute`], so
//! after each frame the app calls [`Studio::absorb_ui`], which records them like commands (a
//! revision, an undo step, a journal entry with `"source": "ui"`) and keeps the project whole:
//! image documents the UI opened become assets, and an image of the project closed in the UI is
//! opened again from its last snapshot.

use serde_json::{Value, json};

use super::{EngineMarks, Studio};
use crate::assets::{AssetId, AssetKind};
use crate::receipt::{Receipt, Revisions};

/// Where each engine's own command journal stood (its length).
#[derive(Clone, Copy, Default, PartialEq, Debug)]
pub(crate) struct JournalMarks {
    timeline: usize,
    motion: usize,
    image: usize,
}

/// The entries `journal` gained since it had `before` of them (it drops its oldest entries when
/// long, so a shorter journal means "everything recent").
fn since(journal: &[(String, Value)], before: usize) -> Vec<(String, Value)> {
    let from = if journal.len() >= before { before } else { journal.len().saturating_sub(50) };
    journal.get(from..).unwrap_or_default().to_vec()
}

/// Studio: the engine commands the studio refuses when an engine's UI asks for them (the agent
/// path refuses them in [`Studio::execute`]). Install with the engine sessions' `host_guard`.
pub fn timeline_guard(id: &str) -> Option<&'static str> {
    crate::manifest::reserved(&format!("timeline:{id}"))
}
pub fn motion_guard(id: &str) -> Option<&'static str> {
    crate::manifest::reserved(&format!("motion:{id}"))
}
pub fn image_guard(id: &str) -> Option<&'static str> {
    crate::manifest::reserved(&format!("image:{id}"))
}

impl Studio {
    pub(crate) fn journal_marks(&self) -> JournalMarks {
        JournalMarks { timeline: self.timeline.journal.len(), motion: self.motion.journal.len(), image: self.image.journal.len() }
    }

    /// Remember where every engine stands now: the next [`Studio::absorb_ui`] reports only what
    /// changed after this.
    pub(crate) fn settle(&mut self) {
        self.settled = Some((self.marks(), self.journal_marks()));
    }

    /// Record what an engine's UI changed since the last command or absorb. `workspace` names the
    /// UI (`edit`, `motion`, `image`). Returns the journal entry, or `None` when nothing changed.
    pub fn absorb_ui(&mut self, workspace: &str) -> Option<Value> {
        let (before, jm) = match self.settled.take() {
            Some(s) => s,
            None => (self.marks(), self.journal_marks()),
        };
        let mut warnings = Vec::new();
        let mut created = Vec::new();
        self.keep_project_images(&mut warnings);
        self.adopt_ui_images(&mut created);
        self.publish();
        let after: EngineMarks = self.marks();
        let changed = Self::changed(&before, &after);
        let commands: Vec<Value> = [("timeline", since(&self.timeline.journal, jm.timeline)), ("motion", since(&self.motion.journal, jm.motion)), ("image", since(&self.image.journal, jm.image))]
            .into_iter()
            .flat_map(|(d, v)| v.into_iter().map(move |(id, p)| json!({"id": format!("{d}:{id}"), "params": p})))
            .collect();
        self.settle();
        if changed.is_empty() && warnings.is_empty() {
            return None;
        }
        let rev_before = self.revision;
        if !changed.is_empty() {
            self.revision += 1;
            self.undo.push(changed.clone());
        }
        let receipt = Receipt {
            command: format!("ui:{workspace}"),
            domain: workspace.to_string(),
            changed,
            created,
            revision: Revisions { before: rev_before, after: self.revision },
            checked: vec!["made in the engine's own interface by the person using it"],
            warnings,
            ..Default::default()
        };
        let entry = json!({"command": format!("ui:{workspace}"), "source": "ui", "commands": commands, "receipt": receipt});
        self.journal.push(entry.clone());
        Some(entry)
    }

    /// Image documents opened in the image UI (File ▸ Open, New, from the clipboard…) become
    /// image assets.
    fn adopt_ui_images(&mut self, created: &mut Vec<u64>) {
        let known: Vec<photocraft_doc::DocId> = self.image_docs.values().copied().collect();
        let new: Vec<(photocraft_doc::DocId, String)> = self.image.documents().iter().filter(|d| !known.contains(&d.doc.id)).map(|d| (d.doc.id, d.doc.name.clone())).collect();
        for (doc, name) in new {
            let name = if name.trim().is_empty() { format!("Image {}", self.next_asset) } else { name };
            let id = self.add_asset(AssetKind::Image, &name, None);
            self.image_docs.insert(id, doc);
            created.push(id.0);
        }
    }

    /// An image asset's document closed in the UI: open it again from its last published
    /// snapshot, so nothing nested in a composition or the timeline goes offline.
    fn keep_project_images(&mut self, warnings: &mut Vec<String>) {
        let missing: Vec<AssetId> = self.image_docs.iter().filter(|(_, d)| !self.image.documents().iter().any(|s| s.doc.id == **d)).map(|(a, _)| *a).collect();
        for id in missing {
            let snap = self.shared_images.read().unwrap_or_else(|e| e.into_inner()).get(&id).map(|s| s.doc.clone());
            let name = self.assets.get(&id).map(|a| a.name.clone()).unwrap_or_default();
            match snap {
                Some(doc) => {
                    let i = self.image.add_document((*doc).clone(), None);
                    if let Some(d) = self.image.documents().get(i).map(|d| d.doc.id) {
                        self.image_docs.insert(id, d);
                    }
                    warnings.push(format!("“{name}” is part of the project, so it stays open (its undo history starts again); remove it from the project to close it"));
                }
                None => warnings.push(format!("“{name}” was closed and had no saved state to reopen; it is offline")),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Studio;

    #[test]
    fn ui_edits_become_journal_entries() {
        let mut s = Studio::new();
        let r = s.execute("studio:image.new", json!({"width": 32, "height": 32})).unwrap();
        let asset = r["result"]["asset"].as_u64().unwrap();
        let rev = s.revision();
        assert!(s.absorb_ui("image").is_none(), "nothing happened in the UI");
        // what the image UI does: run engine commands on the session directly
        s.image.execute("shape.create", json!({"kind": "rect", "rect": [0, 0, 8, 8], "fill": "#ff0000"})).unwrap();
        let e = s.absorb_ui("image").unwrap();
        assert_eq!(e["source"], "ui");
        assert_eq!(e["receipt"]["changed"], json!([format!("image:{asset}")]));
        assert_eq!(e["commands"][0]["id"], "image:shape.create");
        assert_eq!(s.revision(), rev + 1);
        // undo reaches it like any other change
        s.execute("studio:undo", json!({})).unwrap();
        assert_eq!(s.revision(), rev + 2);
    }

    #[test]
    fn documents_opened_in_the_ui_become_assets_and_project_images_stay_open() {
        let mut s = Studio::new();
        let r = s.execute("studio:image.new", json!({"name": "Badge", "width": 16, "height": 16})).unwrap();
        let badge = AssetId(r["result"]["asset"].as_u64().unwrap());
        s.image.execute("file.new", json!({"width": 8, "height": 8, "name": "Sketch"})).unwrap();
        let e = s.absorb_ui("image").unwrap();
        assert_eq!(e["receipt"]["created"].as_array().unwrap().len(), 1);
        assert!(s.assets().any(|a| a.name == "Sketch"));
        // closing the badge in the UI keeps it in the project
        let i = s.image.documents().iter().position(|d| Some(d.doc.id) == s.image_doc(badge).map(|x| x.doc.id)).unwrap();
        s.image.close(i);
        let e = s.absorb_ui("image").unwrap();
        assert!(e["receipt"]["warnings"][0].as_str().unwrap().contains("stays open"));
        assert!(s.image_doc(badge).is_some());
    }

    #[test]
    fn studio_commands_are_not_reported_again() {
        let mut s = Studio::new();
        s.execute("studio:comp.new", json!({})).unwrap();
        s.execute("motion:layer.newSolid", json!({"asset": 1, "color": "#000000"})).unwrap();
        assert!(s.absorb_ui("motion").is_none());
    }

    #[test]
    fn guards_name_the_studio_alternative() {
        assert_eq!(timeline_guard("file.open"), Some("studio:project.open"));
        assert_eq!(motion_guard("layer.newSolid"), None);
        assert_eq!(image_guard("file.closeAll"), Some("studio:project.new"));
    }
}
