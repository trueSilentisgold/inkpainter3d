//! Brush settings as the Properties panel edits them.

use crate::model::ChannelValues;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// The painting tools of phase 3, Painter's 1 and 2 keys.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tool {
    #[default]
    Paint,
    Erase,
}

/// Texture set channels: what layers and brushes paint. The order is Painter's and the
/// order of the channel textures on the GPU.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Channel {
    BaseColor,
    Height,
    Roughness,
    Metallic,
    Normal,
    Emissive,
    Opacity,
}

/// How a channel's values are stored and blended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChannelKind {
    /// An sRGB color (base color, emissive), blended in linear space.
    Color,
    /// A single linear value.
    Scalar,
    /// A tangent-space normal, encoded like a normal map texel (0.5, 0.5, 1 is flat).
    Normal,
}

impl Channel {
    pub const COUNT: usize = 7;
    pub const ALL: [Self; Self::COUNT] = [
        Self::BaseColor,
        Self::Height,
        Self::Roughness,
        Self::Metallic,
        Self::Normal,
        Self::Emissive,
        Self::Opacity,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::BaseColor => "Base color",
            Self::Height => "Height",
            Self::Roughness => "Roughness",
            Self::Metallic => "Metallic",
            Self::Normal => "Normal",
            Self::Emissive => "Emissive",
            Self::Opacity => "Opacity",
        }
    }

    /// The short names on Painter's material buttons.
    pub fn short_label(self) -> &'static str {
        match self {
            Self::BaseColor => "color",
            Self::Height => "height",
            Self::Roughness => "rough",
            Self::Metallic => "metal",
            Self::Normal => "nrm",
            Self::Emissive => "emiss",
            Self::Opacity => "op",
        }
    }

    pub fn kind(self) -> ChannelKind {
        match self {
            Self::BaseColor | Self::Emissive => ChannelKind::Color,
            Self::Normal => ChannelKind::Normal,
            _ => ChannelKind::Scalar,
        }
    }

    /// Color channels hold sRGB colors, the others linear data.
    pub fn is_color(self) -> bool {
        self.kind() == ChannelKind::Color
    }

    pub fn index(self) -> usize {
        self as usize
    }

    /// A stable name for files (projects, export templates); never changes.
    pub fn key(self) -> &'static str {
        match self {
            Self::BaseColor => "base_color",
            Self::Height => "height",
            Self::Roughness => "roughness",
            Self::Metallic => "metallic",
            Self::Normal => "normal",
            Self::Emissive => "emissive",
            Self::Opacity => "opacity",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|c| c.key() == key)
    }
}

/// A set of channels, e.g. the channels a brush paints.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ChannelSet(u8);

impl ChannelSet {
    pub const NONE: Self = Self(0);
    pub const ALL: Self = Self((1 << Channel::ALL.len()) - 1);

    pub fn contains(self, channel: Channel) -> bool {
        self.0 & (1 << channel.index()) != 0
    }

    pub fn set(&mut self, channel: Channel, on: bool) {
        if on {
            self.0 |= 1 << channel.index();
        } else {
            self.0 &= !(1 << channel.index());
        }
    }

    pub fn iter(self) -> impl Iterator<Item = Channel> {
        Channel::ALL.into_iter().filter(move |&c| self.contains(c))
    }

    pub fn bits(self) -> u32 {
        self.0 as u32
    }
}

/// What the brush paints: a value per channel, and which channels it paints at all.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BrushMaterial {
    pub channels: ChannelSet,
    /// Colors are sRGB.
    pub values: ChannelValues,
    /// The value painted into masks, 0 (black) to 1 (white).
    pub grayscale: f32,
}

impl Default for BrushMaterial {
    fn default() -> Self {
        Self {
            channels: ChannelSet::ALL,
            values: ChannelValues {
                base_color: [1.0; 3],
                roughness: 0.3,
                ..ChannelValues::default()
            },
            grayscale: 1.0,
        }
    }
}

/// A grayscale brush shape loaded from an image. White paints, black does not.
#[derive(Debug)]
pub struct AlphaImage {
    /// Unique within the running program; lets the renderer upload each image once.
    pub id: u64,
    pub name: String,
    pub width: u32,
    pub height: u32,
    /// Row by row from the top, one byte per pixel.
    pub pixels: Vec<u8>,
}

impl AlphaImage {
    pub fn new(name: String, width: u32, height: u32, pixels: Vec<u8>) -> Self {
        static NEXT_ID: AtomicU64 = AtomicU64::new(1);
        assert_eq!(pixels.len(), (width * height) as usize);
        Self {
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            name,
            width,
            height,
            pixels,
        }
    }
}

