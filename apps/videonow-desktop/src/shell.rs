//! The application shell: the window around the three editors.

use std::path::PathBuf;

use serde_json::{Value, json};

use effectcraft_ui_egui::EffectcraftApp;
use filmcraft_ui_egui::FilmcraftApp;
use photocraft_ui_egui::PhotocraftApp;
use videonow_core::{AssetId, AssetKind, Studio};

use crate::agent::Agent;
use crate::dialogs::{self, Purpose};
use crate::lend;
use crate::palette::{self, Palette};

/// The three editors.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Workspace {
    /// Sequences: the timeline editor.
    Edit,
    /// Compositions: the motion editor.
    Motion,
    /// Image documents: the image editor.
    Image,
}

impl Workspace {
    pub const ALL: [Workspace; 3] = [Workspace::Edit, Workspace::Motion, Workspace::Image];

    pub fn label(self) -> &'static str {
        match self {
            Workspace::Edit => "Edit",
            Workspace::Motion => "Motion",
            Workspace::Image => "Image",
        }
    }

    /// The name in `ui:<name>` receipts.
    pub fn id(self) -> &'static str {
        match self {
            Workspace::Edit => "edit",
            Workspace::Motion => "motion",
            Workspace::Image => "image",
        }
    }

    fn index(self) -> usize {
        match self {
            Workspace::Edit => 0,
            Workspace::Motion => 1,
            Workspace::Image => 2,
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "edit" | "timeline" => Some(Workspace::Edit),
            "motion" | "comp" | "composition" => Some(Workspace::Motion),
            "image" | "photo" => Some(Workspace::Image),
            _ => None,
        }
    }
}

/// How the application starts.
#[derive(Clone, Debug)]
pub struct Options {
    /// A `.videonow` project to open.
    pub open: Option<PathBuf>,
    /// Start with the demo project.
    pub demo: bool,
    /// Turn agent access on at start.
    pub agent: bool,
    /// The agent port (0: any free port).
    pub agent_port: u16,
    pub workspace: Workspace,
    /// Native file dialogs (off in offscreen runs, where nobody could answer one).
    pub dialogs: bool,
    /// Let the editors draw with the GPU when the window has one.
    pub gpu: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self { open: None, demo: false, agent: false, agent_port: 0, workspace: Workspace::Edit, dialogs: true, gpu: true }
    }
}

/// What to do once unsaved changes are dealt with.
#[derive(Clone, Debug, PartialEq)]
pub enum Next {
    Quit,
    New,
    Open(Option<PathBuf>),
    Demo,
}

pub struct VideoNowApp {
    pub studio: Studio,
    pub film: FilmcraftApp,
    pub motion: EffectcraftApp,
    pub photo: PhotocraftApp,
    pub workspace: Workspace,
    pub show_assets: bool,
    pub show_agent: bool,
    pub palette: Palette,
    pub(crate) agent: Agent,
    /// The status line: message, is it an error, when it was set.
    pub status: Option<(String, bool, f64)>,
    /// "Save changes?" before `Next`.
    pub(crate) confirm: Option<Next>,
    /// After the save a confirmation asked for, do this.
    pub(crate) after_save: Option<Next>,
    pub(crate) dialog: Option<dialogs::Pending>,
    pub(crate) options: Options,
    pub(crate) mark: Option<egui::TextureHandle>,
    /// Each workspace's input options (the image editor turns Ctrl+wheel into sideways scrolling,
    /// which the timeline uses to zoom).
    input_options: [Option<egui::InputOptions>; 3],
    zoom_with_keyboard: [bool; 3],
    styled: Workspace,
    quitting: bool,
    pub(crate) expanded: Option<usize>,
    pub(crate) now: f64,
    started: bool,
    ctx: Option<egui::Context>,
}

