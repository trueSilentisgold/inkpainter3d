//! Paint and Eraser: pointer input to strokes, the brush cursor, the tool shortcuts and
//! the brush settings panels.

use super::InkApp;
use eframe::egui::{self, Color32, Key, PointerButton, RichText, Stroke as Line};
use glam::{Vec2, Vec3};
use ink_core::{
    Alpha, AlphaImage, Brush, Channel, Command, Dab, PressureCurve, SizeSpace, Stroke, StrokePath,
    StrokeSpace, Tool, snap_direction, surface_dab, uv_dab,
};
use std::sync::Arc;

/// The two views the brush paints in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum View {
    /// The 3D view.
    Model,
    /// The 2D view (UV layout).
    Uv,
}

/// Where the last stroke ended, for Shift+click lines. Kept on the model, not on screen,
/// so the line still starts there after the view moved.
#[derive(Clone, Copy, Debug, PartialEq)]
enum LineStart {
    Surface(Vec3),
    Uv(Vec2),
}

/// Brush tools and their settings, plus the stroke being painted.
pub struct PaintState {
    pub tool: Tool,
    /// Settings per tool: Painter keeps them separately for Paint and Eraser.
    pub paint_brush: Brush,
    pub erase_brush: Brush,
    pub lazy_mouse: bool,
    /// Lazy mouse distance in percent of the view height (Painter's Distance).
    pub lazy_distance: f32,
    /// Alphas loaded from images, offered next to the built-in tips.
    pub alphas: Vec<Arc<AlphaImage>>,
    /// Latest pen pressure; 1 without a pen.
    pub pressure: f32,
    stroke: Option<ActiveStroke>,
    line_start: Option<LineStart>,
    adjust: Option<Adjust>,
}

impl Default for PaintState {
    fn default() -> Self {
        Self {
            tool: Tool::Paint,
            paint_brush: Brush::default(),
            erase_brush: Brush::default(),
            lazy_mouse: false,
            lazy_distance: 8.0,
            alphas: Vec::new(),
            pressure: 1.0,
            stroke: None,
            line_start: None,
            adjust: None,
        }
    }
}

impl PaintState {
    pub fn brush(&self) -> &Brush {
        match self.tool {
            Tool::Paint => &self.paint_brush,
            Tool::Erase => &self.erase_brush,
        }
    }

    pub fn brush_mut(&mut self) -> &mut Brush {
        match self.tool {
            Tool::Paint => &mut self.paint_brush,
            Tool::Erase => &mut self.erase_brush,
        }
    }

    pub fn is_painting(&self) -> bool {
        self.stroke.is_some()
    }

    /// A stroke or a Ctrl + drag adjustment is under way; the views don't navigate then.
    pub fn is_busy(&self) -> bool {
        self.stroke.is_some() || self.adjust.is_some()
    }

    /// Forgets strokes and lines that belong to the previous model.
    pub fn reset(&mut self) {
        self.stroke = None;
        self.line_start = None;
        self.adjust = None;
    }
}

struct ActiveStroke {
    view: View,
    set: usize,
    /// The paint layer (of a layer, mask or paint effect) the stroke goes into.
    paint: u64,
    id: u64,
    path: StrokePath,
    /// The brush as the stroke started; switching tools or settings mid-stroke doesn't
    /// change it.
    brush: Brush,
}

/// Ctrl + drag changes brush settings: right button size and hardness, left button flow
/// and angle.
struct Adjust {
    button: PointerButton,
    start: Vec2,
    view: View,
    brush: Brush,
}

fn vec2(p: egui::Pos2) -> Vec2 {
    Vec2::new(p.x, p.y)
}

fn pos2(v: Vec2) -> egui::Pos2 {
    egui::pos2(v.x, v.y)
}

/// Straight lines snap to multiples of this angle with Ctrl+Shift.
const LINE_SNAP: f32 = std::f32::consts::PI / 12.0;

impl InkApp {
    /// Largest side of the model's bounding box; brush sizes in object space refer to it.
    fn model_size(&self) -> f32 {
        let b = self.model.bounds();
        (b.max - b.min).max_element().max(1e-6)
    }

