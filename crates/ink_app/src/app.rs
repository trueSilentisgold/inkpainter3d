use eframe::egui::{self, Color32, Key, PointerButton, RichText, Sense};
use eframe::egui_wgpu::RenderState;
use glam::Vec2;
use ink_core::{AlphaMode, Mesh, Model};
use ink_render::{
    DisplayMode, Highlight, OrbitCamera, ShadingMode, ViewportRenderer, ViewportSettings,
    texture_set_color, wgpu,
};
use std::path::{Path, PathBuf};

const RESOLUTIONS: [u32; 6] = [128, 256, 512, 1024, 2048, 4096];

pub struct InkApp {
    model: Model,
    camera: OrbitCamera,
    settings: ViewportSettings,
    renderer: ViewportRenderer,
    viewport_texture: Option<egui::TextureId>,
    environment_name: String,
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

        let mut app = Self {
            model: Model::from_mesh(Mesh::default()),
            camera: OrbitCamera::default(),
            settings: ViewportSettings::default(),
            renderer,
            viewport_texture: None,
            environment_name: environment.name,
            selected_set: 0,
            expanded: Vec::new(),
            focus: None,
            hover: Highlight::default(),
            hover_info: None,
            snap_angles: None,
            status: format!("GPU: {} ({:?})", info.name, info.backend),
        };
        app.set_model(render_state, Model::from_mesh(Mesh::uv_sphere(1.0, 64, 32)));

