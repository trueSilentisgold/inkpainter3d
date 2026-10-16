//! Polygon Fill: which triangles a click fills. A fill stroke stores the triangles it
//! covers, so it replays like any stroke, at any texture resolution.

use crate::Model;
use glam::{Vec2, Vec3};
use std::collections::HashMap;
use std::sync::Arc;

/// What a fill covers, as the Polygon Fill tool's modes name it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FillShape {
    /// The clicked triangle.
    Triangle,
    /// The clicked polygon: the triangle and, if it came from a quad, the other half.
    #[default]
    Polygon,
    /// The whole mesh object, in the painted texture set.
    Mesh,
    /// The UV island the triangle belongs to.
    UvIsland,
}

impl FillShape {
    pub const ALL: [Self; 4] = [Self::Triangle, Self::Polygon, Self::Mesh, Self::UvIsland];

    pub fn label(self) -> &'static str {
        match self {
            Self::Triangle => "Triangle",
            Self::Polygon => "Polygon",
            Self::Mesh => "Mesh",
            Self::UvIsland => "UV island",
        }
    }
}

/// The triangles a fill stroke paints.
#[derive(Clone, Debug, PartialEq)]
pub enum FillTarget {
    /// Triangle indices of the mesh.
    Triangles(Arc<Vec<u32>>),
    /// Every triangle of a mesh object that belongs to the painted texture set.
    Object(usize),
}

/// A point as an integer key, so equal positions find each other.
fn key2(v: Vec2) -> (i32, i32) {
    ((v.x * 1e5).round() as i32, (v.y * 1e5).round() as i32)
}

fn key3(v: Vec3) -> (i32, i32, i32) {
    (
        (v.x * 1e4).round() as i32,
        (v.y * 1e4).round() as i32,
        (v.z * 1e4).round() as i32,
    )
}

impl Model {
    /// Triangles of the parts that match.
    fn triangles_where(&self, accept: impl Fn(&crate::Part) -> bool) -> impl Iterator<Item = u32> {
        self.parts
            .iter()
            .filter(move |p| accept(p))
            .flat_map(|p| (p.indices.start / 3)..(p.indices.end / 3))
    }

    /// What clicking `triangle` fills in the given mode.
    pub fn fill_target(&self, shape: FillShape, triangle: u32) -> FillTarget {
        match shape {
            FillShape::Triangle => FillTarget::Triangles(Arc::new(vec![triangle])),
            FillShape::Polygon => FillTarget::Triangles(Arc::new(self.polygon_of(triangle))),
            FillShape::Mesh => {
                FillTarget::Object(self.parts[self.part_of_triangle(triangle)].object)
            }
            FillShape::UvIsland => FillTarget::Triangles(Arc::new(self.uv_island(triangle))),
        }
    }

    /// The triangle that covers `uv` in the texture set, if any.
    pub fn triangle_at_uv(&self, set: usize, uv: Vec2) -> Option<u32> {
        self.triangles_where(|p| p.texture_set == set && self.is_part_visible(p))
            .find(|&t| {
                let [a, b, c] = self.triangle_vertices(t).map(|v| self.mesh.uvs[v]);
                let sign = |p: Vec2, q: Vec2, r: Vec2| (p - r).perp_dot(q - r);
                let (d1, d2, d3) = (sign(uv, a, b), sign(uv, b, c), sign(uv, c, a));
                let negative = d1 < 0.0 || d2 < 0.0 || d3 < 0.0;
                let positive = d1 > 0.0 || d2 > 0.0 || d3 > 0.0;
                !(negative && positive)
            })
    }

    /// The longest edge of a triangle as a pair of position keys, ordered.
    fn longest_edge(&self, triangle: u32) -> ((i32, i32, i32), (i32, i32, i32)) {
        let p = self
            .triangle_vertices(triangle)
            .map(|v| self.mesh.positions[v]);
        let edges = [(0, 1), (1, 2), (2, 0)];
        let (a, b) = edges
            .into_iter()
            .max_by(|&(a, b), &(c, d)| {
                p[a].distance_squared(p[b])
                    .total_cmp(&p[c].distance_squared(p[d]))
            })
            .unwrap_or((0, 1));
        let (ka, kb) = (key3(p[a]), key3(p[b]));
        (ka.min(kb), ka.max(kb))
    }

    fn face_normal(&self, triangle: u32) -> Vec3 {
        let [a, b, c] = self
            .triangle_vertices(triangle)
            .map(|v| self.mesh.positions[v]);
        (b - a).cross(c - a).normalize_or_zero()
    }

