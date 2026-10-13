//! Help > About: the logo, the version, who made it and what it is built on.

use super::{InkApp, dialog};
use crate::theme;
use eframe::egui::{self, RichText};

impl InkApp {
    /// The About dialog while it is open. Escape and a click outside close it.
    pub(super) fn about_dialog(&mut self, ui: &mut egui::Ui) {
        if !self.about_open {
            return;
        }
        let mut close = false;
        let modal = dialog(ui, "about").show(ui.ctx(), |ui| {
            ui.set_width(420.0);
            ui.vertical_centered(|ui| {
                ui.add(
                    egui::Image::new(egui::include_image!(
                        "../../../../assets/branding/inkrabbit-logo.svg"
                    ))
                    .fit_to_exact_size(egui::Vec2::splat(72.0)),
                );
                ui.add_space(6.0);
                ui.label(
                    RichText::new("inkpainter3d")
                        .font(theme::medium(22.0))
                        .color(theme::TEXT_STRONG),
                );
                ui.label(
                    RichText::new(format!("Version {}", env!("CARGO_PKG_VERSION")))
                        .small()
                        .color(theme::TEXT_WEAK),
                );
                ui.add_space(2.0);
                ui.label(
                    RichText::new("Free, open-source 3D texture painting").color(theme::TEXT_WEAK),
                );
            });
            ui.add_space(8.0);
            ui.separator();
            ui.add_space(4.0);
            ui.label("Free software under the GNU GPL 3.0 or later.");
            ui.horizontal(|ui| {
                ui.label("Made by");
                ui.hyperlink_to("INKRABBIT", "https://inkrabbit.de");
            });
            theme::group_label(ui, "Credits");
            ui.label("Studio Small 09 environment by Poly Haven (CC0)");
            ui.label("Space Grotesk and JetBrains Mono fonts (SIL Open Font License 1.1)");
            ui.add_space(10.0);
            ui.vertical_centered(|ui| {
                close = ui.button("Close").clicked();
            });
        });
        if close || modal.should_close() {
            self.about_open = false;
        }
    }
}
