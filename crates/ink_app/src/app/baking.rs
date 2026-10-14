//! The baking window: bakes the mesh maps of texture sets (ambient occlusion, curvature,
//! position, thickness, world normal, and from high-poly meshes normal, height and ID) on the
//! GPU in a background thread, and keeps them with the texture sets.

use super::InkApp;
use crate::theme;
use eframe::egui::{self, Key, RichText};
use eframe::egui_wgpu::RenderState;
use ink_core::{BakeSettings, Channel, Fill, HighPoly, IdSource, Matching, MeshMap, Model, Texels};
use ink_render::bake::{BakeJob, Baker};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{Receiver, channel};

/// The maps a bake produced, with the index of their texture set.
type Baked = Vec<(usize, MeshMap, ink_core::TextureImage)>;

/// A bake running in a thread.
struct Job {
    /// Thousandths done.
    progress: Arc<AtomicU32>,
    cancel: Arc<AtomicBool>,
    done: Receiver<Result<Baked, String>>,
    /// Which texture sets are baked, for the message at the end.
    sets: usize,
    /// The model the bake started from; a bake for another model is dropped.
    generation: u64,
}

#[derive(Default)]
pub struct BakingState {
    pub open: bool,
    /// Texture sets to bake; indices follow the model's.
    selected: Vec<bool>,
    job: Option<Job>,
    /// Incremented when the model is replaced.
    pub generation: u64,
    /// Thumbnails of the baked maps of the selected set, by map.
    previews: Vec<(MeshMap, egui::TextureHandle)>,
    preview_set: Option<(usize, u64)>,
    error: Option<String>,
}

impl BakingState {
    /// The model was replaced: a running bake is not wanted any more.
    pub fn reset(&mut self) {
        if let Some(job) = &self.job {
            job.cancel.store(true, Ordering::Relaxed);
        }
        self.job = None;
        self.generation += 1;
        self.selected.clear();
        self.previews.clear();
        self.preview_set = None;
        self.error = None;
    }
}

impl InkApp {
    pub(super) fn baking_shortcut(&mut self, ui: &egui::Ui) {
        let open = ui.input(|i| i.modifiers.command && i.modifiers.shift && i.key_pressed(Key::B));
        if open && !ui.ctx().text_edit_focused() {
            self.baking.open = true;
        }
    }

    /// Starts baking the selected texture sets.
    fn start_bake(&mut self, rs: &RenderState) {
        let sets: Vec<usize> = (0..self.model.texture_sets.len())
            .filter(|&i| self.baking.selected.get(i).copied().unwrap_or(false))
            .collect();
        if sets.is_empty() {
            return;
        }
        let model = self.model.clone();
        let settings = self.project.bake.clone();
        let (device, queue) = (rs.device.clone(), rs.queue.clone());
        let (tx, rx) = channel();
        let progress = Arc::new(AtomicU32::new(0));
        let cancel = Arc::new(AtomicBool::new(false));
        let (p, c) = (progress.clone(), cancel.clone());
        let count = sets.len();
        std::thread::spawn(move || {
            let result = bake_sets(&device, &queue, &model, &settings, &sets, &p, &c);
            let _ = tx.send(result);
        });
        self.baking.error = None;
        self.baking.job = Some(Job {
            progress,
            cancel,
            done: rx,
            sets: count,
            generation: self.baking.generation,
        });
    }

    /// Takes a finished bake's maps.
    fn poll_bake(&mut self, ctx: &egui::Context) {
        let Some(job) = &self.baking.job else {
            return;
        };
        match job.done.try_recv() {
            Ok(result) => {
                let (generation, sets) = (job.generation, job.sets);
                self.baking.job = None;
                if generation != self.baking.generation {
                    return;
                }
                match result {
                    Ok(maps) => {
                        let count = maps.len();
                        for (set, map, image) in maps {
                            if let Some(set) = self.model.texture_sets.get_mut(set) {
                                set.mesh_maps.insert(map, Arc::new(image));
                            }
                        }
                        self.project.bakes += 1;
                        self.baking.preview_set = None;
                        self.status = format!("Baked {count} mesh maps for {sets} texture set(s)");
                    }
                    Err(error) => {
                        self.status = format!("Baking: {error}");
                        self.baking.error = (error != "Cancelled").then_some(error);
                    }
                }
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                ctx.request_repaint_after(std::time::Duration::from_millis(100));
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.baking.job = None;
                self.baking.error = Some("The bake stopped unexpectedly".to_owned());
            }
        }
    }