    /// A triangle and, if it is half of a flat quad, the other half: the neighbour across
    /// the longest edge of both, in the same plane. (The mesh is triangulated, so quads are
    /// recognized by their diagonal.)
    fn polygon_of(&self, triangle: u32) -> Vec<u32> {
        let part = &self.parts[self.part_of_triangle(triangle)];
        let edge = self.longest_edge(triangle);
        let normal = self.face_normal(triangle);
        let other = ((part.indices.start / 3)..(part.indices.end / 3)).find(|&t| {
            t != triangle && self.longest_edge(t) == edge && self.face_normal(t).dot(normal) > 0.999
        });
        let mut triangles = vec![triangle];
        triangles.extend(other);
        triangles
    }

    /// The triangles connected to `triangle` through shared UV points, in its object and
    /// texture set.
    fn uv_island(&self, triangle: u32) -> Vec<u32> {
        let home = &self.parts[self.part_of_triangle(triangle)];
        let (set, object) = (home.texture_set, home.object);
        let candidates: Vec<u32> = self
            .triangles_where(|p| p.texture_set == set && p.object == object)
            .collect();
        let mut at: HashMap<(i32, i32), Vec<u32>> = HashMap::new();
        for &t in &candidates {
            for v in self.triangle_vertices(t) {
                at.entry(key2(self.mesh.uvs[v])).or_default().push(t);
            }
        }
        let mut seen = std::collections::HashSet::from([triangle]);
        let mut stack = vec![triangle];
        let mut island = Vec::new();
        while let Some(t) = stack.pop() {
            island.push(t);
            for v in self.triangle_vertices(t) {
                for &next in &at[&key2(self.mesh.uvs[v])] {
                    if seen.insert(next) {
                        stack.push(next);
                    }
                }
            }
        }
        island.sort_unstable();
        island
    }

    /// The triangles of the texture set that a fill target covers (no hidden parts).
    pub fn fill_triangles(&self, target: &FillTarget, set: usize) -> Vec<u32> {
        // Files from elsewhere may name triangles that don't exist.
        let accept = |t: u32| {
            (t as usize) < self.mesh.triangle_count()
                && self.parts[self.part_of_triangle(t)].texture_set == set
        };
        match target {
            FillTarget::Triangles(list) => list.iter().copied().filter(|&t| accept(t)).collect(),
            FillTarget::Object(object) => self
                .triangles_where(|p| p.texture_set == set && p.object == *object)
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Mesh;

    /// A flat 2 x 1 grid of quads (four triangles), each quad its own UV island.
    fn grid() -> Model {
        let mut mesh = Mesh::default();
        let mut quad = |x: f32| {
            let base = mesh.positions.len() as u32;
            for (px, py) in [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)] {
                mesh.positions.push(Vec3::new(x + px, py, 0.0));
                mesh.normals.push(Vec3::Z);
                mesh.uvs
                    .push(Vec2::new((x * 0.5 + px * 0.5) * 0.9, py * 0.5));
            }
            mesh.indices.extend([0, 1, 2, 0, 2, 3].map(|i| base + i));
        };
        quad(0.0);
        quad(2.0);
        mesh.name = "grid".into();
        Model::from_mesh(mesh)
    }

    #[test]
    fn polygon_is_the_quad() {
        let model = grid();
        let FillTarget::Triangles(t) = model.fill_target(FillShape::Polygon, 1) else {
            panic!()
        };
        assert_eq!(*t, vec![1, 0]);
        let FillTarget::Triangles(t) = model.fill_target(FillShape::Triangle, 3) else {
            panic!()
        };
        assert_eq!(*t, vec![3]);
    }

    #[test]
    fn islands_and_meshes() {
        let model = grid();
        let FillTarget::Triangles(island) = model.fill_target(FillShape::UvIsland, 2) else {
            panic!()
        };
        assert_eq!(*island, vec![2, 3]);
        assert_eq!(model.fill_target(FillShape::Mesh, 0), FillTarget::Object(0));
        assert_eq!(model.fill_triangles(&FillTarget::Object(0), 0).len(), 4);
    }

    #[test]
    fn triangles_are_found_by_uv() {
        let model = grid();
        assert_eq!(model.triangle_at_uv(0, Vec2::new(0.4, 0.1)), Some(0));
        assert_eq!(model.triangle_at_uv(0, Vec2::new(0.6, 0.3)), None);
    }
}
