mod export;
mod layers;
mod painting;
mod project;
mod views;

use crate::pen::{PenInput, PenNotice};
use eframe::egui::{self, Color32, Key, RichText, Sense};
use eframe::egui_wgpu::RenderState;
use ink_core::{AlphaMode, History, Mesh, Model};
use ink_render::{
    DisplayMode, Highlight, OrbitCamera, ShadingMode, UvCamera, UvView, ViewportRenderer,
    ViewportSettings, texture_set_color,
};
use layers::LayersState;
use painting::PaintState;
use project::{Leave, ProjectState};
use std::path::{Path, PathBuf};
use views::Layout;

const RESOLUTIONS: [u32; 6] = [128, 256, 512, 1024, 2048, 4096];

pub struct InkApp {
    model: Model,
    camera: OrbitCamera,
    settings: ViewportSettings,
    renderer: ViewportRenderer,
    viewport_texture: Option<egui::TextureId>,
    environment_name: String,
    /// The panorama file shown, None for the default one; saved with the project.
    environment_path: Option<PathBuf>,
    /// Texture set whose settings (and later layers) are edited.
    selected_set: usize,
    /// Texture sets expanded in the list to show their meshes.
    expanded: Vec<bool>,
    /// Visibility before focus mode started, restored when it ends.
    focus: Option<Vec<bool>>,
    /// What the pointer is over in the texture set list this frame.
    hover: Highlight,
    /// Mesh and texture set under the cursor in the viewport.
    hover_info: Option<String>,
    /// Unsnapped camera angles while snap-rotating (Alt+Shift+LMB).
    snap_angles: Option<(f32, f32)>,
    status: String,
    paint: PaintState,
    layers: LayersState,
    /// Whether the pointer was over the layer stack last frame; Ctrl+C / X / V and Delete
    /// act on layers only there, as in Painter.
    layers_hovered: bool,
    history: History,
    layout: Layout,
    /// F4: 2D view on the left, 3D on the right.
    layout_swapped: bool,
    uv_view: UvView,
    uv_texture: Option<egui::TextureId>,
    uv_camera: UvCamera,
    uv_display: DisplayMode,
    uv_wireframe: bool,
    /// Unsnapped 2D view rotation and its pivot while rotating with Alt + left mouse.
    uv_rotation: Option<(f32, glam::Vec2)>,
    /// Keep the UV square framed in the 2D view while its size changes, until the user
    /// pans, zooms or rotates it.
    uv_auto_frame: bool,
    uv_framed_size: Option<[u32; 2]>,
    pen: PenInput,
    /// The tool to go back to when the pen's eraser end leaves.
    tool_before_eraser: Option<ink_core::Tool>,
    project: ProjectState,
    /// The Export textures window, while it is open.
    export: Option<export::ExportWindow>,
    /// The window closes at the end of the frame.
    quit_requested: bool,
}

