//! Brush strokes stored as data. The pixels are computed from these on the GPU, so strokes
//! can be painted again at any texture resolution.

use crate::brush::{Alpha, BrushMaterial, Tool};
use glam::{Mat4, Vec2, Vec3};

/// Where a stroke was painted.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum StrokeSpace {
    /// On the model in the 3D view; dabs are in world space.
    #[default]
    Surface,
    /// In the 2D view; dabs are in UV space (`position` is `(u, v, 0)`).
    Uv,
}

/// One stamp of the brush tip.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Dab {
    /// World position, or `(u, v, 0)` for [`StrokeSpace::Uv`].
    pub position: Vec3,
    /// Surface normal; the alpha lies in the plane perpendicular to it.
    pub normal: Vec3,
    /// Direction of the top of the alpha, perpendicular to `normal`.
    pub up: Vec3,
    /// Direction from the camera towards the dab.
    pub view: Vec3,
    /// In world units, or UV units for [`StrokeSpace::Uv`].
    pub radius: f32,
    /// Paint this dab adds, 0 to 1: the brush's flow with the pen pressure applied.
    pub flow: f32,
}

impl Dab {
    /// Interpolates between two dabs, `t` from 0 (`self`) to 1 (`other`).
    pub fn lerp(&self, other: &Self, t: f32) -> Self {
        let normal = self.normal.lerp(other.normal, t).normalize_or(self.normal);
        let up = self.up.lerp(other.up, t);
        let up = (up - normal * up.dot(normal)).normalize_or(self.up);
        Self {
            position: self.position.lerp(other.position, t),
            normal,
            up,
            view: self.view.lerp(other.view, t).normalize_or(self.view),
            radius: self.radius + (other.radius - self.radius) * t,
            flow: self.flow + (other.flow - self.flow) * t,
        }
    }
}

/// The camera a surface stroke was painted from. Texels hidden behind other surfaces in
/// this view get no paint, as in Painter.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StrokeCamera {
    pub view_proj: Mat4,
    pub eye: Vec3,
    /// Unit view direction.
    pub forward: Vec3,
    /// Width / height of the view.
    pub aspect: f32,
    pub orthographic: bool,
    /// World height of the view: at distance 1 for perspective cameras
    /// (`2 tan(fov / 2)`), everywhere for orthographic ones.
    pub height: f32,
}

/// A brush stroke: the brush settings that shape the pixels, and the dabs.
#[derive(Clone, Debug)]
pub struct Stroke {
    /// Unique within the model; see [`crate::Model::next_stroke_id`].
    pub id: u64,
    pub tool: Tool,
    pub space: StrokeSpace,
    pub alpha: Alpha,
    pub hardness: f32,
    /// Maximum opacity of the whole stroke, 0 to 1.
    pub opacity: f32,
    /// Cosine of the brush's backface culling angle.
    pub backface_cos: f32,
    pub material: BrushMaterial,
    /// The view of a surface stroke, for occlusion; None paints through everything.
    pub camera: Option<StrokeCamera>,
    /// Objects that were hidden while painting: they get no paint and hide nothing.
    pub hidden_objects: Vec<usize>,
    /// Texture sets that were hidden while painting: they get no paint and hide nothing.
    pub hidden_sets: Vec<usize>,
    pub dabs: Vec<Dab>,
}

impl Stroke {
    /// Whether the stroke paints the part's triangles (if they are in its texture set).
    /// Hidden objects and texture sets get no paint.
    pub fn paints(&self, part: &crate::Part) -> bool {
        !self.hidden_objects.contains(&part.object) && !self.hidden_sets.contains(&part.texture_set)
    }

    /// Whether the part hid what lay behind it when the stroke was painted.
    pub fn occludes(&self, part: &crate::Part) -> bool {
        !self.hidden_objects.contains(&part.object) && !self.hidden_sets.contains(&part.texture_set)
    }
}

/// Painted content of a texture set. Becomes one layer of the layer stack later.
#[derive(Clone, Debug, Default)]
pub struct PaintLayer {
    pub strokes: Vec<Stroke>,
}

