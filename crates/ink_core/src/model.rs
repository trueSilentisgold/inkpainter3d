use crate::mesh::{Aabb, Mesh};
use glam::Vec3;
use std::collections::HashMap;
use std::ops::Range;

/// Name of the texture set for triangles without a material, as in Painter.
pub const DEFAULT_MATERIAL: &str = "DefaultMaterial";

/// An object from the model file, e.g. "Torso". Its triangles are spread over one
/// [`Part`] per material it uses.
#[derive(Clone, Debug)]
pub struct MeshObject {
    pub name: String,
    pub visible: bool,
}

/// How a texture set's opacity channel is shown in the viewport.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AlphaMode {
    #[default]
    Opaque,
    /// Pixels below 50 % opacity are cut out, e.g. hair or fur cards.
    Test,
    /// Pixels are blended by their opacity, e.g. glass.
    Blend,
}

/// Uniform channel values. Colors are sRGB.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ChannelValues {
    pub base_color: [f32; 3],
    pub opacity: f32,
    pub roughness: f32,
    pub metallic: f32,
    pub emissive: [f32; 3],
}

impl Default for ChannelValues {
    fn default() -> Self {
        Self {
            base_color: [0.8, 0.8, 0.8],
            opacity: 1.0,
            roughness: 0.5,
            metallic: 0.0,
            emissive: [0.0; 3],
        }
    }
}

/// One material slot of the model. Every texture set has its own textures.
#[derive(Clone, Debug)]
pub struct TextureSet {
    /// Material name from the model file; used for export file names.
    pub name: String,
    pub visible: bool,
    /// Texture size in pixels (square).
    pub resolution: u32,
    pub alpha_mode: AlphaMode,
    /// Content of the channels until the layer stack exists; becomes the bottom fill layer.
    pub fill: ChannelValues,
}

impl TextureSet {
    pub const DEFAULT_RESOLUTION: u32 = 2048;

    fn new(name: String) -> Self {
        Self {
            name,
            visible: true,
            resolution: Self::DEFAULT_RESOLUTION,
            alpha_mode: AlphaMode::default(),
            fill: ChannelValues::default(),
        }
    }
}

/// A contiguous range of [`Model::mesh`] indices that belongs to one object and one texture set.
#[derive(Clone, Debug, PartialEq)]
pub struct Part {
    pub object: usize,
    pub texture_set: usize,
    pub indices: Range<u32>,
    pub bounds: Aabb,
}

impl Part {
    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }
}

/// An imported model: one triangle mesh split into objects and texture sets.
#[derive(Clone, Debug)]
pub struct Model {
    pub name: String,
    pub mesh: Mesh,
    pub objects: Vec<MeshObject>,
    /// Sorted by name, like Painter's texture set list.
    pub texture_sets: Vec<TextureSet>,
    /// Grouped by texture set, then object.
    pub parts: Vec<Part>,
}

/// Result of [`Model::pick`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hit {
    pub part: usize,
    /// Index of the triangle in the mesh.
    pub triangle: usize,
    /// Distance along the ray.
    pub distance: f32,
    pub position: Vec3,
}

impl Model {
    /// A model with a single object and texture set, e.g. for the primitives.
    pub fn from_mesh(mesh: Mesh) -> Self {
        let mut builder = ModelBuilder::default();
        let object = builder.object(&mesh.name);
        let set = builder.texture_set(DEFAULT_MATERIAL);
        let name = mesh.name.clone();
        let base = builder.mesh.positions.len() as u32;
        builder.mesh.positions.extend(&mesh.positions);
        builder.mesh.normals.extend(&mesh.normals);
        builder.mesh.uvs.extend(&mesh.uvs);
        for tri in mesh.indices.as_chunks::<3>().0 {
            builder.triangle(object, set, tri.map(|i| i + base));
        }
        builder.build(name)
    }

    pub fn bounds(&self) -> Aabb {
        self.mesh.bounds()
    }

