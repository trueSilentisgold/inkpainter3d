//! Model and environment import (and later texture export).

use anyhow::{Context, Result, bail};
use glam::{Mat3, Mat4, Vec2, Vec3};
use ink_core::{AlphaImage, DEFAULT_MATERIAL, Model, ModelBuilder};
use std::collections::HashMap;
use std::path::Path;

/// File extensions accepted by [`load_model`].
pub const SUPPORTED_EXTENSIONS: &[&str] = &["fbx", "glb", "gltf", "obj"];

/// File extensions accepted by [`load_environment`].
pub const ENVIRONMENT_EXTENSIONS: &[&str] = &["hdr", "exr"];

/// Image formats [`load_alpha`] reads.
pub const ALPHA_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg"];

/// Loads a model file in world space, keeping its objects and materials. Every material
/// becomes a texture set.
pub fn load_model(path: &Path) -> Result<Model> {
    let ext = extension(path);
    let mut builder = ModelBuilder::default();
    match ext.as_str() {
        "fbx" => load_fbx(path, &mut builder)?,
        "glb" | "gltf" => load_gltf(path, &mut builder)?,
        "obj" => load_obj(path, &mut builder)?,
        _ => bail!("Unsupported file format: .{ext}"),
    }
    if !builder.has_triangles() {
        bail!("{} contains no triangles", path.display());
    }
    let name = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Model".into());
    Ok(builder.build(name))
}

fn extension(path: &Path) -> String {
    path.extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default()
}

