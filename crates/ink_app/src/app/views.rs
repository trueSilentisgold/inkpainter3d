//! The 3D and 2D views: navigation, picking, rendering and their overlays.

use super::InkApp;
use super::painting::View;
use eframe::egui::{self, Color32, PointerButton, Sense};
use eframe::egui_wgpu::RenderState;
use glam::Vec2;
use ink_render::{DisplayMode, Highlight, wgpu};

/// How the views share the window, like Painter's F1 to F4.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layout {
    /// 3D and 2D side by side (F1).
    Both,
    /// 3D only (F2).
    Model,
    /// 2D only (F3).
    Uv,
}

impl InkApp {
    /// Lays out the views in the central area.
    pub(super) fn views(&mut self, ui: &mut egui::Ui, rs: &RenderState) {
        let rect = ui.available_rect_before_wrap();
        let mut views = match self.layout {
            Layout::Both => {
                let half = rect.width() * 0.5;
                let left = egui::Rect::from_min_max(
                    rect.min,
                    egui::pos2(rect.min.x + half - 1.0, rect.max.y),
                );
                let right = egui::Rect::from_min_max(
                    egui::pos2(rect.min.x + half + 1.0, rect.min.y),
                    rect.max,
                );
                vec![(View::Model, left), (View::Uv, right)]
            }
            Layout::Model => vec![(View::Model, rect)],
            Layout::Uv => vec![(View::Uv, rect)],
        };
        if self.layout == Layout::Both && self.layout_swapped {
            let (a, b) = (views[0].1, views[1].1);
            views[0].1 = b;
            views[1].1 = a;
        }
        for (view, rect) in views {
            ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| match view {
                View::Model => self.model_view(ui, rs),
                View::Uv => self.uv_view(ui, rs),
            });
        }
    }

    // ---- 3D view ----

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
            } else if shift && right && !alt && !ctrl {
                self.settings.environment_rotation += delta.x * 0.01;
            }
        }
        if response.hovered() && !self.paint.is_painting() {
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

    fn model_view(&mut self, ui: &mut egui::Ui, rs: &RenderState) {
        let (rect, response) = ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
        self.navigate(ui, &response, rect);
        self.pick(ui, &response, rect);
        self.paint_input(ui, &response, rect, View::Model);

        let size = pixel_size(ui, rect);
        let device = &rs.device;
        self.renderer
            .sync_texture_sets(device, &rs.queue, &self.model);
        if self.renderer.resize(device, size) {
            let view = self
                .renderer
                .output_view()
                .expect("targets exist after resize");
            register(rs, &mut self.viewport_texture, view);
        }
        self.renderer.render(
            device,
            &rs.queue,
            &self.camera,
            &self.settings,
            &self.model,
            self.hover,
        );
        show(ui, self.viewport_texture, rect);

        display_combo(ui, rect, "display mode", &mut self.settings.display);
        self.draw_brush_cursor(ui, rect, View::Model);
        hint(
            ui,
            rect,
            "Paint: LMB   Line: Shift+LMB   Size / hardness: Ctrl+RMB   Flow / angle: Ctrl+LMB   \
             Rotate: Alt+LMB   Pan: Alt+MMB   Zoom: Alt+RMB   Light: Shift+RMB",
        );
    }

    // ---- 2D view ----

    fn navigate_uv(&mut self, ui: &egui::Ui, response: &egui::Response, rect: egui::Rect) {
        let (alt, shift, ctrl) =
            ui.input(|i| (i.modifiers.alt, i.modifiers.shift, i.modifiers.command));
        let delta = response.drag_delta();
        let delta = Vec2::new(delta.x, delta.y);
        let size = Vec2::new(rect.width(), rect.height());
        let pointer = response
            .interact_pointer_pos()
            .or(response.hover_pos())
            .map(|p| Vec2::new(p.x - rect.min.x, p.y - rect.min.y))
            .unwrap_or(size * 0.5);
        let left = response.dragged_by(PointerButton::Primary);
        let middle = response.dragged_by(PointerButton::Middle);
        let right = response.dragged_by(PointerButton::Secondary);

        // Same navigation as in 3D: Alt+LMB rotates (Shift snaps to 90°), Alt+MMB or
        // Ctrl+Alt+LMB pans, Alt+RMB or the wheel zooms.
        if alt && left && !ctrl {
            let angle = self.uv_rotation.get_or_insert(self.uv_camera.angle);
            *angle += delta.x * 0.01;
            let mut target = *angle;
            if shift {
                let step = std::f32::consts::FRAC_PI_2;
                target = (target / step).round() * step;
            }
            self.uv_camera.rotate_to(target, pointer, size);
        } else {
            self.uv_rotation = None;
            if alt && (middle || (left && ctrl)) {
                self.uv_camera.pan(delta, size);
            } else if alt && right {
                self.uv_camera
                    .zoom((delta.x - delta.y) * 0.015, size * 0.5, size);
            }
        }
        if response.hovered() && !self.paint.is_painting() {
            let scroll = ui.input(|i| i.smooth_scroll_delta().y);
            if scroll != 0.0 {
                self.uv_camera.zoom(scroll / 50.0, pointer, size);
            }
        }
    }

    fn uv_view(&mut self, ui: &mut egui::Ui, rs: &RenderState) {
        let (rect, response) = ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
        self.navigate_uv(ui, &response, rect);
        self.paint_input(ui, &response, rect, View::Uv);

        let size = pixel_size(ui, rect);
        let device = &rs.device;
        self.renderer
            .sync_texture_sets(device, &rs.queue, &self.model);
        if self.uv_view.resize(device, size) {
            let view = self
                .uv_view
                .output_view()
                .expect("targets exist after resize");
            register(rs, &mut self.uv_texture, view);
        }
        self.uv_view.render(
            device,
            &rs.queue,
            &self.renderer,
            &self.model,
            self.selected_set,
            &self.uv_camera,
            self.uv_display,
            self.uv_wireframe,
        );
        show(ui, self.uv_texture, rect);

        display_combo(ui, rect, "uv display mode", &mut self.uv_display);
        if let Some(set) = self.model.texture_sets.get(self.selected_set) {
            ui.painter().text(
                rect.right_top() + egui::vec2(-10.0, 12.0),
                egui::Align2::RIGHT_TOP,
                format!("2D  ·  {}", set.name),
                egui::FontId::proportional(13.0),
                Color32::from_gray(170),
            );
        }
        self.draw_brush_cursor(ui, rect, View::Uv);
        hint(
            ui,
            rect,
            "Paint: LMB   Rotate: Alt+LMB   Pan: Alt+MMB   Zoom: Alt+RMB / Wheel",
        );
    }

    /// Frames the model in 3D and the UV square in 2D (F).
    pub(super) fn frame_views(&mut self) {
        self.camera.frame(&self.model.bounds());
        self.uv_camera.frame();
    }

    pub(super) fn reset_hover(&mut self) {
        self.hover = Highlight::default();
    }
}