impl InkApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> anyhow::Result<Self> {
        let render_state = cc
            .wgpu_render_state
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("inkpainter3d requires the wgpu renderer"))?;
        let info = render_state.adapter.get_info();
        log::info!("GPU: {} ({:?})", info.name, info.backend);
        let (device, queue) = (&render_state.device, &render_state.queue);

        let mut renderer = ViewportRenderer::new(device, queue);
        let environment = ink_io::default_environment();
        renderer.set_environment(
            device,
            queue,
            environment.width,
            environment.height,
            &environment.pixels,
        );

        let uv_view = UvView::new(device, &renderer);
        let mut app = Self {
            model: Model::from_mesh(Mesh::default()),
            camera: OrbitCamera::default(),
            settings: ViewportSettings::default(),
            renderer,
            viewport_texture: None,
            environment_name: environment.name,
            environment_path: None,
            selected_set: 0,
            expanded: Vec::new(),
            focus: None,
            hover: Highlight::default(),
            hover_info: None,
            snap_angles: None,
            status: format!("GPU: {} ({:?})", info.name, info.backend),
            paint: PaintState::default(),
            layers: LayersState::default(),
            layers_hovered: false,
            history: History::default(),
            // Painter opens with the 3D and 2D views side by side.
            layout: Layout::Both,
            layout_swapped: false,
            uv_view,
            uv_texture: None,
            uv_camera: UvCamera::default(),
            uv_display: DisplayMode::Material,
            uv_wireframe: true,
            uv_rotation: None,
            uv_auto_frame: true,
            uv_framed_size: None,
            pen: PenInput::new(cc),
            tool_before_eraser: None,
            project: ProjectState::default(),
            export: None,
            quit_requested: false,
        };
        app.set_model(render_state, Model::from_mesh(Mesh::uv_sphere(1.0, 64, 32)));
        app.mark_saved();

        // `inkpainter3d project.ink3d` opens a project, `inkpainter3d model.glb` starts one.
        if let Some(path) = std::env::args_os().nth(1) {
            let path = PathBuf::from(path);
            if is_project(&path) {
                app.start_open(path);
            } else {
                app.load_model(render_state, &path);
            }
        }
        Ok(app)
    }

    fn set_model(&mut self, rs: &RenderState, model: Model) {
        self.renderer.set_model(&rs.device, &rs.queue, &model);
        self.uv_view.set_model(&rs.device, &model);
        self.paint.reset();
        self.layers.reset(model.texture_sets.len());
        self.history.clear();
        self.uv_auto_frame = true;
        self.uv_framed_size = None;
        self.camera.frame(&model.bounds());
        self.selected_set = 0;
        // Expand single-set models right away; they have nothing else to show.
        self.expanded = vec![model.texture_sets.len() == 1; model.texture_sets.len()];
        self.focus = None;
        self.model = model;
    }

    /// A new project from a mesh file with the default settings, without asking.
    fn load_model(&mut self, rs: &RenderState, path: &Path) {
        match ink_io::load_model(path) {
            Ok(model) => {
                self.status = format!(
                    "Loaded {} ({} objects, {} texture sets, {} triangles)",
                    path.display(),
                    model.objects.len(),
                    model.texture_sets.len(),
                    model.mesh.triangle_count()
                );
                self.set_model(rs, model);
                self.project.source = Some(path.display().to_string());
                self.mark_saved();
            }
            Err(err) => {
                log::error!("{err:#}");
                self.status = format!("Error: {err:#}");
            }
        }
    }

    fn load_environment(&mut self, rs: &RenderState, path: &Path) {
        match ink_io::load_environment(path) {
            Ok(env) => {
                self.renderer.set_environment(
                    &rs.device,
                    &rs.queue,
                    env.width,
                    env.height,
                    &env.pixels,
                );
                self.status = format!("Environment: {}", path.display());
                self.environment_name = env.name;
                self.environment_path = Some(path.to_owned());
            }
            Err(err) => {
                log::error!("{err:#}");
                self.status = format!("Error: {err:#}");
            }
        }
    }

    fn pick_file(name: &str, extensions: &[&str]) -> Option<PathBuf> {
        rfd::FileDialog::new()
            .add_filter(name, extensions)
            .pick_file()
    }

    // ---- Texture set visibility ----

    fn select_set(&mut self, set: usize) {
        self.selected_set = set;
        if self.focus.is_some() {
            self.isolate_selected();
        }
    }

    fn isolate_selected(&mut self) {
        for (i, set) in self.model.texture_sets.iter_mut().enumerate() {
            set.visible = i == self.selected_set;
        }
    }

    /// Focus mode (Alt+Q): shows only the selected texture set until toggled again.
    fn toggle_focus(&mut self) {
        match self.focus.take() {
            Some(previous) => {
                for (set, visible) in self.model.texture_sets.iter_mut().zip(previous) {
                    set.visible = visible;
                }
            }
            None => {
                self.focus = Some(self.model.texture_sets.iter().map(|s| s.visible).collect());
                self.isolate_selected();
            }
        }
    }

    fn set_all_visible(&mut self, visible: impl Fn(bool) -> bool) {
        self.focus = None;
        for set in &mut self.model.texture_sets {
            set.visible = visible(set.visible);
        }
    }

    // ---- Panels ----

    fn menu_bar(&mut self, ui: &mut egui::Ui, rs: &RenderState) {
        egui::MenuBar::new().ui(ui, |ui| {
            ui.menu_button("File", |ui| {
                self.project_menu(ui, rs);
                ui.separator();
                if ui
                    .add_enabled(
                        !self.model.texture_sets.is_empty(),
                        egui::Button::new("Export textures…").shortcut_text("Ctrl+Shift+E"),
                    )
                    .clicked()
                {
                    self.open_export_window();
                }
                ui.separator();
                if ui.button("Load environment…").clicked()
                    && let Some(path) =
                        Self::pick_file("HDR panoramas", ink_io::ENVIRONMENT_EXTENSIONS)
                {
                    self.load_environment(rs, &path);
                }
                ui.separator();
                if ui.button("Quit").clicked() {
                    ui.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            });
            ui.menu_button("Edit", |ui| {
                if ui
                    .add_enabled(
                        self.history.can_undo(),
                        egui::Button::new("Undo").shortcut_text("Ctrl+Z"),
                    )
                    .clicked()
                {
                    self.undo();
                }
                if ui
                    .add_enabled(
                        self.history.can_redo(),
                        egui::Button::new("Redo").shortcut_text("Ctrl+Y"),
                    )
                    .clicked()
                {
                    self.redo();
                }
                ui.separator();
                if ui
                    .add_enabled(
                        !self.model.texture_sets.is_empty(),
                        egui::Button::new("Project configuration…"),
                    )
                    .clicked()
                {
                    self.open_configuration();
                }
            });
            ui.menu_button("Primitives", |ui| {
                let primitive = if ui.button("Sphere").clicked() {
                    Some(Mesh::uv_sphere(1.0, 64, 32))
                } else if ui.button("Cube").clicked() {
                    Some(Mesh::cube(1.5))
                } else {
                    None
                };
                if let Some(mesh) = primitive {
                    let model = Box::new(Model::from_mesh(mesh));
                    self.leave_project(
                        rs,
                        Leave::Replace {
                            model,
                            source: None,
                        },
                    );
                }
            });
            ui.menu_button("View", |ui| {
                if ui.button("Frame model (F)").clicked() {
                    self.frame_views();
                }
                ui.checkbox(&mut self.camera.orthographic, "Orthographic (F5 / F6)");
                ui.separator();
                ui.radio_value(&mut self.layout, Layout::Both, "3D / 2D (F1)");
                ui.radio_value(&mut self.layout, Layout::Model, "3D only (F2)");
                ui.radio_value(&mut self.layout, Layout::Uv, "2D only (F3)");
                ui.checkbox(&mut self.layout_swapped, "Swap 3D and 2D (F4)");
                ui.checkbox(&mut self.uv_wireframe, "UV wireframe in 2D");
                ui.separator();
                for mode in DisplayMode::ALL {
                    ui.radio_value(&mut self.settings.display, mode, mode.label());
                }
            });
        });
    }

    fn texture_set_list(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("TEXTURE SET LIST").small().strong());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.menu_button("☰", |ui| {
                    if ui.button("Show all").clicked() {
                        self.set_all_visible(|_| true);
                    }
                    if ui.button("Hide all").clicked() {
                        self.set_all_visible(|_| false);
                    }
                    if ui.button("Invert show / hide").clicked() {
                        self.set_all_visible(|v| !v);
                    }
                    ui.separator();
                    if ui.button("Show all meshes").clicked() {
                        for object in &mut self.model.objects {
                            object.visible = true;
                        }
                    }
                    if ui.button("Expand all").clicked() {
                        self.expanded.fill(true);
                    }
                    if ui.button("Collapse all").clicked() {
                        self.expanded.fill(false);
                    }
                });
                let focus = self.focus.is_some();
                if ui
                    .selectable_label(focus, "◎ Focus")
                    .on_hover_text("Show only the selected texture set (Alt+Q)")
                    .clicked()
                {
                    self.toggle_focus();
                }
            });
        });
        ui.separator();

        egui::ScrollArea::vertical()
            .id_salt("texture sets")
            .max_height(260.0)
            .show(ui, |ui| {
                for set in 0..self.model.texture_sets.len() {
                    self.texture_set_row(ui, set);
                    if self.expanded[set] {
                        for object in self.model.objects_in_set(set) {
                            self.object_row(ui, set, object);
                        }
                    }
                }
            });
    }

    fn texture_set_row(&mut self, ui: &mut egui::Ui, set: usize) {
        let selected = set == self.selected_set;
        let row = ui.horizontal(|ui| {
            let arrow = if self.expanded[set] { "⏷" } else { "⏵" };
            if ui
                .add(egui::Button::new(arrow).frame(false))
                .on_hover_text("Show the meshes using this material")
                .clicked()
            {
                self.expanded[set] = !self.expanded[set];
            }
            let visible = self.model.texture_sets[set].visible;
            if eye(ui, visible).clicked() {
                self.focus = None;
                self.model.texture_sets[set].visible = !visible;
            }
            let [r, g, b] = texture_set_color(set).map(|c| (c * 255.0) as u8);
            let (swatch, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), Sense::hover());
            ui.painter()
                .rect_filled(swatch, 2.0, Color32::from_rgb(r, g, b));
            let ts = &self.model.texture_sets[set];
            let resolution = ts.resolution;
            let mut name = RichText::new(&ts.name);
            if !visible {
                name = name.weak();
            }
            if ui.selectable_label(selected, name).clicked() {
                self.select_set(set);
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    RichText::new(format!("{resolution}×{resolution}"))
                        .small()
                        .weak(),
                );
            });
        });
        if row.response.contains_pointer() {
            self.hover = Highlight {
                texture_set: Some(set),
                object: None,
            };
        }
    }

    fn object_row(&mut self, ui: &mut egui::Ui, set: usize, object: usize) {
        let row = ui.horizontal(|ui| {
            ui.add_space(26.0);
            let visible = self.model.objects[object].visible;
            if eye(ui, visible).clicked() {
                self.model.objects[object].visible = !visible;
            }
            let mut name = RichText::new(&self.model.objects[object].name);
            if !visible || !self.model.texture_sets[set].visible {
                name = name.weak();
            }
            ui.label(name);
            let other_sets = self.model.sets_of_object(object).len() - 1;
            if other_sets > 0 {
                ui.label(RichText::new("⧉").weak()).on_hover_text(format!(
                    "This mesh also uses {other_sets} other material{}",
                    if other_sets == 1 { "" } else { "s" }
                ));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    RichText::new(format!("{} tris", self.model.triangle_count(set, object)))
                        .small()
                        .weak(),
                );
            });
        });
        if row.response.contains_pointer() {
            self.hover = Highlight {
                texture_set: Some(set),
                object: Some(object),
            };
        }
    }

    fn layers_section(&mut self, ui: &mut egui::Ui) {
        let response = egui::CollapsingHeader::new(RichText::new("LAYERS").small().strong())
            .id_salt("layers")
            .default_open(true)
            .show(ui, |ui| self.layer_stack_panel(ui));
        let rect = response
            .body_response
            .map_or(response.header_response.rect, |b| {
                b.rect.union(response.header_response.rect)
            });
        self.layers_hovered = ui.rect_contains_pointer(rect);
    }

    fn properties_section(&mut self, ui: &mut egui::Ui) {
        let title = self
            .selection_properties_title()
            .unwrap_or(match self.paint.tool {
                ink_core::Tool::Paint => "PROPERTIES - PAINT",
                ink_core::Tool::Erase => "PROPERTIES - ERASER",
            });
        egui::CollapsingHeader::new(RichText::new(title).small().strong())
            .id_salt("properties")
            .default_open(true)
            .show(ui, |ui| {
                if !self.selection_properties(ui) {
                    self.properties_panel(ui);
                }
            });
    }

    fn texture_set_settings(&mut self, ui: &mut egui::Ui) {
        let Some(set) = self.model.texture_sets.get_mut(self.selected_set) else {
            return;
        };
        egui::Grid::new("texture set settings")
            .num_columns(2)
            .spacing([12.0, 6.0])
            .show(ui, |ui| {
                ui.label("Name");
                ui.label(RichText::new(&set.name).strong());
                ui.end_row();
                ui.label("Size");
                egui::ComboBox::from_id_salt("resolution")
                    .selected_text(format!("{0} × {0}", set.resolution))
                    .show_ui(ui, |ui| {
                        for r in RESOLUTIONS {
                            ui.selectable_value(&mut set.resolution, r, format!("{r} × {r}"));
                        }
                    });
                ui.end_row();
                ui.label("Opacity");
                egui::ComboBox::from_id_salt("alpha mode")
                    .selected_text(alpha_label(set.alpha_mode))
                    .show_ui(ui, |ui| {
                        for mode in [AlphaMode::Opaque, AlphaMode::Test, AlphaMode::Blend] {
                            ui.selectable_value(&mut set.alpha_mode, mode, alpha_label(mode));
                        }
                    });
                ui.end_row();
            });
    }

    fn display_settings(&mut self, ui: &mut egui::Ui, rs: &RenderState) {
        let mut load = false;
        let s = &mut self.settings;
        ui.horizontal(|ui| {
            ui.selectable_value(&mut s.shading, ShadingMode::Pbr, "PBR");
            ui.selectable_value(&mut s.shading, ShadingMode::Toon, "Stylized");
        });
        ui.add_space(6.0);
        egui::Grid::new("display")
            .num_columns(2)
            .spacing([12.0, 6.0])
            .show(ui, |ui| {
                ui.label("Environment");
                ui.horizontal(|ui| {
                    ui.label(&self.environment_name);
                    load = ui.small_button("Load…").clicked();
                });
                ui.end_row();
                ui.label("Rotation");
                ui.drag_angle(&mut s.environment_rotation)
                    .on_hover_text("Or drag with Shift + right mouse button in the viewport");
                ui.end_row();
                ui.label("Exposure");
                ui.add(egui::Slider::new(&mut s.exposure, -5.0..=5.0).suffix(" EV"));
                ui.end_row();
                ui.label("Background");
                ui.add(egui::Slider::new(&mut s.background_opacity, 0.0..=1.0));
                ui.end_row();
                ui.label("Background blur");
                ui.add(egui::Slider::new(&mut s.background_blur, 0.0..=1.0));
                ui.end_row();
                if s.shading == ShadingMode::Toon {
                    ui.label("Shade bands");
                    ui.add(egui::Slider::new(&mut s.toon_bands, 2..=6));
                    ui.end_row();
                    ui.label("Ink outline");
                    ui.checkbox(&mut s.outline, "");
                    ui.end_row();
                    if s.outline {
                        ui.label("Outline color");
                        srgb_edit(ui, &mut s.outline_color);
                        ui.end_row();
                        ui.label("Outline width");
                        ui.add(egui::Slider::new(&mut s.outline_width, 0.0..=8.0).suffix(" px"));
                        ui.end_row();
                    }
                }
            });
        ui.add_space(6.0);
        if ui.button("Reset").clicked() {
            *s = ViewportSettings {
                display: s.display,
                ..Default::default()
            };
        }
        if load && let Some(path) = Self::pick_file("HDR panoramas", ink_io::ENVIRONMENT_EXTENSIONS)
        {
            self.load_environment(rs, &path);
        }
    }

    // ---- Viewport ----

    fn shortcuts(&mut self, ui: &egui::Ui) {
        if ui.ctx().text_edit_focused() {
            return;
        }
        let (alt, shift) = ui.input(|i| (i.modifiers.alt, i.modifiers.shift));
        // With Ctrl the keys mean something else: Ctrl+F4 closes the project.
        let pressed = |key| ui.input(|i| !i.modifiers.command && i.key_pressed(key));
        if pressed(Key::F) {
            self.frame_views();
        }
        if pressed(Key::F1) {
            self.layout = Layout::Both;
        }
        if pressed(Key::F2) {
            self.layout = Layout::Model;
        }
        if pressed(Key::F3) {
            self.layout = Layout::Uv;
        }
        if pressed(Key::F4) {
            self.layout_swapped = !self.layout_swapped;
        }
        if pressed(Key::F5) {
            self.camera.orthographic = false;
        }
        if pressed(Key::F6) {
            self.camera.orthographic = true;
        }
        if pressed(Key::M) {
            self.settings.display = DisplayMode::Material;
        }
        if pressed(Key::C) {
            self.settings.display = self.settings.display.cycle_channel(!shift);
        }
        if alt && pressed(Key::Q) {
            self.toggle_focus();
        }
    }
}

