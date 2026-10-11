//! Model import (and later texture export).

use anyhow::{Context, Result, bail};
use glam::{Mat3, Mat4, Vec2, Vec3};
use ink_core::Mesh;
use std::collections::HashMap;
use std::path::Path;

/// File extensions accepted by [`load_mesh`].
pub const SUPPORTED_EXTENSIONS: &[&str] = &["fbx", "glb", "gltf", "obj"];

/// Loads a model file and merges all of its meshes into a single [`Mesh`] in world space.
pub fn load_mesh(path: &Path) -> Result<Mesh> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    let mut mesh = match ext.as_str() {
        "fbx" => load_fbx(path)?,
        "glb" | "gltf" => load_gltf(path)?,
        "obj" => load_obj(path)?,
        _ => bail!("Unsupported file format: .{ext}"),
    };
    if mesh.indices.is_empty() {
        bail!("{} contains no triangles", path.display());
    }
    mesh.ensure_attributes();
    mesh.name = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Model".into());
    Ok(mesh)
}

fn load_fbx(path: &Path) -> Result<Mesh> {
    let path_str = path
        .to_str()
        .with_context(|| format!("Path is not valid UTF-8: {}", path.display()))?;
    let opts = ufbx::LoadOpts {
        // Convert whatever the exporter used (Z-up, centimeters, ...) to our Y-up meters.
        target_axes: ufbx::CoordinateAxes::right_handed_y_up(),
        target_unit_meters: 1.0,
        space_conversion: ufbx::SpaceConversion::ModifyGeometry,
        generate_missing_normals: true,
        ignore_animation: true,
        load_external_files: false,
        ..Default::default()
    };
    let scene = ufbx::load_file(path_str, opts)
        .map_err(|e| anyhow::anyhow!("Failed to read {}: {}", path.display(), e.description))?;

    let mut out = Mesh::default();
    // FBX stores attributes per face corner; weld identical corners back into shared vertices.
    let mut welded: HashMap<[u32; 8], u32> = HashMap::new();
    let mut tri = Vec::new();
    for node in scene.nodes.iter() {
        let Some(mesh) = node.mesh.as_ref() else {
            continue;
        };
        let to_world = &node.geometry_to_world;
        let normal_matrix = ufbx::matrix_for_normals(to_world);
        let flips_winding = ufbx::matrix_determinant(to_world) < 0.0;
        tri.resize(mesh.max_face_triangles * 3, 0);

        for &face in mesh.faces.iter() {
            let count = mesh.triangulate_face(&mut tri, face) as usize;
            for t in tri[..count * 3].as_chunks::<3>().0 {
                let corners = if flips_winding {
                    [t[0], t[2], t[1]]
                } else {
                    [t[0], t[1], t[2]]
                };
                for corner in corners {
                    let corner = corner as usize;
                    let p = ufbx::transform_position(to_world, mesh.vertex_position[corner]);
                    let n = if mesh.vertex_normal.exists {
                        let n =
                            ufbx::transform_direction(&normal_matrix, mesh.vertex_normal[corner]);
                        Vec3::new(n.x as f32, n.y as f32, n.z as f32).normalize_or(Vec3::Y)
                    } else {
                        Vec3::ZERO
                    };
                    // FBX has its UV origin bottom-left; we use top-left like glTF.
                    let uv = if mesh.vertex_uv.exists {
                        let uv = mesh.vertex_uv[corner];
                        Vec2::new(uv.x as f32, 1.0 - uv.y as f32)
                    } else {
                        Vec2::ZERO
                    };
                    let p = Vec3::new(p.x as f32, p.y as f32, p.z as f32);
                    let key = [p.x, p.y, p.z, n.x, n.y, n.z, uv.x, uv.y].map(f32::to_bits);
                    let index = *welded.entry(key).or_insert_with(|| {
                        out.positions.push(p);
                        out.normals.push(n);
                        out.uvs.push(uv);
                        out.positions.len() as u32 - 1
                    });
                    out.indices.push(index);
                }
            }
        }
    }
    if out.normals.contains(&Vec3::ZERO) {
        out.compute_normals();
    }
    Ok(out)
}

