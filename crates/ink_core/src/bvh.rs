use crate::mesh::{Aabb, Mesh};
use glam::{Vec2, Vec3};

const LEAF_SIZE: usize = 4;

/// Bounding volume hierarchy over the triangles of a mesh. Finds the triangle under the
/// brush and the triangles a brush dab can touch without testing every triangle.
#[derive(Clone, Debug, Default)]
pub struct Bvh {
    nodes: Vec<Node>,
    /// Triangle indices (into `mesh.indices` / 3), ordered so every leaf is one range.
    triangles: Vec<u32>,
}

#[derive(Clone, Debug)]
struct Node {
    bounds: Aabb,
    /// Leaf: first entry in `triangles`. Inner node: index of the left child; the right
    /// child follows it.
    start: u32,
    /// Number of triangles of a leaf; 0 for inner nodes.
    count: u32,
}

/// Closest triangle along a ray, see [`Bvh::raycast`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RayHit {
    pub triangle: u32,
    pub distance: f32,
    /// Weights of the triangle's second and third corner.
    pub barycentric: Vec2,
}

impl Bvh {
    pub fn new(mesh: &Mesh) -> Self {
        let tris = mesh.indices.as_chunks::<3>().0;
        let bounds: Vec<Aabb> = tris
            .iter()
            .map(|t| {
                let [a, b, c] = t.map(|i| mesh.positions[i as usize]);
                Aabb {
                    min: a.min(b).min(c),
                    max: a.max(b).max(c),
                }
            })
            .collect();
        let centers: Vec<Vec3> = bounds.iter().map(Aabb::center).collect();
        let mut bvh = Self {
            nodes: Vec::with_capacity(2 * tris.len() / LEAF_SIZE + 1),
            triangles: (0..tris.len() as u32).collect(),
        };
        if !tris.is_empty() {
            bvh.nodes.push(Node {
                bounds: union(&bounds, &bvh.triangles),
                start: 0,
                count: tris.len() as u32,
            });
            bvh.split(0, &bounds, &centers);
        }
        bvh
    }

    fn split(&mut self, node: usize, bounds: &[Aabb], centers: &[Vec3]) {
        let (start, count) = (
            self.nodes[node].start as usize,
            self.nodes[node].count as usize,
        );
        if count <= LEAF_SIZE {
            return;
        }
        let range = &mut self.triangles[start..start + count];
        let mut min = Vec3::splat(f32::MAX);
        let mut max = Vec3::splat(f32::MIN);
        for &t in range.iter() {
            min = min.min(centers[t as usize]);
            max = max.max(centers[t as usize]);
        }
        let extent = max - min;
        let axis = if extent.x >= extent.y && extent.x >= extent.z {
            0
        } else if extent.y >= extent.z {
            1
        } else {
            2
        };
        if extent[axis] <= 0.0 {
            // All centers coincide; splitting would not separate anything.
            return;
        }
        let mid = count / 2;
        range.select_nth_unstable_by(mid, |&a, &b| {
            centers[a as usize][axis].total_cmp(&centers[b as usize][axis])
        });
        let left = self.nodes.len();
        for (first, n) in [(start, mid), (start + mid, count - mid)] {
            self.nodes.push(Node {
                bounds: union(bounds, &self.triangles[first..first + n]),
                start: first as u32,
                count: n as u32,
            });
        }
        self.nodes[node].start = left as u32;
        self.nodes[node].count = 0;
        self.split(left, bounds, centers);
        self.split(left + 1, bounds, centers);
    }

    /// The closest triangle hit by the ray among those `accept` lets through. Triangles are
    /// hit from both sides.
    pub fn raycast(
        &self,
        mesh: &Mesh,
        origin: Vec3,
        dir: Vec3,
        accept: impl Fn(u32) -> bool,
    ) -> Option<RayHit> {
        let inv_dir = dir.recip();
        let mut best: Option<RayHit> = None;
        let mut stack = Vec::with_capacity(64);
        if !self.nodes.is_empty() {
            stack.push(0);
        }
        while let Some(index) = stack.pop() {
            let node = &self.nodes[index];
            let max = best.map_or(f32::INFINITY, |h| h.distance);
            if !ray_hits_box(origin, inv_dir, &node.bounds, max) {
                continue;
            }
            if node.count == 0 {
                stack.push(node.start as usize);
                stack.push(node.start as usize + 1);
                continue;
            }
            let leaf = &self.triangles[node.start as usize..(node.start + node.count) as usize];
            for &t in leaf {
                if !accept(t) {
                    continue;
                }
                let [a, b, c] = corners(mesh, t);
                if let Some((distance, barycentric)) = ray_triangle(origin, dir, a, b, c)
                    && best.is_none_or(|h| distance < h.distance)
                {
                    best = Some(RayHit {
                        triangle: t,
                        distance,
                        barycentric,
                    });
                }
            }
        }
        best
    }

    /// Appends the triangles whose bounds touch the sphere and that `accept` lets through.
    pub fn triangles_near(
        &self,
        center: Vec3,
        radius: f32,
        accept: impl Fn(u32) -> bool,
        out: &mut Vec<u32>,
    ) {
        let mut stack = Vec::with_capacity(64);
        if !self.nodes.is_empty() {
            stack.push(0);
        }
        while let Some(index) = stack.pop() {
            let node = &self.nodes[index];
            let closest = center.clamp(node.bounds.min, node.bounds.max);
            if closest.distance_squared(center) > radius * radius {
                continue;
            }
            if node.count == 0 {
                stack.push(node.start as usize);
                stack.push(node.start as usize + 1);
                continue;
            }
            let leaf = &self.triangles[node.start as usize..(node.start + node.count) as usize];
            out.extend(leaf.iter().copied().filter(|&t| accept(t)));
        }
    }
}

