//! Mesh maps: what baking computes from the model's surface and from high-poly meshes.
//! The generators of phase 9 read them to put wear into edges and dirt into crevices.

use crate::{Mesh, Model, TextureImage};
use glam::Vec3;
use std::path::PathBuf;
use std::sync::Arc;

/// A mesh map.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MeshMap {
    /// How much of the sky a point sees: 1 open, 0 enclosed.
    AmbientOcclusion,
    /// Convex edges bright, concave creases dark, flat 0.5.
    Curvature,
    /// The surface normal in world space, encoded like a normal map.
    WorldNormal,
    /// The position in the model's bounding box, 0 to 1 per axis.
    Position,
    /// How thick the object is under the surface: 0 thin, 1 as thick as the maximum distance.
    Thickness,
    /// The high-poly's normals in the low-poly's tangent space (OpenGL).
    Normal,
    /// The high-poly's height above the low-poly surface: 0.5 on the surface.
    Height,
    /// A color per material or object of the high-poly.
    Id,
}

impl MeshMap {
    pub const ALL: [Self; 8] = [
        Self::AmbientOcclusion,
        Self::Curvature,
        Self::WorldNormal,
        Self::Position,
        Self::Thickness,
        Self::Normal,
        Self::Height,
        Self::Id,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::AmbientOcclusion => "Ambient occlusion",
            Self::Curvature => "Curvature",
            Self::WorldNormal => "World space normal",
            Self::Position => "Position",
            Self::Thickness => "Thickness",
            Self::Normal => "Normal",
            Self::Height => "Height",
            Self::Id => "ID",
        }
    }

    /// A stable name for files; never changes.
    pub fn key(self) -> &'static str {
        match self {
            Self::AmbientOcclusion => "ambient_occlusion",
            Self::Curvature => "curvature",
            Self::WorldNormal => "world_normal",
            Self::Position => "position",
            Self::Thickness => "thickness",
            Self::Normal => "normal",
            Self::Height => "height",
            Self::Id => "id",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|m| m.key() == key)
    }

    /// Maps that need a high-poly mesh; the others come from the model itself.
    pub fn needs_high_poly(self) -> bool {
        matches!(self, Self::Normal | Self::Height | Self::Id)
    }

    /// Whether the map is a color or vector (3 channels) rather than a single value.
    pub fn is_vector(self) -> bool {
        matches!(
            self,
            Self::WorldNormal | Self::Position | Self::Normal | Self::Id
        )
    }

    /// The value of a texel nothing was baked into, e.g. outside the UV islands.
    pub fn background(self) -> [f32; 3] {
        match self {
            Self::AmbientOcclusion | Self::Thickness => [1.0; 3],
            Self::Curvature | Self::Height => [0.5; 3],
            Self::WorldNormal | Self::Normal => [0.5, 0.5, 1.0],
            Self::Position | Self::Id => [0.0; 3],
        }
    }
}

/// What the Id map colors.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum IdSource {
    /// One color per material of the high-poly.
    #[default]
    Material,
    /// One color per mesh object of the high-poly.
    Object,
}

impl IdSource {
    pub const ALL: [Self; 2] = [Self::Material, Self::Object];

    pub fn label(self) -> &'static str {
        match self {
            Self::Material => "Material",
            Self::Object => "Mesh object",
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Self::Material => "material",
            Self::Object => "object",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|s| s.key() == key)
    }
}

/// How low-poly and high-poly meshes find each other.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Matching {
    /// Every high-poly mesh counts for every low-poly one.
    #[default]
    Always,
    /// A low-poly object named `name_low` only sees high-poly objects named `name_high`
    /// (anything after the base name is ignored); objects without a suffix see everything.
    ByName,
}

impl Matching {
    pub const ALL: [Self; 2] = [Self::Always, Self::ByName];

    pub fn label(self) -> &'static str {
        match self {
            Self::Always => "Always",
            Self::ByName => "By mesh name",
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Self::Always => "always",
            Self::ByName => "by_name",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|m| m.key() == key)
    }
}

