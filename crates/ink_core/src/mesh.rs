use glam::{Vec2, Vec3, Vec4};
use std::collections::HashMap;
use std::f32::consts::PI;

/// Triangle mesh with per-vertex attributes. All attribute vectors have the same length.
#[derive(Clone, Debug, Default)]
pub struct Mesh {
    pub name: String,
    pub positions: Vec<Vec3>,
    pub normals: Vec<Vec3>,
    /// UVs with the origin at the top left, like glTF and wgpu textures.
    pub uvs: Vec<Vec2>,
    /// MikkTSpace tangents: xyz is the tangent, w the bitangent sign. The bitangent
    /// `w * normal.cross(tangent)` points towards decreasing `uv.y`, i.e. up in the image,
    /// which is what OpenGL-style normal maps (Blender, Unity, Godot) expect.
    pub tangents: Vec<Vec4>,
    pub indices: Vec<u32>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Aabb {
    pub min: Vec3,
    pub max: Vec3,
}

impl Aabb {
    pub fn center(&self) -> Vec3 {
        (self.min + self.max) * 0.5
    }

    pub fn radius(&self) -> f32 {
        (self.max - self.min).length() * 0.5
    }
}

impl Mesh {
    pub fn vertex_count(&self) -> usize {
        self.positions.len()
    }

    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }

    pub fn bounds(&self) -> Aabb {
        let mut min = Vec3::splat(f32::MAX);
        let mut max = Vec3::splat(f32::MIN);
        for p in &self.positions {
            min = min.min(*p);
            max = max.max(*p);
        }
        if self.positions.is_empty() {
            min = Vec3::ZERO;
            max = Vec3::ZERO;
        }
        Aabb { min, max }
    }

    /// Recomputes smooth, area-weighted vertex normals from the triangles.
    pub fn compute_normals(&mut self) {
        self.normals = self.smooth_normals();
    }

    fn smooth_normals(&self) -> Vec<Vec3> {
        let mut normals = vec![Vec3::ZERO; self.positions.len()];
        for tri in self.indices.as_chunks::<3>().0 {
            let [a, b, c] = [tri[0] as usize, tri[1] as usize, tri[2] as usize];
            let n = (self.positions[b] - self.positions[a])
                .cross(self.positions[c] - self.positions[a]);
            normals[a] += n;
            normals[b] += n;
            normals[c] += n;
        }
        normals
            .into_iter()
            .map(|n| n.normalize_or(Vec3::Y))
            .collect()
    }

    /// Fills in missing normals, UVs and tangents so every attribute has one entry per vertex.
    /// Zero normals count as missing and get smooth normals.
    pub fn ensure_attributes(&mut self) {
        self.normals.resize(self.positions.len(), Vec3::ZERO);
        if self.normals.contains(&Vec3::ZERO) {
            let smooth = self.smooth_normals();
            for (n, s) in self.normals.iter_mut().zip(smooth) {
                if *n == Vec3::ZERO {
                    *n = s;
                }
            }
        }
        if self.uvs.len() != self.positions.len() {
            self.uvs = vec![Vec2::ZERO; self.positions.len()];
        }
        if self.tangents.len() != self.positions.len() {
            self.compute_tangents();
        }
    }

    /// Computes MikkTSpace tangents, the standard that Blender, Unity, Godot and the common
    /// bakers use, so normal maps look the same everywhere. MikkTSpace works per triangle
    /// corner; vertices whose corners disagree are split.
    pub fn compute_tangents(&mut self) {
        let mut corners = Corners {
            mesh: self,
            tangents: vec![Vec4::new(1.0, 0.0, 0.0, 1.0); self.indices.len()],
        };
        // The error type has no variants: generation cannot fail.
        let _ = bevy_mikktspace::generate_tangents(&mut corners);
        let corner_tangents = corners.tangents;

        let mut tangents: Vec<Option<Vec4>> = vec![None; self.positions.len()];
        let mut splits: HashMap<(u32, [u32; 4]), u32> = HashMap::new();
        for (corner, t) in corner_tangents.into_iter().enumerate() {
            let v = self.indices[corner];
            match tangents[v as usize] {
                None => tangents[v as usize] = Some(t),
                Some(existing) if same_tangent(existing, t) => {}
                Some(_) => {
                    let key = (v, t.to_array().map(f32::to_bits));
                    let index = *splits.entry(key).or_insert_with(|| {
                        self.positions.push(self.positions[v as usize]);
                        self.normals.push(self.normals[v as usize]);
                        self.uvs.push(self.uvs[v as usize]);
                        tangents.push(Some(t));
                        self.positions.len() as u32 - 1
                    });
                    self.indices[corner] = index;
                }
            }
        }
        // Vertices not referenced by any triangle get an arbitrary tangent.
        self.tangents = tangents
            .into_iter()
            .map(|t| t.unwrap_or(Vec4::new(1.0, 0.0, 0.0, 1.0)))
            .collect();
    }

