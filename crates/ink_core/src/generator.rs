//! Generators: procedural masks computed per texel from the mesh maps (curvature, ambient
//! occlusion, position, thickness, world normal) and noise. Each kind has a description of
//! its parameters, so the interface, the project files and the shader all work from the same
//! list.

use crate::MeshMap;

/// The number of parameter slots of a generator; the shader reads them in this order.
pub const MAX_PARAMS: usize = 16;

/// What a parameter looks like and means.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ParamKind {
    /// A number in `min..=max`.
    Float,
    /// A whole number.
    Int,
    /// One of the named choices; the value is the index.
    Choice(&'static [&'static str]),
}

/// One parameter of a generator.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Param {
    /// A stable name for files; never changes.
    pub key: &'static str,
    pub label: &'static str,
    pub min: f32,
    pub max: f32,
    pub default: f32,
    pub kind: ParamKind,
    /// Whether a slider should be logarithmic.
    pub log: bool,
}

const fn float(key: &'static str, label: &'static str, min: f32, max: f32, default: f32) -> Param {
    Param {
        key,
        label,
        min,
        max,
        default,
        kind: ParamKind::Float,
        log: false,
    }
}

const fn log_float(
    key: &'static str,
    label: &'static str,
    min: f32,
    max: f32,
    default: f32,
) -> Param {
    Param {
        log: true,
        ..float(key, label, min, max, default)
    }
}

const fn int(key: &'static str, label: &'static str, min: f32, max: f32, default: f32) -> Param {
    Param {
        kind: ParamKind::Int,
        ..float(key, label, min, max, default)
    }
}

const fn choice(
    key: &'static str,
    label: &'static str,
    names: &'static [&'static str],
    default: f32,
) -> Param {
    Param {
        key,
        label,
        min: 0.0,
        max: (names.len() - 1) as f32,
        default,
        kind: ParamKind::Choice(names),
        log: false,
    }
}

const OFF_ON: &[&str] = &["Off", "On"];

const NOISE_TYPES: &[&str] = &["Perlin", "Fractal", "Cells", "Clouds"];
const AXES: &[&str] = &["X", "Y", "Z"];
const CURVATURE_PARTS: &[&str] = &["Convex", "Concave", "Both"];

const NOISE_PARAMS: &[Param] = &[
    choice("type", "Type", NOISE_TYPES, 1.0),
    log_float("scale", "Scale", 1.0, 256.0, 8.0),
    int("detail", "Detail", 1.0, 8.0, 4.0),
    float("roughness", "Roughness", 0.0, 1.0, 0.5),
    int("seed", "Seed", 0.0, 1000.0, 0.0),
    float("contrast", "Contrast", 0.0, 8.0, 1.0),
    float("balance", "Balance", -1.0, 1.0, 0.0),
    choice("invert", "Invert", OFF_ON, 0.0),
];

const MASKBUILDER_PARAMS: &[Param] = &[
    float("curvature", "Curvature", -1.0, 1.0, 0.7),
    float("occlusion", "Ambient occlusion", -1.0, 1.0, 0.0),
    float("thickness", "Thickness", -1.0, 1.0, 0.0),
    float("height", "Height", -1.0, 1.0, 0.0),
    float("noise", "Noise", 0.0, 1.0, 0.4),
    log_float("noise_scale", "Noise scale", 1.0, 256.0, 16.0),
    int("noise_detail", "Noise detail", 1.0, 8.0, 4.0),
    int("seed", "Seed", 0.0, 1000.0, 0.0),
    float("contrast", "Contrast", 0.0, 8.0, 2.0),
    float("balance", "Balance", -1.0, 1.0, 0.0),
    choice("invert", "Invert", OFF_ON, 0.0),
];

const EDGEWEAR_PARAMS: &[Param] = &[
    float("amount", "Amount", 0.0, 1.0, 0.5),
    float("width", "Edge softness", 0.0, 1.0, 0.3),
    float("noise", "Noise", 0.0, 1.0, 0.5),
    log_float("noise_scale", "Noise scale", 1.0, 256.0, 24.0),
    int("noise_detail", "Noise detail", 1.0, 8.0, 5.0),
    int("seed", "Seed", 0.0, 1000.0, 0.0),
    choice("invert", "Invert", OFF_ON, 0.0),
];