impl VideoNowApp {
    /// The application, created with the window (`cc` gives the egui context and the GPU).
    pub fn new(cc: &eframe::CreationContext<'_>, options: Options) -> Self {
        let ctx = &cc.egui_ctx;
        // The image editor configures the context once (fonts, plugins, input options); keep its
        // input options for its own workspace and give the others egui's.
        let defaults = ctx.options(|o| (o.input_options.clone(), o.zoom_with_keyboard));
        PhotocraftApp::setup_context(ctx, Default::default());
        let photo_opts = ctx.options(|o| (o.input_options.clone(), o.zoom_with_keyboard));
        ctx.options_mut(|o| {
            o.input_options = defaults.0.clone();
            o.zoom_with_keyboard = defaults.1;
        });

        let mut motion_session = effectcraft_host::session();
        if let Some(dir) = config_dir().map(|d| d.join("motion")) {
            if std::fs::create_dir_all(&dir).is_ok() {
                motion_session.config = Some(std::sync::Arc::new(effectcraft_engine::config::DirConfig::new(dir)));
            }
        }
        motion_session.load_settings();
        let mut studio = Studio::with_sessions(filmcraft_engine::Session::default(), motion_session, photocraft_engine::Session::new());
        studio.set_actor("ui");

        // each editor is built around the studio's own session, then gives it back
        let mut film = FilmcraftApp::new(std::mem::take(&mut studio.timeline));
        std::mem::swap(&mut film.session, &mut studio.timeline);
        let mut motion = EffectcraftApp::new(std::mem::take(&mut studio.motion));
        std::mem::swap(&mut motion.session, &mut studio.motion);
        let mut photo = PhotocraftApp::new(std::mem::take(&mut studio.images), crate::photo_services::services(options.dialogs));
        std::mem::swap(&mut photo.session, &mut studio.images);

        motion.ui.start_screen = false;
        photo.custom_titlebar = false;
        photo.integrated_titlebar = false;
        film.integrated_titlebar = false;
        if options.dialogs {
            install_film_hooks(&mut film);
            install_motion_hooks(&mut motion);
        }
        if options.gpu
            && let Some(rs) = cc.wgpu_render_state.clone()
        {
            film.set_wgpu(rs.clone());
            photo.set_wgpu(rs);
        }

        let mut app = Self {
            studio,
            film,
            motion,
            photo,
            workspace: options.workspace,
            show_assets: true,
            show_agent: false,
            palette: Palette::default(),
            agent: Agent::new(),
            status: None,
            confirm: None,
            after_save: None,
            dialog: None,
            mark: crate::brand::mark_texture(ctx),
            input_options: [Some(defaults.0.clone()), Some(defaults.0), Some(photo_opts.0)],
            zoom_with_keyboard: [defaults.1, defaults.1, photo_opts.1],
            styled: options.workspace,
            quitting: false,
            expanded: None,
            now: 0.0,
            started: false,
            ctx: Some(ctx.clone()),
            options,
        };
        // the starting workspace's input options
        let w = app.workspace.index();
        if let Some(opts) = app.input_options[w].clone() {
            let zoom = app.zoom_with_keyboard[w];
            ctx.options_mut(|o| {
                o.input_options = opts;
                o.zoom_with_keyboard = zoom;
            });
        }
        if let Some(p) = app.options.open.clone() {
            app.open_path(&p);
        } else if app.options.demo {
            app.build_demo();
        }
        if app.options.agent {
            app.show_agent = true;
            app.agent.start(app.options.agent_port, ctx);
        }
        app
    }

    /// Run a command as `actor` (`ui`, `palette`); the error is a message for the person.
    pub fn run(&mut self, actor: &str, id: &str, params: Value) -> Result<Value, String> {
        self.studio.as_actor(actor, None, |s| s.execute(id, params)).map_err(|e| e.to_string())
    }

    pub(crate) fn say(&mut self, text: impl Into<String>, error: bool) {
        let text = text.into();
        if error {
            log::warn!("{text}");
        }
        self.status = Some((text, error, self.now));
    }