    /// Maps a screen point (and pen pressure) in a view to a dab, or None where the brush
    /// cannot paint.
    fn projector<'a>(
        &'a self,
        view: View,
        rect: egui::Rect,
        brush: &'a Brush,
    ) -> Box<dyn Fn(Vec2, f32) -> Option<Dab> + 'a> {
        let origin = vec2(rect.min);
        let size = Vec2::new(rect.width(), rect.height());
        match view {
            View::Model => {
                let model_size = self.model_size();
                Box::new(move |p, pressure| {
                    let (start, dir) = self.camera.ray(p - origin, size);
                    let hit = self.model.pick(start, dir)?;
                    let (size_factor, flow_factor) = brush.pressure.factors(pressure);
                    let radius = brush
                        .world_radius(model_size, self.camera.view_height_at(hit.position))
                        * size_factor;
                    Some(surface_dab(
                        &hit,
                        dir,
                        self.camera.up(),
                        brush.angle,
                        radius,
                        brush.flow * flow_factor,
                    ))
                })
            }
            View::Uv => Box::new(move |p, pressure| {
                let uv = self.uv_camera.to_uv(p - origin, size);
                let (size_factor, flow_factor) = brush.pressure.factors(pressure);
                let radius = brush.uv_radius(self.uv_camera.height) * size_factor;
                // As in Painter, only the 0-1 UV square can be painted; dabs centered just
                // outside still reach into it.
                let inside = -radius..=1.0 + radius;
                if !inside.contains(&uv.x) || !inside.contains(&uv.y) {
                    return None;
                }
                // Keep the alpha upright on screen when the view is rotated.
                let angle = brush.angle + self.uv_camera.angle.to_degrees();
                Some(uv_dab(uv, angle, radius, brush.flow * flow_factor))
            }),
        }
    }

    /// Lazy mouse string length in pixels for a view.
    fn lazy_pixels(&self, rect: egui::Rect) -> f32 {
        if self.paint.lazy_mouse {
            self.paint.lazy_distance / 100.0 * rect.height()
        } else {
            0.0
        }
    }

    /// Where the next Shift+click line starts in this view, on screen.
    fn line_start_on_screen(&self, view: View, rect: egui::Rect) -> Option<Vec2> {
        let origin = vec2(rect.min);
        let size = Vec2::new(rect.width(), rect.height());
        match (view, self.paint.line_start?) {
            (View::Model, LineStart::Surface(p)) => {
                let screen = self.camera.project(p, size)?;
                Some(screen + origin)
            }
            (View::Uv, LineStart::Uv(uv)) => Some(self.uv_camera.to_pixel(uv, size) + origin),
            _ => None,
        }
    }

    /// Handles the brush in a view: painting with the left button, straight lines with
    /// Shift, brush adjustments with Ctrl + drag.
    pub(super) fn paint_input(
        &mut self,
        ui: &egui::Ui,
        response: &egui::Response,
        rect: egui::Rect,
        view: View,
    ) {
        let events = ui.input(|i| i.events.clone());
        for event in events {
            match event {
                // Pens arrive as touches whose force is the pressure (see pen.rs).
                egui::Event::Touch { phase, force, .. } => match phase {
                    egui::TouchPhase::Start | egui::TouchPhase::Move => {
                        if let Some(force) = force {
                            self.paint.pressure = force.clamp(0.0, 1.0);
                        }
                    }
                    _ => self.paint.pressure = 1.0,
                },
                egui::Event::PointerButton {
                    pos,
                    button,
                    pressed: true,
                    modifiers,
                } => {
                    let mine = rect.contains(pos) && response.hovered();
                    if !mine || modifiers.alt || self.paint.is_busy() {
                        continue;
                    }
                    let p = vec2(pos);
                    if modifiers.command
                        && !modifiers.shift
                        && matches!(button, PointerButton::Primary | PointerButton::Secondary)
                    {
                        self.paint.adjust = Some(Adjust {
                            button,
                            start: p,
                            view,
                            brush: self.paint.brush().clone(),
                        });
                    } else if button == PointerButton::Primary {
                        match self.line_start_on_screen(view, rect) {
                            Some(from) if modifiers.shift => {
                                let to = if modifiers.command {
                                    snap_direction(from, p, LINE_SNAP)
                                } else {
                                    p
                                };
                                self.paint_line(view, rect, from, to);
                            }
                            _ => self.begin_stroke(view, rect, p),
                        }
                    }
                }
                egui::Event::PointerButton {
                    button,
                    pressed: false,
                    ..
                } => {
                    if self
                        .paint
                        .adjust
                        .as_ref()
                        .is_some_and(|a| a.button == button)
                    {
                        self.paint.adjust = None;
                    }
                    if button == PointerButton::Primary {
                        self.end_stroke();
                    }
                }
                egui::Event::PointerMoved(pos) => {
                    if self.paint.stroke.as_ref().is_some_and(|s| s.view == view) {
                        self.continue_stroke(rect, vec2(pos));
                    }
                    if self.paint.adjust.as_ref().is_some_and(|a| a.view == view) {
                        self.adjust_brush(vec2(pos));
                    }
                }
                _ => {}
            }
        }
    }

    /// Ends a stroke or adjustment whose view is no longer shown (F2 / F3 while painting),
    /// whose button release got lost, or when the window loses focus.
    pub(super) fn end_lost_painting(&mut self, ui: &egui::Ui, shown: &[(View, egui::Rect)]) {
        let shown_view = |view: View| shown.iter().any(|&(v, _)| v == view);
        let lost_focus = ui.input(|i| {
            i.events
                .iter()
                .any(|e| matches!(e, egui::Event::WindowFocused(false)))
        });
        let primary_down = ui.input(|i| i.pointer.primary_down());
        if self
            .paint
            .stroke
            .as_ref()
            .is_some_and(|s| !shown_view(s.view) || !primary_down || lost_focus)
        {
            self.end_stroke();
        }
        let adjust_down = |button| ui.input(|i| i.pointer.button_down(button));
        if self
            .paint
            .adjust
            .as_ref()
            .is_some_and(|a| !shown_view(a.view) || !adjust_down(a.button) || lost_focus)
        {
            self.paint.adjust = None;
        }
    }

    /// Adds a stroke with the brush to the paint target.
    fn new_stroke(
        &mut self,
        view: View,
        rect: egui::Rect,
        brush: &Brush,
    ) -> Option<(usize, u64, u64)> {
        let Some((set, paint)) = self.paint_target() else {
            self.status = self.not_paintable_reason().to_owned();
            return None;
        };
        // A layer edit still going on (a value being typed) is recorded before the stroke.
        self.commit_stack_edit();
        let stroke = Stroke {
            id: self.model.next_id(),
            tool: self.paint.tool,
            space: match view {
                View::Model => StrokeSpace::Surface,
                View::Uv => StrokeSpace::Uv,
            },
            alpha: brush.alpha.clone(),
            hardness: brush.hardness,
            opacity: brush.opacity,
            backface_cos: brush.backface_cos(),
            material: brush.material,
            camera: (view == View::Model).then(|| {
                self.camera
                    .stroke_camera(rect.width() / rect.height().max(1.0))
            }),
            hidden_objects: (0..self.model.objects.len())
                .filter(|&o| !self.model.objects[o].visible)
                .collect(),
            hidden_sets: (0..self.model.texture_sets.len())
                .filter(|&s| !self.model.texture_sets[s].visible)
                .collect(),
            dabs: Vec::new(),
        };
        let id = stroke.id;
        self.model.texture_sets[set]
            .stack
            .paint_mut(paint)?
            .strokes
            .push(Arc::new(stroke));
        Some((set, paint, id))
    }

    fn push_dabs(&mut self, set: usize, paint: u64, id: u64, dabs: Vec<Dab>) {
        if let Some(layer) = self.model.texture_sets[set].stack.paint_mut(paint) {
            layer.extend_stroke(id, dabs);
        }
    }

    fn begin_stroke(&mut self, view: View, rect: egui::Rect, p: Vec2) {
        let brush = self.paint.brush().clone();
        let Some((set, paint, id)) = self.new_stroke(view, rect, &brush) else {
            return;
        };
        let mut dabs = Vec::new();
        let path = {
            let project = self.projector(view, rect, &brush);
            let lazy = self.lazy_pixels(rect);
            StrokePath::start(
                p,
                self.paint.pressure,
                brush.spacing,
                lazy,
                &project,
                &mut dabs,
            )
        };
        self.push_dabs(set, paint, id, dabs);
        self.paint.stroke = Some(ActiveStroke {
            view,
            set,
            paint,
            id,
            path,
            brush,
        });
    }

    fn continue_stroke(&mut self, rect: egui::Rect, p: Vec2) {
        let Some(mut active) = self.paint.stroke.take() else {
            return;
        };
        let mut dabs = Vec::new();
        {
            let project = self.projector(active.view, rect, &active.brush);
            active
                .path
                .move_to(p, self.paint.pressure, &project, &mut dabs);
        }
        self.push_dabs(active.set, active.paint, active.id, dabs);
        self.paint.stroke = Some(active);
    }

    fn end_stroke(&mut self) {
        if let Some(active) = self.paint.stroke.take() {
            self.finish_stroke(active.set, active.paint, active.id);
        }
    }

    /// Records a finished stroke for undo and remembers where it ended for Shift+click.
    fn finish_stroke(&mut self, set: usize, paint: u64, id: u64) {
        let Some(layer) = self.model.texture_sets[set].stack.paint_mut(paint) else {
            return;
        };
        let strokes = &mut layer.strokes;
        let Some(stroke) = strokes.last().filter(|s| s.id == id) else {
            return;
        };
        match stroke.dabs.last() {
            Some(end) => {
                self.paint.line_start = Some(match stroke.space {
                    StrokeSpace::Surface => LineStart::Surface(end.position),
                    StrokeSpace::Uv => LineStart::Uv(end.position.truncate()),
                });
                self.history.record(Command::add_stroke(set, paint, id));
            }
            // Nothing was hit; leave no empty step in the history.
            None => {
                strokes.pop();
            }
        }
    }

    /// Shift+click: a straight stroke from the end of the last one.
    fn paint_line(&mut self, view: View, rect: egui::Rect, from: Vec2, to: Vec2) {
        let brush = self.paint.brush().clone();
        let Some((set, paint, id)) = self.new_stroke(view, rect, &brush) else {
            return;
        };
        let mut dabs = Vec::new();
        {
            let project = self.projector(view, rect, &brush);
            let pressure = self.paint.pressure;
            let mut path =
                StrokePath::start(from, pressure, brush.spacing, 0.0, &project, &mut dabs);
            path.line_to(to, pressure, &project, &mut dabs);
        }
        self.push_dabs(set, paint, id, dabs);
        self.finish_stroke(set, paint, id);
    }

    fn adjust_brush(&mut self, p: Vec2) {
        let Some(adjust) = &self.paint.adjust else {
            return;
        };
        let delta = p - adjust.start;
        let start = adjust.brush.clone();
        let button = adjust.button;
        let brush = self.paint.brush_mut();
        match button {
            PointerButton::Secondary => {
                brush.size = (start.size * (delta.x * 0.01).exp())
                    .clamp(*Brush::SIZE_RANGE.start(), *Brush::SIZE_RANGE.end());
                brush.hardness = (start.hardness - delta.y * 0.005).clamp(0.0, 1.0);
            }
            _ => {
                brush.flow = (start.flow + delta.x * 0.005).clamp(0.0, 1.0);
                brush.angle = (start.angle - delta.y * 0.5).rem_euclid(360.0);
            }
        }
    }

    // ---- Undo ----

    pub(super) fn undo(&mut self) {
        if self.paint.is_painting() {
            return;
        }
        self.commit_stack_edit();
        match self.history.undo(&mut self.model) {
            Some(label) => self.status = format!("Undo: {label}"),
            None => self.status = "Nothing to undo".into(),
        }
    }

    pub(super) fn redo(&mut self) {
        if self.paint.is_painting() {
            return;
        }
        self.commit_stack_edit();
        match self.history.redo(&mut self.model) {
            Some(label) => self.status = format!("Redo: {label}"),
            None => self.status = "Nothing to redo".into(),
        }
    }

    /// Tool and brush shortcuts: 1 Paint, 2 Eraser, [ and ] size, D lazy mouse, X inverts
    /// the grayscale painted into masks, Ctrl+Z / Ctrl+Y undo and redo.
    pub(super) fn paint_shortcuts(&mut self, ui: &egui::Ui) {
        if ui.ctx().text_edit_focused() {
            return;
        }
        let modifiers = ui.input(|i| i.modifiers);
        let pressed = |key| ui.input(|i| i.key_pressed(key));
        // Before the Ctrl check: on German layouts [ and ] are AltGr+8 / 9, which Windows
        // reports as Ctrl+Alt.
        if pressed(Key::OpenBracket) {
            self.paint.brush_mut().step_size(-1.0);
        }
        if pressed(Key::CloseBracket) {
            self.paint.brush_mut().step_size(1.0);
        }
        if modifiers.command {
            if pressed(Key::Z) {
                if modifiers.shift {
                    self.redo();
                } else {
                    self.undo();
                }
            }
            if pressed(Key::Y) {
                self.redo();
            }
            return;
        }
        if pressed(Key::Num1) {
            self.paint.tool = Tool::Paint;
        }
        if pressed(Key::Num2) {
            self.paint.tool = Tool::Erase;
        }
        if pressed(Key::D) {
            self.paint.lazy_mouse = !self.paint.lazy_mouse;
        }
        if pressed(Key::X) && self.paints_grayscale() {
            let gray = &mut self.paint.paint_brush.material.grayscale;
            *gray = 1.0 - *gray;
        }
    }

    // ---- Cursor ----

    /// Draws the brush outline where it would paint, the lazy mouse string, the guide
    /// for Shift+click lines and the values changed with Ctrl + drag.
    pub(super) fn draw_brush_cursor(&self, ui: &egui::Ui, rect: egui::Rect, view: View) {
        let Some(pointer) = ui.input(|i| i.pointer.latest_pos()) else {
            return;
        };
        let modifiers = ui.input(|i| i.modifiers);
        let painter = ui.painter_at(rect);
        let active = self.paint.stroke.as_ref().filter(|s| s.view == view);
        if !rect.contains(pointer) && active.is_none() {
            return;
        }
        if let Some(adjust) = self.paint.adjust.as_ref().filter(|a| a.view == view) {
            let brush = self.paint.brush();
            let text = match adjust.button {
                PointerButton::Secondary => {
                    format!("Size {:.2}   Hardness {:.2}", brush.size, brush.hardness)
                }
                _ => format!("Flow {:.0}   Angle {:.0}°", brush.flow * 100.0, brush.angle),
            };
            painter.text(
                pos2(adjust.start) + egui::vec2(16.0, -16.0),
                egui::Align2::LEFT_BOTTOM,
                text,
                egui::FontId::proportional(13.0),
                Color32::WHITE,
            );
            self.draw_outline(&painter, rect, view, adjust.start);
            return;
        }
        if modifiers.alt && active.is_none() {
            return;
        }
        let p = vec2(pointer);
        let brush_pos = active.map_or(p, |s| s.path.brush());
        if self.paint.lazy_mouse {
            let radius = self.lazy_pixels(rect);
            painter.circle_filled(pointer, radius, Color32::from_white_alpha(14));
            if active.is_some() {
                painter.line_segment(
                    [pointer, pos2(brush_pos)],
                    Line::new(1.0, Color32::from_gray(150)),
                );
            }
        }
        if modifiers.shift
            && active.is_none()
            && let Some(from) = self.line_start_on_screen(view, rect)
        {
            let to = if modifiers.command {
                snap_direction(from, p, LINE_SNAP)
            } else {
                p
            };
            painter.extend(egui::Shape::dashed_line(
                &[pos2(from), pos2(to)],
                Line::new(1.5, Color32::WHITE),
                6.0,
                4.0,
            ));
        }
        self.draw_outline(&painter, rect, view, brush_pos);
    }

    /// The brush's outline at screen point `p`: a circle on the surface in 3D, a circle in
    /// UV space in 2D.
    fn draw_outline(&self, painter: &egui::Painter, rect: egui::Rect, view: View, p: Vec2) {
        let (brush, pressure) = match &self.paint.stroke {
            Some(stroke) => (&stroke.brush, self.paint.pressure),
            None => (self.paint.brush(), 1.0),
        };
        let (size_factor, _) = brush.pressure.factors(pressure);
        let points: Vec<egui::Pos2> = match view {
            View::Model => {
                let origin = vec2(rect.min);
                let size = Vec2::new(rect.width(), rect.height());
                let (start, dir) = self.camera.ray(p - origin, size);
                let Some(hit) = self.model.pick(start, dir) else {
                    return;
                };
                let radius = brush
                    .world_radius(self.model_size(), self.camera.view_height_at(hit.position))
                    * size_factor;
                let dab = surface_dab(&hit, dir, self.camera.up(), brush.angle, radius, 1.0);
                let right = dab.up.cross(dab.normal);
                (0..=64)
                    .filter_map(|i| {
                        let a = i as f32 / 64.0 * std::f32::consts::TAU;
                        let world = dab.position + (right * a.cos() + dab.up * a.sin()) * radius;
                        self.camera.project(world, size).map(|s| pos2(s + origin))
                    })
                    .collect()
            }
            View::Uv => {
                let size = Vec2::new(rect.width(), rect.height());
                let radius = brush.uv_radius(self.uv_camera.height) * size_factor
                    / self.uv_camera.uv_per_pixel(size);
                (0..=64)
                    .map(|i| {
                        let a = i as f32 / 64.0 * std::f32::consts::TAU;
                        pos2(p + Vec2::new(a.cos(), a.sin()) * radius)
                    })
                    .collect()
            }
        };
        if points.len() < 2 {
            return;
        }
        painter.add(egui::Shape::line(
            points.clone(),
            Line::new(2.5, Color32::from_black_alpha(140)),
        ));
        painter.extend(egui::Shape::dashed_line(
            &points,
            Line::new(1.2, Color32::WHITE),
            5.0,
            4.0,
        ));
        painter.circle_filled(pos2(p), 1.5, Color32::WHITE);
    }

    // ---- Panels ----

    /// Painter's tool bar on the left: one button per tool.
    pub(super) fn tool_strip(&mut self, ui: &mut egui::Ui) {
        ui.add_space(6.0);
        for (tool, label, key) in [(Tool::Paint, "Paint", "1"), (Tool::Erase, "Eraser", "2")] {
            let selected = self.paint.tool == tool;
            if ui
                .add_sized([64.0, 28.0], egui::Button::selectable(selected, label))
                .on_hover_text(format!("{label} ({key})"))
                .clicked()
            {
                self.paint.tool = tool;
            }
        }
    }

    /// Painter's contextual tool bar above the views.
    pub(super) fn tool_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let tool = match self.paint.tool {
                Tool::Paint => "Paint",
                Tool::Erase => "Eraser",
            };
            ui.label(RichText::new(tool).strong());
            ui.separator();
            let brush = self.paint.brush_mut();
            ui.label("Size");
            ui.add(
                egui::DragValue::new(&mut brush.size)
                    .range(Brush::SIZE_RANGE)
                    .speed(0.1)
                    .max_decimals(2),
            );
            pressure_toggle(ui, &mut brush.pressure.size, "Size follows pen pressure");
            ui.label("Flow");
            percent(ui, &mut brush.flow, 0.0..=100.0);
            pressure_toggle(ui, &mut brush.pressure.flow, "Flow follows pen pressure");
            ui.label("Stroke opacity");
            percent(ui, &mut brush.opacity, 0.0..=100.0);
            ui.label("Spacing");
            percent(ui, &mut brush.spacing, 1.0..=1000.0);
            ui.separator();
            ui.checkbox(&mut self.paint.lazy_mouse, "Lazy mouse")
                .on_hover_text("Trails the brush behind the pointer for smooth strokes (D)");
            ui.add_enabled(
                self.paint.lazy_mouse,
                egui::DragValue::new(&mut self.paint.lazy_distance)
                    .range(1.0..=50.0)
                    .speed(0.2)
                    .prefix("Distance "),
            );
            ui.separator();
            let curve = &mut self.paint.brush_mut().pressure.curve;
            egui::ComboBox::from_id_salt("pressure curve")
                .selected_text(curve_label(*curve))
                .show_ui(ui, |ui| {
                    for c in [
                        PressureCurve::Linear,
                        PressureCurve::EaseIn,
                        PressureCurve::EaseInOut,
                    ] {
                        ui.selectable_value(curve, c, curve_label(c));
                    }
                })
                .response
                .on_hover_text("Pen pressure curve");
        });
    }

    /// The Properties panel for the current tool: brush, alpha and material.
    pub(super) fn properties_panel(&mut self, ui: &mut egui::Ui) {
        let tool = self.paint.tool;
        let grayscale = self.paints_grayscale();
        let mut load_alpha = false;
        let alphas = self.paint.alphas.clone();
        let brush = self.paint.brush_mut();

        section(ui, "BRUSH");
        egui::Grid::new("brush")
            .num_columns(2)
            .spacing([12.0, 6.0])
            .show(ui, |ui| {
                ui.label("Size");
                ui.horizontal(|ui| {
                    ui.add(
                        egui::Slider::new(&mut brush.size, Brush::SIZE_RANGE)
                            .logarithmic(true)
                            .max_decimals(2),
                    );
                    pressure_toggle(ui, &mut brush.pressure.size, "Size follows pen pressure");
                });
                ui.end_row();
                ui.label("Size space");
                egui::ComboBox::from_id_salt("size space")
                    .selected_text(size_space_label(brush.size_space))
                    .show_ui(ui, |ui| {
                        for space in [SizeSpace::Object, SizeSpace::Viewport] {
                            ui.selectable_value(
                                &mut brush.size_space,
                                space,
                                size_space_label(space),
                            );
                        }
                    });
                ui.end_row();
                ui.label("Flow");
                ui.horizontal(|ui| {
                    percent_slider(ui, &mut brush.flow, 0.0..=100.0);
                    pressure_toggle(ui, &mut brush.pressure.flow, "Flow follows pen pressure");
                });
                ui.end_row();
                ui.label("Stroke opacity");
                percent_slider(ui, &mut brush.opacity, 0.0..=100.0);
                ui.end_row();
                ui.label("Spacing");
                let mut spacing = brush.spacing * 100.0;
                if ui
                    .add(
                        egui::Slider::new(&mut spacing, 1.0..=1000.0)
                            .logarithmic(true)
                            .suffix(" %"),
                    )
                    .changed()
                {
                    brush.spacing = spacing / 100.0;
                }
                ui.end_row();
                ui.label("Angle");
                ui.add(egui::Slider::new(&mut brush.angle, 0.0..=360.0).suffix("°"));
                ui.end_row();
                ui.label("Backface culling");
                ui.horizontal(|ui| {
                    ui.checkbox(&mut brush.backface_culling, "");
                    ui.add_enabled(
                        brush.backface_culling,
                        egui::Slider::new(&mut brush.backface_angle, 0.0..=180.0).suffix("°"),
                    );
                });
                ui.end_row();
                if brush.pressure.size {
                    ui.label("Minimum size");
                    percent_slider(ui, &mut brush.pressure.min_size, 0.0..=100.0);
                    ui.end_row();
                }
                if brush.pressure.flow {
                    ui.label("Minimum flow");
                    percent_slider(ui, &mut brush.pressure.min_flow, 0.0..=100.0);
                    ui.end_row();
                }
            });

        section(ui, "ALPHA");
        egui::Grid::new("alpha")
            .num_columns(2)
            .spacing([12.0, 6.0])
            .show(ui, |ui| {
                ui.label("Shape");
                ui.horizontal(|ui| {
                    egui::ComboBox::from_id_salt("alpha")
                        .selected_text(brush.alpha.name().to_owned())
                        .show_ui(ui, |ui| {
                            let mut choices = vec![Alpha::Round, Alpha::Square];
                            choices.extend(alphas.iter().cloned().map(Alpha::Image));
                            for alpha in choices {
                                let selected = brush.alpha.same(&alpha);
                                if ui.selectable_label(selected, alpha.name()).clicked() {
                                    brush.alpha = alpha;
                                }
                            }
                        });
                    load_alpha = ui.small_button("Load…").clicked();
                });
                ui.end_row();
                if !matches!(brush.alpha, Alpha::Image(_)) {
                    ui.label("Hardness");
                    ui.add(egui::Slider::new(&mut brush.hardness, 0.0..=1.0));
                    ui.end_row();
                }
            });

        let material = &mut brush.material;
        if grayscale {
            // Masks and their paint effects hold one grayscale value; the eraser just
            // takes paint away.
            if tool == Tool::Paint {
                section(ui, "GRAYSCALE");
                ui.horizontal(|ui| {
                    ui.add(egui::Slider::new(&mut material.grayscale, 0.0..=1.0));
                    if ui
                        .button("⬌")
                        .on_hover_text("Invert: paint black instead of white (X)")
                        .clicked()
                    {
                        material.grayscale = 1.0 - material.grayscale;
                    }
                });
            }
        } else {
            section(
                ui,
                match tool {
                    Tool::Paint => "MATERIAL",
                    Tool::Erase => "CHANNELS",
                },
            );
            Self::material_grid(ui, tool, material);
        }

        if load_alpha
            && let Some(path) = rfd::FileDialog::new()
                .add_filter("Alpha images", ink_io::ALPHA_EXTENSIONS)
                .pick_file()
        {
            self.load_alpha(&path);
        }
    }

    /// Channel toggles and values of the brush material.
    fn material_grid(ui: &mut egui::Ui, tool: Tool, material: &mut ink_core::BrushMaterial) {
        egui::Grid::new("material")
            .num_columns(2)
            .spacing([12.0, 6.0])
            .show(ui, |ui| {
                for channel in Channel::ALL {
                    let mut on = material.channels.contains(channel);
                    if ui
                        .toggle_value(&mut on, channel.label())
                        .on_hover_text(match tool {
                            Tool::Paint => "Paint this channel",
                            Tool::Erase => "Erase this channel",
                        })
                        .changed()
                    {
                        material.channels.set(channel, on);
                    }
                    if tool == Tool::Paint {
                        ui.add_enabled_ui(on, |ui| {
                            super::channel_value_edit(ui, &mut material.values, channel);
                        });
                    } else {
                        ui.label("");
                    }
                    ui.end_row();
                }
            });
    }

    pub(super) fn load_alpha(&mut self, path: &std::path::Path) {
        match ink_io::load_alpha(path) {
            Ok(image) => {
                let image = Arc::new(image);
                self.status = format!("Alpha: {}", path.display());
                self.paint.brush_mut().alpha = Alpha::Image(image.clone());
                self.paint.alphas.push(image);
            }
            Err(err) => {
                log::error!("{err:#}");
                self.status = format!("Error: {err:#}");
            }
        }
    }
}