const DIRT_PARAMS: &[Param] = &[
    float("amount", "Amount", 0.0, 1.0, 0.5),
    float("occlusion", "Ambient occlusion", 0.0, 1.0, 0.7),
    float("concavity", "Concavity", 0.0, 1.0, 0.5),
    float("dust", "Dust on top", 0.0, 1.0, 0.2),
    float("noise", "Noise", 0.0, 1.0, 0.5),
    log_float("noise_scale", "Noise scale", 1.0, 256.0, 10.0),
    int("noise_detail", "Noise detail", 1.0, 8.0, 4.0),
    int("seed", "Seed", 0.0, 1000.0, 0.0),
    float("contrast", "Contrast", 0.0, 8.0, 1.5),
    choice("invert", "Invert", OFF_ON, 0.0),
];

const POSITIONGRADIENT_PARAMS: &[Param] = &[
    choice("axis", "Axis", AXES, 1.0),
    float("position", "Position", 0.0, 1.0, 0.5),
    float("softness", "Softness", 0.0, 1.0, 0.4),
    float("noise", "Noise", 0.0, 1.0, 0.0),
    log_float("noise_scale", "Noise scale", 1.0, 256.0, 12.0),
    int("seed", "Seed", 0.0, 1000.0, 0.0),
    choice("invert", "Invert", OFF_ON, 0.0),
];

const LIGHT_PARAMS: &[Param] = &[
    float("azimuth", "Azimuth", 0.0, 360.0, 45.0),
    float("elevation", "Elevation", 0.0, 90.0, 45.0),
    float("contrast", "Contrast", 0.0, 8.0, 1.0),
    float("balance", "Balance", -1.0, 1.0, 0.0),
    choice("invert", "Invert", OFF_ON, 0.0),
];

const AMBIENTOCCLUSION_PARAMS: &[Param] = &[
    float("contrast", "Contrast", 0.0, 8.0, 1.0),
    float("balance", "Balance", -1.0, 1.0, 0.0),
    choice("invert", "Invert", OFF_ON, 0.0),
];

const CURVATURE_PARAMS: &[Param] = &[
    float("contrast", "Contrast", 0.0, 8.0, 2.0),
    float("balance", "Balance", -1.0, 1.0, 0.0),
    choice("invert", "Invert", OFF_ON, 0.0),
    choice("part", "Part", CURVATURE_PARTS, 0.0),
];

/// The kinds of generators.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum GeneratorKind {
    /// Noise in object space, or in UV space without a Position map.
    Noise,
    /// Mixes curvature, occlusion, thickness, height and noise into one mask.
    MaskBuilder,
    /// Worn edges: convex curvature broken up by noise.
    EdgeWear,
    /// Dirt in crevices and dust on upward faces.
    Dirt,
    /// A gradient along an axis of the model's bounds.
    PositionGradient,
    /// A directional light on the model's normals.
    Light,
    AmbientOcclusion,
    Curvature,
    Thickness,
}