    /// Makes thumbnails of the selected set's maps when they changed.
    fn baking_previews(&mut self, ctx: &egui::Context) {
        let key = (self.selected_set, self.project.bakes);
        if self.baking.preview_set == Some(key) {
            return;
        }
        self.baking.preview_set = Some(key);
        self.baking.previews.clear();
        let Some(set) = self.model.texture_sets.get(self.selected_set) else {
            return;
        };
        for (map, image) in &set.mesh_maps.maps {
            let Texels::Rgba16(texels) = &image.texels else {
                continue;
            };
            // Every n-th texel is enough for a thumbnail.
            let side = 96usize;
            let step = (image.width as usize / side).max(1);
            let mut pixels = Vec::with_capacity(side * side);
            for y in 0..side.min(image.height as usize / step) {
                for x in 0..side.min(image.width as usize / step) {
                    let i = ((y * step) * image.width as usize + x * step) * 4;
                    pixels.push(egui::Color32::from_rgb(
                        (texels[i] >> 8) as u8,
                        (texels[i + 1] >> 8) as u8,
                        (texels[i + 2] >> 8) as u8,
                    ));
                }
            }
            let w = side.min(image.width as usize / step);
            let h = side.min(image.height as usize / step);
            let color = egui::ColorImage {
                size: [w, h],
                pixels,
                source_size: egui::vec2(w as f32, h as f32),
            };
            self.baking.previews.push((
                *map,
                ctx.load_texture(
                    format!("mesh map {}", map.key()),
                    color,
                    egui::TextureOptions::LINEAR,
                ),
            ));
        }
    }