/// Settings of a bake. Distances are fractions of the model's bounding box diagonal, so they
/// mean the same on any model.
#[derive(Clone, Debug, PartialEq)]
pub struct BakeSettings {
    /// The maps to bake; ones that need a high-poly are skipped without one.
    pub maps: Vec<MeshMap>,
    /// Samples per texel and side: 1, 2 or 4.
    pub subsampling: u32,
    /// How far (texels) baked values reach past the edges of UV islands.
    pub dilation: u32,
    /// Where rays to the high-poly start, above the low-poly surface.
    pub front_distance: f32,
    /// How far below the surface they still find the high-poly.
    pub rear_distance: f32,
    /// Rays follow the smooth vertex normals; off, they follow the faces.
    pub average_normals: bool,
    /// High-poly triangles seen from behind are ignored.
    pub ignore_backfaces: bool,
    pub matching: Matching,
    pub id_source: IdSource,
    pub ao_rays: u32,
    pub ao_distance: f32,
    /// Whether the low-poly's own geometry occludes (off: only the high-poly does).
    pub ao_self_occlusion: bool,
    pub thickness_rays: u32,
    pub thickness_distance: f32,
    /// How wide the neighborhood is that curvature looks at.
    pub curvature_radius: f32,
    /// The high-poly files, kept so the bake can be repeated.
    pub high_poly: Vec<PathBuf>,
}

impl Default for BakeSettings {
    fn default() -> Self {
        Self {
            maps: vec![
                MeshMap::AmbientOcclusion,
                MeshMap::Curvature,
                MeshMap::WorldNormal,
                MeshMap::Position,
                MeshMap::Thickness,
                MeshMap::Normal,
                MeshMap::Height,
                MeshMap::Id,
            ],
            subsampling: 1,
            dilation: 16,
            front_distance: 0.01,
            rear_distance: 0.01,
            average_normals: true,
            ignore_backfaces: true,
            matching: Matching::default(),
            id_source: IdSource::default(),
            ao_rays: 64,
            ao_distance: 0.3,
            ao_self_occlusion: true,
            thickness_rays: 64,
            thickness_distance: 0.3,
            curvature_radius: 0.01,
            high_poly: Vec::new(),
        }
    }
}

impl BakeSettings {
    pub fn is_on(&self, map: MeshMap) -> bool {
        self.maps.contains(&map)
    }

    pub fn set(&mut self, map: MeshMap, on: bool) {
        self.maps.retain(|m| *m != map);
        if on {
            self.maps.push(map);
            self.maps
                .sort_by_key(|m| MeshMap::ALL.iter().position(|a| a == m));
        }
    }
}

/// The maps baked for one texture set.
#[derive(Clone, Debug, Default)]
pub struct MeshMaps {
    pub maps: Vec<(MeshMap, Arc<TextureImage>)>,
}

impl MeshMaps {
    pub fn get(&self, map: MeshMap) -> Option<&Arc<TextureImage>> {
        self.maps.iter().find(|(m, _)| *m == map).map(|(_, i)| i)
    }

    /// Puts the image in, replacing the map's earlier one.
    pub fn insert(&mut self, map: MeshMap, image: Arc<TextureImage>) {
        match self.maps.iter_mut().find(|(m, _)| *m == map) {
            Some(entry) => entry.1 = image,
            None => {
                self.maps.push((map, image));
                self.maps
                    .sort_by_key(|(m, _)| MeshMap::ALL.iter().position(|a| a == m));
            }
        }
    }

    pub fn is_empty(&self) -> bool {
        self.maps.is_empty()
    }
}

/// High-poly geometry to bake from: triangles in world space with the group each belongs
/// to, so rays can be matched to meshes and the Id map can color them.
#[derive(Clone, Debug, Default)]
pub struct HighPoly {
    pub mesh: Mesh,
    /// Per triangle: index into `object_names`.
    pub triangle_object: Vec<u32>,
    /// Per triangle: index into `material_names`.
    pub triangle_material: Vec<u32>,
    pub object_names: Vec<String>,
    pub material_names: Vec<String>,
}