    /// Show `w`, with its own look and input options.
    pub fn switch_to(&mut self, w: Workspace, ctx: &egui::Context) {
        if w == self.workspace && self.styled == w {
            return;
        }
        let from = self.workspace.index();
        let (input, zoom) = ctx.options(|o| (o.input_options.clone(), o.zoom_with_keyboard));
        self.input_options[from] = Some(input);
        self.zoom_with_keyboard[from] = zoom;
        self.workspace = w;
        let to = w.index();
        if let Some(opts) = self.input_options[to].clone() {
            let zoom = self.zoom_with_keyboard[to];
            ctx.options_mut(|o| {
                o.input_options = opts;
                o.zoom_with_keyboard = zoom;
            });
        }
        self.apply_look(ctx);
    }

    fn apply_look(&mut self, ctx: &egui::Context) {
        match self.workspace {
            Workspace::Edit => filmcraft_ui_egui::theme::apply_visuals(ctx, &self.film.tokens),
            Workspace::Motion => effectcraft_ui_egui::theme::apply_visuals(ctx, &self.motion.tokens),
            Workspace::Image => photocraft_ui_egui::theme::apply(ctx, self.photo.ui.theme),
        }
        self.styled = self.workspace;
    }

    /// Open an asset in its editor.
    pub fn open_asset(&mut self, id: AssetId, ctx: &egui::Context) {
        let Some(a) = self.studio.asset_info(id).cloned() else { return };
        match a.kind {
            AssetKind::Composition => {
                if let Some(c) = a.comp {
                    self.studio.motion.open_comp(effectcraft_project::ItemId(c));
                }
                self.switch_to(Workspace::Motion, ctx);
            }
            AssetKind::Image => match self.studio.open_image(id) {
                Ok(i) => {
                    self.studio.images.set_active(i);
                    self.switch_to(Workspace::Image, ctx);
                }
                Err(e) => self.say(e.to_string(), true),
            },
        }
    }

    /// The image and composition the palette fills in for `image:` / `motion:` commands.
    pub fn here(&self) -> palette::Context {
        palette::Context {
            image: self.studio.images.active().and_then(|d| self.studio.asset_of_document(d.doc.id)).map(|a| a.0),
            comp: self.studio.motion.active_comp_id().and_then(|c| self.studio.asset_of_comp(c.0)).map(|a| a.0),
        }
    }

    /// The asset selected in the active editor, if it is a nested composition or image: a clip
    /// or project item in Edit, a layer in Motion.
    pub fn selected_asset(&self) -> Option<AssetId> {
        let s = &self.studio;
        match self.workspace {
            Workspace::Edit => {
                let st = &s.timeline.state;
                if let Some(q) = s.timeline.active_sequence() {
                    for t in q.video_tracks.iter().chain(&q.audio_tracks) {
                        for c in &t.items {
                            if st.selection.contains(&c.id)
                                && let Some(a) = s.asset_of_timeline_item(c.item.0)
                            {
                                return Some(a);
                            }
                        }
                    }
                }
                st.project_selection.iter().find_map(|i| s.asset_of_timeline_item(i.0))
            }
            Workspace::Motion => {
                let c = s.motion.active_comp()?;
                let sel = &s.motion.state.selected_layers;
                c.layers.iter().filter(|l| sel.contains(&l.id)).find_map(|l| match &l.source {
                    effectcraft_project::LayerSource::Comp { item } => s.asset_of_comp(item.0),
                    effectcraft_project::LayerSource::Footage { item } => match &s.motion.project.items.get(item)?.kind {
                        effectcraft_project::ItemKind::Footage(f) => videonow_core::assets::parse_uri(&f.path).map(|(_, id)| id),
                        _ => None,
                    },
                    _ => None,
                })
            }
            Workspace::Image => None,
        }
    }

    // ---------- projects ----------

    /// Ask to save unsaved changes first, then do `next`.
    pub fn request(&mut self, next: Next) {
        if self.studio.is_dirty() {
            self.confirm = Some(next);
        } else {
            self.proceed(next);
        }
    }