impl eframe::App for InkApp {
    fn raw_input_hook(&mut self, ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        // Tab would move keyboard focus to a button, which turns every shortcut off. In
        // Painter it hides the UI (not there yet); in text fields it keeps working.
        if !ctx.text_edit_focused() {
            raw_input
                .events
                .retain(|e| !matches!(e, egui::Event::Key { key: Key::Tab, .. }));
        }
        for notice in self.pen.feed(ctx, raw_input) {
            match notice {
                // Flipping the pen erases, like in most painting programs.
                PenNotice::EraserIn => {
                    self.tool_before_eraser.get_or_insert(self.paint.tool);
                    self.paint.tool = ink_core::Tool::Erase;
                }
                PenNotice::EraserOut => {
                    if let Some(tool) = self.tool_before_eraser.take() {
                        self.paint.tool = tool;
                    }
                }
            }
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let Some(rs) = frame.wgpu_render_state().cloned() else {
            return;
        };

        let dropped: Vec<PathBuf> = ui.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .map(|f| f.path().to_path_buf())
                .filter(|p| !p.as_os_str().is_empty())
                .collect()
        });
        if let Some(path) = dropped.first() {
            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .map(str::to_ascii_lowercase)
                .unwrap_or_default();
            if is_project(path) {
                self.leave_project(&rs, Leave::Open(path.clone()));
            } else if ink_io::ENVIRONMENT_EXTENSIONS.contains(&ext.as_str()) {
                self.load_environment(&rs, path);
            } else if ink_io::ALPHA_EXTENSIONS.contains(&ext.as_str()) {
                self.load_alpha(path);
            } else {
                // As in Painter: a dropped mesh starts a new project.
                self.ask_new_project(Some(path));
            }
        }

