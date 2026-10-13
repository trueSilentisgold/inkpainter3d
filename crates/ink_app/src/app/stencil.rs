//! The stencil: an image laid over the views that the brush paints through. S + drag
//! moves (middle button), rotates (left) and scales (right) it, N ignores it.

use super::InkApp;
use super::painting::View;
use crate::theme;
use eframe::egui::{self, Key, PointerButton};
use glam::Vec2;
use ink_core::{Stencil, StrokeStencil, stencil::compose};
use std::sync::Arc;

/// The stencil and what the app keeps around it.
#[derive(Default)]
pub struct StencilState {
    pub stencil: Stencil,
    /// N is held: strokes ignore the stencil.
    pub ignore: bool,
    /// The overlay's texture, with the image id it was made from.
    texture: Option<(u64, egui::TextureHandle)>,
    drag: Option<Drag>,
}

struct Drag {
    button: PointerButton,
    view: View,
    last: Vec2,
}

fn vec2(p: egui::Pos2) -> Vec2 {
    Vec2::new(p.x, p.y)
}

impl InkApp {
    /// Where the stencil's center is on screen.
    fn stencil_center(&self, rect: egui::Rect) -> Vec2 {
        let half = rect.height() * 0.5;
        let c = self.paint.stencil.stencil.center;
        Vec2::new(rect.center().x + c.x * half, rect.center().y - c.y * half)
    }

    /// Handles S + drag. Returns whether the event belongs to the stencil.
    pub(super) fn stencil_event(
        &mut self,
        ui: &egui::Ui,
        event: &egui::Event,
        rect: egui::Rect,
        hovered: bool,
        view: View,
    ) -> bool {
        match *event {
            egui::Event::PointerButton {
                pos,
                button,
                pressed: true,
                ..
            } if rect.contains(pos)
                && hovered
                && ui.input(|i| i.key_down(Key::S))
                && self.paint.stencil.stencil.image.is_some()
                && !self.paint.is_busy() =>
            {
                self.paint.stencil.drag = Some(Drag {
                    button,
                    view,
                    last: vec2(pos),
                });
                true
            }
            egui::Event::PointerButton {
                button,
                pressed: false,
                ..
            } => match &self.paint.stencil.drag {
                Some(drag) if drag.button == button => {
                    self.paint.stencil.drag = None;
                    true
                }
                _ => false,
            },
            egui::Event::PointerMoved(pos) => {
                let Some(drag) = self.paint.stencil.drag.as_mut().filter(|d| d.view == view) else {
                    return self.paint.stencil.drag.is_some();
                };
                let (last, button) = (drag.last, drag.button);
                drag.last = vec2(pos);
                let now = vec2(pos);
                let half = rect.height().max(1.0) * 0.5;
                match button {
                    PointerButton::Middle => {
                        let d = (now - last) / half;
                        self.paint.stencil.stencil.translate(Vec2::new(d.x, -d.y));
                    }
                    PointerButton::Primary => {
                        // Turn around the center by the angle the pointer moved through.
                        let center = self.stencil_center(rect);
                        let (a, b) = (last - center, now - center);
                        if a.length() > 1.0 && b.length() > 1.0 {
                            let turn = (-b.y).atan2(b.x) - (-a.y).atan2(a.x);
                            let stencil = &mut self.paint.stencil.stencil;
                            stencil.rotation =
                                (stencil.rotation + turn.to_degrees()).rem_euclid(360.0);
                        }
                    }
                    PointerButton::Secondary => {
                        let d = now - last;
                        self.paint.stencil.stencil.scale((d.x * 0.01).exp());
                    }
                    _ => {}
                }
                true
            }
            _ => false,
        }
    }

    /// The stencil as the next stroke in this view keeps it.
    pub(super) fn stroke_stencil(&self, view: View, rect: egui::Rect) -> Option<StrokeStencil> {
        if self.paint.stencil.ignore {
            return None;
        }
        let stencil = &self.paint.stencil.stencil;
        let image = stencil.active()?.clone();
        let affine = stencil.view_affine()?;
        let affine = match view {
            View::Model => affine,
            View::Uv => {
                // The 2D view shows the UV square: map UV to the view's coordinates first.
                let size = Vec2::new(rect.width(), rect.height());
                let half = size.y.max(1.0) * 0.5;
                let to_view = |uv: Vec2| {
                    let p = self.uv_camera.to_pixel(uv, size);
                    Vec2::new((p.x - size.x * 0.5) / half, -(p.y - size.y * 0.5) / half)
                };
                let o = to_view(Vec2::ZERO);
                let (ux, uy) = (to_view(Vec2::X) - o, to_view(Vec2::Y) - o);
                compose(affine, [ux.x, uy.x, o.x, ux.y, uy.y, o.y])
            }
        };
        Some(StrokeStencil { image, affine })
    }

