//! Projects, as Painter's File menu: new, open, recent files, close, save, save as, save as
//! copy, the question about unsaved changes, the project configuration and autosave.
//!
//! Saving and opening run on a thread of their own; the model is cloned for saving, which
//! is cheap (strokes and images are shared).

use super::{InkApp, RESOLUTIONS};
use eframe::egui::{self, Key, RichText};
use eframe::egui_wgpu::RenderState;
use ink_core::export::ExportSettings;
use ink_core::{Mesh, Model, NormalFormat};
use ink_io::project::{ImageCache, PROJECT_EXTENSION, Project, ViewState};
use ink_render::ShadingMode;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// Projects listed under Recent files.
const RECENT_KEPT: usize = 10;
/// How often a project with unsaved changes is backed up.
const AUTOSAVE_INTERVAL: Duration = Duration::from_secs(10 * 60);
/// Backups kept per project.
const AUTOSAVES_KEPT: usize = 3;

/// What replaces or ends the current project, once the question about its unsaved changes
/// is answered.
pub(super) enum Leave {
    /// A new project with this model (made from a mesh file, or a primitive).
    Replace {
        model: Box<Model>,
        source: Option<String>,
    },
    Open(PathBuf),
    Close,
    Quit,
}

/// The New Project dialog's fields.
#[derive(Clone, Debug)]
pub(super) struct NewProject {
    pub file: String,
    pub resolution: u32,
    pub normal_format: NormalFormat,
}

