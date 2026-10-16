//! Core data model of inkpainter3d: meshes, texture sets, their layer stacks and brush
//! strokes.

pub mod bake;
pub mod brush;
pub mod bvh;
pub mod export;
pub mod fill;
pub mod generator;
pub mod history;
pub mod layer;
pub mod mesh;
pub mod model;
pub mod stencil;
pub mod stroke;
pub mod texture;

pub use bake::{BakeSettings, HighPoly, IdSource, Matching, MeshMap, MeshMaps};
pub use brush::{
    Alignment, Alpha, AlphaImage, Brush, BrushMaterial, Channel, ChannelKind, ChannelSet, Jitter,
    PressureCurve, PressureMapping, Rng, SizeSpace, Tool,
};
pub use bvh::{Bvh, RayHit};
pub use fill::{FillShape, FillTarget};
pub use generator::{Generator, GeneratorKind};
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
pub use stencil::{Stencil, StrokeStencil};
pub use stroke::{
    Dab, DabSpacer, LazyMouse, PaintLayer, Stroke, StrokeCamera, StrokePath, StrokeSpace,
    shape_dabs, snap_direction, surface_dab, uv_dab,
};
pub use texture::{Texels, TextureImage};