/// Places dabs along the path of a stroke at a fixed spacing.
#[derive(Clone, Debug)]
pub struct DabSpacer {
    /// Distance between dabs as a fraction of the diameter.
    spacing: f32,
    last: Option<Dab>,
    /// Distance travelled since the last dab.
    travelled: f32,
}

impl DabSpacer {
    /// Upper bound of dabs per path segment, against runaway loops with tiny brushes.
    const MAX_DABS_PER_SEGMENT: usize = 4096;

    pub fn new(spacing: f32) -> Self {
        Self {
            spacing: spacing.max(0.001),
            last: None,
            travelled: 0.0,
        }
    }

    /// The last point added, if the path is not broken.
    pub fn last(&self) -> Option<&Dab> {
        self.last.as_ref()
    }

    /// Continues the path to `point` and appends the dabs that fall on the way. The first
    /// point of a path is always a dab.
    pub fn add(&mut self, point: Dab, out: &mut Vec<Dab>) {
        let Some(last) = self.last.replace(point) else {
            out.push(point);
            self.travelled = 0.0;
            return;
        };
        let length = last.position.distance(point.position);
        if length <= 0.0 {
            return;
        }
        if length > 2.0 * last.radius.max(point.radius) {
            // The path jumped, e.g. across a silhouette onto a surface further back: don't
            // place dabs in the air between the two.
            out.push(point);
            self.travelled = 0.0;
            return;
        }
        let mut t = 0.0;
        for _ in 0..Self::MAX_DABS_PER_SEGMENT {
            let here = last.lerp(&point, t);
            let step = (self.spacing * 2.0 * here.radius).max(length * 1e-4);
            // A dab that is already overdue (the radius shrank) goes here, not behind us.
            let need = (step - self.travelled).max(0.0);
            let remaining = (1.0 - t) * length;
            if need > remaining {
                self.travelled += remaining;
                return;
            }
            t += need / length;
            self.travelled = 0.0;
            out.push(last.lerp(&point, t.min(1.0)));
        }
    }

    /// The pointer left the surface: the next point starts a new path with a dab.
    pub fn break_path(&mut self) {
        self.last = None;
        self.travelled = 0.0;
    }
}

/// Lazy mouse (Painter: D): the brush trails the pointer on a string of `distance`
/// pixels and only moves when the string is taut, which smooths out jittery movements.
#[derive(Clone, Debug, Default)]
pub struct LazyMouse {
    /// String length in screen pixels; 0 follows the pointer directly.
    pub distance: f32,
    brush: Option<Vec2>,
}

impl LazyMouse {
    pub fn new(distance: f32) -> Self {
        Self {
            distance,
            brush: None,
        }
    }

    /// Where the brush is, once [`Self::follow`] was called.
    pub fn brush(&self) -> Option<Vec2> {
        self.brush
    }

    /// Starts at `pointer` without a string.
    pub fn reset(&mut self, pointer: Vec2) {
        self.brush = Some(pointer);
    }

    /// Pulls the brush towards the pointer. Returns the new brush position if it moved.
    pub fn follow(&mut self, pointer: Vec2) -> Option<Vec2> {
        let brush = self.brush.get_or_insert(pointer);
        let offset = pointer - *brush;
        let length = offset.length();
        if length <= self.distance {
            return None;
        }
        *brush = pointer - offset / length * self.distance;
        Some(*brush)
    }
}

/// A dab on the surface the pointer hit. `camera_up` orients the alpha so it appears
/// upright on screen before `angle` (degrees, counter-clockwise) rotates it.
pub fn surface_dab(
    hit: &crate::Hit,
    ray_dir: Vec3,
    camera_up: Vec3,
    angle: f32,
    radius: f32,
    flow: f32,
) -> Dab {
    let normal = hit.normal;
    let up = (camera_up - normal * camera_up.dot(normal))
        .try_normalize()
        .unwrap_or_else(|| normal.any_orthonormal_vector());
    let up = glam::Quat::from_axis_angle(normal, angle.to_radians()) * up;
    Dab {
        position: hit.position,
        normal,
        up,
        view: ray_dir,
        radius,
        flow,
    }
}