fn load_gltf(path: &Path) -> Result<Mesh> {
    let (doc, buffers, _images) =
        gltf::import(path).with_context(|| format!("Failed to read {}", path.display()))?;
    let mut out = Mesh::default();
    let scene = doc.default_scene().or_else(|| doc.scenes().next());
    match scene {
        Some(scene) => {
            for node in scene.nodes() {
                append_gltf_node(&node, Mat4::IDENTITY, &buffers, &mut out);
            }
        }
        // No scene: take every mesh untransformed.
        None => {
            for mesh in doc.meshes() {
                append_gltf_mesh(&mesh, Mat4::IDENTITY, &buffers, &mut out);
            }
        }
    }
    Ok(out)
}

fn append_gltf_node(
    node: &gltf::Node,
    parent: Mat4,
    buffers: &[gltf::buffer::Data],
    out: &mut Mesh,
) {
    let transform = parent * Mat4::from_cols_array_2d(&node.transform().matrix());
    if let Some(mesh) = node.mesh() {
        append_gltf_mesh(&mesh, transform, buffers, out);
    }
    for child in node.children() {
        append_gltf_node(&child, transform, buffers, out);
    }
}

fn append_gltf_mesh(
    mesh: &gltf::Mesh,
    transform: Mat4,
    buffers: &[gltf::buffer::Data],
    out: &mut Mesh,
) {
    let normal_matrix = Mat3::from_mat4(transform).inverse().transpose();
    let flips_winding = transform.determinant() < 0.0;
    for prim in mesh.primitives() {
        if prim.mode() != gltf::mesh::Mode::Triangles {
            continue;
        }
        let reader = prim.reader(|b| Some(&buffers[b.index()]));
        let Some(positions) = reader.read_positions() else {
            continue;
        };
        let base = out.positions.len() as u32;
        let positions: Vec<Vec3> = positions
            .map(|p| transform.transform_point3(Vec3::from(p)))
            .collect();
        let count = positions.len();

        let normals: Vec<Vec3> = reader
            .read_normals()
            .map(|n| {
                n.map(|n| (normal_matrix * Vec3::from(n)).normalize_or(Vec3::Y))
                    .collect()
            })
            .unwrap_or_default();
        let uvs: Vec<Vec2> = reader
            .read_tex_coords(0)
            .map(|t| t.into_f32().map(Vec2::from).collect())
            .unwrap_or_default();
        let indices: Vec<u32> = match reader.read_indices() {
            Some(i) => i.into_u32().collect(),
            None => (0..count as u32).collect(),
        };

        out.positions.extend(positions);
        out.normals.extend(pad(normals, count, Vec3::ZERO));
        out.uvs.extend(pad(uvs, count, Vec2::ZERO));
        for tri in indices.as_chunks::<3>().0 {
            let (a, b, c) = (tri[0] + base, tri[1] + base, tri[2] + base);
            if flips_winding {
                out.indices.extend_from_slice(&[a, c, b]);
            } else {
                out.indices.extend_from_slice(&[a, b, c]);
            }
        }
    }
}

fn load_obj(path: &Path) -> Result<Mesh> {
    let (models, _materials) = tobj::load_obj(path, &tobj::GPU_LOAD_OPTIONS)
        .with_context(|| format!("Failed to read {}", path.display()))?;
    let mut out = Mesh::default();
    let mut missing_normals = false;
    for model in models {
        let m = model.mesh;
        let base = out.positions.len() as u32;
        let count = m.positions.len() / 3;
        out.positions.extend(
            m.positions
                .as_chunks::<3>()
                .0
                .iter()
                .map(|p| Vec3::new(p[0], p[1], p[2])),
        );
        let normals: Vec<Vec3> = m
            .normals
            .as_chunks::<3>()
            .0
            .iter()
            .map(|n| Vec3::new(n[0], n[1], n[2]))
            .collect();
        missing_normals |= normals.len() != count;
        out.normals.extend(pad(normals, count, Vec3::ZERO));
        // OBJ has its UV origin bottom-left; we use top-left like glTF.
        let uvs: Vec<Vec2> = m
            .texcoords
            .as_chunks::<2>()
            .0
            .iter()
            .map(|t| Vec2::new(t[0], 1.0 - t[1]))
            .collect();
        out.uvs.extend(pad(uvs, count, Vec2::ZERO));
        out.indices.extend(m.indices.iter().map(|i| i + base));
    }
    if missing_normals {
        out.compute_normals();
    }
    Ok(out)
}

