//! Smart materials and smart masks in the shelf: presets of layers or of a mask, the ones
//! made in code (`ink_io::starter`) and the user's own saved from the layer stack.

use super::{Action, Category, Event, InkApp, cell, library_dir, safe_name};
use eframe::egui::{self, TextureHandle};
use ink_core::Layer;
use ink_io::project::smart::{self, Smart, SmartKind};
use std::path::PathBuf;
use std::sync::Arc;

/// Where a preset comes from.
#[derive(Clone, Debug)]
pub enum SmartRef {
    /// Made in code.
    Builtin(Arc<Smart>),
    File(PathBuf),
}

pub struct SmartItem {
    pub name: String,
    pub kind: SmartKind,
    pub source: SmartRef,
    pub thumb: Option<TextureHandle>,
}

#[derive(Default)]
pub struct SmartShelf {
    pub items: Vec<SmartItem>,
    /// The name typed for saving the selection.
    pub name: String,
}

impl Category {
    /// The kind of preset a smart category lists.
    pub(super) fn smart_kind(self) -> Option<SmartKind> {
        match self {
            Self::SmartMaterials => Some(SmartKind::Material),
            Self::SmartMasks => Some(SmartKind::Mask),
            _ => None,
        }
    }
}

/// A picture of a preset: its color, lit from above, with a darker lower edge.
pub fn swatch_picture(rgb: [u8; 3], mask: bool) -> egui::ColorImage {
    let (w, h) = (96usize, 56usize);
    let mut pixels = Vec::with_capacity(w * h);
    for y in 0..h {
        let t = y as f32 / (h - 1) as f32;
        let shade = 1.12 - 0.42 * t;
        for x in 0..w {
            let mut c = rgb.map(|v| (v as f32 * shade).clamp(0.0, 255.0) as u8);
            if mask {
                // Masks show as a checker of the color and its dark side.
                if ((x / 12) + (y / 12)) % 2 == 0 {
                    c = c.map(|v| v / 3);
                }
            }
            pixels.push(egui::Color32::from_rgb(c[0], c[1], c[2]));
        }
    }
    egui::ColorImage {
        size: [w, h],
        pixels,
        source_size: egui::vec2(w as f32, h as f32),
    }
}

impl InkApp {
    /// Lists the presets: the ones made in code, then the files of the library. Thumbnails of
    /// files are made by a thread.
    pub(super) fn smart_rescan(&mut self, ctx: &egui::Context, generation: u64) {
        let mut items = Vec::new();
        for smart in ink_io::starter::all() {
            let smart = Arc::new(smart);
            let mask = smart.kind == SmartKind::Mask;
            items.push(SmartItem {
                name: smart.name.clone(),
                kind: smart.kind,
                thumb: Some(ctx.load_texture(
                    format!("smart {}", smart.name),
                    swatch_picture(smart.swatch(), mask),
                    egui::TextureOptions::LINEAR,
                )),
                source: SmartRef::Builtin(smart),
            });
        }
        let mut files = Vec::new();
        for category in [Category::SmartMaterials, Category::SmartMasks] {
            let Some(dir) = library_dir(category) else {
                continue;
            };
            for path in super::list_files(&dir, &[smart::EXTENSION]) {
                items.push(SmartItem {
                    name: super::file_stem(&path),
                    kind: category.smart_kind().unwrap_or(SmartKind::Material),
                    source: SmartRef::File(path.clone()),
                    thumb: None,
                });
                files.push(path);
            }
        }
        self.shelf.smart.items = items;
        let (tx, ctx) = (self.shelf.tx.clone(), ctx.clone());
        std::thread::spawn(move || {
            for path in files {
                if let Ok((name, kind, swatch)) = smart::preview(&path) {
                    let _ = tx.send(Event::SmartThumb {
                        generation,
                        path,
                        name,
                        image: swatch_picture(swatch, kind == SmartKind::Mask),
                    });
                    ctx.request_repaint();
                }
            }
        });
    }

    /// Loads a preset's layers (files in a thread) and applies it.
    pub(super) fn smart_apply(&mut self, ctx: &egui::Context, source: SmartRef) {
        match source {
            SmartRef::Builtin(smart) => self.apply_smart(&smart),
            SmartRef::File(path) => {
                self.shelf
                    .jobs
                    .push(format!("Loading {}", super::file_stem(&path)));
                let (tx, ctx) = (self.shelf.tx.clone(), ctx.clone());
                std::thread::spawn(move || {
                    let _ = tx.send(Event::Smart(smart::load(&path)));
                    ctx.request_repaint();
                });
            }
        }
    }