impl GeneratorKind {
    pub const ALL: [Self; 9] = [
        Self::MaskBuilder,
        Self::EdgeWear,
        Self::Dirt,
        Self::PositionGradient,
        Self::Light,
        Self::Noise,
        Self::AmbientOcclusion,
        Self::Curvature,
        Self::Thickness,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Noise => "Noise",
            Self::MaskBuilder => "Mask builder",
            Self::EdgeWear => "Edge wear",
            Self::Dirt => "Dirt",
            Self::PositionGradient => "Position gradient",
            Self::Light => "Light",
            Self::AmbientOcclusion => "Ambient occlusion",
            Self::Curvature => "Curvature",
            Self::Thickness => "Thickness",
        }
    }

    /// A stable name for files; never changes.
    pub fn key(self) -> &'static str {
        match self {
            Self::Noise => "noise",
            Self::MaskBuilder => "mask_builder",
            Self::EdgeWear => "edge_wear",
            Self::Dirt => "dirt",
            Self::PositionGradient => "position_gradient",
            Self::Light => "light",
            Self::AmbientOcclusion => "ambient_occlusion",
            Self::Curvature => "curvature",
            Self::Thickness => "thickness",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.key() == key)
    }

    /// The number the shader switches on (`GEN_*` in generators.wgsl).
    pub fn code(self) -> u32 {
        match self {
            Self::Noise => 0,
            Self::MaskBuilder => 1,
            Self::EdgeWear => 2,
            Self::Dirt => 3,
            Self::PositionGradient => 4,
            Self::Light => 5,
            Self::AmbientOcclusion => 6,
            Self::Curvature => 7,
            Self::Thickness => 8,
        }
    }

    /// The mesh maps that make the generator meaningful.
    pub fn mesh_maps(self) -> &'static [MeshMap] {
        match self {
            Self::Noise => &[MeshMap::Position],
            Self::MaskBuilder => &[
                MeshMap::Curvature,
                MeshMap::AmbientOcclusion,
                MeshMap::Thickness,
                MeshMap::Position,
            ],
            Self::EdgeWear => &[MeshMap::Curvature, MeshMap::Position],
            Self::Dirt => &[
                MeshMap::AmbientOcclusion,
                MeshMap::Curvature,
                MeshMap::WorldNormal,
                MeshMap::Position,
            ],
            Self::PositionGradient => &[MeshMap::Position],
            Self::Light => &[MeshMap::WorldNormal],
            Self::AmbientOcclusion => &[MeshMap::AmbientOcclusion],
            Self::Curvature => &[MeshMap::Curvature],
            Self::Thickness => &[MeshMap::Thickness],
        }
    }

    /// The parameters, in the order of the shader's slots.
    pub fn params(self) -> &'static [Param] {
        match self {
            Self::Noise => NOISE_PARAMS,
            Self::MaskBuilder => MASKBUILDER_PARAMS,
            Self::EdgeWear => EDGEWEAR_PARAMS,
            Self::Dirt => DIRT_PARAMS,
            Self::PositionGradient => POSITIONGRADIENT_PARAMS,
            Self::Light => LIGHT_PARAMS,
            Self::AmbientOcclusion | Self::Thickness => AMBIENTOCCLUSION_PARAMS,
            Self::Curvature => CURVATURE_PARAMS,
        }
    }
}

/// A generator with its settings.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Generator {
    pub kind: GeneratorKind,
    /// Values of `kind.params()`, in order; the rest is unused.
    pub values: [f32; MAX_PARAMS],
}

impl Generator {
    /// A generator with the defaults of its kind.
    pub fn new(kind: GeneratorKind) -> Self {
        let mut values = [0.0; MAX_PARAMS];
        for (v, p) in values.iter_mut().zip(kind.params()) {
            *v = p.default;
        }
        Self { kind, values }
    }

    /// The value of the parameter with this key (its default if the kind has none).
    pub fn get(&self, key: &str) -> f32 {
        self.kind
            .params()
            .iter()
            .position(|p| p.key == key)
            .map_or(0.0, |i| self.values[i])
    }

    /// Sets a parameter by key, clamped to its range; unknown keys are ignored.
    pub fn set(&mut self, key: &str, value: f32) {
        if let Some(i) = self.kind.params().iter().position(|p| p.key == key) {
            let p = &self.kind.params()[i];
            let value = if value.is_finite() { value } else { p.default };
            self.values[i] = value.clamp(p.min, p.max);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_kind_fits_the_shader_slots_and_has_unique_keys() {
        for kind in GeneratorKind::ALL {
            let params = kind.params();
            assert!(params.len() <= MAX_PARAMS, "{kind:?}");
            let mut keys: Vec<_> = params.iter().map(|p| p.key).collect();
            keys.sort_unstable();
            keys.dedup();
            assert_eq!(keys.len(), params.len(), "{kind:?} repeats a key");
            for p in params {
                assert!(
                    p.min <= p.default && p.default <= p.max,
                    "{kind:?} {}",
                    p.key
                );
            }
            assert_eq!(GeneratorKind::from_key(kind.key()), Some(kind));
        }
        let mut codes: Vec<_> = GeneratorKind::ALL.iter().map(|k| k.code()).collect();
        codes.sort_unstable();
        assert_eq!(
            codes,
            (0..GeneratorKind::ALL.len() as u32).collect::<Vec<_>>()
        );
    }

    #[test]
    fn settings_are_clamped_and_found_by_key() {
        let mut g = Generator::new(GeneratorKind::Dirt);
        assert_eq!(g.get("amount"), 0.5);
        g.set("amount", 7.0);
        assert_eq!(g.get("amount"), 1.0);
        g.set("amount", f32::NAN);
        assert_eq!(g.get("amount"), 0.5);
        g.set("missing", 1.0);
        assert_eq!(g.get("missing"), 0.0);
    }
}