fn pad<T: Copy>(mut v: Vec<T>, len: usize, fill: T) -> Vec<T> {
    v.resize(len, fill);
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_obj_without_normals() {
        let dir = std::env::temp_dir().join("ink_io_test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("quad.obj");
        std::fs::write(
            &path,
            "v 0 0 0\nv 1 0 0\nv 1 1 0\nv 0 1 0\nvt 0 0\nvt 1 0\nvt 1 1\nvt 0 1\nf 1/1 2/2 3/3 4/4\n",
        )
        .unwrap();
        let mesh = load_mesh(&path).unwrap();
        assert_eq!(mesh.triangle_count(), 2);
        assert_eq!(mesh.name, "quad");
        assert!(mesh.normals.iter().all(|n| (*n - Vec3::Z).length() < 1e-5));
        // v flipped: OBJ (0,0) bottom-left becomes (0,1)
        assert!(mesh.uvs.contains(&Vec2::new(0.0, 1.0)));
    }

    #[test]
    fn loads_ascii_fbx_in_centimeters() {
        // Minimal ASCII FBX: one quad, 100 units wide, file units in centimeters.
        let fbx = r#"; FBX 7.4.0 project file
FBXHeaderExtension:  {
	FBXHeaderVersion: 1003
	FBXVersion: 7400
}
GlobalSettings:  {
	Version: 1000
	Properties70:  {
		P: "UpAxis", "int", "Integer", "",1
		P: "UpAxisSign", "int", "Integer", "",1
		P: "FrontAxis", "int", "Integer", "",2
		P: "FrontAxisSign", "int", "Integer", "",1
		P: "CoordAxis", "int", "Integer", "",0
		P: "CoordAxisSign", "int", "Integer", "",1
		P: "UnitScaleFactor", "double", "Number", "",1
	}
}
Objects:  {
	Geometry: 1000, "Geometry::Quad", "Mesh" {
		Vertices: *12 {
			a: 0,0,0,100,0,0,100,100,0,0,100,0
		}
		PolygonVertexIndex: *4 {
			a: 0,1,2,-4
		}
		LayerElementUV: 0 {
			Version: 101
			Name: "UVMap"
			MappingInformationType: "ByPolygonVertex"
			ReferenceInformationType: "IndexToDirect"
			UV: *8 {
				a: 0,0,1,0,1,1,0,1
			}
			UVIndex: *4 {
				a: 0,1,2,3
			}
		}
		Layer: 0 {
			Version: 100
			LayerElement:  {
				Type: "LayerElementUV"
				TypedIndex: 0
			}
		}
	}
	Model: 2000, "Model::Quad", "Mesh" {
		Version: 232
	}
}
Connections:  {
	C: "OO",1000,2000
	C: "OO",2000,0
}
"#;
        let dir = std::env::temp_dir().join("ink_io_test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("quad.fbx");
        std::fs::write(&path, fbx).unwrap();

        let mesh = load_mesh(&path).unwrap();
        assert_eq!(mesh.triangle_count(), 2);
        assert_eq!(mesh.vertex_count(), 4, "shared corners should be welded");
        let bounds = mesh.bounds();
        assert!(
            (bounds.max - Vec3::new(1.0, 1.0, 0.0)).length() < 1e-4,
            "cm -> m: {:?}",
            bounds.max
        );
        assert!(mesh.normals.iter().all(|n| (*n - Vec3::Z).length() < 1e-4));
        assert!(mesh.uvs.contains(&Vec2::new(0.0, 1.0)));
    }

    #[test]
    fn rejects_unknown_extension() {
        assert!(load_mesh(Path::new("model.xyz")).is_err());
    }
}
