//! The command palette: every command of every engine (and the studio's), searchable, run with
//! JSON parameters, answered with its receipt. `Ctrl+Shift+P` / `F1`.
//!
//! It is the same command surface agents use, so a person can try exactly what an agent would
//! run and see the same receipt.

use serde_json::{Value, json};
use videonow_core::manifest::{CommandInfo, Effect};

use crate::brand;

#[derive(Default)]
pub struct Palette {
    pub open: bool,
    query: String,
    selected: usize,
    chosen: Option<CommandInfo>,
    params: String,
    result: Option<Result<Value, String>>,
    commands: Vec<CommandInfo>,
    focus_query: bool,
    /// A destructive command waits for a second click.
    armed: bool,
}

/// The asset the palette fills in when a command needs one and the parameters name none.
#[derive(Clone, Copy, Default)]
pub struct Context {
    pub image: Option<u64>,
    pub comp: Option<u64>,
}

fn effect_label(e: Effect) -> (&'static str, egui::Color32) {
    match e {
        Effect::Read => ("read", brand::TEXT_DIM),
        Effect::Edit => ("edit", brand::HUMAN),
        Effect::Destructive => ("destructive", egui::Color32::from_rgb(0xe4, 0x57, 0x2e)),
        Effect::FileWrite => ("file write", brand::WARN),
        Effect::External => ("external", brand::AGENT),
    }
}

/// Lower score is better; `None` when a word of the query is missing.
fn score(c: &CommandInfo, words: &[String]) -> Option<usize> {
    let hay = format!("{} {}", c.id, c.label).to_ascii_lowercase();
    let mut s = 0;
    for w in words {
        let at = hay.find(w.as_str())?;
        s += at;
    }
    // shorter ids first, studio commands before engine ones at equal scores
    Some(s * 4 + c.id.len() / 8 + usize::from(c.domain != "studio"))
}

impl Palette {
    pub fn toggle(&mut self) {
        self.open = !self.open;
        if self.open {
            self.focus_query = true;
            self.chosen = None;
            self.result = None;
            self.armed = false;
        }
    }

    pub fn set_result(&mut self, r: Result<Value, String>) {
        self.result = Some(r);
        self.armed = false;
    }

    /// Open the palette on one command (the asset menu's "Run a command on …").
    pub fn open_on(&mut self, id: &str, params: Value) {
        self.ensure_commands();
        self.open = true;
        self.chosen = self.commands.iter().find(|c| c.id == id).cloned();
        self.params = serde_json::to_string_pretty(&params).unwrap_or_else(|_| "{}".into());
        self.result = None;
        self.armed = false;
    }

    fn ensure_commands(&mut self) {
        if self.commands.is_empty() {
            self.commands = videonow_core::manifest::all();
        }
    }