/// A dab in the 2D view at `uv`. The alpha's top points to the top of the image before
/// `angle` (degrees, counter-clockwise on screen) rotates it.
pub fn uv_dab(uv: Vec2, angle: f32, radius: f32, flow: f32) -> Dab {
    // UV y points down, so counter-clockwise on screen is clockwise in UV.
    let up = Vec2::from_angle(-angle.to_radians()).rotate(Vec2::NEG_Y);
    Dab {
        position: uv.extend(0.0),
        normal: Vec3::Z,
        up: up.extend(0.0),
        view: Vec3::NEG_Z,
        radius,
        flow,
    }
}

/// Turns pointer movement into dabs: the lazy mouse smooths the movement, the path is
/// walked in small screen steps, each step is projected onto the surface, and the
/// spacer places the dabs.
#[derive(Clone, Debug)]
pub struct StrokePath {
    lazy: LazyMouse,
    spacer: DabSpacer,
    /// Brush position and pressure after the last step.
    brush: (Vec2, f32),
    /// Screen position and pressure of the last point the spacer got; None after the path
    /// left the surface.
    last: Option<(Vec2, f32)>,
    /// Longest screen step between projections, in pixels.
    pub screen_step: f32,
}

impl StrokePath {
    /// Starts a path at `pointer` and places its first dab, if the pointer is on the
    /// surface. `project` maps a screen point and pressure to a dab, or None off the model.
    pub fn start(
        pointer: Vec2,
        pressure: f32,
        spacing: f32,
        lazy_distance: f32,
        mut project: impl FnMut(Vec2, f32) -> Option<Dab>,
        out: &mut Vec<Dab>,
    ) -> Self {
        let mut lazy = LazyMouse::new(lazy_distance);
        lazy.reset(pointer);
        let mut path = Self {
            lazy,
            spacer: DabSpacer::new(spacing),
            brush: (pointer, pressure),
            last: None,
            screen_step: 2.0,
        };
        match project(pointer, pressure) {
            Some(dab) => path.feed(pointer, pressure, dab, &mut project, out, 0),
            None => path.spacer.break_path(),
        }
        path
    }

    /// Where the brush is on screen; trails the pointer with lazy mouse.
    pub fn brush(&self) -> Vec2 {
        self.brush.0
    }

    /// The pointer moved to `pointer`.
    pub fn move_to(
        &mut self,
        pointer: Vec2,
        pressure: f32,
        project: impl FnMut(Vec2, f32) -> Option<Dab>,
        out: &mut Vec<Dab>,
    ) {
        if let Some(target) = self.lazy.follow(pointer) {
            self.walk(target, pressure, project, out);
        }
    }

    /// Moves the brush straight to `target`, ignoring the lazy mouse; for straight lines.
    pub fn line_to(
        &mut self,
        target: Vec2,
        pressure: f32,
        project: impl FnMut(Vec2, f32) -> Option<Dab>,
        out: &mut Vec<Dab>,
    ) {
        self.walk(target, pressure, project, out);
        self.lazy.reset(target);
    }

    fn walk(
        &mut self,
        target: Vec2,
        pressure: f32,
        mut project: impl FnMut(Vec2, f32) -> Option<Dab>,
        out: &mut Vec<Dab>,
    ) {
        let (from, from_pressure) = self.brush;
        let steps = subdivide(from, target, self.screen_step);
        let count = (from.distance(target) / self.screen_step.max(0.01))
            .ceil()
            .max(1.0);
        for (i, point) in steps.enumerate().skip(1) {
            let p = from_pressure + (pressure - from_pressure) * (i as f32 / count);
            match project(point, p) {
                Some(dab) => self.feed(point, p, dab, &mut project, out, Self::REFINE_DEPTH),
                None => {
                    self.spacer.break_path();
                    self.last = None;
                }
            }
        }
        self.brush = (target, pressure);
    }