        self.poll_project_jobs(ui, &rs);
        self.handle_close_request(ui);
        // Dialogs take the keyboard.
        let dialog = self.project.dialog_open() || self.export.is_some();
        if !dialog {
            self.shortcuts(ui);
            self.paint_shortcuts(ui);
            self.project_shortcuts(ui, &rs);
            self.export_shortcut(ui);
        }
        self.reset_hover();
        egui::Panel::top("menu").show(ui, |ui| self.menu_bar(ui, &rs));
        // Everything that edits the layer stack runs between these two calls, so that its
        // changes become undo steps; undo and redo (above) and strokes (in the views,
        // below) record their own.
        self.validate_selection();
        let stack_before = self.begin_stack_edits(ui);
        if !dialog {
            self.layer_shortcuts(ui, self.layers_hovered);
        }
        egui::Panel::top("tool bar").show(ui, |ui| self.tool_bar(ui));
        egui::Panel::bottom("status").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(&self.status).small());
                if let Some(info) = &self.hover_info {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(RichText::new(info).small());
                    });
                }
            });
        });
        egui::Panel::left("tools")
            .resizable(false)
            .exact_size(76.0)
            .show(ui, |ui| self.tool_strip(ui));
        egui::Panel::right("panels")
            .default_size(340.0)
            .show(ui, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    self.texture_set_list(ui);
                    ui.add_space(10.0);
                    self.layers_section(ui);
                    self.properties_section(ui);
                    egui::CollapsingHeader::new(
                        RichText::new("TEXTURE SET SETTINGS").small().strong(),
                    )
                    .default_open(true)
                    .show(ui, |ui| self.texture_set_settings(ui));
                    egui::CollapsingHeader::new(RichText::new("DISPLAY SETTINGS").small().strong())
                        .default_open(true)
                        .show(ui, |ui| self.display_settings(ui, &rs));
                });
            });
        self.end_stack_edits(ui, stack_before);
        self.sync_system_clipboard(ui);
        self.validate_selection();
        egui::CentralPanel::no_frame().show(ui, |ui| self.views(ui, &rs));
        self.project_dialogs(ui, &rs);
        self.export_window(ui, &rs);
        self.update_title(ui);
        if std::mem::take(&mut self.quit_requested) {
            ui.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        // Full composites run a few chunks per frame.
        if self.renderer.is_compositing() {
            ui.ctx().request_repaint();
        }
    }
}