/// The shape of a dab.
#[derive(Clone, Debug)]
pub enum Alpha {
    /// Round tip whose edge falloff is set by the hardness.
    Round,
    /// Square tip whose edge falloff is set by the hardness.
    Square,
    Image(Arc<AlphaImage>),
}

impl Alpha {
    pub fn name(&self) -> &str {
        match self {
            Self::Round => "Round",
            Self::Square => "Square",
            Self::Image(image) => &image.name,
        }
    }

    pub fn same(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Round, Self::Round) | (Self::Square, Self::Square) => true,
            (Self::Image(a), Self::Image(b)) => a.id == b.id,
            _ => false,
        }
    }
}

/// What the brush size is relative to (Painter's Size Space).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SizeSpace {
    /// Relative to the model's bounding box, so the brush covers the same part of the
    /// model at any zoom.
    #[default]
    Object,
    /// Relative to the view, so the brush keeps its size on screen when zooming.
    Viewport,
}

/// How pen pressure maps to the brush parameters.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PressureCurve {
    Linear,
    /// Light pressure has little effect; the default in Painter.
    #[default]
    EaseIn,
    EaseInOut,
}

impl PressureCurve {
    pub fn apply(self, pressure: f32) -> f32 {
        let p = pressure.clamp(0.0, 1.0);
        match self {
            Self::Linear => p,
            Self::EaseIn => p * p,
            Self::EaseInOut => p * p * (3.0 - 2.0 * p),
        }
    }
}

/// Which brush parameters follow the pen pressure. As in Painter, stroke opacity cannot:
/// it applies to the finished stroke.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PressureMapping {
    pub size: bool,
    pub flow: bool,
    /// Size at the lightest pressure, as a fraction of the full size.
    pub min_size: f32,
    /// Flow at the lightest pressure, as a fraction of the full flow.
    pub min_flow: f32,
    pub curve: PressureCurve,
}

impl Default for PressureMapping {
    fn default() -> Self {
        Self {
            size: true,
            flow: false,
            min_size: 0.05,
            min_flow: 0.05,
            curve: PressureCurve::default(),
        }
    }
}

impl PressureMapping {
    /// Size and flow multipliers at this pen pressure (1 for a mouse).
    pub fn factors(&self, pressure: f32) -> (f32, f32) {
        let p = self.curve.apply(pressure);
        let factor = |on: bool, min: f32| if on { min + (1.0 - min) * p } else { 1.0 };
        (
            factor(self.size, self.min_size),
            factor(self.flow, self.min_flow),
        )
    }
}

/// How a surface dab projects its tip onto the model (Painter's Alignment).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Alignment {
    /// The tip follows the surface: distances are measured in 3D, so it wraps around
    /// curved surfaces and edges.
    #[default]
    TangentWrap,
    /// The tip is projected straight along the surface normal at the dab.
    TangentPlanar,
    /// The tip is projected along the view direction, like a decal from the camera.
    Camera,
}

impl Alignment {
    pub const ALL: [Self; 3] = [Self::TangentWrap, Self::TangentPlanar, Self::Camera];

    pub fn label(self) -> &'static str {
        match self {
            Self::TangentWrap => "Tangent wrap",
            Self::TangentPlanar => "Tangent planar",
            Self::Camera => "Camera",
        }
    }

    /// A stable name for files; never changes.
    pub fn key(self) -> &'static str {
        match self {
            Self::TangentWrap => "tangent_wrap",
            Self::TangentPlanar => "tangent_planar",
            Self::Camera => "camera",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|a| a.key() == key)
    }

    /// The code the shader reads (`ALIGN_*` in paint_dabs.wgsl).
    pub fn code(self) -> u32 {
        self as u32
    }
}

/// Random variation per dab, each 0 (none) to 1 (full), as in Painter's Jitter section.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Jitter {
    /// Up to this fraction of the size is taken away from each dab.
    pub size: f32,
    /// Up to this fraction of the flow is taken away from each dab.
    pub flow: f32,
    /// Random rotation of up to ±180° times this.
    pub angle: f32,
    /// Random offset of up to this many radii.
    pub position: f32,
}

impl Jitter {
    pub fn is_none(&self) -> bool {
        *self == Self::default()
    }