impl Default for NewProject {
    fn default() -> Self {
        Self {
            file: String::new(),
            resolution: ink_core::TextureSet::DEFAULT_RESOLUTION,
            normal_format: NormalFormat::default(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SaveKind {
    /// Save or Save as: the project lives at the path afterwards.
    Save,
    /// Save as copy: the project stays where it was.
    Copy,
    /// A backup; changes count as unsaved still.
    Autosave,
}

struct Saving {
    path: PathBuf,
    kind: SaveKind,
    /// What was saved, see [`InkApp::project_state`].
    state: (u64, u64),
    /// What to do once the save went well.
    then: Option<Leave>,
    job: JoinHandle<(anyhow::Result<()>, ImageCache)>,
}

type Opening = (PathBuf, JoinHandle<anyhow::Result<(Project, ImageCache)>>);

pub(super) struct ProjectState {
    /// Where the project is saved; None until it is.
    pub path: Option<PathBuf>,
    /// The mesh file it was made from.
    pub source: Option<String>,
    /// [`InkApp::project_state`] at the last save; different means unsaved changes.
    saved: (u64, u64),
    /// ... and at the last backup.
    autosaved: (u64, u64),
    last_backup: Instant,
    /// Encoded images of the last save; with the save thread while it runs.
    images: ImageCache,
    saving: Option<Saving>,
    opening: Option<Opening>,
    /// Waiting for the answer to "save the changes?".
    leave: Option<Leave>,
    /// To do once the frame can: what followed a save that another one waited for.
    after_save: Option<Leave>,
    /// The window may close: the question was answered.
    quit: bool,
    new_dialog: Option<NewProject>,
    /// The Project configuration window's normal map format while it is open.
    configuration: Option<NormalFormat>,
    pub recent: Vec<PathBuf>,
    /// The export window's settings, saved with the project.
    pub export: ExportSettings,
    title: String,
}

impl Default for ProjectState {
    fn default() -> Self {
        Self {
            path: None,
            source: None,
            saved: (0, 0),
            autosaved: (0, 0),
            last_backup: Instant::now(),
            images: ImageCache::default(),
            saving: None,
            opening: None,
            leave: None,
            after_save: None,
            quit: false,
            new_dialog: None,
            configuration: None,
            recent: load_recent(),
            export: ExportSettings::default(),
            title: String::new(),
        }
    }
}

impl ProjectState {
    pub fn is_busy(&self) -> bool {
        self.saving.is_some() || self.opening.is_some()
    }

    /// Whether a dialog of this module is open.
    pub fn dialog_open(&self) -> bool {
        self.leave.is_some() || self.new_dialog.is_some() || self.configuration.is_some()
    }

    fn add_recent(&mut self, path: &Path) {
        self.recent.retain(|p| p != path);
        self.recent.insert(0, path.to_owned());
        self.recent.truncate(RECENT_KEPT);
        save_recent(&self.recent);
    }
}

impl InkApp {
    /// The project's name: its file name, "Untitled" before it is saved.
    pub(super) fn project_name(&self) -> String {
        self.project
            .path
            .as_ref()
            .and_then(|p| p.file_stem())
            .map_or_else(
                || "Untitled".to_owned(),
                |s| s.to_string_lossy().into_owned(),
            )
    }

    /// Identifies what a save writes: the history's state and a hash of the settings that
    /// are no undo steps (texture set settings, visibility, the normal map format).
    fn project_state(&self) -> (u64, u64) {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        let model = &self.model;
        model.normal_format.hash(&mut hasher);
        for set in &model.texture_sets {
            (&set.name, set.visible, set.resolution, set.alpha_mode as u8).hash(&mut hasher);
        }
        for object in &model.objects {
            object.visible.hash(&mut hasher);
        }
        let export = &self.project.export;
        (
            &export.directory,
            export.template.key,
            export.format,
            export.bit_depth,
            export.padding,
            export.dilation,
        )
            .hash(&mut hasher);
        (self.history.state(), hasher.finish())
    }

    pub(super) fn has_unsaved_changes(&self) -> bool {
        self.project_state() != self.project.saved
    }

    pub(super) fn mark_saved(&mut self) {
        let state = self.project_state();
        self.project.saved = state;
        self.project.autosaved = state;
        self.project.last_backup = Instant::now();
    }

    /// Replaces or ends the project, asking first if it has unsaved changes.
    pub(super) fn leave_project(&mut self, rs: &RenderState, leave: Leave) {
        if self.has_unsaved_changes() {
            self.project.leave = Some(leave);
        } else {
            self.do_leave(rs, leave);
        }
    }

    fn do_leave(&mut self, rs: &RenderState, leave: Leave) {
        match leave {
            Leave::Replace { model, source } => {
                self.set_model(rs, *model);
                self.project.path = None;
                self.project.source = source;
                self.project.export = ExportSettings::default();
                self.project.images = ImageCache::default();
                self.mark_saved();
            }
            Leave::Open(path) => self.start_open(path),
            Leave::Close => {
                self.set_model(rs, Model::from_mesh(Mesh::default()));
                self.project.path = None;
                self.project.source = None;
                self.project.export = ExportSettings::default();
                self.project.images = ImageCache::default();
                self.mark_saved();
                self.status = "Project closed".into();
            }
            Leave::Quit => {
                self.project.quit = true;
                self.quit_requested = true;
            }
        }
    }

    /// A new project from a mesh file with the New Project dialog's settings.
    fn new_project(&mut self, rs: &RenderState, settings: &NewProject) {
        let path = Path::new(&settings.file);
        match ink_io::load_model(path) {
            Ok(mut model) => {
                for set in &mut model.texture_sets {
                    set.resolution = settings.resolution;
                }
                model.normal_format = settings.normal_format;
                self.status = format!(
                    "New project from {} ({} objects, {} texture sets, {} triangles)",
                    path.display(),
                    model.objects.len(),
                    model.texture_sets.len(),
                    model.mesh.triangle_count()
                );
                self.leave_project(
                    rs,
                    Leave::Replace {
                        model: Box::new(model),
                        source: Some(settings.file.clone()),
                    },
                );
            }
            Err(err) => {
                log::error!("{err:#}");
                self.status = format!("Error: {err:#}");
            }
        }
    }

    /// Opens the New Project dialog for a mesh file, e.g. one dropped onto the window.
    pub(super) fn ask_new_project(&mut self, file: Option<&Path>) {
        self.project.new_dialog = Some(NewProject {
            file: file.map(|f| f.display().to_string()).unwrap_or_default(),
            ..Default::default()
        });
    }

    /// Starts reading a project file; [`Self::poll_project_jobs`] shows it once read.
    pub(super) fn start_open(&mut self, path: PathBuf) {
        if self.project.opening.is_some() {
            return;
        }
        self.status = format!("Opening {}…", path.display());
        let file = path.clone();
        let job = std::thread::spawn(move || ink_io::project::load(&file));
        self.project.opening = Some((path, job));
    }

    fn open_finished(
        &mut self,
        rs: &RenderState,
        path: PathBuf,
        result: anyhow::Result<(Project, ImageCache)>,
    ) {
        match result {
            Ok((project, images)) => {
                let Project {
                    model,
                    source,
                    view,
                    export,
                } = project;
                self.status = format!(
                    "Opened {} ({} texture sets)",
                    path.display(),
                    model.texture_sets.len()
                );
                self.set_model(rs, model);
                if let Some(view) = view {
                    self.apply_view_state(rs, &view);
                }
                self.project.export = export.unwrap_or_default();
                self.project.path = Some(path.clone());
                self.project.source = source;
                self.project.images = images;
                self.project.add_recent(&path);
                self.mark_saved();
            }
            Err(err) => {
                log::error!("{err:#}");
                self.status = format!("Error: {err:#}");
                // A recent file that can't be opened leaves the list.
                if !path.exists() {
                    self.project.recent.retain(|p| p != &path);
                    save_recent(&self.project.recent);
                }
            }
        }
    }

    /// Ctrl+S: saves in place, or asks where if the project was never saved.
    pub(super) fn save_project(&mut self, then: Option<Leave>) {
        match self.project.path.clone() {
            Some(path) => self.start_save(path, SaveKind::Save, then),
            None => self.save_project_as(then),
        }
    }

    fn save_project_as(&mut self, then: Option<Leave>) {
        if let Some(path) = self.pick_project_path("Save project as") {
            self.start_save(path, SaveKind::Save, then);
        }
    }

    fn save_project_copy(&mut self) {
        if let Some(path) = self.pick_project_path("Save project as copy") {
            self.start_save(path, SaveKind::Copy, None);
        }
    }

    fn pick_project_path(&self, title: &str) -> Option<PathBuf> {
        let mut dialog = rfd::FileDialog::new()
            .set_title(title)
            .add_filter("inkpainter3d projects", &[PROJECT_EXTENSION]);
        let suggested = self.project.path.clone().or_else(|| {
            self.project
                .source
                .as_ref()
                .map(|s| ink_io::project::default_path(Path::new(s)))
        });
        if let Some(path) = &suggested {
            if let Some(dir) = path.parent() {
                dialog = dialog.set_directory(dir);
            }
            if let Some(name) = path.file_name() {
                dialog = dialog.set_file_name(name.to_string_lossy());
            }
        }
        let path = dialog.save_file()?;
        // Linux file dialogs don't add the extension.
        Some(
            if path.extension().is_some_and(|e| e == PROJECT_EXTENSION) {
                path
            } else {
                let mut name = path.into_os_string();
                name.push(".");
                name.push(PROJECT_EXTENSION);
                PathBuf::from(name)
            },
        )
    }

    fn start_save(&mut self, path: PathBuf, kind: SaveKind, then: Option<Leave>) {
        // One save at a time: the images go with it.
        self.wait_for_save();
        let project = Project {
            model: self.model.clone(),
            source: self.project.source.clone(),
            view: Some(self.view_state()),
            export: Some(self.project.export.clone()),
        };
        let mut images = std::mem::take(&mut self.project.images);
        let file = path.clone();
        let job = std::thread::spawn(move || {
            let result = ink_io::project::save(&file, &project, &mut images);
            (result, images)
        });
        if kind != SaveKind::Autosave {
            self.status = format!("Saving {}…", path.display());
        }
        self.project.saving = Some(Saving {
            path,
            kind,
            state: self.project_state(),
            then,
            job,
        });
    }

    /// Blocks until the save in progress is done. What was to follow it waits for the
    /// next frame.
    fn wait_for_save(&mut self) {
        if let Some(saving) = self.project.saving.take()
            && let Some(then) = self.save_finished(saving)
        {
            self.project.after_save = Some(then);
        }
    }

    /// Waits for a save and takes its result. Returns what was to follow it, if it went
    /// well.
    fn save_finished(&mut self, saving: Saving) -> Option<Leave> {
        let Saving {
            path,
            kind,
            state,
            then,
            job,
        } = saving;
        let result = job.join();
        let result = match result {
            Ok((result, images)) => {
                self.project.images = images;
                result
            }
            Err(_) => Err(anyhow::anyhow!("Saving failed")),
        };
        match result {
            Ok(()) => match kind {
                SaveKind::Save => {
                    self.project.path = Some(path.clone());
                    self.project.saved = state;
                    self.project.autosaved = state;
                    self.project.last_backup = Instant::now();
                    self.project.add_recent(&path);
                    self.status = format!("Saved {}", path.display());
                    return then;
                }
                SaveKind::Copy => self.status = format!("Saved a copy as {}", path.display()),
                SaveKind::Autosave => {
                    self.project.autosaved = state;
                    prune_backups(&path);
                    log::info!("Autosaved {}", path.display());
                }
            },
            Err(err) => {
                log::error!("{err:#}");
                self.status = format!("Error: {err:#}");
            }
        }
        None
    }

    /// Picks up finished saves and opened projects, and backs the project up when it is
    /// time. Call once per frame.
    pub(super) fn poll_project_jobs(&mut self, ui: &egui::Ui, rs: &RenderState) {
        if self
            .project
            .saving
            .as_ref()
            .is_some_and(|s| s.job.is_finished())
            && let Some(saving) = self.project.saving.take()
            && let Some(then) = self.save_finished(saving)
        {
            self.do_leave(rs, then);
        }
        if let Some(then) = self.project.after_save.take() {
            self.do_leave(rs, then);
        }
        if self
            .project
            .opening
            .as_ref()
            .is_some_and(|(_, job)| job.is_finished())
            && let Some((path, job)) = self.project.opening.take()
        {
            let result = job
                .join()
                .unwrap_or_else(|_| Err(anyhow::anyhow!("Opening failed")));
            self.open_finished(rs, path, result);
        }
        if self.project.is_busy() {
            ui.ctx().request_repaint_after(Duration::from_millis(100));
        }
        self.autosave();
    }

    /// Backs the project up every few minutes while it has unsaved changes, as Painter's
    /// autosave: next to the project, or in the data folder before it was saved.
    fn autosave(&mut self) {
        if self.project.last_backup.elapsed() < AUTOSAVE_INTERVAL
            || self.project.is_busy()
            || self.project.dialog_open()
            || self.paint.is_busy()
            || self.model.texture_sets.is_empty()
        {
            return;
        }
        self.project.last_backup = Instant::now();
        let state = self.project_state();
        if state == self.project.saved || state == self.project.autosaved {
            return;
        }
        let name = self.project_name();
        let dir = match &self.project.path {
            Some(path) => path.parent().map(Path::to_owned),
            None => data_dir().map(|d| d.join("autosave")),
        };
        let Some(dir) = dir else { return };
        if std::fs::create_dir_all(&dir).is_err() {
            return;
        }
        let path = dir.join(format!(
            "{name}{BACKUP_MARK}{}.{PROJECT_EXTENSION}",
            timestamp()
        ));
        self.start_save(path, SaveKind::Autosave, None);
    }

    /// The camera and display settings saved with the project.
    fn view_state(&self) -> ViewState {
        let (c, s) = (&self.camera, &self.settings);
        ViewState {
            camera_target: c.target.to_array(),
            camera_yaw: c.yaw,
            camera_pitch: c.pitch,
            camera_distance: c.distance,
            camera_fov_y: c.fov_y,
            orthographic: c.orthographic,
            stylized: s.shading == ShadingMode::Toon,
            environment: self
                .environment_path
                .as_ref()
                .map(|p| p.display().to_string()),
            environment_rotation: s.environment_rotation,
            exposure: s.exposure,
            background_opacity: s.background_opacity,
            background_blur: s.background_blur,
            toon_bands: s.toon_bands,
            outline: s.outline,
            outline_color: s.outline_color,
            outline_width: s.outline_width,
        }
    }

    fn apply_view_state(&mut self, rs: &RenderState, view: &ViewState) {
        let c = &mut self.camera;
        c.target = glam::Vec3::from_array(view.camera_target);
        c.yaw = view.camera_yaw;
        c.pitch = view.camera_pitch;
        c.distance = view.camera_distance;
        c.fov_y = view.camera_fov_y;
        c.orthographic = view.orthographic;
        let s = &mut self.settings;
        s.shading = if view.stylized {
            ShadingMode::Toon
        } else {
            ShadingMode::Pbr
        };
        s.environment_rotation = view.environment_rotation;
        s.exposure = view.exposure;
        s.background_opacity = view.background_opacity;
        s.background_blur = view.background_blur;
        s.toon_bands = view.toon_bands;
        s.outline = view.outline;
        s.outline_color = view.outline_color;
        s.outline_width = view.outline_width;
        match &view.environment {
            // A panorama that is gone leaves the current one.
            Some(path) if self.environment_path.as_deref() != Some(Path::new(path)) => {
                self.load_environment(rs, Path::new(path));
            }
            None if self.environment_path.is_some() => self.load_default_environment(rs),
            _ => {}
        }
    }

    // ---- UI ----

    /// The project entries of the File menu.
    pub(super) fn project_menu(&mut self, ui: &mut egui::Ui, rs: &RenderState) {
        if ui
            .add(egui::Button::new("New…").shortcut_text("Ctrl+N"))
            .clicked()
        {
            self.ask_new_project(None);
        }
        if ui
            .add(egui::Button::new("Open…").shortcut_text("Ctrl+O"))
            .clicked()
        {
            self.pick_and_open(rs);
        }
        ui.add_enabled_ui(!self.project.recent.is_empty(), |ui| {
            ui.menu_button("Recent files", |ui| {
                for path in self.project.recent.clone() {
                    let name = path
                        .file_name()
                        .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
                    if ui
                        .button(name)
                        .on_hover_text(path.display().to_string())
                        .clicked()
                    {
                        self.leave_project(rs, Leave::Open(path));
                    }
                }
            });
        });
        if ui
            .add(egui::Button::new("Close").shortcut_text("Ctrl+F4"))
            .clicked()
        {
            self.leave_project(rs, Leave::Close);
        }
        ui.separator();
        let has_model = !self.model.texture_sets.is_empty();
        if ui
            .add_enabled(has_model, egui::Button::new("Save").shortcut_text("Ctrl+S"))
            .clicked()
        {
            self.save_project(None);
        }
        if ui
            .add_enabled(has_model, egui::Button::new("Save as…"))
            .clicked()
        {
            self.save_project_as(None);
        }
        if ui
            .add_enabled(has_model, egui::Button::new("Save as copy…"))
            .clicked()
        {
            self.save_project_copy();
        }
    }

    fn pick_and_open(&mut self, rs: &RenderState) {
        let mut dialog =
            rfd::FileDialog::new().add_filter("inkpainter3d projects", &[PROJECT_EXTENSION]);
        if let Some(dir) = self.project.recent.first().and_then(|p| p.parent()) {
            dialog = dialog.set_directory(dir);
        }
        if let Some(path) = dialog.pick_file() {
            self.leave_project(rs, Leave::Open(path));
        }
    }

    /// Ctrl+N, Ctrl+O, Ctrl+S and Ctrl+F4, as in Painter.
    pub(super) fn project_shortcuts(&mut self, ui: &egui::Ui, rs: &RenderState) {
        if self.project.dialog_open() || ui.ctx().text_edit_focused() {
            return;
        }
        let command = ui.input(|i| i.modifiers.command && !i.modifiers.shift && !i.modifiers.alt);
        let pressed = |key| ui.input(|i| i.key_pressed(key));
        if command && pressed(Key::N) {
            self.ask_new_project(None);
        }
        if command && pressed(Key::O) {
            self.pick_and_open(rs);
        }
        // Not in the middle of a stroke: it is recorded when it ends.
        if command
            && pressed(Key::S)
            && !self.model.texture_sets.is_empty()
            && !self.paint.is_busy()
        {
            self.save_project(None);
        }
        if command && pressed(Key::F4) {
            self.leave_project(rs, Leave::Close);
        }
    }

    /// The window's close button and Alt+F4 ask about unsaved changes first. Before the
    /// window closes, saves and exported files are finished.
    pub(super) fn handle_close_request(&mut self, ui: &egui::Ui) {
        if !ui.input(|i| i.viewport().close_requested()) {
            return;
        }
        if !self.project.quit && self.has_unsaved_changes() {
            ui.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            if self.project.leave.is_none() {
                self.project.leave = Some(Leave::Quit);
            }
        } else {
            self.wait_for_save();
            self.stop_export();
        }
    }

    /// Shows the project's name in the window title, with a star for unsaved changes.
    pub(super) fn update_title(&mut self, ui: &egui::Ui) {
        let star = if self.has_unsaved_changes() { "*" } else { "" };
        let title = format!("{}{star} - inkpainter3d", self.project_name());
        if title != self.project.title {
            ui.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.project.title = title;
        }
    }

    /// The dialogs: New Project, Project configuration and "save the changes?".
    pub(super) fn project_dialogs(&mut self, ui: &mut egui::Ui, rs: &RenderState) {
        self.new_project_dialog(ui, rs);
        self.configuration_dialog(ui);
        self.unsaved_changes_dialog(ui, rs);
    }

    fn new_project_dialog(&mut self, ui: &mut egui::Ui, rs: &RenderState) {
        let Some(mut settings) = self.project.new_dialog.take() else {
            return;
        };
        let mut create = false;
        let mut cancel = false;
        let modal = egui::Modal::new(egui::Id::new("new project")).show(ui.ctx(), |ui| {
            ui.set_width(460.0);
            ui.heading("New project");
            ui.add_space(8.0);
            egui::Grid::new("new project grid")
                .num_columns(2)
                .spacing([12.0, 8.0])
                .show(ui, |ui| {
                    ui.label("File");
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut settings.file)
                                .desired_width(300.0)
                                .hint_text("Mesh file (FBX, glTF, OBJ)"),
                        );
                        if ui.button("Select…").clicked()
                            && let Some(path) =
                                Self::pick_file("3D models", ink_io::SUPPORTED_EXTENSIONS)
                        {
                            settings.file = path.display().to_string();
                        }
                    });
                    ui.end_row();
                    ui.label("Resolution");
                    egui::ComboBox::from_id_salt("new project resolution")
                        .selected_text(format!("{0} × {0}", settings.resolution))
                        .show_ui(ui, |ui| {
                            for r in RESOLUTIONS {
                                ui.selectable_value(
                                    &mut settings.resolution,
                                    r,
                                    format!("{r} × {r}"),
                                );
                            }
                        });
                    ui.end_row();
                    ui.label("Normal map format");
                    normal_format_combo(ui, "new project normals", &mut settings.normal_format);
                    ui.end_row();
                });
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                let valid = !settings.file.trim().is_empty();
                create = ui.add_enabled(valid, egui::Button::new("OK")).clicked()
                    || (valid && ui.input(|i| i.key_pressed(Key::Enter)));
                cancel = ui.button("Cancel").clicked();
            });
        });
        if create {
            self.new_project(rs, &settings);
        } else if !cancel && !modal.should_close() {
            self.project.new_dialog = Some(settings);
        }
    }

    /// Edit > Project configuration: the mesh the project was made from and the normal map
    /// format.
    fn configuration_dialog(&mut self, ui: &mut egui::Ui) {
        let Some(mut format) = self.project.configuration.take() else {
            return;
        };
        let mut apply = false;
        let mut cancel = false;
        let modal = egui::Modal::new(egui::Id::new("project configuration")).show(ui.ctx(), |ui| {
            ui.set_width(420.0);
            ui.heading("Project configuration");
            ui.add_space(8.0);
            egui::Grid::new("project configuration grid")
                .num_columns(2)
                .spacing([12.0, 8.0])
                .show(ui, |ui| {
                    ui.label("Mesh");
                    ui.label(self.project.source.as_deref().unwrap_or("—"));
                    ui.end_row();
                    ui.label("Normal map format");
                    normal_format_combo(ui, "project normals", &mut format);
                    ui.end_row();
                });
            ui.label(
                RichText::new(
                    "The format of the document channels' normal map in exports. Normal maps \
                     in layers keep their own format.",
                )
                .small()
                .weak(),
            );
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                apply = ui.button("OK").clicked();
                cancel = ui.button("Cancel").clicked();
            });
        });
        if apply {
            self.model.normal_format = format;
        } else if !cancel && !modal.should_close() {
            self.project.configuration = Some(format);
        }
    }

    pub(super) fn open_configuration(&mut self) {
        self.project.configuration = Some(self.model.normal_format);
    }

    fn unsaved_changes_dialog(&mut self, ui: &mut egui::Ui, rs: &RenderState) {
        if self.project.leave.is_none() {
            return;
        }
        let name = self.project_name();
        let mut answer = None;
        let modal = egui::Modal::new(egui::Id::new("unsaved changes")).show(ui.ctx(), |ui| {
            ui.set_width(380.0);
            ui.heading("Unsaved changes");
            ui.add_space(6.0);
            ui.label(format!("Save the changes to “{name}” first?"));
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if ui.button("Save").clicked() {
                    answer = Some(true);
                }
                if ui.button("Don't save").clicked() {
                    answer = Some(false);
                }
                if ui.button("Cancel").clicked() {
                    answer = None;
                    self.project.leave = None;
                }
            });
        });
        if modal.should_close() {
            self.project.leave = None;
        }
        match answer {
            Some(true) => {
                let leave = self.project.leave.take();
                self.save_project(leave);
            }
            Some(false) => {
                if let Some(leave) = self.project.leave.take() {
                    self.do_leave(rs, leave);
                }
            }
            None => {}
        }
    }
}