fn is_project(path: &Path) -> bool {
    path.extension()
        .is_some_and(|e| e.eq_ignore_ascii_case(ink_io::project::PROJECT_EXTENSION))
}

/// Visibility toggle in the texture set list.
fn eye(ui: &mut egui::Ui, visible: bool) -> egui::Response {
    let (text, color) = if visible {
        ("👁", ui.visuals().text_color())
    } else {
        ("👁", ui.visuals().weak_text_color().gamma_multiply(0.4))
    };
    ui.add(egui::Button::new(RichText::new(text).color(color)).frame(false))
        .on_hover_text(if visible { "Hide" } else { "Show" })
}

fn alpha_label(mode: AlphaMode) -> &'static str {
    match mode {
        AlphaMode::Opaque => "Opaque",
        AlphaMode::Test => "Alpha test",
        AlphaMode::Blend => "Alpha blend",
    }
}

/// Edits one channel of uniform channel values: a color button, a normal or a slider.
fn channel_value_edit(
    ui: &mut egui::Ui,
    values: &mut ink_core::ChannelValues,
    channel: ink_core::Channel,
) -> egui::Response {
    use ink_core::ChannelKind;
    match channel.kind() {
        ChannelKind::Color => srgb_edit(ui, values.vector_mut(channel).expect("color channel")),
        ChannelKind::Normal => {
            // Encoded like a normal map texel, shown as such.
            let normal = values.vector_mut(channel).expect("normal channel");
            ui.color_edit_button_rgb(normal)
        }
        ChannelKind::Scalar => {
            let range = if channel == ink_core::Channel::Height {
                -1.0..=1.0
            } else {
                0.0..=1.0
            };
            let value = values.scalar_mut(channel).expect("scalar channel");
            ui.add(egui::Slider::new(value, range))
        }
    }
}

/// Color button for an sRGB color stored as floats.
fn srgb_edit(ui: &mut egui::Ui, color: &mut [f32; 3]) -> egui::Response {
    let mut srgb = color.map(|c| (c.clamp(0.0, 1.0) * 255.0).round() as u8);
    let response = ui.color_edit_button_srgb(&mut srgb);
    if response.changed() {
        *color = srgb.map(|c| c as f32 / 255.0);
    }
    response
}
