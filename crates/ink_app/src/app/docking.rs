//! The dock: the viewport and every panel are tabs that can be moved, split, stacked and
//! pulled out into their own windows. The layout is saved when the program closes, and can
//! be saved to and loaded from files, like Painter's window layouts.

use super::{InkApp, project::config_dir};
use crate::theme;
use eframe::egui::{self, Id, WidgetText};
use eframe::egui_wgpu::RenderState;
use egui_dock::{DockArea, DockState, NodeIndex, TabViewer};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// A tab of the dock.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub(super) enum Tab {
    Viewport,
    TextureSetList,
    Layers,
    Properties,
    TextureSetSettings,
    Display,
    Shelf,
}

impl Tab {
    /// The panels that can be shown and hidden from the Window menu.
    pub(super) const PANELS: [Self; 6] = [
        Self::TextureSetList,
        Self::Layers,
        Self::Properties,
        Self::TextureSetSettings,
        Self::Display,
        Self::Shelf,
    ];

    pub(super) fn title(self) -> &'static str {
        match self {
            Self::Viewport => "Viewport",
            Self::TextureSetList => "Texture set list",
            Self::Layers => "Layers",
            Self::Properties => "Properties",
            Self::TextureSetSettings => "Texture set settings",
            Self::Display => "Display settings",
            Self::Shelf => "Shelf",
        }
    }
}

/// Painter's default arrangement: the viewport in the middle, the texture sets and layers
/// stacked on the right, with the properties and settings as tabs below them.
pub(super) fn default_layout() -> DockState<Tab> {
    let mut state = DockState::new(vec![Tab::Viewport]);
    let tree = state.main_surface_mut();
    let [left, right] = tree.split_right(NodeIndex::root(), 0.72, vec![Tab::TextureSetList]);
    tree.split_below(left, 0.74, vec![Tab::Shelf]);
    let [_, layers] = tree.split_below(right, 0.16, vec![Tab::Layers]);
    tree.split_below(
        layers,
        0.42,
        vec![Tab::Properties, Tab::TextureSetSettings, Tab::Display],
    );
    state
}

/// Reads a saved layout. A layout without the viewport is refused: it would leave nothing
/// to paint in.
fn parse_layout(text: &str) -> Option<DockState<Tab>> {
    // Rectangles that were never laid out are not finite, which JSON writes as null. The dock
    // computes all of them again on the next frame.
    let text = text
        .replace("\"x\":null", "\"x\":0.0")
        .replace("\"y\":null", "\"y\":0.0");
    let state: DockState<Tab> = serde_json::from_str(&text).ok()?;
    state.find_tab(&Tab::Viewport).is_some().then_some(state)
}

fn layout_path() -> Option<PathBuf> {
    config_dir().map(|d| d.join("layout.json"))
}

/// The layout saved by the last session, or the default one.
pub(super) fn load_layout() -> DockState<Tab> {
    layout_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|t| parse_layout(&t))
        .unwrap_or_else(default_layout)
}

fn write_layout(state: &DockState<Tab>, path: &Path) -> anyhow::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, serde_json::to_string_pretty(state)?)?;
    Ok(())
}

struct Viewer<'a> {
    app: &'a mut InkApp,
    rs: &'a RenderState,
}

impl TabViewer for Viewer<'_> {
    type Tab = Tab;

    fn id(&mut self, tab: &mut Tab) -> Id {
        Id::new(("dock tab", *tab))
    }

    fn title(&mut self, tab: &mut Tab) -> WidgetText {
        tab.title().into()
    }

    fn ui(&mut self, ui: &mut egui::Ui, tab: &mut Tab) {
        match tab {
            Tab::Viewport => self.app.views(ui, self.rs),
            Tab::TextureSetList => self.app.texture_set_list(ui),
            Tab::Layers => {
                self.app.layer_stack_panel(ui);
                self.app.shelf_drop_on_layers(ui);
                self.app.layers_hovered = ui.rect_contains_pointer(ui.max_rect());
            }
            Tab::Properties => self.app.properties_section(ui),
            Tab::TextureSetSettings => self.app.texture_set_settings(ui),
            Tab::Display => self.app.display_settings(ui, self.rs),
            Tab::Shelf => self.app.shelf_panel(ui),
        }
    }

    fn is_closeable(&self, tab: &Tab) -> bool {
        *tab != Tab::Viewport
    }

    fn allowed_in_windows(&self, tab: &mut Tab) -> bool {
        *tab != Tab::Viewport
    }

    fn clear_background(&self, tab: &Tab) -> bool {
        *tab != Tab::Viewport
    }

    fn tab_style_override(
        &self,
        tab: &Tab,
        global: &egui_dock::TabStyle,
    ) -> Option<egui_dock::TabStyle> {
        // The viewport runs right up to the edges of its tab.
        (*tab == Tab::Viewport).then(|| {
            let mut style = global.clone();
            style.tab_body.inner_margin = egui::Margin::ZERO;
            style
        })
    }

    fn scroll_bars(&self, tab: &Tab) -> [bool; 2] {
        // The viewport paints its own area; the panels scroll vertically. Not sideways: their
        // content adapts to the width it gets.
        [false, *tab != Tab::Viewport]
    }
}