fn normal_format_combo(ui: &mut egui::Ui, id: &str, format: &mut NormalFormat) {
    egui::ComboBox::from_id_salt(id)
        .selected_text(format.label())
        .show_ui(ui, |ui| {
            for f in [NormalFormat::DirectX, NormalFormat::OpenGl] {
                ui.selectable_value(format, f, f.label());
            }
        });
}

/// Marks backups in file names: `Cat_autosave_2026-10-12_09-30-00.ink3d`.
const BACKUP_MARK: &str = "_autosave_";

/// Deletes all but the newest backups of the project the backup belongs to.
fn prune_backups(backup: &Path) {
    let (Some(dir), Some(name)) = (backup.parent(), backup.file_name()) else {
        return;
    };
    let name = name.to_string_lossy();
    let Some(prefix) = name
        .find(BACKUP_MARK)
        .map(|i| &name[..i + BACKUP_MARK.len()])
    else {
        return;
    };
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let suffix = format!(".{PROJECT_EXTENSION}");
    let mut backups: Vec<PathBuf> = entries
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.file_name()
                .map(|n| n.to_string_lossy())
                .is_some_and(|n| n.starts_with(prefix) && n.ends_with(&suffix))
        })
        .collect();
    // The timestamps sort by name.
    backups.sort();
    let excess = backups.len().saturating_sub(AUTOSAVES_KEPT);
    for old in &backups[..excess] {
        let _ = std::fs::remove_file(old);
    }
}