    pub(crate) fn proceed(&mut self, next: Next) {
        match next {
            Next::Quit => {
                self.quitting = true;
                if let Some(c) = &self.ctx {
                    c.request_repaint();
                }
            }
            Next::New => match self.run("ui", "studio:project.new", json!({})) {
                Ok(_) => self.say("New project", false),
                Err(e) => self.say(e, true),
            },
            Next::Open(Some(p)) => self.open_path(&p),
            Next::Open(None) => self.ask(Purpose::OpenProject),
            Next::Demo => {
                if self.run("ui", "studio:project.new", json!({})).is_ok() {
                    self.build_demo();
                }
            }
        }
    }

    /// Save; `as_new` (or a project never saved) asks where first. `then` runs after a save.
    pub fn save(&mut self, as_new: bool, then: Option<Next>) {
        match self.studio.project_path().map(|p| p.to_path_buf()) {
            Some(p) if !as_new => {
                self.save_to(&p);
                if let Some(n) = then
                    && !self.studio.is_dirty()
                {
                    self.proceed(n);
                }
            }
            _ => {
                self.after_save = then;
                self.ask(Purpose::SaveProject);
            }
        }
    }

    pub fn save_to(&mut self, path: &std::path::Path) {
        let p = videonow_core::project::project_dir(&path.to_string_lossy());
        match self.run("ui", "studio:project.save", json!({"path": p.to_string_lossy()})) {
            Ok(_) => self.say(format!("Saved {}", p.display()), false),
            Err(e) => self.say(format!("Could not save: {e}"), true),
        }
    }

    pub fn open_path(&mut self, path: &std::path::Path) {
        match self.run("ui", "studio:project.open", json!({"path": path.to_string_lossy()})) {
            Ok(r) => {
                self.say(format!("Opened {} ({} assets)", path.display(), r["result"]["assets"]), false);
            }
            Err(e) => self.say(format!("Could not open {}: {e}", path.display()), true),
        }
    }

    pub fn build_demo(&mut self) {
        let r = self.studio.as_actor("demo", None, videonow_core::demo::build_quietly);
        match r {
            Ok(_) => self.say("Demo project: a badge image inside an animated title on a sequence", false),
            Err(e) => self.say(format!("demo: {e}"), true),
        }
    }

    fn ask(&mut self, purpose: Purpose) {
        if !self.options.dialogs {
            self.say("File dialogs are off in this window", true);
            return;
        }
        // set by `ui` before any request can arrive
        let ctx = self.ctx.clone();
        if let Some(ctx) = ctx {
            let start = self.studio.project_path().and_then(|p| p.parent()).map(|p| p.to_path_buf());
            self.dialog = Some(dialogs::show(purpose, start, &ctx));
        }
    }

    fn poll_dialog(&mut self) {
        let Some(d) = &self.dialog else { return };
        let Some(answer) = d.poll() else { return };
        let purpose = d.purpose;
        self.dialog = None;
        let then = self.after_save.take();
        match (purpose, answer) {
            (Purpose::OpenProject, Some(p)) => self.open_path(&p),
            (Purpose::SaveProject, Some(p)) => {
                self.save_to(&p);
                if let Some(n) = then
                    && !self.studio.is_dirty()
                {
                    self.proceed(n);
                }
            }
            (_, None) => {}
        }
    }

    /// What the editors asked the shell for (their Save, Open, New items).
    fn host_requests(&mut self) {
        for (domain, id, _params) in lend::take_host_requests(&mut self.studio) {
            log::info!("{domain} editor: {id} handled by the shell");
            match id.as_str() {
                "file.save" | "file.saveAll" => self.save(false, None),
                "file.saveAs" => self.save(true, None),
                "file.open" | "file.openRecent" | "file.revert" => self.request(Next::Open(None)),
                "file.newProject" | "file.newProjectFromTemplate" | "file.newFromTemplate" | "file.closeProject" | "file.closeAllProjects" => self.request(Next::New),
                "file.openDemoProject" => self.request(Next::Demo),
                other => self.say(format!("{domain}:{other} is not available in VideoNow"), true),
            }
        }
    }