    pub fn is_part_visible(&self, part: &Part) -> bool {
        self.texture_sets[part.texture_set].visible && self.objects[part.object].visible
    }

    /// Objects with triangles in the texture set, in object order.
    pub fn objects_in_set(&self, set: usize) -> Vec<usize> {
        let mut objects: Vec<usize> = self
            .parts
            .iter()
            .filter(|p| p.texture_set == set)
            .map(|p| p.object)
            .collect();
        objects.sort_unstable();
        objects.dedup();
        objects
    }

    /// Texture sets used by the object.
    pub fn sets_of_object(&self, object: usize) -> Vec<usize> {
        let mut sets: Vec<usize> = self
            .parts
            .iter()
            .filter(|p| p.object == object)
            .map(|p| p.texture_set)
            .collect();
        sets.sort_unstable();
        sets.dedup();
        sets
    }

    /// Number of triangles of `object` that use texture set `set`.
    pub fn triangle_count(&self, set: usize, object: usize) -> usize {
        self.parts
            .iter()
            .filter(|p| p.texture_set == set && p.object == object)
            .map(Part::triangle_count)
            .sum()
    }

    /// Finds the closest triangle hit by the ray, considering only visible parts.
    /// Triangles are hit from both sides.
    pub fn pick(&self, origin: Vec3, dir: Vec3) -> Option<Hit> {
        let inv_dir = dir.recip();
        let mut best: Option<Hit> = None;
        for (index, part) in self.parts.iter().enumerate() {
            if !self.is_part_visible(part) {
                continue;
            }
            let max = best.map_or(f32::INFINITY, |h| h.distance);
            if !ray_hits_box(origin, inv_dir, &part.bounds, max) {
                continue;
            }
            let indices =
                &self.mesh.indices[part.indices.start as usize..part.indices.end as usize];
            for (i, tri) in indices.as_chunks::<3>().0.iter().enumerate() {
                let [a, b, c] = tri.map(|v| self.mesh.positions[v as usize]);
                if let Some(t) = ray_triangle(origin, dir, a, b, c)
                    && best.is_none_or(|h| t < h.distance)
                {
                    best = Some(Hit {
                        part: index,
                        triangle: part.indices.start as usize / 3 + i,
                        distance: t,
                        position: origin + dir * t,
                    });
                }
            }
        }
        best
    }
}

/// Collects vertices and triangles per object and material during import.
#[derive(Default)]
pub struct ModelBuilder {
    /// Vertices go here directly; indices are added with [`Self::triangle`].
    pub mesh: Mesh,
    objects: Vec<MeshObject>,
    object_lookup: HashMap<String, usize>,
    texture_sets: Vec<TextureSet>,
    set_lookup: HashMap<String, usize>,
    /// Triangles per (object, texture set), in order of first use.
    buckets: Vec<((usize, usize), Vec<u32>)>,
    bucket_lookup: HashMap<(usize, usize), usize>,
}

impl ModelBuilder {
    /// Adds a new object. Objects may share a name.
    pub fn add_object(&mut self, name: &str) -> usize {
        self.objects.push(MeshObject {
            name: name.to_owned(),
            visible: true,
        });
        self.objects.len() - 1
    }

    /// Returns the object with this name, adding it if needed.
    pub fn object(&mut self, name: &str) -> usize {
        if let Some(&i) = self.object_lookup.get(name) {
            return i;
        }
        let i = self.add_object(name);
        self.object_lookup.insert(name.to_owned(), i);
        i
    }

    /// Returns the texture set for this material name, adding it if needed.
    pub fn texture_set(&mut self, material: &str) -> usize {
        if let Some(&i) = self.set_lookup.get(material) {
            return i;
        }
        self.texture_sets.push(TextureSet::new(material.to_owned()));
        let i = self.texture_sets.len() - 1;
        self.set_lookup.insert(material.to_owned(), i);
        i
    }