/// The current time (UTC) for file names: `2026-10-12_09-30-00`.
fn timestamp() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs()) as i64;
    let (days, time) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Civil date from days since 1970 (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}_{:02}-{:02}-{:02}",
        time / 3600,
        time % 3600 / 60,
        time % 60
    )
}

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .map(PathBuf::from)
}

/// Where settings go: `~/.config/inkpainter3d` on Linux, `%APPDATA%\inkpainter3d` on
/// Windows.
fn config_dir() -> Option<PathBuf> {
    let base = if cfg!(windows) {
        std::env::var_os("APPDATA").map(PathBuf::from)
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .filter(|d| !d.is_empty())
            .map(PathBuf::from)
            .or_else(|| home().map(|h| h.join(".config")))
    };
    base.map(|b| b.join("inkpainter3d"))
}

/// Where data goes: `~/.local/share/inkpainter3d` on Linux, `%LOCALAPPDATA%\inkpainter3d`
/// on Windows.
fn data_dir() -> Option<PathBuf> {
    let base = if cfg!(windows) {
        std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
    } else {
        std::env::var_os("XDG_DATA_HOME")
            .filter(|d| !d.is_empty())
            .map(PathBuf::from)
            .or_else(|| home().map(|h| h.join(".local").join("share")))
    };
    base.map(|b| b.join("inkpainter3d"))
}

