mod app;
mod pen;
mod theme;

use eframe::egui_wgpu::{WgpuConfiguration, WgpuSetup, WgpuSetupCreateNew};
use ink_render::wgpu;
use std::sync::Arc;

fn main() -> eframe::Result {
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("info,wgpu_core=warn,wgpu_hal=warn"),
    )
    .init();

    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("inkpainter3d")
            .with_icon(
                eframe::icon_data::from_png_bytes(include_bytes!(
                    "../../../assets/branding/app-icon-256.png"
                ))
                .expect("the app icon is a valid PNG"),
            )
            .with_inner_size([1440.0, 900.0])
            .with_min_inner_size([960.0, 600.0])
            .with_drag_and_drop(true),
        renderer: eframe::Renderer::Wgpu,
        wgpu_options: wgpu_options(),
        ..Default::default()
    };
    eframe::run_native(
        "inkpainter3d",
        options,
        Box::new(|cc| Ok(Box::new(app::InkApp::new(cc)?))),
    )
}

/// Asks for everything the GPU can do instead of wgpu's portable defaults: the paint tile
/// pools grow past 256 pages, and tri-planar projections bind the whole mesh as storage.
fn wgpu_options() -> WgpuConfiguration {
    let mut setup = WgpuSetupCreateNew::without_display_handle();
    setup.device_descriptor = Arc::new(|adapter| wgpu::DeviceDescriptor {
        label: Some("inkpainter3d device"),
        required_limits: adapter.limits(),
        ..Default::default()
    });
    WgpuConfiguration {
        wgpu_setup: WgpuSetup::CreateNew(setup),
        ..Default::default()
    }
}