    /// Draws the stencil over a view.
    pub(super) fn draw_stencil(&mut self, ui: &egui::Ui, rect: egui::Rect) {
        let state = &mut self.paint.stencil;
        let Some(image) = state.stencil.image.clone() else {
            return;
        };
        if !state.stencil.enabled {
            return;
        }
        if state.texture.as_ref().is_none_or(|(id, _)| *id != image.id) {
            let color = egui::ColorImage::from_gray(
                [image.width as usize, image.height as usize],
                &image.pixels,
            );
            let handle = ui
                .ctx()
                .load_texture("stencil", color, egui::TextureOptions::LINEAR);
            state.texture = Some((image.id, handle));
        }
        let Some((_, texture)) = &state.texture else {
            return;
        };
        let half = rect.height() * 0.5;
        let aspect = image.width as f32 / image.height.max(1) as f32;
        let size = egui::vec2(
            state.stencil.size * aspect * half,
            state.stencil.size * half,
        );
        let center = egui::pos2(
            rect.center().x + state.stencil.center.x * half,
            rect.center().y - state.stencil.center.y * half,
        );
        let mut mesh = egui::Mesh::with_texture(texture.id());
        mesh.add_rect_with_uv(
            egui::Rect::from_center_size(center, size),
            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            egui::Color32::from_white_alpha((state.stencil.overlay * 255.0) as u8),
        );
        mesh.rotate(
            egui::emath::Rot2::from_angle(-state.stencil.rotation.to_radians()),
            center,
        );
        ui.painter_at(rect).add(egui::Shape::mesh(mesh));
    }

    /// The Stencil group of the Properties panel.
    pub(super) fn stencil_group(&mut self, ui: &mut egui::Ui) {
        let mut load = false;
        let state = &mut self.paint.stencil;
        theme::group(ui, "Stencil", false, |ui| {
            theme::property_grid("stencil").show(ui, |ui| {
                ui.label("Image");
                ui.horizontal(|ui| {
                    let name = state
                        .stencil
                        .image
                        .as_ref()
                        .map_or("None".to_owned(), |i| i.name.clone());
                    ui.label(name);
                    load = ui.button("Load…").clicked();
                    if state.stencil.image.is_some()
                        && theme::icon_button(ui, theme::icons::DELETE, "Remove the stencil")
                            .clicked()
                    {
                        state.stencil.image = None;
                    }
                });
                ui.end_row();
                if state.stencil.image.is_some() {
                    ui.label("Enabled");
                    ui.checkbox(&mut state.stencil.enabled, "")
                        .on_hover_text("Paint through the stencil. Hold N to ignore it.");
                    ui.end_row();
                    ui.label("Size");
                    ui.add(
                        egui::Slider::new(&mut state.stencil.size, 0.05..=6.0)
                            .logarithmic(true)
                            .max_decimals(2),
                    );
                    ui.end_row();
                    ui.label("Rotation");
                    ui.add(egui::Slider::new(&mut state.stencil.rotation, 0.0..=360.0).suffix("°"));
                    ui.end_row();
                    ui.label("Overlay");
                    ui.add(egui::Slider::new(&mut state.stencil.overlay, 0.0..=1.0));
                    ui.end_row();
                    ui.label("");
                    if ui
                        .button("Reset position")
                        .on_hover_text(
                            "Move to the middle. S + LMB rotates, MMB moves, RMB scales.",
                        )
                        .clicked()
                    {
                        state.stencil.center = Vec2::ZERO;
                        state.stencil.rotation = 0.0;
                        state.stencil.size = 1.0;
                    }
                    ui.end_row();
                }
            });
        });
        if load
            && let Some(path) = rfd::FileDialog::new()
                .add_filter("Images", ink_io::ALPHA_EXTENSIONS)
                .pick_file()
        {
            self.load_stencil(&path);
        }
    }

    pub(super) fn load_stencil(&mut self, path: &std::path::Path) {
        match ink_io::load_alpha(path) {
            Ok(image) => {
                self.paint.stencil.stencil.image = Some(Arc::new(image));
                self.paint.stencil.stencil.enabled = true;
                self.status = format!("Stencil: {}", path.display());
            }
            Err(error) => self.status = format!("{error:#}"),
        }
    }
}