    pub fn triangle(&mut self, object: usize, texture_set: usize, tri: [u32; 3]) {
        let key = (object, texture_set);
        let bucket = *self.bucket_lookup.entry(key).or_insert_with(|| {
            self.buckets.push((key, Vec::new()));
            self.buckets.len() - 1
        });
        self.buckets[bucket].1.extend_from_slice(&tri);
    }

    pub fn has_triangles(&self) -> bool {
        self.buckets.iter().any(|(_, b)| !b.is_empty())
    }

    /// Sorts the texture sets by name, drops objects and sets without triangles and
    /// fills in missing vertex attributes.
    pub fn build(self, name: String) -> Model {
        let Self {
            mut mesh,
            objects,
            texture_sets,
            buckets,
            ..
        } = self;

        // Keep only used sets and objects; sets sorted by name, objects in file order.
        let mut used_sets: Vec<usize> = buckets.iter().map(|((_, s), _)| *s).collect();
        used_sets.sort_unstable();
        used_sets.dedup();
        used_sets.sort_by_cached_key(|&s| texture_sets[s].name.to_lowercase());
        let mut set_map = vec![usize::MAX; texture_sets.len()];
        for (new, &old) in used_sets.iter().enumerate() {
            set_map[old] = new;
        }
        let mut used_objects: Vec<usize> = buckets.iter().map(|((o, _), _)| *o).collect();
        used_objects.sort_unstable();
        used_objects.dedup();
        let mut object_map = vec![usize::MAX; objects.len()];
        for (new, &old) in used_objects.iter().enumerate() {
            object_map[old] = new;
        }

        let mut buckets: Vec<((usize, usize), Vec<u32>)> = buckets
            .into_iter()
            .map(|((o, s), tris)| ((object_map[o], set_map[s]), tris))
            .collect();
        buckets.sort_by_key(|((o, s), _)| (*s, *o));

        mesh.indices.clear();
        let mut ranges = Vec::with_capacity(buckets.len());
        for ((object, texture_set), tris) in buckets {
            let start = mesh.indices.len() as u32;
            mesh.indices.extend(tris);
            ranges.push((object, texture_set, start..mesh.indices.len() as u32));
        }
        // May split vertices for tangents, but never changes the index ranges.
        mesh.ensure_attributes();
        mesh.name = name.clone();

        let parts = ranges
            .into_iter()
            .map(|(object, texture_set, indices)| {
                let mut min = Vec3::splat(f32::MAX);
                let mut max = Vec3::splat(f32::MIN);
                for &i in &mesh.indices[indices.start as usize..indices.end as usize] {
                    let p = mesh.positions[i as usize];
                    min = min.min(p);
                    max = max.max(p);
                }
                Part {
                    object,
                    texture_set,
                    indices,
                    bounds: Aabb { min, max },
                }
            })
            .collect();

        Model {
            name,
            mesh,
            objects: used_objects
                .into_iter()
                .map(|o| objects[o].clone())
                .collect(),
            texture_sets: used_sets
                .into_iter()
                .map(|s| texture_sets[s].clone())
                .collect(),
            parts,
        }
    }
}

fn ray_hits_box(origin: Vec3, inv_dir: Vec3, b: &Aabb, max: f32) -> bool {
    let t0 = (b.min - origin) * inv_dir;
    let t1 = (b.max - origin) * inv_dir;
    let near = t0.min(t1).max_element();
    let far = t0.max(t1).min_element();
    near <= far && far >= 0.0 && near <= max
}