        // Allow `inkpainter3d model.glb` from the command line.
        if let Some(path) = std::env::args_os().nth(1) {
            app.load_model(render_state, Path::new(&path));
        }
        Ok(app)
    }

    fn set_model(&mut self, rs: &RenderState, model: Model) {
        self.renderer.set_model(&rs.device, &rs.queue, &model);
        self.camera.frame(&model.bounds());
        self.selected_set = 0;
        // Expand single-set models right away; they have nothing else to show.
        self.expanded = vec![model.texture_sets.len() == 1; model.texture_sets.len()];
        self.focus = None;
        self.model = model;
    }

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
                if ui.button("Open model…").clicked()
                    && let Some(path) = Self::pick_file("3D models", ink_io::SUPPORTED_EXTENSIONS)
                {
                    self.load_model(rs, &path);
                }
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
            ui.menu_button("Primitives", |ui| {
                if ui.button("Sphere").clicked() {
                    self.set_model(rs, Model::from_mesh(Mesh::uv_sphere(1.0, 64, 32)));
                }
                if ui.button("Cube").clicked() {
                    self.set_model(rs, Model::from_mesh(Mesh::cube(1.5)));
                }
            });
            ui.menu_button("View", |ui| {
                if ui.button("Frame model (F)").clicked() {
                    self.camera.frame(&self.model.bounds());
                }
                ui.checkbox(&mut self.camera.orthographic, "Orthographic (F5 / F6)");
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

        ui.add_space(8.0);
        ui.label(
            RichText::new("Fill (until the layer stack arrives)")
                .small()
                .weak(),
        );
        let fill = &mut set.fill;
        egui::Grid::new("fill")
            .num_columns(2)
            .spacing([12.0, 6.0])
            .show(ui, |ui| {
                ui.label("Base color");
                srgb_edit(ui, &mut fill.base_color);
                ui.end_row();
                ui.label("Roughness");
                ui.add(egui::Slider::new(&mut fill.roughness, 0.0..=1.0));
                ui.end_row();
                ui.label("Metallic");
                ui.add(egui::Slider::new(&mut fill.metallic, 0.0..=1.0));
                ui.end_row();
                ui.label("Emissive");
                srgb_edit(ui, &mut fill.emissive);
                ui.end_row();
                ui.label("Opacity");
                ui.add(egui::Slider::new(&mut fill.opacity, 0.0..=1.0));
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
        if ui.ctx().egui_wants_keyboard_input() {
            return;
        }
        let (alt, shift) = ui.input(|i| (i.modifiers.alt, i.modifiers.shift));
        let pressed = |key| ui.input(|i| i.key_pressed(key));
        if pressed(Key::F) {
            self.camera.frame(&self.model.bounds());
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

    fn navigate(&mut self, ui: &egui::Ui, response: &egui::Response, rect: egui::Rect) {
        let (alt, shift, ctrl) =
            ui.input(|i| (i.modifiers.alt, i.modifiers.shift, i.modifiers.command));
        let delta = response.drag_delta();
        let left = response.dragged_by(PointerButton::Primary);
        let middle = response.dragged_by(PointerButton::Middle);
        let right = response.dragged_by(PointerButton::Secondary);

        // Painter navigation: Alt+LMB rotate (Shift snaps to 90°), Alt+MMB or Ctrl+Alt+LMB
        // pan, Alt+RMB or wheel zoom, Shift+RMB rotate the environment.
        if alt && left && shift && !ctrl {
            const SPEED: f32 = 0.008;
            let (yaw, pitch) = self
                .snap_angles
                .get_or_insert((self.camera.yaw, self.camera.pitch));
            *yaw -= delta.x * SPEED;
            *pitch += delta.y * SPEED;
            let (yaw, pitch) = (*yaw, *pitch);
            self.camera.set_snapped(yaw, pitch);
        } else {
            self.snap_angles = None;
            if alt && left && !ctrl {
                self.camera.orbit(delta.x, delta.y);
            } else if alt && (middle || (left && ctrl)) {
                self.camera.pan(delta.x, delta.y, rect.height());
            } else if alt && right {
                self.camera.zoom((delta.x - delta.y) * 0.015);
            } else if shift && right && !alt {
                self.settings.environment_rotation += delta.x * 0.01;
            }
        }
        if response.hovered() {
            let scroll = ui.input(|i| i.smooth_scroll_delta().y);
            if scroll != 0.0 {
                self.camera.zoom(scroll / 50.0);
            }
        }
    }

    /// Texture set selection with Ctrl+Alt+right click, and what is under the cursor.
    fn pick(&mut self, ui: &egui::Ui, response: &egui::Response, rect: egui::Rect) {
        let pointer = response.hover_pos().filter(|_| !response.dragged());
        let hit = pointer.and_then(|pos| {
            let local = pos - rect.min;
            let (origin, dir) = self.camera.ray(
                Vec2::new(local.x, local.y),
                Vec2::new(rect.width(), rect.height()),
            );
            self.model.pick(origin, dir)
        });
        let part = hit.map(|h| self.model.parts[h.part].clone());
        self.hover_info = part.as_ref().map(|p| {
            format!(
                "{}  ·  {}",
                self.model.objects[p.object].name, self.model.texture_sets[p.texture_set].name
            )
        });

        let (alt, ctrl) = ui.input(|i| (i.modifiers.alt, i.modifiers.command));
        if alt
            && ctrl
            && response.secondary_clicked()
            && let Some(part) = part
        {
            self.select_set(part.texture_set);
            self.expanded[part.texture_set] = true;
            self.status = format!(
                "Selected texture set {}",
                self.model.texture_sets[part.texture_set].name
            );
        }
    }

    fn viewport(&mut self, ui: &mut egui::Ui, rs: &RenderState) {
        let (rect, response) = ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
        self.navigate(ui, &response, rect);
        self.pick(ui, &response, rect);

        let ppp = ui.ctx().pixels_per_point();
        let size = [
            (rect.width() * ppp).round() as u32,
            (rect.height() * ppp).round() as u32,
        ];
        let device = &rs.device;
        self.renderer
            .sync_texture_sets(device, &rs.queue, &self.model);
        if self.renderer.resize(device, size) {
            let view = self
                .renderer
                .output_view()
                .expect("targets exist after resize");
            let mut egui_renderer = rs.renderer.write();
            match self.viewport_texture {
                Some(id) => egui_renderer.update_egui_texture_from_wgpu_texture(
                    device,
                    view,
                    wgpu::FilterMode::Linear,
                    id,
                ),
                None => {
                    self.viewport_texture = Some(egui_renderer.register_native_texture(
                        device,
                        view,
                        wgpu::FilterMode::Linear,
                    ))
                }
            }
        }
        self.renderer.render(
            device,
            &rs.queue,
            &self.camera,
            &self.settings,
            &self.model,
            self.hover,
        );

        if let Some(id) = self.viewport_texture {
            let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
            ui.painter().image(id, rect, uv, Color32::WHITE);
        }

        // Display mode dropdown, like Painter's at the top of each view.
        let combo_rect =
            egui::Rect::from_min_size(rect.min + egui::vec2(10.0, 8.0), egui::vec2(180.0, 24.0));
        ui.scope_builder(egui::UiBuilder::new().max_rect(combo_rect), |ui| {
            egui::ComboBox::from_id_salt("display mode")
                .selected_text(self.settings.display.label())
                .width(160.0)
                .show_ui(ui, |ui| {
                    for mode in DisplayMode::ALL {
                        ui.selectable_value(&mut self.settings.display, mode, mode.label());
                    }
                });
        });
        ui.painter().text(
            rect.left_bottom() + egui::vec2(10.0, -10.0),
            egui::Align2::LEFT_BOTTOM,
            "Rotate: Alt+LMB   Pan: Alt+MMB   Zoom: Alt+RMB / Wheel   Light: Shift+RMB   \
             Select set: Ctrl+Alt+RMB   Frame: F",
            egui::FontId::proportional(12.0),
            Color32::from_gray(140),
        );
    }
}

impl eframe::App for InkApp {
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
            if ink_io::ENVIRONMENT_EXTENSIONS.contains(&ext.as_str()) {
                self.load_environment(&rs, path);
            } else {
                self.load_model(&rs, path);
            }
        }

        self.shortcuts(ui);
        self.hover = Highlight::default();
        egui::Panel::top("menu").show(ui, |ui| self.menu_bar(ui, &rs));
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
        egui::Panel::right("panels")
            .default_size(330.0)
            .show(ui, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    self.texture_set_list(ui);
                    ui.add_space(10.0);
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
        egui::CentralPanel::no_frame().show(ui, |ui| self.viewport(ui, &rs));
    }
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

/// Color button for an sRGB color stored as floats.
fn srgb_edit(ui: &mut egui::Ui, color: &mut [f32; 3]) -> egui::Response {
    let mut srgb = color.map(|c| (c.clamp(0.0, 1.0) * 255.0).round() as u8);
    let response = ui.color_edit_button_srgb(&mut srgb);
    if response.changed() {
        *color = srgb.map(|c| c as f32 / 255.0);
    }
    response
}