fn corners(mesh: &Mesh, triangle: u32) -> [Vec3; 3] {
    let i = triangle as usize * 3;
    [0, 1, 2].map(|k| mesh.positions[mesh.indices[i + k] as usize])
}

fn union(bounds: &[Aabb], triangles: &[u32]) -> Aabb {
    let mut min = Vec3::splat(f32::MAX);
    let mut max = Vec3::splat(f32::MIN);
    for &t in triangles {
        min = min.min(bounds[t as usize].min);
        max = max.max(bounds[t as usize].max);
    }
    Aabb { min, max }
}

fn ray_hits_box(origin: Vec3, inv_dir: Vec3, b: &Aabb, max: f32) -> bool {
    let t0 = (b.min - origin) * inv_dir;
    let t1 = (b.max - origin) * inv_dir;
    let near = t0.min(t1).max_element();
    let far = t0.max(t1).min_element();
    near <= far && far >= 0.0 && near <= max
}

/// Möller–Trumbore, two-sided. Returns the distance along the ray and the barycentric
/// weights of `b` and `c`.
fn ray_triangle(origin: Vec3, dir: Vec3, a: Vec3, b: Vec3, c: Vec3) -> Option<(f32, Vec2)> {
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
    (t > 0.0).then_some((t, Vec2::new(u, v)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn brute_force(mesh: &Mesh, origin: Vec3, dir: Vec3) -> Option<(u32, f32)> {
        let mut best: Option<(u32, f32)> = None;
        for t in 0..mesh.triangle_count() as u32 {
            let [a, b, c] = corners(mesh, t);
            if let Some((d, _)) = ray_triangle(origin, dir, a, b, c)
                && best.is_none_or(|(_, bd)| d < bd)
            {
                best = Some((t, d));
            }
        }
        best
    }

    #[test]
    fn raycast_matches_brute_force() {
        let mesh = Mesh::uv_sphere(1.0, 48, 24);
        let bvh = Bvh::new(&mesh);
        for i in 0..200 {
            let a = i as f32 * 0.37;
            let origin = Vec3::new(a.cos() * 3.0, (a * 0.7).sin() * 2.0, a.sin() * 3.0);
            let target = Vec3::new((a * 1.3).sin() * 0.8, (a * 0.4).cos() * 0.8, 0.1);
            let dir = (target - origin).normalize();
            let fast = bvh.raycast(&mesh, origin, dir, |_| true);
            let slow = brute_force(&mesh, origin, dir);
            match (fast, slow) {
                (Some(f), Some((_, d))) => assert!((f.distance - d).abs() < 1e-5),
                (None, None) => {}
                other => panic!("ray {i}: {other:?}"),
            }
        }
    }

    #[test]
    fn raycast_respects_filter_and_reports_barycentrics() {
        let mesh = Mesh::cube(2.0);
        let bvh = Bvh::new(&mesh);
        let origin = Vec3::new(0.3, 0.2, 5.0);
        let hit = bvh.raycast(&mesh, origin, Vec3::NEG_Z, |_| true).unwrap();
        assert!((hit.distance - 4.0).abs() < 1e-5);
        let [a, b, c] = corners(&mesh, hit.triangle);
        let p = a + (b - a) * hit.barycentric.x + (c - a) * hit.barycentric.y;
        assert!(p.distance(Vec3::new(0.3, 0.2, 1.0)) < 1e-5);

        // Skipping the front face's triangles finds the back face.
        let front = hit.triangle;
        let pair = |t: u32| t / 2 == front / 2;
        let back = bvh
            .raycast(&mesh, origin, Vec3::NEG_Z, |t| !pair(t))
            .unwrap();
        assert!((back.distance - 6.0).abs() < 1e-5);
    }

    #[test]
    fn triangles_near_finds_every_touched_triangle() {
        let mesh = Mesh::uv_sphere(1.0, 32, 16);
        let bvh = Bvh::new(&mesh);
        let center = Vec3::new(0.0, 0.0, 1.0);
        let mut found = Vec::new();
        bvh.triangles_near(center, 0.3, |_| true, &mut found);
        assert!(!found.is_empty());
        for t in 0..mesh.triangle_count() as u32 {
            let near = corners(&mesh, t).iter().any(|p| p.distance(center) < 0.3);
            if near {
                assert!(found.contains(&t), "missed triangle {t}");
            }
        }
        assert!(
            found.len() < mesh.triangle_count() / 4,
            "culls far triangles"
        );
    }

    #[test]
    fn empty_mesh_has_no_hits() {
        let mesh = Mesh::default();
        let bvh = Bvh::new(&mesh);
        assert!(bvh.raycast(&mesh, Vec3::ZERO, Vec3::X, |_| true).is_none());
        let mut out = Vec::new();
        bvh.triangles_near(Vec3::ZERO, 1.0, |_| true, &mut out);
        assert!(out.is_empty());
    }
}
