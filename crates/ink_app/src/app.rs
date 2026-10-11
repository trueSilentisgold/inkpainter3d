use eframe::egui::{self, Color32, Key, PointerButton, Sense};
use eframe::egui_wgpu::RenderState;
use ink_core::Mesh;
use ink_render::{OrbitCamera, ShadingMode, ViewportRenderer, ViewportSettings, wgpu};
use std::path::{Path, PathBuf};

pub struct InkApp {
    mesh: Mesh,
    camera: OrbitCamera,
    settings: ViewportSettings,
    renderer: ViewportRenderer,
    viewport_texture: Option<egui::TextureId>,
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

        let mut app = Self {
            mesh: Mesh::default(),
            camera: OrbitCamera::default(),
            settings: ViewportSettings::default(),
            renderer: ViewportRenderer::new(&render_state.device),
            viewport_texture: None,
            status: format!("GPU: {} ({:?})", info.name, info.backend),
        };
        app.set_mesh(&render_state.device, Mesh::uv_sphere(1.0, 64, 32));

        // Allow `inkpainter3d model.glb` from the command line.
        if let Some(path) = std::env::args_os().nth(1) {
            app.load_model(&render_state.device, Path::new(&path));
        }
        Ok(app)
    }

    fn set_mesh(&mut self, device: &wgpu::Device, mesh: Mesh) {
        self.renderer.set_mesh(device, &mesh);
        self.camera.frame(&mesh.bounds());
        self.mesh = mesh;
    }

    fn load_model(&mut self, device: &wgpu::Device, path: &Path) {
        match ink_io::load_mesh(path) {
            Ok(mesh) => {
                self.status = format!(
                    "Loaded {} ({} vertices, {} triangles)",
                    path.display(),
                    mesh.vertex_count(),
                    mesh.triangle_count()
                );
                self.set_mesh(device, mesh);
            }
            Err(err) => {
                log::error!("{err:#}");
                self.status = format!("Error: {err:#}");
            }
        }
    }

    fn pick_model_file() -> Option<PathBuf> {
        rfd::FileDialog::new()
            .add_filter("3D models", ink_io::SUPPORTED_EXTENSIONS)
            .pick_file()
    }

    fn menu_bar(&mut self, ui: &mut egui::Ui, device: &wgpu::Device) {
        egui::MenuBar::new().ui(ui, |ui| {
            ui.menu_button("File", |ui| {
                if ui.button("Open model…").clicked()
                    && let Some(path) = Self::pick_model_file()
                {
                    self.load_model(device, &path);
                }
                ui.separator();
                if ui.button("Quit").clicked() {
                    ui.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            });
            ui.menu_button("Primitives", |ui| {
                if ui.button("Sphere").clicked() {
                    self.set_mesh(device, Mesh::uv_sphere(1.0, 64, 32));
                }
                if ui.button("Cube").clicked() {
                    self.set_mesh(device, Mesh::cube(1.5));
                }
            });
            ui.menu_button("View", |ui| {
                if ui.button("Frame model (F)").clicked() {
                    self.camera.frame(&self.mesh.bounds());
                }
            });
        });
    }

    fn layers_panel(&self, ui: &mut egui::Ui) {
        ui.heading("Layers");
        ui.separator();
        ui.label(egui::RichText::new("The layer stack arrives in milestone 3.").weak());
        ui.add_space(12.0);
        ui.heading("Mesh");
        ui.separator();
        egui::Grid::new("mesh_info").num_columns(2).show(ui, |ui| {
            ui.label("Name");
            ui.label(&self.mesh.name);
            ui.end_row();
            ui.label("Vertices");
            ui.label(self.mesh.vertex_count().to_string());
            ui.end_row();
            ui.label("Triangles");
            ui.label(self.mesh.triangle_count().to_string());
            ui.end_row();
        });
    }

    fn properties_panel(&mut self, ui: &mut egui::Ui) {
        let s = &mut self.settings;
        ui.heading("Shading");
        ui.separator();
        ui.horizontal(|ui| {
            ui.selectable_value(&mut s.shading, ShadingMode::Pbr, "PBR");
            ui.selectable_value(&mut s.shading, ShadingMode::Toon, "Stylized");
        });
        ui.add_space(8.0);

        egui::Grid::new("material")
            .num_columns(2)
            .spacing([12.0, 6.0])
            .show(ui, |ui| {
                ui.label("Base color");
                ui.color_edit_button_rgb(&mut s.base_color);
                ui.end_row();
                ui.label("Roughness");
                ui.add(egui::Slider::new(&mut s.roughness, 0.0..=1.0));
                ui.end_row();
                if s.shading == ShadingMode::Pbr {
                    ui.label("Metallic");
                    ui.add(egui::Slider::new(&mut s.metallic, 0.0..=1.0));
                    ui.end_row();
                } else {
                    ui.label("Shade bands");
                    ui.add(egui::Slider::new(&mut s.toon_bands, 2..=6));
                    ui.end_row();
                    ui.label("Ink outline");
                    ui.checkbox(&mut s.outline, "");
                    ui.end_row();
                    if s.outline {
                        ui.label("Outline color");
                        ui.color_edit_button_rgb(&mut s.outline_color);
                        ui.end_row();
                        ui.label("Outline width");
                        ui.add(egui::Slider::new(&mut s.outline_width, 0.0..=8.0).suffix(" px"));
                        ui.end_row();
                    }
                }
            });

        ui.add_space(12.0);
        ui.heading("Lighting");
        ui.separator();
        egui::Grid::new("lighting")
            .num_columns(2)
            .spacing([12.0, 6.0])
            .show(ui, |ui| {
                ui.label("Rotation");
                ui.drag_angle(&mut s.light_yaw);
                ui.end_row();
                ui.label("Elevation");
                ui.add(egui::Slider::new(&mut s.light_pitch, -1.5..=1.5));
                ui.end_row();
                ui.label("Intensity");
                ui.add(egui::Slider::new(&mut s.light_intensity, 0.0..=10.0));
                ui.end_row();
                ui.label("Exposure");
                ui.add(egui::Slider::new(&mut s.exposure, 0.1..=4.0).logarithmic(true));
                ui.end_row();
            });
        ui.add_space(8.0);
        if ui.button("Reset").clicked() {
            *s = ViewportSettings::default();
        }
    }

    fn viewport(&mut self, ui: &mut egui::Ui, render_state: &RenderState) {
        let (rect, response) = ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());

        // Navigation: Alt+LMB or RMB orbits, MMB or Shift+RMB pans, wheel zooms.
        // Plain LMB stays free for painting.
        let (alt, shift) = ui.input(|i| (i.modifiers.alt, i.modifiers.shift));
        let delta = response.drag_delta();
        let orbiting = (response.dragged_by(PointerButton::Primary) && alt)
            || (response.dragged_by(PointerButton::Secondary) && !shift);
        let panning = response.dragged_by(PointerButton::Middle)
            || (response.dragged_by(PointerButton::Secondary) && shift);
        if orbiting {
            self.camera.orbit(delta.x, delta.y);
        } else if panning {
            self.camera.pan(delta.x, delta.y, rect.height());
        }
        if response.hovered() {
            let scroll = ui.input(|i| i.smooth_scroll_delta().y);
            if scroll != 0.0 {
                self.camera.zoom(scroll / 50.0);
            }
            if ui.input(|i| i.key_pressed(Key::F)) {
                self.camera.frame(&self.mesh.bounds());
            }
        }

        let ppp = ui.ctx().pixels_per_point();
        let size = [
            (rect.width() * ppp).round() as u32,
            (rect.height() * ppp).round() as u32,
        ];
        let device = &render_state.device;
        if self.renderer.resize(device, size) {
            let view = self
                .renderer
                .output_view()
                .expect("targets exist after resize");
            let mut egui_renderer = render_state.renderer.write();
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
        self.renderer
            .render(device, &render_state.queue, &self.camera, &self.settings);

        if let Some(id) = self.viewport_texture {
            let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
            ui.painter().image(id, rect, uv, Color32::WHITE);
        }
        ui.painter().text(
            rect.left_bottom() + egui::vec2(10.0, -10.0),
            egui::Align2::LEFT_BOTTOM,
            "Orbit: Alt+LMB / RMB   Pan: MMB / Shift+RMB   Zoom: Wheel   Frame: F",
            egui::FontId::proportional(12.0),
            Color32::from_gray(140),
        );
    }
}

impl eframe::App for InkApp {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let Some(render_state) = frame.wgpu_render_state().cloned() else {
            return;
        };
        let device = &render_state.device;

        let dropped: Vec<PathBuf> = ui.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .map(|f| f.path().to_path_buf())
                .filter(|p| !p.as_os_str().is_empty())
                .collect()
        });
        if let Some(path) = dropped.first() {
            self.load_model(device, path);
        }

        egui::Panel::top("menu").show(ui, |ui| self.menu_bar(ui, device));
        egui::Panel::bottom("status").show(ui, |ui| {
            ui.label(egui::RichText::new(&self.status).small());
        });
        egui::Panel::left("layers")
            .default_size(220.0)
            .show(ui, |ui| self.layers_panel(ui));
        egui::Panel::right("properties")
            .default_size(280.0)
            .show(ui, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| self.properties_panel(ui));
            });
        egui::CentralPanel::no_frame().show(ui, |ui| self.viewport(ui, &render_state));
    }
}