    /// Puts a preset into the layer stack: a material's layers above the selected layer, a
    /// mask on the selected layer (replacing its mask).
    pub(super) fn apply_smart(&mut self, smart: &Smart) {
        match smart.kind {
            SmartKind::Material => {
                let anchor = self.selection().map(|s| s.layer);
                let Some(texture_set) = self.model.texture_sets.get_mut(self.selected_set) else {
                    return;
                };
                let stack = &mut texture_set.stack;
                let mut place = stack.place_above(anchor);
                let mut first = None;
                for layer in &smart.layers {
                    let copy = layer.duplicate(&mut self.model.ids);
                    let id = copy.id;
                    if stack.insert(place, copy).is_ok() {
                        first.get_or_insert(id);
                        place.index += 1;
                    }
                }
                if let Some(id) = first {
                    self.label("Add smart material");
                    self.select(super::super::layers::Selection::layer(id));
                    self.status = format!("Added the smart material {}", smart.name);
                }
            }
            SmartKind::Mask => {
                let Some(layer_id) = self.selection().map(|s| s.layer) else {
                    self.status = "Select a layer to put the mask on".to_owned();
                    return;
                };
                let Some(carrier) = smart.layers.first() else {
                    return;
                };
                let mask = carrier.duplicate(&mut self.model.ids).mask;
                if let Some(layer) = self.stack_mut().and_then(|s| s.find_mut(layer_id)) {
                    layer.mask = mask;
                    self.label("Add smart mask");
                    self.status = format!("Added the smart mask {}", smart.name);
                }
            }
        }
    }

    /// Saves the selected layers as a smart material.
    pub(super) fn save_smart_material(&mut self, ctx: &egui::Context, name: String) {
        let Some(selection) = self.selection().cloned() else {
            self.status = "Select the layers to save".to_owned();
            return;
        };
        let Some(stack) = self.stack() else {
            return;
        };
        let mut layers: Vec<(usize, Layer)> = selection
            .layers()
            .into_iter()
            .filter_map(|id| Some((stack.place(id)?.index, stack.find(id)?.clone())))
            .collect();
        layers.sort_by_key(|(i, _)| *i);
        let layers: Vec<Layer> = layers.into_iter().map(|(_, l)| l).collect();
        if layers.is_empty() {
            return;
        }
        self.write_smart(ctx, Smart::material(name, layers), Category::SmartMaterials);
    }

    /// Saves the mask of the selected layer as a smart mask.
    pub(super) fn save_smart_mask(&mut self, ctx: &egui::Context, name: String) {
        let Some(id) = self.selection().map(|s| s.layer) else {
            self.status = "Select a layer with a mask".to_owned();
            return;
        };
        let Some(mask) = self
            .stack()
            .and_then(|s| s.find(id))
            .and_then(|l| l.mask.clone())
        else {
            self.status = "The selected layer has no mask".to_owned();
            return;
        };
        let smart = Smart::mask(name, mask, &mut self.model.ids);
        self.write_smart(ctx, smart, Category::SmartMasks);
    }

    fn write_smart(&mut self, ctx: &egui::Context, smart: Smart, category: Category) {
        let Some(dir) = library_dir(category) else {
            return;
        };
        let path = dir.join(format!("{}.{}", safe_name(&smart.name), smart::EXTENSION));
        let result = std::fs::create_dir_all(&dir)
            .map_err(anyhow::Error::from)
            .and_then(|()| smart::save(&path, &smart));
        match result {
            Ok(()) => {
                self.status = format!("Saved {} to the shelf", smart.name);
                self.shelf_rescan(ctx);
            }
            Err(error) => self.status = format!("Failed to save: {error:#}"),
        }
    }

    /// The presets of one kind as a grid of cells.
    pub(super) fn smart_grid(
        &mut self,
        ui: &mut egui::Ui,
        kind: SmartKind,
        filter: &str,
        actions: &mut Vec<Action>,
    ) {
        let category = match kind {
            SmartKind::Material => Category::SmartMaterials,
            SmartKind::Mask => Category::SmartMasks,
        };
        ui.horizontal_wrapped(|ui| {
            for item in self.shelf.smart.items.iter().filter(|i| i.kind == kind) {
                if !filter.is_empty() && !item.name.to_lowercase().contains(filter) {
                    continue;
                }
                let response = cell(ui, &item.name, item.thumb.as_ref(), false, false);
                response.dnd_set_drag_payload(super::ShelfDrag::Smart(item.source.clone()));
                if response.clicked() {
                    actions.push(Action::ApplySmart(item.source.clone()));
                }
                let tip = match kind {
                    SmartKind::Material => "Click to add its layers above the selected layer.",
                    SmartKind::Mask => "Click to put it on the selected layer.",
                };
                response.clone().on_hover_text(tip);
                if let SmartRef::File(path) = &item.source {
                    response.context_menu(|ui| {
                        if ui.button("Delete").clicked() {
                            actions.push(Action::Delete(category, path.clone()));
                            ui.close();
                        }
                    });
                }
            }
        });
    }

    /// The toolbar of the smart categories: saving the selection.
    pub(super) fn smart_toolbar(
        &mut self,
        ui: &mut egui::Ui,
        kind: SmartKind,
        actions: &mut Vec<Action>,
    ) {
        let hint = match kind {
            SmartKind::Material => "Name for the selected layers",
            SmartKind::Mask => "Name for the selected layer's mask",
        };
        ui.add(
            egui::TextEdit::singleline(&mut self.shelf.smart.name)
                .hint_text(hint)
                .desired_width(super::field_width(ui, 190.0)),
        );
        let ready = !self.shelf.smart.name.trim().is_empty();
        let label = match kind {
            SmartKind::Material => "Save layers",
            SmartKind::Mask => "Save mask",
        };
        if ui.add_enabled(ready, egui::Button::new(label)).clicked() {
            actions.push(Action::SaveSmart(
                kind,
                self.shelf.smart.name.trim().to_owned(),
            ));
            self.shelf.smart.name.clear();
        }
    }
}