/// Recent files, one path per line.
fn load_recent() -> Vec<PathBuf> {
    config_dir()
        .and_then(|d| std::fs::read_to_string(d.join("recent-files.txt")).ok())
        .map(|text| {
            text.lines()
                .filter(|l| !l.is_empty())
                .map(PathBuf::from)
                .take(RECENT_KEPT)
                .collect()
        })
        .unwrap_or_default()
}

fn save_recent(recent: &[PathBuf]) {
    let Some(dir) = config_dir() else { return };
    let text: String = recent
        .iter()
        .map(|p| format!("{}\n", p.display()))
        .collect();
    if let Err(err) = std::fs::create_dir_all(&dir)
        .and_then(|_| std::fs::write(dir.join("recent-files.txt"), text))
    {
        log::warn!("Could not save the recent files: {err}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamps_sort_like_time() {
        let t = timestamp();
        assert_eq!(t.len(), "2026-10-12_09-30-00".len(), "{t}");
        assert!(t.as_str() > "2026-01-01");
    }

    #[test]
    fn keeps_the_newest_backups() {
        let dir = std::env::temp_dir().join(format!("ink_backups_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for i in 0..5 {
            let name = format!("Cat{BACKUP_MARK}2026-10-1{i}_00-00-00.{PROJECT_EXTENSION}");
            std::fs::write(dir.join(name), b"").unwrap();
        }
        std::fs::write(
            dir.join(format!("Dog{BACKUP_MARK}2026-10-10_00-00-00.ink3d")),
            b"",
        )
        .unwrap();
        prune_backups(&dir.join(format!("Cat{BACKUP_MARK}2026-10-14_00-00-00.ink3d")));
        let mut left: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();
        assert_eq!(left.len(), AUTOSAVES_KEPT + 1, "{left:?}");
        assert!(left[0].starts_with("Cat") && left[0].contains("10-12"));
        assert!(left.iter().any(|n| n.starts_with("Dog")));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