    /// Varies a dab. `rng` gives numbers in 0..1; the same stream gives the same dab.
    pub fn apply(&self, dab: &mut crate::Dab, mut rng: impl FnMut() -> f32) {
        // Always draw all five numbers so changing one setting does not shift the others.
        let (size, flow, angle, ox, oy) = (rng(), rng(), rng(), rng(), rng());
        dab.radius *= 1.0 - self.size * size;
        dab.flow *= 1.0 - self.flow * flow;
        let normal = dab.normal;
        if self.angle > 0.0 {
            let turn = (angle * 2.0 - 1.0) * 180.0 * self.angle;
            dab.up = glam::Quat::from_axis_angle(normal, turn.to_radians()) * dab.up;
        }
        if self.position > 0.0 {
            let right = dab.up.cross(normal);
            dab.position +=
                (right * (ox * 2.0 - 1.0) + dab.up * (oy * 2.0 - 1.0)) * self.position * dab.radius;
        }
    }
}

/// A small deterministic random stream (splitmix64), so jittered strokes replay exactly.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ 0xD1B5_4A32_D192_ED03)
    }

    /// A number in 0..1.
    pub fn next_f32(&mut self) -> f32 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^= z >> 31;
        (z >> 40) as f32 / (1u64 << 24) as f32
    }
}

/// Settings of the Paint and Eraser tools, as in Painter's Properties panel and contextual
/// toolbar. Values Painter shows in percent are fractions here, except `size`.
#[derive(Clone, Debug)]
pub struct Brush {
    /// 0.01 to 100, like Painter; see [`SizeSpace`] for what it is relative to.
    pub size: f32,
    pub size_space: SizeSpace,
    /// Paint added per dab, 0 to 1.
    pub flow: f32,
    /// Maximum opacity of one stroke, 0 to 1.
    pub opacity: f32,
    /// Distance between dabs as a fraction of the brush diameter.
    pub spacing: f32,
    /// Rotation of the alpha in degrees, counter-clockwise.
    pub angle: f32,
    /// Soft falloff (0) to hard edge (1) of the round and square tips.
    pub hardness: f32,
    /// Turns the alpha to the direction of the stroke (Painter's Follow Path).
    pub follow_path: bool,
    pub jitter: Jitter,
    pub alignment: Alignment,
    pub alpha: Alpha,
    /// Whether surfaces turned away from the camera are skipped.
    pub backface_culling: bool,
    /// Surfaces turned further than this from the camera (degrees) are not painted.
    pub backface_angle: f32,
    pub pressure: PressureMapping,
    /// Channels and values the brush paints; the Eraser uses only the channels.
    pub material: BrushMaterial,
}

impl Default for Brush {
    fn default() -> Self {
        // Painter's defaults on a new project: size 10, flow 100, stroke opacity 100,
        // spacing 20, backface culling on at 90 degrees.
        Self {
            size: 10.0,
            size_space: SizeSpace::default(),
            flow: 1.0,
            opacity: 1.0,
            spacing: 0.2,
            angle: 0.0,
            hardness: 0.0,
            follow_path: false,
            jitter: Jitter::default(),
            alignment: Alignment::default(),
            alpha: Alpha::Round,
            backface_culling: true,
            backface_angle: 90.0,
            pressure: PressureMapping::default(),
            material: BrushMaterial::default(),
        }
    }
}

impl Brush {
    /// Smallest and largest [`Self::size`].
    pub const SIZE_RANGE: std::ops::RangeInclusive<f32> = 0.01..=100.0;
    pub const SPACING_RANGE: std::ops::RangeInclusive<f32> = 0.01..=10.0;

    /// Brush radius in world units at full pressure.
    ///
    /// `model_size` is the largest side of the model's bounding box; `view_height` the
    /// world size of the viewport's height at the painted point.
    pub fn world_radius(&self, model_size: f32, view_height: f32) -> f32 {
        self.fraction()
            * 0.5
            * match self.size_space {
                // Size 100 is as wide as the model.
                SizeSpace::Object => model_size,
                // Size 100 is as wide as the view is high.
                SizeSpace::Viewport => view_height,
            }
    }

    /// Brush radius in UV units for the 2D view, where `view_height` is the UV height the
    /// view shows.
    pub fn uv_radius(&self, view_height: f32) -> f32 {
        self.fraction()
            * 0.5
            * match self.size_space {
                // Size 100 spans the UV square.
                SizeSpace::Object => 1.0,
                SizeSpace::Viewport => view_height,
            }
    }

    fn fraction(&self) -> f32 {
        self.size
            .clamp(*Self::SIZE_RANGE.start(), *Self::SIZE_RANGE.end())
            / 100.0
    }