    /// After an editor ran: its requests to the shell, then the studio accounts for its edits.
    fn after_editor(&mut self) {
        self.host_requests();
        self.studio.sync_editors(self.workspace.id());
    }

    fn run_agent_jobs(&mut self) {
        let jobs = self.agent.take_jobs(64);
        if jobs.is_empty() {
            return;
        }
        let policy = self.agent.permissions.policy(self.studio.project_path());
        for job in jobs {
            let r = self.studio.as_actor("agent", Some(policy.clone()), |s| std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| job(s))));
            if r.is_err() {
                self.say("An agent command failed unexpectedly; the project is unchanged by it", true);
            }
        }
    }

    fn lent_failed(&mut self, what: &str, e: String) {
        self.say(format!("The {what} editor hit an error ({e}); your project is still open. Save to be safe."), true);
    }

    /// Lend the active editor its session for `f`.
    fn with_editor(&mut self, f: impl FnOnce(&mut dyn eframe::App)) {
        let r = match self.workspace {
            Workspace::Edit => lend::film(&mut self.studio, &mut self.film, |a| f(a)),
            Workspace::Motion => lend::motion(&mut self.studio, &mut self.motion, |a| f(a)),
            Workspace::Image => lend::photo(&mut self.studio, &mut self.photo, |a| f(a)),
        };
        if let Err(e) = r {
            let what = self.workspace.label().to_ascii_lowercase();
            self.lent_failed(&what, e);
        }
    }

    /// Turn agent access on (port 0: any free port); returns where it listens and the token a
    /// client sends first.
    pub fn start_agent(&mut self, port: u16, ctx: &egui::Context) -> Option<(std::net::SocketAddr, String)> {
        self.agent.start(port, ctx);
        Some((self.agent.addr?, self.agent.token.clone()?))
    }

    /// Let the agent run destructive commands, or not (the Agent panel's checkbox).
    pub fn allow_agent_destructive(&mut self, allow: bool) {
        self.agent.permissions.destructive = allow;
    }

    pub fn quitting(&self) -> bool {
        self.quitting
    }
}

/// `~/.config/videonow` (or the platform's equivalent).
pub fn config_dir() -> Option<PathBuf> {
    if let Some(d) = std::env::var_os("VIDEONOW_CONFIG_DIR") {
        return Some(PathBuf::from(d));
    }
    if let Some(d) = std::env::var_os("XDG_CONFIG_HOME") {
        return Some(PathBuf::from(d).join("videonow"));
    }
    if let Some(d) = std::env::var_os("APPDATA") {
        return Some(PathBuf::from(d).join("VideoNow"));
    }
    std::env::var_os("HOME").map(|h| {
        let h = PathBuf::from(h);
        if cfg!(target_os = "macos") { h.join("Library/Application Support/VideoNow") } else { h.join(".config/videonow") }
    })
}

fn install_film_hooks(app: &mut FilmcraftApp) {
    app.hooks.pick_files = Some(Box::new(|exts: &[&str]| {
        rfd::FileDialog::new().add_filter("Media", exts).pick_files().unwrap_or_default().into_iter().map(|p| p.to_string_lossy().to_string()).collect()
    }));
    app.hooks.pick_file_for_relink = Some(Box::new(|exts: &[&str], _hint| rfd::FileDialog::new().add_filter("Media", exts).pick_file().map(|p| p.to_string_lossy().to_string())));
    app.hooks.pick_save_as = Some(Box::new(|filter: &str, exts: &[&str], name: &str| {
        let path = std::path::Path::new(name);
        let mut d = rfd::FileDialog::new().add_filter(filter, exts).set_file_name(path.file_name().map_or(name.into(), |n| n.to_string_lossy()));
        if let Some(dir) = path.parent().filter(|d| d.is_dir()) {
            d = d.set_directory(dir);
        }
        d.save_file().map(|p| p.to_string_lossy().to_string())
    }));
    app.hooks.pick_folder = Some(Box::new(|| rfd::FileDialog::new().pick_folder().map(|p| p.to_string_lossy().to_string())));
    app.hooks.pick_open_file = Some(Box::new(|filter: &str, exts: &[&str]| rfd::FileDialog::new().add_filter(filter, exts).pick_file().map(|p| p.to_string_lossy().to_string())));
    app.hooks.open_url = Some(Box::new(|url: &str| open::that_detached(url).map_err(|e| e.to_string())));
}

