//! Core data model of inkpainter3d: meshes, texture sets, brush strokes and (later) the
//! layer stack.

pub mod brush;
pub mod bvh;
pub mod history;
pub mod mesh;
pub mod model;
pub mod stroke;

pub use brush::{
    Alpha, AlphaImage, Brush, BrushMaterial, Channel, ChannelSet, PressureCurve, PressureMapping,
    SizeSpace, Tool,
};
pub use bvh::{Bvh, RayHit};
pub use history::{Command, History};
pub use mesh::{Aabb, Mesh};
pub use model::{
    AlphaMode, ChannelValues, DEFAULT_MATERIAL, Hit, MeshObject, Model, ModelBuilder, Part,
    TextureSet,
};
pub use stroke::{
    Dab, DabSpacer, LazyMouse, PaintLayer, Stroke, StrokeCamera, StrokePath, StrokeSpace,
    snap_direction, surface_dab, uv_dab,
};
