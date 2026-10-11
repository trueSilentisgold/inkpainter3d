//! Core data model of inkpainter3d: meshes, texture sets, their layer stacks and brush
//! strokes.

pub mod brush;
pub mod bvh;
pub mod history;
pub mod layer;
pub mod mesh;
pub mod model;
pub mod stroke;
pub mod texture;

pub use brush::{
    Alpha, AlphaImage, Brush, BrushMaterial, Channel, ChannelKind, ChannelSet, PressureCurve,
    PressureMapping, SizeSpace, Tool,
};
pub use bvh::{Bvh, RayHit};
pub use history::{Command, History};
pub use layer::{
    BlendMode, Blending, ChannelBlend, Content, Effect, EffectKind, Fill, Folder, GrayFill, Layer,
    LayerKind, LayerStack, Levels, Mask, Place, Projection, ProjectionMode,
};
pub use mesh::{Aabb, Mesh};
pub use model::{
    AlphaMode, ChannelValues, DEFAULT_MATERIAL, FLAT_NORMAL, Hit, Ids, MeshObject, Model,
    ModelBuilder, NormalFormat, Part, TextureSet,
};
pub use stroke::{
    Dab, DabSpacer, LazyMouse, PaintLayer, Stroke, StrokeCamera, StrokePath, StrokeSpace,
    snap_direction, surface_dab, uv_dab,
};
pub use texture::{Texels, TextureImage};