/// Möller–Trumbore, two-sided. Returns the distance along the ray.
fn ray_triangle(origin: Vec3, dir: Vec3, a: Vec3, b: Vec3, c: Vec3) -> Option<f32> {
    let e1 = b - a;
    let e2 = c - a;
    let p = dir.cross(e2);
    let det = e1.dot(p);
    if det.abs() < 1e-12 {
        return None;
    }
    let inv = 1.0 / det;
    let s = origin - a;
    let u = s.dot(p) * inv;
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = s.cross(e1);
    let v = dir.dot(q) * inv;
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let t = e2.dot(q) * inv;
    (t > 0.0).then_some(t)
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec2;

    /// Two quads: "Left" uses materials B and A, "Right" uses A.
    fn two_objects() -> Model {
        let mut b = ModelBuilder::default();
        let quad = |b: &mut ModelBuilder, x: f32| {
            let base = b.mesh.positions.len() as u32;
            for (dx, dy) in [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)] {
                b.mesh.positions.push(Vec3::new(x + dx, dy, 0.0));
                b.mesh.normals.push(Vec3::Z);
                b.mesh.uvs.push(Vec2::new(dx, 1.0 - dy));
            }
            base
        };
        let left = b.add_object("Left");
        let mat_b = b.texture_set("mat_b");
        let mat_a = b.texture_set("mat_a");
        let q = quad(&mut b, 0.0);
        b.triangle(left, mat_b, [q, q + 1, q + 2]);
        b.triangle(left, mat_a, [q, q + 2, q + 3]);
        let right = b.add_object("Right");
        let q = quad(&mut b, 2.0);
        b.triangle(right, mat_a, [q, q + 1, q + 2]);
        b.triangle(right, mat_a, [q, q + 2, q + 3]);
        b.add_object("Empty");
        b.build("Test".into())
    }

    #[test]
    fn keeps_objects_and_materials() {
        let model = two_objects();
        let names: Vec<&str> = model.texture_sets.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["mat_a", "mat_b"], "sets sorted by name");
        let objects: Vec<&str> = model.objects.iter().map(|o| o.name.as_str()).collect();
        assert_eq!(objects, ["Left", "Right"], "empty objects dropped");

        assert_eq!(model.objects_in_set(0), [0, 1]);
        assert_eq!(model.objects_in_set(1), [0]);
        assert_eq!(model.sets_of_object(0), [0, 1]);
        assert_eq!(model.triangle_count(0, 1), 2);
        assert_eq!(model.triangle_count(1, 0), 1);

        // Parts are contiguous and grouped by texture set.
        let mut end = 0;
        let mut last_set = 0;
        for part in &model.parts {
            assert_eq!(part.indices.start, end);
            assert!(part.texture_set >= last_set);
            end = part.indices.end;
            last_set = part.texture_set;
        }
        assert_eq!(end as usize, model.mesh.indices.len());
        assert_eq!(model.mesh.tangents.len(), model.mesh.positions.len());
    }

    #[test]
    fn pick_respects_visibility() {
        let mut model = two_objects();
        let down = Vec3::NEG_Z;
        // The upper-left triangle of "Left" uses mat_a.
        let hit = model.pick(Vec3::new(0.2, 0.8, 5.0), down).unwrap();
        let part = &model.parts[hit.part];
        assert_eq!(model.texture_sets[part.texture_set].name, "mat_a");
        assert_eq!(model.objects[part.object].name, "Left");
        assert!((hit.distance - 5.0).abs() < 1e-5);
        // From behind as well.
        assert!(model.pick(Vec3::new(0.2, 0.8, -5.0), Vec3::Z).is_some());

        model.texture_sets[0].visible = false;
        assert!(model.pick(Vec3::new(0.2, 0.8, 5.0), down).is_none());
        assert!(
            model.pick(Vec3::new(0.8, 0.2, 5.0), down).is_some(),
            "mat_b still visible"
        );
        model.objects[0].visible = false;
        assert!(model.pick(Vec3::new(0.8, 0.2, 5.0), down).is_none());
        assert!(
            model.pick(Vec3::new(2.5, 0.5, 5.0), down).is_none(),
            "Right uses mat_a"
        );
    }

    #[test]
    fn from_mesh_uses_default_material() {
        let model = Model::from_mesh(Mesh::cube(1.0));
        assert_eq!(model.texture_sets.len(), 1);
        assert_eq!(model.texture_sets[0].name, DEFAULT_MATERIAL);
        assert_eq!(model.objects[0].name, "Cube");
        assert_eq!(model.parts.len(), 1);
        assert_eq!(model.parts[0].triangle_count(), 12);
    }
}