fn section(ui: &mut egui::Ui, title: &str) {
    ui.add_space(6.0);
    ui.label(RichText::new(title).small().weak());
}

fn pressure_toggle(ui: &mut egui::Ui, on: &mut bool, hint: &str) {
    ui.toggle_value(on, RichText::new("✒").small())
        .on_hover_text(hint);
}

/// Drag value for a fraction shown in percent.
fn percent(ui: &mut egui::Ui, value: &mut f32, range: std::ops::RangeInclusive<f32>) {
    let mut shown = *value * 100.0;
    if ui
        .add(
            egui::DragValue::new(&mut shown)
                .range(range)
                .speed(0.5)
                .max_decimals(0)
                .suffix(" %"),
        )
        .changed()
    {
        *value = shown / 100.0;
    }
}

/// Slider for a fraction shown in percent.
fn percent_slider(ui: &mut egui::Ui, value: &mut f32, range: std::ops::RangeInclusive<f32>) {
    let mut shown = *value * 100.0;
    if ui
        .add(
            egui::Slider::new(&mut shown, range)
                .max_decimals(0)
                .suffix(" %"),
        )
        .changed()
    {
        *value = shown / 100.0;
    }
}

fn size_space_label(space: SizeSpace) -> &'static str {
    match space {
        SizeSpace::Object => "Object",
        SizeSpace::Viewport => "Viewport",
    }
}

fn curve_label(curve: PressureCurve) -> &'static str {
    match curve {
        PressureCurve::Linear => "Linear",
        PressureCurve::EaseIn => "Ease in",
        PressureCurve::EaseInOut => "Ease in out",
    }
}