fn pixel_size(ui: &egui::Ui, rect: egui::Rect) -> [u32; 2] {
    let ppp = ui.ctx().pixels_per_point();
    [
        (rect.width() * ppp).round() as u32,
        (rect.height() * ppp).round() as u32,
    ]
}

/// Registers a view's output with egui, or points the existing texture id at it.
fn register(rs: &RenderState, id: &mut Option<egui::TextureId>, view: &wgpu::TextureView) {
    let mut egui_renderer = rs.renderer.write();
    match *id {
        Some(id) => egui_renderer.update_egui_texture_from_wgpu_texture(
            &rs.device,
            view,
            wgpu::FilterMode::Linear,
            id,
        ),
        None => {
            *id = Some(egui_renderer.register_native_texture(
                &rs.device,
                view,
                wgpu::FilterMode::Linear,
            ))
        }
    }
}

fn show(ui: &egui::Ui, id: Option<egui::TextureId>, rect: egui::Rect) {
    if let Some(id) = id {
        let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
        ui.painter().image(id, rect, uv, Color32::WHITE);
    }
}

/// Display mode dropdown, like Painter's at the top of each view.
fn display_combo(ui: &mut egui::Ui, rect: egui::Rect, id: &str, display: &mut DisplayMode) {
    let combo_rect =
        egui::Rect::from_min_size(rect.min + egui::vec2(10.0, 8.0), egui::vec2(180.0, 24.0));
    ui.scope_builder(egui::UiBuilder::new().max_rect(combo_rect), |ui| {
        egui::ComboBox::from_id_salt(id)
            .selected_text(display.label())
            .width(160.0)
            .show_ui(ui, |ui| {
                for mode in DisplayMode::ALL {
                    ui.selectable_value(display, mode, mode.label());
                }
            });
    });
}

fn hint(ui: &egui::Ui, rect: egui::Rect, text: &str) {
    ui.painter_at(rect).text(
        rect.left_bottom() + egui::vec2(10.0, -10.0),
        egui::Align2::LEFT_BOTTOM,
        text,
        egui::FontId::proportional(12.0),
        Color32::from_gray(140),
    );
}