    /// Draw the palette; returns a command to run (`id`, params, dry run) when the person asked
    /// for one.
    pub fn show(&mut self, ctx: &egui::Context, here: Context) -> Option<(String, Value, bool)> {
        if !self.open {
            return None;
        }
        self.ensure_commands();
        let mut run = None;
        let mut open = self.open;
        let width = (ctx.content_rect().width() - 80.0).clamp(360.0, 720.0);
        egui::Window::new("Command palette")
            .id(egui::Id::new("videonow-palette"))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .fixed_size([width, 0.0])
            .anchor(egui::Align2::CENTER_TOP, [0.0, 60.0])
            .show(ctx, |ui| {
                if let Some(c) = self.chosen.clone() {
                    run = self.details(ui, &c, here);
                } else {
                    self.search(ui);
                }
            });
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            if self.chosen.is_some() {
                self.chosen = None;
                self.result = None;
                self.focus_query = true;
            } else {
                open = false;
            }
        }
        self.open = open;
        run
    }

    fn search(&mut self, ui: &mut egui::Ui) {
        let r = ui.add(
            egui::TextEdit::singleline(&mut self.query)
                .hint_text(format!("Search {} commands: timeline, motion, image, studio", self.commands.len()))
                .desired_width(f32::INFINITY),
        );
        if self.focus_query {
            r.request_focus();
            self.focus_query = false;
        }
        if r.changed() {
            self.selected = 0;
        }
        let words: Vec<String> = self.query.split_whitespace().map(str::to_ascii_lowercase).collect();
        let mut hits: Vec<(usize, &CommandInfo)> = self.commands.iter().filter_map(|c| score(c, &words).map(|s| (s, c))).collect();
        hits.sort_by_key(|(s, c)| (*s, c.id.len()));
        let total = hits.len();
        hits.truncate(14);
        let (down, up, enter) = ui.input(|i| (i.key_pressed(egui::Key::ArrowDown), i.key_pressed(egui::Key::ArrowUp), i.key_pressed(egui::Key::Enter)));
        if down {
            self.selected = (self.selected + 1).min(hits.len().saturating_sub(1));
        }
        if up {
            self.selected = self.selected.saturating_sub(1);
        }
        ui.add_space(4.0);
        let mut pick = None;
        for (i, (_, c)) in hits.iter().enumerate() {
            let selected = i == self.selected;
            let (label, color) = effect_label(c.effect);
            let resp = ui
                .horizontal(|ui| {
                    let r = ui.selectable_label(selected, egui::RichText::new(&c.id).monospace());
                    ui.label(egui::RichText::new(&c.label).color(brand::TEXT_DIM));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(egui::RichText::new(label).small().color(color));
                        if c.instead.is_some() {
                            ui.label(egui::RichText::new("handled by VideoNow").small().color(brand::TEXT_DIM));
                        }
                    });
                    r
                })
                .inner;
            if resp.clicked() || (selected && enter) {
                pick = Some((*c).clone());
            }
        }
        if total > hits.len() {
            ui.label(egui::RichText::new(format!("… {} more; type more of the name", total - hits.len())).small().color(brand::TEXT_DIM));
        }
        if let Some(c) = pick {
            self.params = "{}".into();
            self.result = None;
            self.chosen = Some(c);
        }
    }

    fn details(&mut self, ui: &mut egui::Ui, c: &CommandInfo, here: Context) -> Option<(String, Value, bool)> {
        let mut run = None;
        ui.horizontal(|ui| {
            if ui.button("‹ Back").clicked() {
                self.chosen = None;
                self.result = None;
                self.focus_query = true;
            }
            ui.label(egui::RichText::new(&c.id).monospace().strong());
            let (label, color) = effect_label(c.effect);
            ui.label(egui::RichText::new(label).color(color));
        });
        ui.label(&c.label);
        if let Some(s) = c.summary {
            ui.label(egui::RichText::new(s).color(brand::TEXT_DIM));
        }
        if let Some(instead) = c.instead {
            ui.colored_label(brand::WARN, format!("In a VideoNow project this is {instead}."));
        }
        ui.add_space(4.0);
        ui.label(egui::RichText::new(format!("Parameters: {}", if c.params.is_empty() { "{}" } else { &c.params })).small().monospace().color(brand::TEXT_DIM));
        let needs_asset = match c.domain {
            "image" => here.image.map(|a| ("image", a)),
            "motion" => here.comp.map(|a| ("composition", a)),
            _ => None,
        };
        if let Some((what, a)) = needs_asset {
            ui.label(egui::RichText::new(format!("Runs on the open {what} (asset {a}) unless `asset` says otherwise.")).small().color(brand::TEXT_DIM));
        }
        ui.add(egui::TextEdit::multiline(&mut self.params).code_editor().desired_rows(4).desired_width(f32::INFINITY));
        ui.horizontal(|ui| {
            let destructive = c.effect == Effect::Destructive;
            let text = if destructive && !self.armed { "Run (destructive)…" } else if destructive { "Click again to run" } else { "Run" };
            let run_clicked = ui.button(text).clicked();
            let dry = c.effect != Effect::Read
                && ui.button("Dry run").on_hover_text("Run it, show the receipt (what would change), and take it back").clicked();
            if run_clicked || dry {
                if destructive && !self.armed && !dry {
                    self.armed = true;
                } else {
                    match serde_json::from_str::<Value>(if self.params.trim().is_empty() { "{}" } else { &self.params }) {
                        Ok(mut p) if p.is_object() => {
                            if let Some((_, a)) = needs_asset
                                && p.get("asset").is_none()
                                && let Some(o) = p.as_object_mut()
                            {
                                o.insert("asset".into(), json!(a));
                            }
                            run = Some((c.id.clone(), p, dry));
                        }
                        Ok(_) => self.result = Some(Err("parameters must be a JSON object, like {\"name\": \"Title\"}".into())),
                        Err(e) => self.result = Some(Err(format!("parameters are not JSON: {e}"))),
                    }
                }
            }
        });
        if let Some(r) = &self.result {
            ui.separator();
            match r {
                Ok(v) => receipt_view(ui, v),
                Err(e) => {
                    ui.colored_label(egui::Color32::from_rgb(0xe4, 0x57, 0x2e), e);
                }
            }
        }
        run
    }
}

/// A command's answer: its result and what the receipt vouches for.
pub fn receipt_view(ui: &mut egui::Ui, v: &Value) {
    let rc = &v["receipt"];
    let changed: Vec<&str> = rc["changed"].as_array().map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
    ui.label(format!(
        "revision {} → {}   changed: {}",
        rc["revision"]["before"],
        rc["revision"]["after"],
        if changed.is_empty() { "nothing".to_string() } else { changed.join(", ") }
    ));
    for w in rc["warnings"].as_array().into_iter().flatten().filter_map(Value::as_str) {
        ui.colored_label(brand::WARN, w);
    }
    for n in rc["not_checked"].as_array().into_iter().flatten().filter_map(Value::as_str) {
        ui.label(egui::RichText::new(format!("not checked: {n}")).small().color(brand::TEXT_DIM));
    }
    let text = serde_json::to_string_pretty(&v["result"]).unwrap_or_default();
    egui::ScrollArea::vertical().max_height(180.0).show(ui, |ui| {
        ui.label(egui::RichText::new(text).monospace().small());
    });
}