    /// How often a screen step is halved to tell a continuous surface from a jump.
    const REFINE_DEPTH: u32 = 10;

    /// Hands the dab at screen point `b` to the spacer. If it lies further from the previous
    /// point than the brush is wide, the step is halved until the surface in between is
    /// known: small brushes and steep surfaces then still get evenly spaced dabs, and only
    /// a real jump (a silhouette) starts a new path.
    fn feed(
        &mut self,
        b: Vec2,
        b_pressure: f32,
        dab: Dab,
        project: &mut impl FnMut(Vec2, f32) -> Option<Dab>,
        out: &mut Vec<Dab>,
        depth: u32,
    ) {
        if let (Some(previous), Some((a, a_pressure))) = (self.spacer.last().copied(), self.last)
            && depth > 0
            && previous.position.distance(dab.position) > 2.0 * previous.radius.max(dab.radius)
        {
            let middle = (a + b) * 0.5;
            let middle_pressure = (a_pressure + b_pressure) * 0.5;
            match project(middle, middle_pressure) {
                Some(m) => {
                    self.feed(middle, middle_pressure, m, project, out, depth - 1);
                    self.feed(b, b_pressure, dab, project, out, depth - 1);
                }
                None => {
                    self.spacer.break_path();
                    self.spacer.add(dab, out);
                    self.last = Some((b, b_pressure));
                }
            }
            return;
        }
        self.spacer.add(dab, out);
        self.last = Some((b, b_pressure));
    }
}

/// Snaps the direction from `from` to `to` to multiples of `step` radians, keeping the
/// length, for straight lines with Ctrl+Shift.
pub fn snap_direction(from: Vec2, to: Vec2, step: f32) -> Vec2 {
    let offset = to - from;
    let length = offset.length();
    if length == 0.0 {
        return to;
    }
    let angle = (offset.y.atan2(offset.x) / step).round() * step;
    from + Vec2::from_angle(angle) * length
}

