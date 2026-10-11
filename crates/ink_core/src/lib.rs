//! Core data model of inkpainter3d: meshes, texture sets and (later) the layer stack.

pub mod mesh;
pub mod model;

pub use mesh::{Aabb, Mesh};
pub use model::{
    AlphaMode, ChannelValues, DEFAULT_MATERIAL, Hit, MeshObject, Model, ModelBuilder, Part,
    TextureSet,
};