    pub fn uv_sphere(radius: f32, segments: u32, rings: u32) -> Self {
        let mut mesh = Mesh {
            name: "Sphere".into(),
            ..Default::default()
        };
        for r in 0..=rings {
            let v = r as f32 / rings as f32;
            let theta = v * PI;
            for s in 0..=segments {
                let u = s as f32 / segments as f32;
                let phi = u * 2.0 * PI;
                let n = Vec3::new(
                    theta.sin() * phi.cos(),
                    theta.cos(),
                    theta.sin() * phi.sin(),
                );
                mesh.positions.push(n * radius);
                mesh.normals.push(n);
                mesh.uvs.push(Vec2::new(u, v));
            }
        }
        let stride = segments + 1;
        for r in 0..rings {
            for s in 0..segments {
                let a = r * stride + s;
                let b = a + stride;
                mesh.indices
                    .extend_from_slice(&[a, a + 1, b, a + 1, b + 1, b]);
            }
        }
        mesh
    }

    pub fn cube(size: f32) -> Self {
        let h = size * 0.5;
        let mut mesh = Mesh {
            name: "Cube".into(),
            ..Default::default()
        };
        // (normal, tangent u, tangent v) per face
        let faces = [
            (Vec3::X, Vec3::NEG_Z, Vec3::Y),
            (Vec3::NEG_X, Vec3::Z, Vec3::Y),
            (Vec3::Y, Vec3::X, Vec3::NEG_Z),
            (Vec3::NEG_Y, Vec3::X, Vec3::Z),
            (Vec3::Z, Vec3::X, Vec3::Y),
            (Vec3::NEG_Z, Vec3::NEG_X, Vec3::Y),
        ];
        for (i, (n, u, v)) in faces.into_iter().enumerate() {
            let base = mesh.positions.len() as u32;
            // Lay the six faces out in a 3x2 UV grid.
            let cell = Vec2::new((i % 3) as f32 / 3.0, (i / 3) as f32 / 2.0);
            for (du, dv) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
                mesh.positions.push((n + u * du + v * dv) * h);
                mesh.normals.push(n);
                let local = Vec2::new((du + 1.0) * 0.5, 1.0 - (dv + 1.0) * 0.5);
                mesh.uvs.push(cell + local * Vec2::new(1.0 / 3.0, 0.5));
            }
            mesh.indices
                .extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
        }
        mesh
    }
}

fn same_tangent(a: Vec4, b: Vec4) -> bool {
    a.w == b.w && a.truncate().dot(b.truncate()) > 0.999
}

/// Adapter that feeds the indexed triangles to MikkTSpace and collects one tangent per corner.
struct Corners<'a> {
    mesh: &'a Mesh,
    tangents: Vec<Vec4>,
}

impl Corners<'_> {
    fn vertex(&self, face: usize, vert: usize) -> usize {
        self.mesh.indices[face * 3 + vert] as usize
    }
}

