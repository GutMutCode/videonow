//! The shell's own panels: the top bar (workspaces, project, palette, agent), the asset list,
//! the Agent panel with its activity log, and the save prompt.

use serde_json::{Value, json};
use videonow_core::{AssetId, AssetKind};

use crate::brand;
use crate::shell::{Next, VideoNowApp, Workspace};

fn chip(ui: &mut egui::Ui, text: &str, color: egui::Color32) {
    ui.label(egui::RichText::new(text).small().color(color));
}

/// Who made a change, as the activity log shows it.
fn actor_style(by: &str) -> (&'static str, egui::Color32) {
    match by {
        "agent" => ("agent", brand::AGENT),
        "palette" => ("palette", brand::ACCENT),
        "ui" => ("you", brand::HUMAN),
        "cli" => ("cli", brand::TEXT_DIM),
        "demo" => ("demo", brand::TEXT_DIM),
        _ => ("api", brand::TEXT_DIM),
    }
}

impl VideoNowApp {
    pub(crate) fn top_bar(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let frame = egui::Frame::NONE.fill(brand::BAR).inner_margin(egui::Margin::symmetric(8, 4)).stroke(egui::Stroke::new(1.0, brand::BAR_LINE));
        egui::Panel::top("videonow-top").exact_size(34.0).frame(frame).show(ui, |ui| {
            ui.horizontal_centered(|ui| {
                if let Some(t) = &self.mark {
                    ui.add(egui::Image::new((t.id(), egui::vec2(20.0, 20.0))));
                }
                ui.label(egui::RichText::new("VideoNow").strong().color(brand::TEXT));
                ui.add_space(12.0);
                for w in Workspace::ALL {
                    let on = self.workspace == w;
                    let text = egui::RichText::new(w.label()).color(if on { brand::TEXT } else { brand::TEXT_DIM });
                    let r = ui.add(egui::Button::selectable(on, text)).on_hover_text(match w {
                        Workspace::Edit => "Sequences: edit, audio, captions, export",
                        Workspace::Motion => "Compositions: layers, keyframes, effects, tracking",
                        Workspace::Image => "Image documents: layers, painting, retouching",
                    });
                    if r.clicked() {
                        self.switch_to(w, &ctx);
                    }
                }
                ui.add_space(8.0);
                // the selection is a nested composition or image: open it where it is edited
                if let Some(a) = self.selected_asset().and_then(|a| self.studio.asset_info(a).cloned()) {
                    let target = if a.kind == AssetKind::Composition { "Motion" } else { "Image" };
                    if ui.button(format!("Edit “{}” in {target} ›", a.name)).on_hover_text("Open the selected composition or image in its editor; changes show up here live").clicked() {
                        self.open_asset(a.id, &ctx);
                    }
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let agent_on = self.agent.is_on();
                    let label = if agent_on { format!("Agent ● {}", self.agent.connected()) } else { "Agent".to_string() };
                    let color = if agent_on { brand::AGENT } else { brand::TEXT_DIM };
                    if ui.add(egui::Button::selectable(self.show_agent, egui::RichText::new(label).color(color))).on_hover_text("Agent access, permissions and the activity log").clicked() {
                        self.show_agent = !self.show_agent;
                    }
                    if ui.button("Commands").on_hover_text("Every command of every engine (Ctrl+Shift+P or F1)").clicked() {
                        self.palette.toggle();
                    }
                    if ui.add(egui::Button::selectable(self.show_assets, "Assets")).clicked() {
                        self.show_assets = !self.show_assets;
                    }
                    ui.separator();
                    let name = self.studio.project_path().and_then(|p| p.file_stem()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "Untitled".into());
                    let dirty = self.studio.is_dirty();
                    ui.menu_button(egui::RichText::new(format!("{name}{}", if dirty { " •" } else { "" })).color(brand::TEXT), |ui| {
                        if ui.button("New Project").clicked() {
                            self.request(Next::New);
                            ui.close();
                        }
                        if ui.button("Open Project…").clicked() {
                            self.request(Next::Open(None));
                            ui.close();
                        }
                        if ui.button("Save").clicked() {
                            self.save(false, None);
                            ui.close();
                        }
                        if ui.button("Save As…").clicked() {
                            self.save(true, None);
                            ui.close();
                        }
                        ui.separator();
                        if ui.button("Demo Project").clicked() {
                            self.request(Next::Demo);
                            ui.close();
                        }
                    })
                    .response
                    .on_hover_text(match self.studio.project_path() {
                        Some(p) => format!("{}{}", p.display(), if dirty { " (unsaved changes)" } else { "" }),
                        None => "Not saved yet".into(),
                    });
                });
            });
        });
    }

    pub(crate) fn assets_panel(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let frame = egui::Frame::NONE.fill(brand::BAR).inner_margin(egui::Margin::same(8)).stroke(egui::Stroke::new(1.0, brand::BAR_LINE));
        egui::Panel::left("videonow-assets").resizable(true).default_size(220.0).size_range(160.0..=420.0).frame(frame).show(ui, |ui| {
            ui.label(egui::RichText::new("PROJECT ASSETS").small().strong().color(brand::TEXT_DIM));
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if ui.button("+ Composition").on_hover_text("A new composition (motion editor), the size of the active sequence").clicked() {
                    let (w, h, fps) = self.sequence_format();
                    let n = self.studio.assets().filter(|a| a.kind == AssetKind::Composition).count() + 1;
                    match self.run("ui", "studio:comp.new", json!({"name": format!("Composition {n}"), "width": w, "height": h, "fps": fps})) {
                        Ok(r) => {
                            if let Some(a) = r["result"]["asset"].as_u64() {
                                self.open_asset(AssetId(a), &ctx);
                            }
                        }
                        Err(e) => self.say(e, true),
                    }
                }
                if ui.button("+ Image").on_hover_text("A new image document (image editor), the size of the active sequence").clicked() {
                    let (w, h, _) = self.sequence_format();
                    let n = self.studio.assets().filter(|a| a.kind == AssetKind::Image).count() + 1;
                    match self.run("ui", "studio:image.new", json!({"name": format!("Image {n}"), "width": w, "height": h, "background": "transparent"})) {
                        Ok(r) => {
                            if let Some(a) = r["result"]["asset"].as_u64() {
                                self.open_asset(AssetId(a), &ctx);
                            }
                        }
                        Err(e) => self.say(e, true),
                    }
                }
            });
            ui.add_space(6.0);
            let assets: Vec<videonow_core::Asset> = self.studio.assets().cloned().collect();
            if assets.is_empty() {
                ui.label(egui::RichText::new("Compositions and images of this project appear here. Make one with the buttons above, or in the Motion and Image editors.").small().color(brand::TEXT_DIM));
            }
            let here = self.here();
            egui::ScrollArea::vertical().show(ui, |ui| {
                for (kind, title) in [(AssetKind::Composition, "Compositions"), (AssetKind::Image, "Images")] {
                    let of_kind: Vec<&videonow_core::Asset> = assets.iter().filter(|a| a.kind == kind).collect();
                    if of_kind.is_empty() {
                        continue;
                    }
                    ui.label(egui::RichText::new(title).small().color(brand::TEXT_DIM));
                    for a in of_kind {
                        let current = match kind {
                            AssetKind::Composition => here.comp == Some(a.id.0) && self.workspace == Workspace::Motion,
                            AssetKind::Image => here.image == Some(a.id.0) && self.workspace == Workspace::Image,
                        };
                        let icon = if kind == AssetKind::Composition { "▶" } else { "◩" };
                        let r = ui.add(egui::Button::selectable(current, format!("{icon}  {}", a.name)).wrap_mode(egui::TextWrapMode::Truncate));
                        let r = r.on_hover_text(format!("asset {} · {}", a.id.0, videonow_core::assets::uri(a.kind, a.id)));
                        if r.clicked() {
                            self.open_asset(a.id, &ctx);
                        }
                        r.context_menu(|ui| {
                            if ui.button("Open in its editor").clicked() {
                                self.open_asset(a.id, &ctx);
                                ui.close();
                            }
                            if ui.button("Place on the timeline at the playhead").clicked() {
                                let at = self.studio.timeline.playhead().seconds().max(0.0);
                                match self.run("ui", "studio:asset.place", json!({"asset": a.id.0, "seconds": at})) {
                                    Ok(_) => self.say(format!("Placed {} on the timeline", a.name), false),
                                    Err(e) => self.say(e, true),
                                }
                                ui.close();
                            }
                            if kind == AssetKind::Image
                                && let Some(comp) = here.comp
                                && ui.button("Add to the open composition").clicked()
                            {
                                match self.run("ui", "studio:image.toComp", json!({"image": a.id.0, "comp": comp})) {
                                    Ok(_) => self.say(format!("Added {} to the composition", a.name), false),
                                    Err(e) => self.say(e, true),
                                }
                                ui.close();
                            }
                            if ui.button("Run a command on it…").clicked() {
                                let id = if kind == AssetKind::Image { "image:image.imageSize" } else { "motion:layer.newSolid" };
                                self.palette.open_on(id, json!({"asset": a.id.0}));
                                ui.close();
                            }
                        });
                    }
                    ui.add_space(6.0);
                }
            });
        });
    }

    /// Width, height and frame rate of the active sequence (1920×1080 at 30 without one).
    fn sequence_format(&self) -> (u32, u32, f64) {
        match self.studio.timeline.active_sequence() {
            Some(q) => (q.settings.width, q.settings.height, q.settings.frame_rate.num as f64 / q.settings.frame_rate.den.max(1) as f64),
            None => (1920, 1080, 30.0),
        }
    }

    pub(crate) fn agent_panel(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let frame = egui::Frame::NONE.fill(brand::BAR).inner_margin(egui::Margin::same(8)).stroke(egui::Stroke::new(1.0, brand::BAR_LINE));
        egui::Panel::right("videonow-agent").resizable(true).default_size(340.0).size_range(260.0..=640.0).frame(frame).show(ui, |ui| {
            ui.label(egui::RichText::new("AGENT").small().strong().color(brand::TEXT_DIM));
            let mut on = self.agent.is_on();
            if ui.checkbox(&mut on, "Allow an agent to work on this project").changed() {
                if on {
                    self.agent.start(self.options.agent_port, &ctx);
                } else {
                    self.agent.stop();
                }
            }
            if let Some(e) = &self.agent.error {
                ui.colored_label(brand::WARN, e);
            }
            if self.agent.is_on() {
                let status = self.agent.last_event.lock().map(|s| s.clone()).unwrap_or_default();
                ui.label(egui::RichText::new(format!("{} connected · {status}", self.agent.connected())).small().color(brand::TEXT_DIM));
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("videonow mcp --attach").monospace().small());
                    if ui.small_button("Copy").on_hover_text("The command an MCP client (Claude Desktop, Claude Code) runs to reach this window").clicked() {
                        ctx.copy_text("videonow mcp --attach".into());
                    }
                });
                ui.add_space(4.0);
                ui.label(egui::RichText::new("The agent may").small().color(brand::TEXT_DIM));
                let p = &mut self.agent.permissions;
                ui.checkbox(&mut p.file_writes, "write files (save, render, export)");
                ui.checkbox(&mut p.destructive, "delete work, open or replace the project");
                ui.checkbox(&mut p.external, "use the network or other applications");
                ui.checkbox(&mut p.project_folder_only, "touch files only in the project's folder");
                ui.label(egui::RichText::new("Edits are always allowed and always undoable; each one gets a receipt below.").small().color(brand::TEXT_DIM));
            } else {
                ui.label(egui::RichText::new("When this is on, an MCP client can run the same commands as the palette on this project. You see every change it makes below, and keep editing alongside it.").small().color(brand::TEXT_DIM));
            }
            ui.separator();
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("ACTIVITY").small().strong().color(brand::TEXT_DIM));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.small_button("Undo last").on_hover_text("studio:undo — undo the last change, in whichever editor made it").clicked() {
                        match self.run("ui", "studio:undo", json!({})) {
                            Ok(r) => self.say(format!("Undid {}", r["result"]["undone"]), false),
                            Err(e) => self.say(e, true),
                        }
                    }
                });
            });
            self.activity(ui);
        });
    }

    fn activity(&mut self, ui: &mut egui::Ui) {
        let journal: Vec<Value> = self.studio.journal().iter().rev().take(200).cloned().collect();
        let total = self.studio.journal().len();
        if journal.is_empty() {
            ui.label(egui::RichText::new("Changes appear here as they happen: yours, the palette's and the agent's, each with what it changed and what nobody checked.").small().color(brand::TEXT_DIM));
            return;
        }
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            for (k, e) in journal.iter().enumerate() {
                let index = total - 1 - k;
                let rc = &e["receipt"];
                let by = rc["by"].as_str().unwrap_or("api");
                let (who, color) = actor_style(by);
                let cmd = e["command"].as_str().unwrap_or("");
                let changed: Vec<&str> = rc["changed"].as_array().map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
                let warnings: Vec<&str> = rc["warnings"].as_array().map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
                let open = self.expanded == Some(index);
                let r = ui
                    .horizontal(|ui| {
                        chip(ui, who, color);
                        let title = if cmd.starts_with("ui:") {
                            let cmds: Vec<&str> = rc["commands"].as_array().map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
                            match cmds.as_slice() {
                                [] => format!("edited in {}", cmd.trim_start_matches("ui:")),
                                [one] => one.to_string(),
                                [first, ..] => format!("{first} +{}", cmds.len() - 1),
                            }
                        } else {
                            cmd.to_string()
                        };
                        ui.add(egui::Label::new(egui::RichText::new(title).monospace().small().color(brand::TEXT)).truncate().sense(egui::Sense::click()))
                    })
                    .inner;
                if r.clicked() {
                    self.expanded = if open { None } else { Some(index) };
                }
                ui.label(egui::RichText::new(format!("  rev {} · {}", rc["revision"]["after"], changed.join(", "))).small().color(brand::TEXT_DIM));
                for w in &warnings {
                    ui.label(egui::RichText::new(format!("  ⚠ {w}")).small().color(brand::WARN));
                }
                if open {
                    let text = serde_json::to_string_pretty(e).unwrap_or_default();
                    ui.label(egui::RichText::new(text).monospace().small());
                }
                ui.add_space(3.0);
            }
        });
    }

    pub(crate) fn confirm_window(&mut self, ctx: &egui::Context) {
        let Some(next) = self.confirm.clone() else { return };
        let name = self.studio.project_path().and_then(|p| p.file_stem()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "Untitled".into());
        let verb = match next {
            Next::Quit => "closing",
            Next::New => "starting a new project",
            Next::Open(_) => "opening another project",
            Next::Demo => "opening the demo project",
        };
        let mut choice = None;
        egui::Window::new("Unsaved changes").id(egui::Id::new("videonow-confirm")).collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(ctx, |ui| {
            ui.label(format!("Save the changes to “{name}” before {verb}?"));
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui.button("Save").clicked() {
                    choice = Some(0);
                }
                if ui.button("Don't Save").clicked() {
                    choice = Some(1);
                }
                if ui.button("Cancel").clicked() {
                    choice = Some(2);
                }
            });
        });
        match choice {
            Some(0) => {
                self.confirm = None;
                self.save(false, Some(next));
            }
            Some(1) => {
                self.confirm = None;
                self.proceed(next);
            }
            Some(2) => self.confirm = None,
            _ => {}
        }
    }

    pub(crate) fn status_toast(&mut self, ctx: &egui::Context) {
        let Some((text, error, at)) = self.status.clone() else { return };
        let age = self.now - at;
        let keep = if error { 10.0 } else { 5.0 };
        if age > keep {
            self.status = None;
            return;
        }
        ctx.request_repaint_after(std::time::Duration::from_millis(500));
        egui::Area::new(egui::Id::new("videonow-status")).anchor(egui::Align2::CENTER_BOTTOM, [0.0, -36.0]).interactable(true).show(ctx, |ui| {
            egui::Frame::popup(ui.style()).fill(brand::BAR).show(ui, |ui| {
                ui.set_max_width(640.0);
                let r = ui.add(egui::Label::new(egui::RichText::new(text).color(if error { brand::WARN } else { brand::TEXT })).sense(egui::Sense::click()));
                if r.clicked() {
                    self.status = None;
                }
            });
        });
    }
}