    /// The baking window.
    pub(super) fn baking_window(&mut self, ui: &mut egui::Ui, rs: &RenderState) {
        if !self.baking.open {
            return;
        }
        self.poll_bake(ui.ctx());
        let sets = self.model.texture_sets.len();
        if self.baking.selected.len() != sets {
            self.baking.selected = vec![true; sets];
        }
        self.baking_previews(ui.ctx());
        let mut open = true;
        let mut start = false;
        let mut layer_for: Option<MeshMap> = None;
        egui::Window::new("Bake mesh maps")
            .open(&mut open)
            .default_width(440.0)
            .resizable(true)
            .show(ui.ctx(), |ui| {
                let running = self.baking.job.is_some();
                // The settings scroll; the button and the results stay in view below them.
                let height = (ui.ctx().content_rect().height() - 330.0).max(200.0);
                egui::ScrollArea::vertical()
                    .max_height(height)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        ui.add_enabled_ui(!running, |ui| self.baking_settings(ui));
                    });
                ui.separator();
                self.baking_footer(ui, &mut start, &mut layer_for);
            });
        self.baking.open = open;
        if start {
            self.start_bake(rs);
        }
        self.baking_layer(layer_for);
    }

    /// The Bake button or the progress, and the maps baked so far.
    fn baking_footer(
        &mut self,
        ui: &mut egui::Ui,
        start: &mut bool,
        layer_for: &mut Option<MeshMap>,
    ) {
        match &self.baking.job {
            Some(job) => {
                let fraction = job.progress.load(Ordering::Relaxed) as f32 / 1000.0;
                ui.add(
                    egui::ProgressBar::new(fraction)
                        .show_percentage()
                        .desired_width(300.0),
                );
                if ui.button("Cancel").clicked() {
                    job.cancel.store(true, Ordering::Relaxed);
                }
            }
            None => {
                let any = self.baking.selected.iter().any(|s| *s);
                if ui.add_enabled(any, theme::primary_button("Bake")).clicked() {
                    *start = true;
                }
                if let Some(error) = &self.baking.error {
                    ui.label(RichText::new(error).color(theme::DANGER));
                }
            }
        }
        if !self.baking.previews.is_empty() {
            ui.separator();
            theme::group_label(ui, "Baked maps of the selected texture set");
            ui.horizontal_wrapped(|ui| {
                for (map, texture) in &self.baking.previews {
                    ui.vertical(|ui| {
                        ui.add(egui::Image::from_texture(egui::load::SizedTexture::new(
                            texture.id(),
                            egui::vec2(72.0, 72.0),
                        )));
                        ui.label(RichText::new(map.label()).small());
                        if ui
                            .small_button("Add as layer")
                            .on_hover_text("Shows the map as a fill layer")
                            .clicked()
                        {
                            *layer_for = Some(*map);
                        }
                    });
                }
            });
        }
    }

    fn baking_layer(&mut self, layer_for: Option<MeshMap>) {
        if let Some(map) = layer_for
            && let Some(image) = self
                .model
                .texture_sets
                .get(self.selected_set)
                .and_then(|s| s.mesh_maps.get(map))
                .cloned()
        {
            let mut fill = Fill {
                channels: ink_core::ChannelSet::NONE,
                ..Fill::default()
            };
            let channel = if map == MeshMap::Normal {
                fill.normal_directx = false;
                Channel::Normal
            } else {
                Channel::BaseColor
            };
            fill.channels.set(channel, true);
            fill.images[channel.index()] = Some(image);
            self.add_fill_layer_with(map.label().to_owned(), fill);
        }
    }

    fn baking_settings(&mut self, ui: &mut egui::Ui) {
        let sets: Vec<(String, u32)> = self
            .model
            .texture_sets
            .iter()
            .map(|s| (s.name.clone(), s.resolution))
            .collect();
        theme::group_label(ui, "Texture sets");
        ui.horizontal_wrapped(|ui| {
            for (i, (name, resolution)) in sets.iter().enumerate() {
                if let Some(selected) = self.baking.selected.get_mut(i) {
                    ui.checkbox(selected, format!("{name} ({resolution})"));
                }
            }
        });
        ui.label(
            RichText::new("The size of the maps is the texture set's size.")
                .small()
                .color(theme::TEXT_WEAK),
        );

        let settings = &mut self.project.bake;
        theme::group_label(ui, "High-poly meshes");
        let mut remove = None;
        for (i, path) in settings.high_poly.iter().enumerate() {
            ui.horizontal(|ui| {
                ui.label(path.file_name().map_or_else(
                    || path.display().to_string(),
                    |n| n.to_string_lossy().into_owned(),
                ));
                if theme::icon_button(ui, theme::icons::DELETE, "Remove").clicked() {
                    remove = Some(i);
                }
            });
        }
        if let Some(i) = remove {
            settings.high_poly.remove(i);
        }
        if ui.button("Add high-poly…").clicked()
            && let Some(files) = rfd::FileDialog::new()
                .add_filter("Meshes", ink_io::SUPPORTED_EXTENSIONS)
                .pick_files()
        {
            for file in files {
                if !settings.high_poly.contains(&file) {
                    settings.high_poly.push(file);
                }
            }
        }

        theme::group_label(ui, "Settings");
        theme::property_grid(ui, "bake settings").show(ui, |ui| {
            ui.label("Antialiasing");
            egui::ComboBox::from_id_salt("bake subsampling")
                .selected_text(format!("{0} × {0}", settings.subsampling))
                .show_ui(ui, |ui| {
                    for n in [1, 2, 4] {
                        ui.selectable_value(&mut settings.subsampling, n, format!("{n} × {n}"));
                    }
                });
            ui.end_row();
            ui.label("Dilation");
            ui.add(
                egui::DragValue::new(&mut settings.dilation)
                    .range(0..=256)
                    .suffix(" px"),
            );
            ui.end_row();
            ui.label("Match meshes");
            egui::ComboBox::from_id_salt("bake matching")
                .selected_text(settings.matching.label())
                .show_ui(ui, |ui| {
                    for m in Matching::ALL {
                        ui.selectable_value(&mut settings.matching, m, m.label());
                    }
                })
                .response
                .on_hover_text("By mesh name pairs name_low with name_high");
            ui.end_row();
            ui.label("Max frontal distance");
            ui.add(
                egui::Slider::new(&mut settings.front_distance, 0.0..=0.5)
                    .custom_formatter(|v, _| format!("{:.1} %", v * 100.0))
                    .custom_parser(|s| {
                        s.trim_end_matches('%')
                            .trim()
                            .parse::<f64>()
                            .ok()
                            .map(|v| v / 100.0)
                    }),
            );
            ui.end_row();
            ui.label("Max rear distance");
            ui.add(
                egui::Slider::new(&mut settings.rear_distance, 0.0..=0.5)
                    .custom_formatter(|v, _| format!("{:.1} %", v * 100.0))
                    .custom_parser(|s| {
                        s.trim_end_matches('%')
                            .trim()
                            .parse::<f64>()
                            .ok()
                            .map(|v| v / 100.0)
                    }),
            );
            ui.end_row();
            ui.label("Average normals");
            ui.checkbox(&mut settings.average_normals, "");
            ui.end_row();
            ui.label("Ignore backfaces");
            ui.checkbox(&mut settings.ignore_backfaces, "");
            ui.end_row();
        });
        ui.label(
            RichText::new("Distances are percent of the model's size.")
                .small()
                .color(theme::TEXT_WEAK),
        );

        theme::group_label(ui, "Maps");
        let has_high = !settings.high_poly.is_empty();
        for map in MeshMap::ALL {
            ui.horizontal(|ui| {
                let usable = !map.needs_high_poly() || has_high;
                let mut on = settings.is_on(map) && usable;
                ui.add_enabled_ui(usable, |ui| {
                    if ui.checkbox(&mut on, map.label()).changed() {
                        settings.set(map, on);
                    }
                });
                if !usable {
                    ui.label(
                        RichText::new("needs a high-poly")
                            .small()
                            .color(theme::TEXT_WEAK),
                    );
                }
            });
            if !settings.is_on(map) {
                continue;
            }
            ui.indent(map.key(), |ui| match map {
                MeshMap::AmbientOcclusion => {
                    ui.horizontal(|ui| {
                        ui.label("Rays");
                        ui.add(egui::DragValue::new(&mut settings.ao_rays).range(1..=1024));
                        ui.label("Distance");
                        percent_drag(ui, &mut settings.ao_distance);
                        ui.checkbox(&mut settings.ao_self_occlusion, "Self occlusion");
                    });
                }
                MeshMap::Thickness => {
                    ui.horizontal(|ui| {
                        ui.label("Rays");
                        ui.add(egui::DragValue::new(&mut settings.thickness_rays).range(1..=1024));
                        ui.label("Distance");
                        percent_drag(ui, &mut settings.thickness_distance);
                    });
                }
                MeshMap::Curvature => {
                    ui.horizontal(|ui| {
                        ui.label("Radius");
                        percent_drag(ui, &mut settings.curvature_radius);
                    });
                }
                MeshMap::Id => {
                    ui.horizontal(|ui| {
                        ui.label("Color by");
                        egui::ComboBox::from_id_salt("bake id source")
                            .selected_text(settings.id_source.label())
                            .show_ui(ui, |ui| {
                                for s in IdSource::ALL {
                                    ui.selectable_value(&mut settings.id_source, s, s.label());
                                }
                            });
                    });
                }
                _ => {}
            });
        }
    }
}