impl HighPoly {
    /// The geometry of an imported model.
    pub fn from_model(model: &Model) -> Self {
        let mut triangle_object = vec![0; model.mesh.triangle_count()];
        let mut triangle_material = vec![0; model.mesh.triangle_count()];
        for part in &model.parts {
            for t in (part.indices.start / 3)..(part.indices.end / 3) {
                triangle_object[t as usize] = part.object as u32;
                triangle_material[t as usize] = part.texture_set as u32;
            }
        }
        Self {
            mesh: model.mesh.clone(),
            triangle_object,
            triangle_material,
            object_names: model.objects.iter().map(|o| o.name.clone()).collect(),
            material_names: model.texture_sets.iter().map(|s| s.name.clone()).collect(),
        }
    }

    /// Several high-poly meshes as one.
    pub fn merge(parts: impl IntoIterator<Item = Self>) -> Self {
        let mut merged = Self::default();
        for part in parts {
            let vertex_base = merged.mesh.positions.len() as u32;
            let (object_base, material_base) = (
                merged.object_names.len() as u32,
                merged.material_names.len() as u32,
            );
            merged.mesh.positions.extend(&part.mesh.positions);
            merged.mesh.normals.extend(&part.mesh.normals);
            merged.mesh.uvs.extend(&part.mesh.uvs);
            merged
                .mesh
                .indices
                .extend(part.mesh.indices.iter().map(|i| i + vertex_base));
            merged
                .triangle_object
                .extend(part.triangle_object.iter().map(|o| o + object_base));
            merged
                .triangle_material
                .extend(part.triangle_material.iter().map(|m| m + material_base));
            merged.object_names.extend(part.object_names);
            merged.material_names.extend(part.material_names);
        }
        merged
    }

    pub fn is_empty(&self) -> bool {
        self.mesh.indices.is_empty()
    }
}

/// The part of an object name before `_low` or `_high` (any case, with an optional number
/// or text after); None if the name has no such suffix.
pub fn match_base(name: &str) -> Option<String> {
    let lower = name.to_lowercase();
    for marker in ["_low", "_high"] {
        if let Some(at) = lower.rfind(marker) {
            return Some(lower[..at].to_owned());
        }
    }
    None
}

/// A color for an id, the same for the same number: golden-ratio hues.
pub fn id_color(id: u32) -> Vec3 {
    let hue = (id as f32 * 0.618_034).fract();
    let (s, v) = (0.65, 0.95);
    let h = hue * 6.0;
    let channel = |n: f32| {
        let k = (n + h) % 6.0;
        v - v * s * k.min(4.0 - k).clamp(0.0, 1.0)
    };
    Vec3::new(channel(5.0), channel(3.0), channel(1.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_pair_by_their_base() {
        assert_eq!(match_base("Crate_low"), Some("crate".to_owned()));
        assert_eq!(match_base("crate_HIGH_01"), Some("crate".to_owned()));
        assert_eq!(match_base("Crate"), None);
    }

    #[test]
    fn colors_differ_per_id() {
        let colors: Vec<Vec3> = (0..8).map(id_color).collect();
        for (i, a) in colors.iter().enumerate() {
            assert!(a.min_element() >= 0.0 && a.max_element() <= 1.0);
            for b in &colors[i + 1..] {
                assert!(a.distance(*b) > 0.1);
            }
        }
        assert_eq!(id_color(3), id_color(3));
    }

    #[test]
    fn settings_toggle_maps_in_order() {
        let mut settings = BakeSettings::default();
        settings.set(MeshMap::Curvature, false);
        assert!(!settings.is_on(MeshMap::Curvature));
        settings.set(MeshMap::Curvature, true);
        let order: Vec<_> = settings
            .maps
            .iter()
            .map(|m| MeshMap::ALL.iter().position(|a| a == m))
            .collect();
        assert!(order.windows(2).all(|w| w[0] < w[1]));
    }

    #[test]
    fn keys_round_trip() {
        for map in MeshMap::ALL {
            assert_eq!(MeshMap::from_key(map.key()), Some(map));
        }
        for m in Matching::ALL {
            assert_eq!(Matching::from_key(m.key()), Some(m));
        }
        for s in IdSource::ALL {
            assert_eq!(IdSource::from_key(s.key()), Some(s));
        }
    }
}
