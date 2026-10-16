use crate::brush::Channel;
use crate::bvh::Bvh;
use crate::layer::LayerStack;
use crate::mesh::{Aabb, Mesh};
use glam::{Vec2, Vec3};
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

/// How normal maps encode the tangent space's y axis: OpenGL (y up, as Blender, Unity and
/// Godot expect) or DirectX (y down). The project's setting, as in Painter: it decides the
/// normal map of the document channels export (and later of baked normal maps). The layer
/// stack is independent of it: normal maps in fills say their own format, and inside the
/// program normals are always OpenGL-style.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum NormalFormat {
    OpenGl,
    /// Painter's default for new projects.
    #[default]
    DirectX,
}

impl NormalFormat {
    pub fn label(self) -> &'static str {
        match self {
            Self::OpenGl => "OpenGL",
            Self::DirectX => "DirectX",
        }
    }
}

/// Uniform channel values. Colors are sRGB; the normal is encoded like a normal map texel.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ChannelValues {
    pub base_color: [f32; 3],
    pub height: f32,
    pub roughness: f32,
    pub metallic: f32,
    pub normal: [f32; 3],
    pub emissive: [f32; 3],
    pub opacity: f32,
}

/// The flat normal, encoded.
pub const FLAT_NORMAL: [f32; 3] = [0.5, 0.5, 1.0];

impl Default for ChannelValues {
    fn default() -> Self {
        Self {
            base_color: [0.8, 0.8, 0.8],
            height: 0.0,
            roughness: 0.5,
            metallic: 0.0,
            normal: FLAT_NORMAL,
            emissive: [0.0; 3],
            opacity: 1.0,
        }
    }
}

impl ChannelValues {
    /// A channel's value; scalar channels are repeated in all three components.
    pub fn get(&self, channel: Channel) -> [f32; 3] {
        match channel {
            Channel::BaseColor => self.base_color,
            Channel::Normal => self.normal,
            Channel::Emissive => self.emissive,
            _ => [self.scalar(channel); 3],
        }
    }

    fn scalar(&self, channel: Channel) -> f32 {
        match channel {
            Channel::Height => self.height,
            Channel::Roughness => self.roughness,
            Channel::Metallic => self.metallic,
            Channel::Opacity => self.opacity,
            _ => self.get(channel)[0],
        }
    }

    /// The value of a scalar channel to edit; None for colors and normals.
    pub fn scalar_mut(&mut self, channel: Channel) -> Option<&mut f32> {
        match channel {
            Channel::Height => Some(&mut self.height),
            Channel::Roughness => Some(&mut self.roughness),
            Channel::Metallic => Some(&mut self.metallic),
            Channel::Opacity => Some(&mut self.opacity),
            _ => None,
        }
    }

    /// The value of a color or normal channel to edit; None for scalars.
    pub fn vector_mut(&mut self, channel: Channel) -> Option<&mut [f32; 3]> {
        match channel {
            Channel::BaseColor => Some(&mut self.base_color),
            Channel::Normal => Some(&mut self.normal),
            Channel::Emissive => Some(&mut self.emissive),
            _ => None,
        }
    }
}

/// One material slot of the model. Every texture set has its own layer stack and textures.
#[derive(Clone, Debug)]
pub struct TextureSet {
    /// Material name from the model file; used for export file names.
    pub name: String,
    pub visible: bool,
    /// Texture size in pixels (square).
    pub resolution: u32,
    pub alpha_mode: AlphaMode,
    pub stack: LayerStack,
    /// What baking computed for this texture set.
    pub mesh_maps: crate::MeshMaps,
}

impl TextureSet {
    pub const DEFAULT_RESOLUTION: u32 = 2048;

    fn new(name: String, stack: LayerStack) -> Self {
        Self {
            name,
            visible: true,
            resolution: Self::DEFAULT_RESOLUTION,
            alpha_mode: AlphaMode::default(),
            stack,
            mesh_maps: Default::default(),
        }
    }
}

/// Hands out ids that are unique within a model: for strokes, layers, paint layers and
/// effects.
#[derive(Clone, Debug, Default)]
pub struct Ids {
    last: u64,
}

impl Ids {
    /// Continues after `last`, e.g. for a model loaded from a project file.
    pub fn after(last: u64) -> Self {
        Self { last }
    }

    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> u64 {
        self.last += 1;
        self.last
    }