/// A percent value of a 0 to 1 fraction.
fn percent_drag(ui: &mut egui::Ui, value: &mut f32) {
    let mut percent = *value * 100.0;
    if ui
        .add(
            egui::DragValue::new(&mut percent)
                .range(0.01..=100.0)
                .speed(0.05)
                .suffix(" %")
                .max_decimals(2),
        )
        .changed()
    {
        *value = percent / 100.0;
    }
}

/// Bakes the texture sets one after the other. Runs in a thread.
fn bake_sets(
    device: &eframe::egui_wgpu::wgpu::Device,
    queue: &eframe::egui_wgpu::wgpu::Queue,
    model: &Model,
    settings: &BakeSettings,
    sets: &[usize],
    progress: &AtomicU32,
    cancel: &AtomicBool,
) -> Result<Baked, String> {
    let high = if settings.high_poly.is_empty() {
        None
    } else {
        let mut meshes = Vec::new();
        for path in &settings.high_poly {
            let loaded = ink_io::load_model(path).map_err(|e| format!("{e:#}"))?;
            meshes.push(HighPoly::from_model(&loaded));
        }
        Some(HighPoly::merge(meshes))
    };
    let baker = Baker::new(device);
    let mut all = Vec::new();
    for (n, &set) in sets.iter().enumerate() {
        let maps = baker.bake(
            device,
            queue,
            &BakeJob {
                model,
                set,
                settings,
                high: high.as_ref(),
            },
            &mut |fraction| {
                let total = (n as f32 + fraction) / sets.len() as f32;
                progress.store((total * 1000.0) as u32, Ordering::Relaxed);
                !cancel.load(Ordering::Relaxed)
            },
        )?;
        all.extend(maps.into_iter().map(|(map, image)| (set, map, image)));
    }
    Ok(all)
}