fn load_fbx(path: &Path, out: &mut ModelBuilder) -> Result<()> {
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

    // FBX stores attributes per face corner; weld identical corners back into shared vertices.
    let mut welded: HashMap<[u32; 8], u32> = HashMap::new();
    let mut tri = Vec::new();
    for node in scene.nodes.iter() {
        let Some(mesh) = node.mesh.as_ref() else {
            continue;
        };
        let object = out.add_object(&node.element.name);
        let sets: Vec<usize> = mesh
            .materials
            .iter()
            .map(|m| out.texture_set(&m.element.name))
            .collect();
        let to_world = &node.geometry_to_world;
        let normal_matrix = ufbx::matrix_for_normals(to_world);
        let flips_winding = ufbx::matrix_determinant(to_world) < 0.0;
        tri.resize(mesh.max_face_triangles * 3, 0);

        for (face_index, &face) in mesh.faces.iter().enumerate() {
            let set = match mesh.face_material.get(face_index) {
                Some(&m) if (m as usize) < sets.len() => sets[m as usize],
                _ => out.texture_set(DEFAULT_MATERIAL),
            };
            let count = mesh.triangulate_face(&mut tri, face) as usize;
            for t in tri[..count * 3].as_chunks::<3>().0 {
                let corners = if flips_winding {
                    [t[0], t[2], t[1]]
                } else {
                    [t[0], t[1], t[2]]
                };
                let vertices = corners.map(|corner| {
                    let corner = corner as usize;
                    let p = ufbx::transform_position(to_world, mesh.vertex_position[corner]);
                    let n = if mesh.vertex_normal.exists {
                        let n =
                            ufbx::transform_direction(&normal_matrix, mesh.vertex_normal[corner]);
                        Vec3::new(n.x as f32, n.y as f32, n.z as f32).normalize_or_zero()
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
                    *welded.entry(key).or_insert_with(|| {
                        out.mesh.positions.push(p);
                        out.mesh.normals.push(n);
                        out.mesh.uvs.push(uv);
                        out.mesh.positions.len() as u32 - 1
                    })
                });
                out.triangle(object, set, vertices);
            }
        }
    }
    Ok(())
}

fn load_gltf(path: &Path, out: &mut ModelBuilder) -> Result<()> {
    let (doc, buffers, _images) =
        gltf::import(path).with_context(|| format!("Failed to read {}", path.display()))?;
    let scene = doc.default_scene().or_else(|| doc.scenes().next());
    match scene {
        Some(scene) => {
            for node in scene.nodes() {
                append_gltf_node(&node, Mat4::IDENTITY, &buffers, out);
            }
        }
        // No scene: take every mesh untransformed.
        None => {
            for mesh in doc.meshes() {
                let name = gltf_mesh_name(&mesh);
                let object = out.add_object(&name);
                append_gltf_mesh(&mesh, object, Mat4::IDENTITY, &buffers, out);
            }
        }
    }
    Ok(())
}

fn gltf_mesh_name(mesh: &gltf::Mesh) -> String {
    mesh.name()
        .map(str::to_owned)
        .unwrap_or_else(|| format!("Mesh {}", mesh.index()))
}

fn append_gltf_node(
    node: &gltf::Node,
    parent: Mat4,
    buffers: &[gltf::buffer::Data],
    out: &mut ModelBuilder,
) {
    let transform = parent * Mat4::from_cols_array_2d(&node.transform().matrix());
    if let Some(mesh) = node.mesh() {
        let name = node
            .name()
            .map(str::to_owned)
            .unwrap_or_else(|| gltf_mesh_name(&mesh));
        let object = out.add_object(&name);
        append_gltf_mesh(&mesh, object, transform, buffers, out);
    }
    for child in node.children() {
        append_gltf_node(&child, transform, buffers, out);
    }
}

fn append_gltf_mesh(
    mesh: &gltf::Mesh,
    object: usize,
    transform: Mat4,
    buffers: &[gltf::buffer::Data],
    out: &mut ModelBuilder,
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
        let material = prim.material();
        let set = match (material.name(), material.index()) {
            (Some(name), _) => out.texture_set(name),
            (None, Some(index)) => out.texture_set(&format!("Material {index}")),
            (None, None) => out.texture_set(DEFAULT_MATERIAL),
        };
        let base = out.mesh.positions.len() as u32;
        let positions: Vec<Vec3> = positions
            .map(|p| transform.transform_point3(Vec3::from(p)))
            .collect();
        let count = positions.len();

        let normals: Vec<Vec3> = reader
            .read_normals()
            .map(|n| {
                n.map(|n| (normal_matrix * Vec3::from(n)).normalize_or_zero())
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

        out.mesh.positions.extend(positions);
        out.mesh.normals.extend(pad(normals, count, Vec3::ZERO));
        out.mesh.uvs.extend(pad(uvs, count, Vec2::ZERO));
        for tri in indices.as_chunks::<3>().0 {
            let (a, b, c) = (tri[0] + base, tri[1] + base, tri[2] + base);
            out.triangle(
                object,
                set,
                if flips_winding { [a, c, b] } else { [a, b, c] },
            );
        }
    }
}

fn load_obj(path: &Path, out: &mut ModelBuilder) -> Result<()> {
    let (models, materials) = tobj::load_obj(path, &tobj::GPU_LOAD_OPTIONS)
        .with_context(|| format!("Failed to read {}", path.display()))?;
    // A missing .mtl file is not an error; the material slots just stay unnamed.
    let materials = materials.unwrap_or_default();
    for model in models {
        let m = model.mesh;
        // tobj splits objects at material changes; merge them back by name.
        let object = out.object(&model.name);
        let set = match m.material_id {
            Some(id) => match materials.get(id) {
                Some(material) => out.texture_set(&material.name),
                None => out.texture_set(&format!("Material {id}")),
            },
            None => out.texture_set(DEFAULT_MATERIAL),
        };
        let base = out.mesh.positions.len() as u32;
        let count = m.positions.len() / 3;
        out.mesh.positions.extend(
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
        out.mesh.normals.extend(pad(normals, count, Vec3::ZERO));
        // OBJ has its UV origin bottom-left; we use top-left like glTF.
        let uvs: Vec<Vec2> = m
            .texcoords
            .as_chunks::<2>()
            .0
            .iter()
            .map(|t| Vec2::new(t[0], 1.0 - t[1]))
            .collect();
        out.mesh.uvs.extend(pad(uvs, count, Vec2::ZERO));
        for tri in m.indices.as_chunks::<3>().0 {
            out.triangle(object, set, tri.map(|i| i + base));
        }
    }
    Ok(())
}

fn pad<T: Copy>(mut v: Vec<T>, len: usize, fill: T) -> Vec<T> {
    v.resize(len, fill);
    v
}

/// Loads a brush alpha: white paints, black does not. Images with transparency use their
/// alpha channel, others their brightness.
pub fn load_alpha(path: &Path) -> Result<AlphaImage> {
    let image = image::open(path)
        .with_context(|| format!("Failed to read {}", path.display()))?
        .into_rgba8();
    let (width, height) = image.dimensions();
    if width == 0 || height == 0 {
        bail!("Alpha image is empty");
    }
    let transparent = image.pixels().any(|p| p[3] < 255);
    let pixels = image
        .pixels()
        .map(|p| {
            if transparent {
                p[3]
            } else {
                let [r, g, b, _] = p.0.map(f32::from);
                (0.2126 * r + 0.7152 * g + 0.0722 * b).round() as u8
            }
        })
        .collect();
    let name = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    Ok(AlphaImage::new(name, width, height, pixels))
}

/// An equirectangular HDR environment, linear RGBA, rows top to bottom.
pub struct EnvironmentImage {
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<[f32; 4]>,
}

/// The environment shown at startup (CC0, Poly Haven "Studio Small 09").
pub fn default_environment() -> EnvironmentImage {
    const DATA: &[u8] = include_bytes!("../../../assets/environments/studio_small_09_1k.hdr");
    decode_environment("Studio Small 09", DATA, image::ImageFormat::Hdr)
        .expect("bundled environment is valid")
}

/// Loads an equirectangular `.hdr` or `.exr` panorama.
pub fn load_environment(path: &Path) -> Result<EnvironmentImage> {
    let format = match extension(path).as_str() {
        "hdr" => image::ImageFormat::Hdr,
        "exr" => image::ImageFormat::OpenExr,
        ext => bail!("Unsupported environment format: .{ext}"),
    };
    let data = std::fs::read(path).with_context(|| format!("Failed to read {}", path.display()))?;
    let name = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    decode_environment(&name, &data, format)
        .with_context(|| format!("Failed to decode {}", path.display()))
}

fn decode_environment(
    name: &str,
    data: &[u8],
    format: image::ImageFormat,
) -> Result<EnvironmentImage> {
    let image = image::load_from_memory_with_format(data, format)?.into_rgba32f();
    let (width, height) = image.dimensions();
    if width < 2 || height < 1 {
        bail!("Environment image is empty");
    }
    let pixels = image
        .into_raw()
        .as_chunks::<4>()
        .0
        .iter()
        .map(|p| p.map(|c| if c.is_finite() { c.max(0.0) } else { 0.0 }))
        .collect();
    Ok(EnvironmentImage {
        name: name.to_owned(),
        width,
        height,
        pixels,
    })
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
        let model = load_model(&path).unwrap();
        let mesh = &model.mesh;
        assert_eq!(mesh.triangle_count(), 2);
        assert_eq!(model.name, "quad");
        assert_eq!(model.texture_sets[0].name, ink_core::DEFAULT_MATERIAL);
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

        let model = load_model(&path).unwrap();
        let mesh = &model.mesh;
        assert_eq!(mesh.triangle_count(), 2);
        assert_eq!(mesh.vertex_count(), 4, "shared corners should be welded");
        assert_eq!(model.objects[0].name, "Quad");
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
        assert!(load_model(Path::new("model.xyz")).is_err());
    }

    fn write_temp(name: &str, contents: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join("ink_io_test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, contents).unwrap();
        path
    }

    fn set_names(model: &Model) -> Vec<&str> {
        model.texture_sets.iter().map(|s| s.name.as_str()).collect()
    }

    #[test]
    fn fbx_keeps_materials_per_polygon() {
        // One object, two quads, each with its own material.
        let fbx = r#"; FBX 7.4.0 project file
FBXHeaderExtension:  {
	FBXHeaderVersion: 1003
	FBXVersion: 7400
}
Objects:  {
	Geometry: 1000, "Geometry::Body", "Mesh" {
		Vertices: *18 {
			a: 0,0,0,1,0,0,1,1,0,0,1,0,2,0,0,2,1,0
		}
		PolygonVertexIndex: *8 {
			a: 0,1,2,-4,1,4,5,-3
		}
		LayerElementMaterial: 0 {
			Version: 101
			Name: ""
			MappingInformationType: "ByPolygon"
			ReferenceInformationType: "IndexToDirect"
			Materials: *2 {
				a: 0,1
			}
		}
		Layer: 0 {
			Version: 100
			LayerElement:  {
				Type: "LayerElementMaterial"
				TypedIndex: 0
			}
		}
	}
	Model: 2000, "Model::Body", "Mesh" {
		Version: 232
	}
	Material: 3000, "Material::Skin", "" {
		Version: 102
	}
	Material: 3001, "Material::Cloth", "" {
		Version: 102
	}
}
Connections:  {
	C: "OO",1000,2000
	C: "OO",2000,0
	C: "OO",3000,2000
	C: "OO",3001,2000
}
"#;
        let model = load_model(&write_temp("materials.fbx", fbx)).unwrap();
        assert_eq!(set_names(&model), ["Cloth", "Skin"]);
        assert_eq!(model.objects.len(), 1);
        assert_eq!(model.objects[0].name, "Body");
        assert_eq!(model.triangle_count(0, 0), 2);
        assert_eq!(model.triangle_count(1, 0), 2);
        assert_eq!(model.sets_of_object(0), [0, 1]);
    }

    #[test]
    fn gltf_keeps_materials_per_primitive() {
        // One triangle used by two primitives with different materials.
        let gltf = r#"{
  "asset": {"version": "2.0"},
  "buffers": [{"byteLength": 36, "uri": "data:application/octet-stream;base64,AAAAAAAAAAAAAAAAAACAPwAAAAAAAAAAAAAAAAAAgD8AAAAA"}],
  "bufferViews": [{"buffer": 0, "byteLength": 36}],
  "accessors": [{"bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC3",
                 "min": [0, 0, 0], "max": [1, 1, 0]}],
  "materials": [{"name": "Red"}, {"name": "Blue"}],
  "meshes": [{"primitives": [
    {"attributes": {"POSITION": 0}, "material": 0},
    {"attributes": {"POSITION": 0}, "material": 1}
  ]}],
  "nodes": [{"name": "Thing", "mesh": 0, "translation": [0, 2, 0]}],
  "scenes": [{"nodes": [0]}],
  "scene": 0
}"#;
        let model = load_model(&write_temp("materials.gltf", gltf)).unwrap();
        assert_eq!(set_names(&model), ["Blue", "Red"]);
        assert_eq!(model.objects[0].name, "Thing");
        assert_eq!(model.parts.len(), 2);
        assert!(
            (model.bounds().min.y - 2.0).abs() < 1e-6,
            "node transform applied"
        );
        assert!(
            model.mesh.normals.iter().all(|n| n.length() > 0.99),
            "missing normals are generated"
        );
    }

    #[test]
    fn obj_keeps_materials_and_merges_objects() {
        write_temp("box.mtl", "newmtl Wood\nKd 1 1 1\nnewmtl Metal\nKd 1 1 1\n");
        let obj = "mtllib box.mtl\no Box\nv 0 0 0\nv 1 0 0\nv 1 1 0\nv 0 1 0\n\
                   usemtl Wood\nf 1 2 3\nusemtl Metal\nf 1 3 4\no Lid\nusemtl Wood\nf 1 2 4\n";
        let model = load_model(&write_temp("box.obj", obj)).unwrap();
        assert_eq!(set_names(&model), ["Metal", "Wood"]);
        let objects: Vec<&str> = model.objects.iter().map(|o| o.name.as_str()).collect();
        assert_eq!(objects, ["Box", "Lid"]);
        assert_eq!(
            model.objects_in_set(1),
            [0, 1],
            "Wood is used by Box and Lid"
        );
        assert_eq!(model.sets_of_object(0), [0, 1], "Box uses Metal and Wood");
    }

    #[test]
    fn loads_bundled_environment() {
        let env = default_environment();
        assert_eq!(env.width, 2 * env.height);
        assert_eq!(env.pixels.len(), (env.width * env.height) as usize);
        assert!(env.pixels.iter().any(|p| p[0] > 1.0), "should be HDR");
    }

    #[test]
    fn alphas_use_transparency_or_brightness() {
        let dir = std::env::temp_dir().join("inkpainter3d-alpha-test");
        std::fs::create_dir_all(&dir).unwrap();
        let gray = dir.join("gray.png");
        image::GrayImage::from_fn(4, 2, |x, _| image::Luma([(x * 80) as u8]))
            .save(&gray)
            .unwrap();
        let alpha = load_alpha(&gray).unwrap();
        assert_eq!(
            (alpha.width, alpha.height, alpha.name.as_str()),
            (4, 2, "gray")
        );
        assert_eq!(&alpha.pixels[..4], &[0, 80, 160, 240]);

        let cutout = dir.join("cutout.png");
        image::RgbaImage::from_fn(2, 1, |x, _| {
            image::Rgba([0, 0, 0, if x == 0 { 0 } else { 200 }])
        })
        .save(&cutout)
        .unwrap();
        assert_eq!(load_alpha(&cutout).unwrap().pixels, [0, 200]);
    }
}