impl bevy_mikktspace::Geometry for Corners<'_> {
    fn num_faces(&self) -> usize {
        self.mesh.indices.len() / 3
    }

    fn num_vertices_of_face(&self, _face: usize) -> usize {
        3
    }

    fn position(&self, face: usize, vert: usize) -> [f32; 3] {
        self.mesh.positions[self.vertex(face, vert)].into()
    }

    fn normal(&self, face: usize, vert: usize) -> [f32; 3] {
        self.mesh.normals[self.vertex(face, vert)].into()
    }

    fn tex_coord(&self, face: usize, vert: usize) -> [f32; 2] {
        // MikkTSpace and the bakers work with the UV origin at the bottom left.
        let uv = self.mesh.uvs[self.vertex(face, vert)];
        [uv.x, 1.0 - uv.y]
    }

    fn set_tangent(
        &mut self,
        tangent_space: Option<bevy_mikktspace::TangentSpace>,
        face: usize,
        vert: usize,
    ) {
        if let Some(ts) = tangent_space {
            self.tangents[face * 3 + vert] = Vec4::from_array(ts.tangent_encoded());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn primitives_are_consistent() {
        for mut mesh in [Mesh::uv_sphere(1.0, 16, 8), Mesh::cube(2.0)] {
            mesh.ensure_attributes();
            assert_eq!(mesh.normals.len(), mesh.positions.len());
            assert_eq!(mesh.uvs.len(), mesh.positions.len());
            assert_eq!(mesh.tangents.len(), mesh.positions.len());
            assert_eq!(mesh.indices.len() % 3, 0);
            assert!(
                mesh.indices
                    .iter()
                    .all(|&i| (i as usize) < mesh.positions.len())
            );
        }
    }

    #[test]
    fn winding_is_counter_clockwise_from_outside() {
        for mesh in [Mesh::uv_sphere(1.0, 16, 8), Mesh::cube(2.0)] {
            for tri in mesh.indices.as_chunks::<3>().0 {
                let [a, b, c] = [tri[0], tri[1], tri[2]].map(|i| mesh.positions[i as usize]);
                let n = (b - a).cross(c - a);
                if n.length_squared() < 1e-12 {
                    continue; // degenerate pole triangles
                }
                assert!(
                    n.dot(a + b + c) > 0.0,
                    "{}: triangle faces inward",
                    mesh.name
                );
            }
        }
    }

    #[test]
    fn tangents_follow_uvs() {
        let mut mesh = Mesh::cube(2.0);
        mesh.ensure_attributes();
        for tri in mesh.indices.as_chunks::<3>().0 {
            let [a, b, c] = tri.map(|i| i as usize);
            let (p0, p1, p2) = (mesh.positions[a], mesh.positions[b], mesh.positions[c]);
            let (uv0, uv1, uv2) = (mesh.uvs[a], mesh.uvs[b], mesh.uvs[c]);
            // Direction in which u grows and in which v shrinks (image up).
            let (e1, e2) = (p1 - p0, p2 - p0);
            let (d1, d2) = (uv1 - uv0, uv2 - uv0);
            let r = 1.0 / (d1.x * d2.y - d2.x * d1.y);
            let du = (e1 * d2.y - e2 * d1.y) * r;
            let dv = (e2 * d1.x - e1 * d2.x) * r;
            for &v in &[a, b, c] {
                let t = mesh.tangents[v];
                let n = mesh.normals[v];
                assert!(t.truncate().dot(n).abs() < 1e-4, "tangent not orthogonal");
                assert!(t.truncate().dot(du) > 0.0, "tangent does not follow +u");
                let bitangent = n.cross(t.truncate()) * t.w;
                assert!(
                    bitangent.dot(-dv) > 0.0,
                    "bitangent does not point up in the image"
                );
            }
        }
    }

    #[test]
    fn mirrored_uvs_split_vertices() {
        // Two triangles sharing an edge, the second with mirrored U: the shared vertices
        // need opposite bitangent signs and must be split.
        let mut mesh = Mesh {
            positions: vec![
                Vec3::new(0.0, 0.0, 0.0),
                Vec3::new(1.0, 0.0, 0.0),
                Vec3::new(0.0, 1.0, 0.0),
                Vec3::new(-1.0, 0.0, 0.0),
            ],
            normals: vec![Vec3::Z; 4],
            uvs: vec![
                Vec2::new(0.0, 1.0),
                Vec2::new(1.0, 1.0),
                Vec2::new(0.0, 0.0),
                Vec2::new(1.0, 1.0),
            ],
            indices: vec![0, 1, 2, 0, 2, 3],
            ..Default::default()
        };
        mesh.ensure_attributes();
        assert_eq!(mesh.tangents.len(), mesh.positions.len());
        let first: Vec<f32> = mesh.indices[..3]
            .iter()
            .map(|&i| mesh.tangents[i as usize].w)
            .collect();
        let second: Vec<f32> = mesh.indices[3..]
            .iter()
            .map(|&i| mesh.tangents[i as usize].w)
            .collect();
        assert!(first.iter().all(|&w| w == first[0]));
        assert!(second.iter().all(|&w| w == second[0]));
        assert_ne!(first[0], second[0]);
        assert!(mesh.positions.len() > 4);
    }
}
