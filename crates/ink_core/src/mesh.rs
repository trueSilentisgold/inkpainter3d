use glam::{Vec2, Vec3};
use std::f32::consts::PI;

/// Triangle mesh with per-vertex attributes. All attribute vectors have the same length.
#[derive(Clone, Debug, Default)]
pub struct Mesh {
    pub name: String,
    pub positions: Vec<Vec3>,
    pub normals: Vec<Vec3>,
    pub uvs: Vec<Vec2>,
    pub indices: Vec<u32>,
}

#[derive(Clone, Copy, Debug)]
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
        let mut normals = vec![Vec3::ZERO; self.positions.len()];
        for tri in self.indices.as_chunks::<3>().0 {
            let [a, b, c] = [tri[0] as usize, tri[1] as usize, tri[2] as usize];
            let n = (self.positions[b] - self.positions[a])
                .cross(self.positions[c] - self.positions[a]);
            normals[a] += n;
            normals[b] += n;
            normals[c] += n;
        }
        self.normals = normals
            .into_iter()
            .map(|n| n.normalize_or(Vec3::Y))
            .collect();
    }

    /// Fills in missing normals/UVs so every attribute has one entry per vertex.
    pub fn ensure_attributes(&mut self) {
        if self.normals.len() != self.positions.len() {
            self.compute_normals();
        }
        if self.uvs.len() != self.positions.len() {
            self.uvs = vec![Vec2::ZERO; self.positions.len()];
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn primitives_are_consistent() {
        for mesh in [Mesh::uv_sphere(1.0, 16, 8), Mesh::cube(2.0)] {
            assert_eq!(mesh.normals.len(), mesh.positions.len());
            assert_eq!(mesh.uvs.len(), mesh.positions.len());
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
}