    /// Scales the size by `steps` keyboard steps ([ and ]), each a fixed ratio.
    pub fn step_size(&mut self, steps: f32) {
        self.size = (self.size * 1.2f32.powf(steps))
            .clamp(*Self::SIZE_RANGE.start(), *Self::SIZE_RANGE.end());
    }

    /// Cosine of the backface culling angle; -1 paints every surface.
    pub fn backface_cos(&self) -> f32 {
        if self.backface_culling {
            self.backface_angle.clamp(0.0, 180.0).to_radians().cos()
        } else {
            -1.0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jitter_is_deterministic_and_bounded() {
        let dab = crate::Dab {
            position: glam::Vec3::ZERO,
            normal: glam::Vec3::Z,
            up: glam::Vec3::Y,
            view: glam::Vec3::NEG_Z,
            radius: 1.0,
            flow: 1.0,
        };
        let jitter = Jitter {
            size: 0.5,
            flow: 0.5,
            angle: 1.0,
            position: 1.0,
        };
        let (mut a, mut b) = (dab, dab);
        let (mut ra, mut rb) = (Rng::new(7), Rng::new(7));
        jitter.apply(&mut a, || ra.next_f32());
        jitter.apply(&mut b, || rb.next_f32());
        assert_eq!(a, b);
        assert!((0.5..=1.0).contains(&a.radius) && (0.5..=1.0).contains(&a.flow));
        assert!(a.position.length() <= 2.0f32.sqrt() + 1e-4);
        assert!((a.up.length() - 1.0).abs() < 1e-4);
        let mut none = dab;
        Jitter::default().apply(&mut none, || 0.9);
        assert_eq!(none.radius, 1.0);
        assert!(Jitter::default().is_none());
    }

    #[test]
    fn rng_covers_the_unit_interval() {
        let mut rng = Rng::new(1);
        let values: Vec<f32> = (0..1000).map(|_| rng.next_f32()).collect();
        assert!(values.iter().all(|v| (0.0..1.0).contains(v)));
        let mean = values.iter().sum::<f32>() / 1000.0;
        assert!((mean - 0.5).abs() < 0.05);
    }

    #[test]
    fn channel_sets() {
        let mut set = ChannelSet::NONE;
        set.set(Channel::Roughness, true);
        set.set(Channel::Emissive, true);
        assert!(set.contains(Channel::Roughness) && !set.contains(Channel::BaseColor));
        assert_eq!(
            set.iter().collect::<Vec<_>>(),
            [Channel::Roughness, Channel::Emissive]
        );
        set.set(Channel::Roughness, false);
        assert_eq!(set.iter().count(), 1);
        assert_eq!(ChannelSet::ALL.iter().count(), Channel::ALL.len());
    }

    #[test]
    fn size_steps_stay_in_range() {
        let mut brush = Brush::default();
        brush.step_size(1.0);
        assert!(brush.size > 10.0);
        brush.step_size(-1.0);
        assert!((brush.size - 10.0).abs() < 1e-4);
        brush.step_size(1000.0);
        assert_eq!(brush.size, 100.0);
    }

    #[test]
    fn radius_follows_size_space() {
        let mut brush = Brush {
            size: 10.0,
            ..Default::default()
        };
        assert!((brush.world_radius(2.0, 50.0) - 0.1).abs() < 1e-6);
        assert!((brush.uv_radius(50.0) - 0.05).abs() < 1e-6);
        brush.size_space = SizeSpace::Viewport;
        assert!((brush.world_radius(2.0, 4.0) - 0.2).abs() < 1e-6);
        assert!((brush.uv_radius(0.5) - 0.025).abs() < 1e-6);
    }

    #[test]
    fn pressure_scales_down_to_the_minimum() {
        let mapping = PressureMapping {
            curve: PressureCurve::Linear,
            ..Default::default()
        };
        assert_eq!(mapping.factors(1.0), (1.0, 1.0));
        let (size, flow) = mapping.factors(0.0);
        assert!((size - 0.05).abs() < 1e-6);
        assert_eq!(flow, 1.0, "flow pressure is off by default");
        for curve in [
            PressureCurve::Linear,
            PressureCurve::EaseIn,
            PressureCurve::EaseInOut,
        ] {
            assert_eq!(curve.apply(0.0), 0.0);
            assert_eq!(curve.apply(1.0), 1.0);
            assert!(curve.apply(0.5) <= 0.5 + 1e-6);
        }
    }

    #[test]
    fn backface_culling_angle() {
        let mut brush = Brush::default();
        assert!(brush.backface_cos().abs() < 1e-6);
        brush.backface_culling = false;
        assert_eq!(brush.backface_cos(), -1.0);
    }
}