    /// The id handed out last; 0 if none.
    pub fn last(&self) -> u64 {
        self.last
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
    /// Built from `mesh`; rebuild it with [`Self::rebuild_bvh`] after changing the geometry.
    bvh: Bvh,
    pub ids: Ids,
    pub normal_format: NormalFormat,
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
    /// Interpolated vertex normal, turned around when the triangle was hit from behind.
    pub normal: Vec3,
    pub uv: Vec2,
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

    /// A model from parts that already fit together, e.g. read from a project file: `parts`
    /// must be contiguous index ranges in order, grouped by texture set, and refer to
    /// existing objects and texture sets.
    pub fn from_parts(
        name: String,
        mesh: Mesh,
        objects: Vec<MeshObject>,
        texture_sets: Vec<TextureSet>,
        parts: Vec<Part>,
        ids: Ids,
    ) -> Self {
        Self {
            name,
            bvh: Bvh::new(&mesh),
            mesh,
            objects,
            texture_sets,
            parts,
            ids,
            normal_format: NormalFormat::default(),
        }
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

    /// A new id for a stroke, layer, paint layer or effect, unique within this model.
    pub fn next_id(&mut self) -> u64 {
        self.ids.next()
    }

    /// Index of the part that contains the triangle.
    pub fn part_of_triangle(&self, triangle: u32) -> usize {
        let index = triangle * 3;
        self.parts
            .partition_point(|p| p.indices.end <= index)
            .min(self.parts.len().saturating_sub(1))
    }

    /// Finds the closest triangle hit by the ray, considering only visible parts.
    /// Triangles are hit from both sides.
    pub fn pick(&self, origin: Vec3, dir: Vec3) -> Option<Hit> {
        let hit = self.bvh.raycast(&self.mesh, origin, dir, |t| {
            self.is_part_visible(&self.parts[self.part_of_triangle(t)])
        })?;
        let corners = self.triangle_vertices(hit.triangle);
        let [wa, wb, wc] = [
            1.0 - hit.barycentric.x - hit.barycentric.y,
            hit.barycentric.x,
            hit.barycentric.y,
        ];
        let lerp3 = |v: &[Vec3]| v[corners[0]] * wa + v[corners[1]] * wb + v[corners[2]] * wc;
        let mut normal = lerp3(&self.mesh.normals).normalize_or(Vec3::Y);
        // Turn the normal to the camera only if the triangle was hit from behind. The
        // interpolated normal alone can point away near silhouettes of smooth low-poly
        // meshes; the face normal follows the vertex normals, so reversed winding is fine.
        let [a, b, c] = corners.map(|v| self.mesh.positions[v]);
        let mut face = (b - a).cross(c - a);
        let vertex_sum: Vec3 = corners.iter().map(|&v| self.mesh.normals[v]).sum();
        if face.dot(vertex_sum) < 0.0 {
            face = -face;
        }
        if face.dot(dir) > 0.0 {
            normal = -normal;
        }
        let uvs = &self.mesh.uvs;
        Some(Hit {
            part: self.part_of_triangle(hit.triangle),
            triangle: hit.triangle as usize,
            distance: hit.distance,
            position: origin + dir * hit.distance,
            normal,
            uv: uvs[corners[0]] * wa + uvs[corners[1]] * wb + uvs[corners[2]] * wc,
        })
    }

    /// Appends the triangles of the parts `accept` lets through that may lie within
    /// `radius` of `center`.
    pub fn triangles_near(
        &self,
        center: Vec3,
        radius: f32,
        accept: impl Fn(&Part) -> bool,
        out: &mut Vec<u32>,
    ) {
        self.bvh.triangles_near(
            center,
            radius,
            |t| accept(&self.parts[self.part_of_triangle(t)]),
            out,
        );
    }

    /// Vertex indices of a triangle.
    pub fn triangle_vertices(&self, triangle: u32) -> [usize; 3] {
        let i = triangle as usize * 3;
        [0, 1, 2].map(|k| self.mesh.indices[i + k] as usize)
    }

    /// The bounding volume hierarchy of the mesh.
    pub fn bvh(&self) -> &Bvh {
        &self.bvh
    }

    /// Must be called after `mesh` positions or indices changed.
    pub fn rebuild_bvh(&mut self) {
        self.bvh = Bvh::new(&self.mesh);
    }
}

/// Collects vertices and triangles per object and material during import.
#[derive(Default)]
pub struct ModelBuilder {
    /// Vertices go here directly; indices are added with [`Self::triangle`].
    pub mesh: Mesh,
    objects: Vec<MeshObject>,
    object_lookup: HashMap<String, usize>,
    /// Material names; the texture sets are made in [`Self::build`].
    materials: Vec<String>,
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
        self.materials.push(material.to_owned());
        let i = self.materials.len() - 1;
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
            materials,
            buckets,
            ..
        } = self;

        // Keep only used sets and objects; sets sorted by name, objects in file order.
        let mut used_sets: Vec<usize> = buckets.iter().map(|((_, s), _)| *s).collect();
        used_sets.sort_unstable();
        used_sets.dedup();
        used_sets.sort_by_cached_key(|&s| materials[s].to_lowercase());
        let mut set_map = vec![usize::MAX; materials.len()];
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

        let mut ids = Ids::default();
        let texture_sets = used_sets
            .into_iter()
            .map(|s| TextureSet::new(materials[s].clone(), LayerStack::new_default(&mut ids)))
            .collect();
        Model {
            name,
            bvh: Bvh::new(&mesh),
            ids,
            mesh,
            objects: used_objects
                .into_iter()
                .map(|o| objects[o].clone())
                .collect(),
            texture_sets,
            parts,
            normal_format: NormalFormat::default(),
        }
    }
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
    fn hit_normals_keep_facing_camera_facing_triangles() {
        // Near the silhouette of a coarse smooth sphere, interpolated normals point away
        // from the camera although the hit triangle faces it.
        let model = Model::from_mesh(Mesh::uv_sphere(1.0, 8, 6));
        let dir = Vec3::new(0.2f32.sin(), 0.0, -0.2f32.cos());
        for i in 0..200 {
            for j in 0..200 {
                let x = i as f32 / 100.0 - 1.0;
                let y = j as f32 / 100.0 - 1.0;
                let origin = Vec3::new(x, y, 0.0) - dir * 5.0;
                let Some(hit) = model.pick(origin, dir) else {
                    continue;
                };
                let [a, b, c] = model
                    .triangle_vertices(hit.triangle as u32)
                    .map(|v| model.mesh.positions[v]);
                let face = (b - a).cross(c - a);
                if face.dot(dir) < 0.0 {
                    assert!(hit.normal.dot(face) > 0.0, "flipped at {x}, {y}");
                }
            }
        }
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
