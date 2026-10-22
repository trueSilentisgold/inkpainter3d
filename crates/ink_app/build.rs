//! Embeds the app icon into the Windows executable, for Explorer and the taskbar.

fn main() {
    println!("cargo:rerun-if-changed=../../assets/branding/app-icon.ico");
    #[cfg(windows)]
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        winresource::WindowsResource::new()
            .set_icon("../../assets/branding/app-icon.ico")
            .compile()
            .expect("the app icon can be embedded");
    }
}
