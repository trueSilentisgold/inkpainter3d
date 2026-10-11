//! GPU rendering: the 3D viewport, the 2D (UV) view and the brush strokes.

pub mod camera;
pub mod environment;
mod paint;
pub mod uv_view;
pub mod viewport;

pub use camera::OrbitCamera;
pub use egui_wgpu::wgpu;
pub use uv_view::{UvCamera, UvView};
pub use viewport::{
    DisplayMode, Highlight, ShadingMode, ViewportRenderer, ViewportSettings, texture_set_color,
};
