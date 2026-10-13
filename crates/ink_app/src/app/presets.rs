//! Brush presets: the ones that ship with the app and the user's own, saved as
//! `.inkbrush` files in the library folder (`<data dir>/shelf/brushes`).

use super::InkApp;
use super::project::data_dir;
use crate::theme::{self, icons};
use eframe::egui;
use ink_core::{Alignment, Alpha, AlphaImage, Brush, Jitter, Rng, SizeSpace};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// A named brush.
#[derive(Clone)]
pub struct Preset {
    pub name: String,
    pub brush: Brush,
    /// The file of a user preset; None for the presets that ship with the app.
    pub file: Option<PathBuf>,
}

/// Where the user's brush presets live.
pub fn user_dir() -> Option<PathBuf> {
    data_dir().map(|d| d.join("shelf").join("brushes"))
}

/// The presets that ship with the app, followed by the user's.
pub fn load_all() -> Vec<Preset> {
    let mut presets = builtin();
    if let Some(dir) = user_dir() {
        presets.extend(load_dir(&dir));
    }
    presets
}

fn load_dir(dir: &Path) -> Vec<Preset> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = entries
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.extension()
                .is_some_and(|e| e == ink_io::preset::EXTENSION)
        })
        .collect();
    files.sort();
    files
        .into_iter()
        .filter_map(|file| {
            let (name, brush) = ink_io::preset::load(&file).ok()?;
            Some(Preset {
                name,
                brush,
                file: Some(file),
            })
        })
        .collect()
}

/// A rough grain: a soft disc broken up by noise, made here so no outside image ships.
fn chalk_alpha() -> Arc<AlphaImage> {
    const N: u32 = 128;
    let mut rng = Rng::new(0xC4A1C);
    let noise: Vec<f32> = (0..N * N).map(|_| rng.next_f32()).collect();
    let at = |x: i32, y: i32| {
        noise[((y.rem_euclid(N as i32)) as u32 * N + x.rem_euclid(N as i32) as u32) as usize]
    };
    let mut pixels = Vec::with_capacity((N * N) as usize);
    for y in 0..N as i32 {
        for x in 0..N as i32 {
            // Blur the noise a little so the grain has some size.
            let mut grain = 0.0;
            for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1), (-1, 0), (0, -1)] {
                grain += at(x + dx, y + dy);
            }
            grain /= 6.0;
            let fx = (x as f32 + 0.5) / N as f32 * 2.0 - 1.0;
            let fy = (y as f32 + 0.5) / N as f32 * 2.0 - 1.0;
            let disc = (1.0 - (fx * fx + fy * fy).sqrt()).clamp(0.0, 1.0);
            let v = ((grain - 0.38) * 6.0).clamp(0.0, 1.0) * (disc * 3.0).min(1.0);
            pixels.push((v * 255.0) as u8);
        }
    }
    Arc::new(AlphaImage::new("Chalk grain".into(), N, N, pixels))
}

fn builtin() -> Vec<Preset> {
    let named = |name: &str, brush: Brush| Preset {
        name: name.to_owned(),
        brush,
        file: None,
    };
    vec![
        named("Soft round", Brush::default()),
        named(
            "Hard round",
            Brush {
                hardness: 1.0,
                ..Brush::default()
            },
        ),
        named(
            "Airbrush",
            Brush {
                flow: 0.12,
                spacing: 0.08,
                ..Brush::default()
            },
        ),
        named(
            "Marker",
            Brush {
                alpha: Alpha::Square,
                hardness: 1.0,
                angle: 45.0,
                follow_path: true,
                spacing: 0.05,
                ..Brush::default()
            },
        ),
        named(
            "Chalk",
            Brush {
                alpha: Alpha::Image(chalk_alpha()),
                flow: 0.7,
                spacing: 0.12,
                jitter: Jitter {
                    angle: 1.0,
                    ..Jitter::default()
                },
                ..Brush::default()
            },
        ),
        named(
            "Scatter dots",
            Brush {
                size: 3.0,
                hardness: 1.0,
                spacing: 2.5,
                jitter: Jitter {
                    size: 0.6,
                    flow: 0.3,
                    angle: 0.0,
                    position: 1.5,
                },
                ..Brush::default()
            },
        ),
        named(
            "Decal stamp",
            Brush {
                size: 25.0,
                hardness: 1.0,
                spacing: 10.0,
                alignment: Alignment::Camera,
                size_space: SizeSpace::Object,
                ..Brush::default()
            },
        ),
    ]
}

/// File name for a preset called `name`.
fn file_name(name: &str) -> String {
    let stem: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    format!("{stem}.{}", ink_io::preset::EXTENSION)
}

impl InkApp {
    /// The Preset group at the top of the Properties panel.
    pub(super) fn preset_group(&mut self, ui: &mut egui::Ui) {
        let mut apply = None;
        let mut save = false;
        let mut delete = None;
        theme::group(ui, "Preset", true, |ui| {
            theme::property_grid("preset").show(ui, |ui| {
                ui.label("Brush");
                ui.horizontal(|ui| {
                    egui::ComboBox::from_id_salt("brush preset")
                        .selected_text(self.preset_name.clone())
                        .width(120.0)
                        .show_ui(ui, |ui| {
                            for (i, preset) in self.presets.iter().enumerate() {
                                if ui.selectable_label(false, &preset.name).clicked() {
                                    apply = Some(i);
                                }
                            }
                        });
                    if let Some(i) = self
                        .presets
                        .iter()
                        .position(|p| p.name == self.preset_name && p.file.is_some())
                        && theme::icon_button(ui, icons::DELETE, "Delete this preset").clicked()
                    {
                        delete = Some(i);
                    }
                });
                ui.end_row();
                ui.label("Save as");
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.preset_draft)
                            .hint_text("Name")
                            .desired_width(120.0),
                    );
                    let ready = !self.preset_draft.trim().is_empty();
                    save = ui.add_enabled(ready, egui::Button::new("Save")).clicked();
                });
                ui.end_row();
            });
        });
        if let Some(i) = apply {
            let preset = self.presets[i].clone();
            // Keep the channels and values: they are the material, not the brush.
            let material = self.paint.brush().material;
            let brush = self.paint.brush_mut();
            *brush = preset.brush;
            brush.material = material;
            self.preset_name = preset.name;
        }
        if save {
            self.save_preset();
        }
        if let Some(i) = delete {
            if let Some(file) = self.presets[i].file.clone() {
                let _ = std::fs::remove_file(file);
            }
            self.presets.remove(i);
            self.preset_name = "Custom".to_owned();
        }
    }

    fn save_preset(&mut self) {
        let name = self.preset_draft.trim().to_owned();
        let Some(dir) = user_dir() else {
            self.status = "No place to keep presets".to_owned();
            return;
        };
        let file = dir.join(file_name(&name));
        let result = std::fs::create_dir_all(&dir)
            .map_err(anyhow::Error::from)
            .and_then(|()| ink_io::preset::save(&file, &name, self.paint.brush()));
        match result {
            Ok(()) => {
                let preset = Preset {
                    name: name.clone(),
                    brush: self.paint.brush().clone(),
                    file: Some(file),
                };
                match self.presets.iter().position(|p| p.file == preset.file) {
                    Some(i) => self.presets[i] = preset,
                    None => self.presets.push(preset),
                }
                self.status = format!("Saved the preset {name}");
                self.preset_name = name;
                self.preset_draft.clear();
            }
            Err(error) => self.status = format!("Failed to save the preset: {error:#}"),
        }
    }
}