/// Points from `from` to `to` (both included) at most `max_step` apart.
pub fn subdivide(from: Vec2, to: Vec2, max_step: f32) -> impl Iterator<Item = Vec2> {
    let steps = (from.distance(to) / max_step.max(0.01)).ceil().max(1.0) as usize;
    (0..=steps).map(move |i| from.lerp(to, i as f32 / steps as f32))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn point(x: f32, radius: f32) -> Dab {
        Dab {
            position: Vec3::new(x, 0.0, 0.0),
            normal: Vec3::Z,
            up: Vec3::Y,
            view: Vec3::NEG_Z,
            radius,
            flow: 1.0,
        }
    }

    #[test]
    fn spacing_is_even_across_samples() {
        // Radius 0.5 and spacing 0.25: a dab every 0.25 units.
        let mut spacer = DabSpacer::new(0.25);
        let mut dabs = Vec::new();
        for i in 0..=10 {
            spacer.add(point(i as f32 * 0.1, 0.5), &mut dabs);
        }
        let xs: Vec<f32> = dabs.iter().map(|d| d.position.x).collect();
        assert_eq!(xs.len(), 5, "{xs:?}");
        for (i, x) in xs.iter().enumerate() {
            assert!((x - i as f32 * 0.25).abs() < 1e-4, "{xs:?}");
        }
    }

    #[test]
    fn spacing_does_not_depend_on_sampling() {
        let run = |samples: usize| {
            let mut spacer = DabSpacer::new(0.1);
            let mut dabs = Vec::new();
            for i in 0..=samples {
                spacer.add(point(i as f32 / samples as f32 * 3.01, 0.2), &mut dabs);
            }
            dabs.len()
        };
        assert_eq!(run(10), run(97));
    }

    #[test]
    fn broken_paths_restart_with_a_dab() {
        let mut spacer = DabSpacer::new(1.0);
        let mut dabs = Vec::new();
        spacer.add(point(0.0, 1.0), &mut dabs);
        spacer.add(point(0.5, 1.0), &mut dabs);
        assert_eq!(dabs.len(), 1);
        spacer.break_path();
        spacer.add(point(5.0, 1.0), &mut dabs);
        assert_eq!(dabs.len(), 2);
        // Standing still adds nothing.
        spacer.add(point(5.0, 1.0), &mut dabs);
        assert_eq!(dabs.len(), 2);
    }

    #[test]
    fn tiny_brushes_do_not_run_away() {
        let mut spacer = DabSpacer::new(0.001);
        let mut dabs = Vec::new();
        spacer.add(point(0.0, 1e-6), &mut dabs);
        spacer.add(point(1e-6, 1e-6), &mut dabs);
        assert!(dabs.len() <= DabSpacer::MAX_DABS_PER_SEGMENT + 1);
    }

    #[test]
    fn jumps_leave_no_dabs_in_between() {
        let mut spacer = DabSpacer::new(0.1);
        let mut dabs = Vec::new();
        spacer.add(point(0.0, 0.1), &mut dabs);
        spacer.add(point(5.0, 0.1), &mut dabs);
        assert_eq!(dabs.len(), 2);
        assert_eq!(dabs[1].position.x, 5.0);
        // Spacing continues from the new point.
        spacer.add(point(5.11, 0.1), &mut dabs);
        assert_eq!(dabs.len(), 7);
    }

    #[test]
    fn interpolated_dabs_stay_orthonormal() {
        let a = point(0.0, 1.0);
        let b = Dab {
            normal: Vec3::X,
            up: Vec3::Y,
            ..point(1.0, 3.0)
        };
        let mid = a.lerp(&b, 0.5);
        assert!((mid.normal.length() - 1.0).abs() < 1e-5);
        assert!(mid.up.dot(mid.normal).abs() < 1e-5);
        assert!((mid.radius - 2.0).abs() < 1e-6);
    }

    #[test]
    fn lazy_mouse_trails_by_its_distance() {
        let mut lazy = LazyMouse::new(10.0);
        lazy.reset(Vec2::ZERO);
        assert_eq!(lazy.follow(Vec2::new(5.0, 0.0)), None, "string still slack");
        let brush = lazy.follow(Vec2::new(30.0, 0.0)).unwrap();
        assert!((brush - Vec2::new(20.0, 0.0)).length() < 1e-5);
        // Without a string the brush follows the pointer.
        let mut direct = LazyMouse::new(0.0);
        direct.reset(Vec2::ZERO);
        assert_eq!(direct.follow(Vec2::ONE), Some(Vec2::ONE));
    }

    #[test]
    fn stroke_paths_project_each_step() {
        // Projects screen x onto the world x axis; misses for x in 40..60.
        let project = |p: Vec2, pressure: f32| {
            (!(40.0..60.0).contains(&p.x)).then(|| Dab {
                flow: pressure,
                ..point(p.x * 0.01, 0.05)
            })
        };
        let mut dabs = Vec::new();
        let mut path = StrokePath::start(Vec2::ZERO, 0.5, 0.5, 0.0, project, &mut dabs);
        assert_eq!(dabs.len(), 1);
        path.move_to(Vec2::new(100.0, 0.0), 1.0, project, &mut dabs);
        assert_eq!(path.brush(), Vec2::new(100.0, 0.0));
        // Dabs 0.05 apart in world space, none over the gap, one where the path resumes.
        assert!(dabs.iter().all(|d| !(0.4..0.6).contains(&d.position.x)));
        assert!(dabs.iter().any(|d| (d.position.x - 0.6).abs() < 0.021));
        assert!(dabs.len() > 15 && dabs.len() < 20, "{}", dabs.len());
        // Pressure is interpolated along the way.
        assert!(dabs.last().unwrap().flow > 0.95);
        assert!(dabs[3].flow > 0.5 && dabs[3].flow < 0.7);
    }

    #[test]
    fn small_brushes_stay_continuous_and_jumps_break() {
        // A 0.1 px wide brush on a surface 1 world unit per pixel, with a step down at
        // x = 50 px (a silhouette onto a surface 10 units further back).
        let project = |p: Vec2, _: f32| {
            let depth = if p.x < 50.0 { 0.0 } else { 10.0 };
            Some(Dab {
                position: Vec3::new(p.x, 0.0, depth),
                ..point(0.0, 0.05)
            })
        };
        let mut dabs = Vec::new();
        let mut path = StrokePath::start(Vec2::ZERO, 1.0, 0.5, 0.0, project, &mut dabs);
        path.move_to(Vec2::new(100.0, 0.0), 1.0, project, &mut dabs);
        let gaps: Vec<f32> = dabs
            .windows(2)
            .map(|w| w[0].position.distance(w[1].position))
            .collect();
        // Spacing 0.5 of the 0.1 diameter: 0.05 apart, except once across the jump.
        let jumps = gaps.iter().filter(|&&g| g > 0.06).count();
        assert_eq!(jumps, 1, "{:?}", gaps.iter().fold(0.0f32, |a, &b| a.max(b)));
        assert!(
            dabs.iter()
                .all(|d| d.position.z == 0.0 || d.position.z == 10.0)
        );
    }

    #[test]
    fn shrinking_brushes_never_place_dabs_behind_the_path() {
        let mut spacer = DabSpacer::new(5.0);
        let mut dabs = Vec::new();
        for i in 0..30 {
            let x = i as f32;
            let start = dabs.len();
            spacer.add(point(x, (10.0 - 0.37 * x).max(0.1)), &mut dabs);
            for d in &dabs[start..] {
                assert!(
                    d.position.x >= x - 1.0 - 1e-4,
                    "dab at {} behind {}",
                    d.position.x,
                    x - 1.0
                );
            }
        }
    }

    #[test]
    fn lazy_paths_trail_the_pointer() {
        let project = |p: Vec2, _| Some(point(p.x, 1.0));
        let mut dabs = Vec::new();
        let mut path = StrokePath::start(Vec2::ZERO, 1.0, 0.5, 20.0, project, &mut dabs);
        path.move_to(Vec2::new(15.0, 0.0), 1.0, project, &mut dabs);
        assert_eq!(path.brush(), Vec2::ZERO);
        path.move_to(Vec2::new(50.0, 0.0), 1.0, project, &mut dabs);
        assert_eq!(path.brush(), Vec2::new(30.0, 0.0));
        path.line_to(Vec2::new(80.0, 0.0), 1.0, project, &mut dabs);
        assert_eq!(path.brush(), Vec2::new(80.0, 0.0));
    }

    #[test]
    fn uv_dabs_point_up_in_the_image() {
        let dab = uv_dab(Vec2::splat(0.5), 0.0, 0.1, 1.0);
        assert!((dab.up - Vec3::NEG_Y).length() < 1e-6);
        // 90° counter-clockwise on screen turns the top to the left.
        let turned = uv_dab(Vec2::splat(0.5), 90.0, 0.1, 1.0);
        assert!((turned.up - Vec3::NEG_X).length() < 1e-6, "{:?}", turned.up);
    }

    #[test]
    fn snapping_keeps_length() {
        let to = snap_direction(Vec2::ZERO, Vec2::new(10.0, 1.0), 15f32.to_radians());
        assert!((to - Vec2::new(10.0f32.hypot(1.0), 0.0)).length() < 1e-4);
        let diagonal = snap_direction(Vec2::ZERO, Vec2::new(10.0, 9.0), 45f32.to_radians());
        assert!((diagonal.x - diagonal.y).abs() < 1e-4);
    }

    #[test]
    fn subdivision_includes_both_ends() {
        let points: Vec<Vec2> = subdivide(Vec2::ZERO, Vec2::new(10.0, 0.0), 3.0).collect();
        assert_eq!(points.first(), Some(&Vec2::ZERO));
        assert_eq!(points.last(), Some(&Vec2::new(10.0, 0.0)));
        assert!(points.windows(2).all(|w| w[0].distance(w[1]) <= 3.0 + 1e-5));
    }
}
