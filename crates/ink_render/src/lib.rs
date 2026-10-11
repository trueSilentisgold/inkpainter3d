//! GPU rendering: the 3D viewport, the 2D (UV) view, the brush strokes and the layer
//! stack compositor.

pub mod camera;
mod compositor;
pub mod environment;
mod paint;
mod region;
mod tiles;
pub mod uv_view;
pub mod viewport;

pub use camera::OrbitCamera;
pub use egui_wgpu::wgpu;
pub use uv_view::{UvCamera, UvView};
pub use viewport::{
    DisplayMode, Highlight, ShadingMode, ViewportRenderer, ViewportSettings, texture_set_color,
};