/// The smallest width or height, in points, a panel can be dragged to.
const MIN_PANEL_SIZE: f32 = 240.0;

impl InkApp {
    /// Shows the dock in the space left by the menu, tool bars and status bar.
    pub(super) fn dock(&mut self, ui: &mut egui::Ui, rs: &RenderState) {
        // The dock state is taken out so that the tabs can borrow the rest of the app.
        let mut state = std::mem::replace(&mut self.dock, DockState::new(Vec::new()));
        let mut style = egui_dock::Style::from_egui(ui.style());
        style.main_surface_border_stroke = egui::Stroke::NONE;
        style.tab_bar.bg_fill = theme::RAISED;
        style.tab_bar.corner_radius = egui::CornerRadius::ZERO;
        style.tab_bar.hline_color = theme::BORDER;
        style.tab.tab_body.inner_margin = egui::Margin::same(6);
        style.tab.tab_body.stroke = egui::Stroke::NONE;
        style.tab.tab_body.bg_fill = theme::PANEL;
        style.tab.tab_body.corner_radius = egui::CornerRadius::ZERO;
        // Panels can't be dragged narrower or lower than this; their content wraps below it.
        style.separator.extra = MIN_PANEL_SIZE;
        style.separator.color_idle = theme::BORDER;
        style.separator.color_hovered = theme::OLIVE;
        style.separator.color_dragged = theme::OLIVE;
        DockArea::new(&mut state)
            .style(style)
            .show_close_buttons(true)
            .show_add_buttons(false)
            .show_leaf_collapse_buttons(false)
            .show_leaf_close_all_buttons(false)
            .show_inside(ui, &mut Viewer { app: self, rs });
        self.dock = state;
    }

    /// The Window menu: which panels are shown, and the layouts.
    pub(super) fn window_menu(&mut self, ui: &mut egui::Ui) {
        for tab in Tab::PANELS {
            let shown = self.dock.find_tab(&tab).is_some();
            if ui.selectable_label(shown, tab.title()).clicked() {
                if let Some(path) = self.dock.find_tab(&tab) {
                    self.dock.remove_tab(path);
                } else {
                    self.dock.push_to_focused_leaf(tab);
                }
                ui.close();
            }
        }
        ui.separator();
        if ui.button("Reset layout").clicked() {
            self.dock = default_layout();
            ui.close();
        }
        if ui.button("Save layout…").clicked() {
            ui.close();
            if let Some(path) = rfd::FileDialog::new()
                .add_filter("inkpainter3d layout", &["json"])
                .set_file_name("layout.json")
                .save_file()
            {
                self.status = match write_layout(&self.dock, &path) {
                    Ok(()) => format!("Layout saved to {}", path.display()),
                    Err(err) => format!("Error: {err:#}"),
                };
            }
        }
        if ui.button("Load layout…").clicked() {
            ui.close();
            if let Some(path) = rfd::FileDialog::new()
                .add_filter("inkpainter3d layout", &["json"])
                .pick_file()
            {
                match std::fs::read_to_string(&path)
                    .ok()
                    .and_then(|t| parse_layout(&t))
                {
                    Some(state) => self.dock = state,
                    None => self.status = format!("Error: not a layout: {}", path.display()),
                }
            }
        }
    }

    /// Remembers the layout for the next start.
    pub(super) fn save_layout(&self) {
        if let Some(path) = layout_path()
            && let Err(err) = write_layout(&self.dock, &path)
        {
            log::warn!("Could not save the layout: {err:#}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layouts_round_trip() {
        let state = default_layout();
        let text = serde_json::to_string(&state).unwrap();
        let back = parse_layout(&text).expect("a saved layout loads");
        for tab in Tab::PANELS {
            assert!(back.find_tab(&tab).is_some(), "{tab:?}");
        }
        assert!(back.find_tab(&Tab::Viewport).is_some());
    }

    #[test]
    fn layouts_without_a_viewport_are_refused() {
        let state = DockState::new(vec![Tab::Layers]);
        let text = serde_json::to_string(&state).unwrap();
        assert!(parse_layout(&text).is_none());
        assert!(parse_layout("not json").is_none());
    }
}