fn install_motion_hooks(app: &mut EffectcraftApp) {
    app.hooks.pick_files = Some(Box::new(|exts: &[&str]| {
        rfd::FileDialog::new().add_filter("Media", exts).pick_files().unwrap_or_default().into_iter().map(|p| p.to_string_lossy().to_string()).collect()
    }));
    app.hooks.pick_folder = Some(Box::new(|| rfd::FileDialog::new().pick_folder().map(|p| p.to_string_lossy().to_string())));
    app.hooks.pick_save_file = Some(Box::new(|name: &str, ext: &str| {
        let path = std::path::Path::new(name);
        let mut d = if ext.is_empty() { rfd::FileDialog::new() } else { rfd::FileDialog::new().add_filter(ext, &[ext]) };
        if let Some(dir) = path.parent().filter(|d| d.is_dir()) {
            d = d.set_directory(dir);
        }
        let file = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        d.set_file_name(file).save_file().map(|p| p.to_string_lossy().to_string())
    }));
}

impl eframe::App for VideoNowApp {
    fn raw_input_hook(&mut self, ctx: &egui::Context, raw: &mut egui::RawInput) {
        // the window's close button: the shell asks about unsaved changes, not one editor
        if !self.quitting
            && let Some(v) = raw.viewports.get_mut(&egui::ViewportId::ROOT)
        {
            let n = v.events.len();
            v.events.retain(|e| !matches!(e, egui::ViewportEvent::Close));
            if v.events.len() != n && self.confirm.is_none() {
                self.request(Next::Quit);
            }
        }
        // the shell's own shortcuts, kept from the editors
        let mut palette = false;
        raw.events.retain(|e| match e {
            egui::Event::Key { key: egui::Key::P, pressed: true, modifiers, .. } if modifiers.command && modifiers.shift => {
                palette = true;
                false
            }
            egui::Event::Key { key: egui::Key::F1, pressed: true, .. } => {
                palette = true;
                false
            }
            _ => true,
        });
        if palette {
            self.palette.toggle();
        }
        self.with_editor(|a| a.raw_input_hook(ctx, raw));
    }

    fn logic(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        self.ctx = Some(ctx.clone());
        self.now = ctx.input(|i| i.time);
        if !self.started {
            self.started = true;
            self.apply_look(ctx);
        }
        self.poll_dialog();
        self.run_agent_jobs();
        if self.quitting {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }
        self.with_editor(|a| a.logic(ctx, frame));
        self.after_editor();
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.top_bar(ui);
        if self.show_assets {
            self.assets_panel(ui);
        }
        if self.show_agent {
            self.agent_panel(ui);
        }
        egui::CentralPanel::no_frame().show(ui, |ui| {
            self.with_editor(|a| a.ui(ui, frame));
        });
        self.after_editor();
        if let Some((id, p, dry)) = self.palette.show(&ctx, self.here()) {
            let req = videonow_core::Request { dry_run: dry, ..Default::default() };
            let r = self.studio.as_actor("palette", None, |s| s.execute_with(&id, p, &req)).map_err(|e| e.to_string());
            if let Err(e) = &r {
                self.say(e.clone(), true);
            }
            self.palette.set_result(r);
        }
        self.confirm_window(&ctx);
        self.status_toast(&ctx);
    }

    fn on_exit(&mut self) {
        self.agent.stop();
        for w in Workspace::ALL {
            self.workspace = w;
            self.with_editor(|a| a.on_exit());
        }
    }
}
