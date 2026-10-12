//! The Export textures window (Ctrl+Shift+E), as Painter's: global settings, the texture
//! sets to export, the files each one gets, and the list of exports.
//!
//! The GPU composites a few blocks per frame; a thread per texture set packs the texels
//! into the output maps as they arrive and writes the files, while the next texture set is
//! composited.

use super::InkApp;
use eframe::egui::{self, Color32, Key, RichText};
use eframe::egui_wgpu::RenderState;
use ink_core::NormalFormat;
use ink_core::export::{
    ExportBlock, ExportSettings, FileFormat, MapImage, Padding, TEMPLATES, Template, file_name,
};
use ink_io::export::MapFile;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread::JoinHandle;

/// Composited blocks waiting to be packed; the GPU waits for the packer beyond this.
const BLOCKS_IN_FLIGHT: usize = 4;
/// Blocks (512² texels) composited per frame, so the window stays responsive.
const BLOCKS_PER_FRAME: usize = 4;
/// Texture sets packed or written at once, for the memory their maps take.
const SETS_IN_FLIGHT: usize = 2;

/// What the packer of a texture set gets.
enum Packing {
    Block(ExportBlock),
    /// Every block was sent: the maps are complete and can be written.
    Complete,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tab {
    Settings,
    Exports,
}

pub(super) struct ExportWindow {
    settings: ExportSettings,
    /// Texture sets to export.
    sets: Vec<bool>,
    /// The texture set whose files are listed.
    preview: usize,
    tab: Tab,
    job: Option<ExportJob>,
    /// The folder of the last export.
    directory: Option<PathBuf>,
    /// Files written, or why not.
    log: Vec<(String, Result<(), String>)>,
}

struct ExportJob {
    directory: PathBuf,
    settings: ExportSettings,
    /// The files of each texture set to export, by texture set.
    paths: Vec<Vec<PathBuf>>,
    /// Texture sets still to composite.
    todo: VecDeque<usize>,
    /// Texture sets in the export, and those composited (or given up) so far.
    sets: usize,
    sets_done: usize,
    /// The texture set being composited.
    current: Option<SetExport>,
    /// Write the texture sets composited before.
    writers: Vec<JoinHandle<()>>,
    results: mpsc::Receiver<(String, Result<(), String>)>,
    sender: mpsc::Sender<(String, Result<(), String>)>,
    files: usize,
    done: usize,
    failed: usize,
}

impl ExportJob {
    /// Stops compositing; what is being written is finished.
    fn stop(&mut self) {
        self.todo.clear();
        if let Some(current) = self.current.take() {
            // Without the last message the packer writes nothing.
            drop(current.packer);
            self.writers.push(current.worker);
            let _ = self.sender.send(("Export".into(), Err("Stopped".into())));
        }
    }

    /// Waits for everything being written.
    fn finish(&mut self) {
        for writer in self.writers.drain(..) {
            let _ = writer.join();
        }
    }
}

/// A texture set on its way from the GPU to its packer.
struct SetExport {
    set: usize,
    /// The next block to composite, of `blocks`.
    next: usize,
    blocks: usize,
    /// Its file names, to report a failure.
    files: Vec<String>,
    packer: mpsc::SyncSender<Packing>,
    worker: JoinHandle<()>,
}

impl InkApp {
    /// Opens the export window with the project's settings.
    pub(super) fn open_export_window(&mut self) {
        if self.model.texture_sets.is_empty() {
            return;
        }
        let mut settings = self.project.export.clone();
        if settings.directory.is_empty() {
            settings.directory = self.default_export_directory();
        }
        self.export = Some(ExportWindow {
            settings,
            sets: vec![true; self.model.texture_sets.len()],
            preview: self.selected_set.min(self.model.texture_sets.len() - 1),
            tab: Tab::Settings,
            job: None,
            directory: None,
            log: Vec::new(),
        });
    }

    /// A folder next to the project or the mesh it was made from.
    fn default_export_directory(&self) -> String {
        let base = self
            .project
            .path
            .as_deref()
            .or(self.project.source.as_deref().map(Path::new))
            .and_then(Path::parent)
            .map(Path::to_owned)
            .or_else(|| std::env::current_dir().ok());
        base.map(|b| b.join("textures").display().to_string())
            .unwrap_or_default()
    }

    pub(super) fn export_shortcut(&mut self, ui: &egui::Ui) {
        let open = ui.input(|i| i.modifiers.command && i.modifiers.shift && i.key_pressed(Key::E));
        // Not during a stroke or a drag: they would change the model under the export.
        let busy = self.paint.is_busy() || ui.input(|i| i.pointer.any_down());
        if open && self.export.is_none() && !busy && !ui.ctx().text_edit_focused() {
            self.open_export_window();
        }
    }

    /// Stops a running export and waits for the files being written, e.g. before the
    /// window closes.
    pub(super) fn stop_export(&mut self) {
        if let Some(job) = self.export.as_mut().and_then(|w| w.job.as_mut()) {
            job.stop();
            job.finish();
        }
    }

    /// Shows the export window while it is open and runs the export.
    pub(super) fn export_window(&mut self, ui: &mut egui::Ui, rs: &RenderState) {
        let Some(mut window) = self.export.take() else {
            return;
        };
        self.run_export(&mut window, rs);
        if window.job.is_some() {
            ui.ctx().request_repaint();
        }
        let mut close = false;
        let modal = egui::Modal::new(egui::Id::new("export textures")).show(ui.ctx(), |ui| {
            ui.set_width(720.0);
            ui.heading("Export textures");
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.selectable_value(&mut window.tab, Tab::Settings, "Export settings");
                ui.selectable_value(&mut window.tab, Tab::Exports, "List of exports");
            });
            ui.separator();
            match window.tab {
                Tab::Settings => self.export_settings_tab(ui, &mut window),
                Tab::Exports => export_list_tab(ui, &window),
            }
            ui.separator();
            ui.horizontal(|ui| {
                let running = window.job.is_some();
                ui.add_enabled_ui(!running, |ui| {
                    if ui.button("Save settings").clicked() {
                        self.project.export = window.settings.clone();
                        close = true;
                    }
                    if ui.button("Cancel").clicked() {
                        close = true;
                    }
                    let any = window.sets.iter().any(|&s| s);
                    let has_directory = !window.settings.directory.trim().is_empty();
                    if ui
                        .add_enabled(any && has_directory, egui::Button::new("Export"))
                        .clicked()
                    {
                        self.project.export = window.settings.clone();
                        self.start_export(&mut window);
                    }
                });
                if running {
                    if ui.button("Stop").clicked()
                        && let Some(job) = &mut window.job
                    {
                        job.stop();
                    }
                    ui.spinner();
                }
            });
        });
        let escape = modal.should_close() && window.job.is_none();
        if !close && !escape {
            self.export = Some(window);
        }
    }

    fn export_settings_tab(&mut self, ui: &mut egui::Ui, window: &mut ExportWindow) {
        let settings = &mut window.settings;
        ui.label(RichText::new("GLOBAL SETTINGS").small().strong());
        egui::Grid::new("export settings")
            .num_columns(2)
            .spacing([12.0, 6.0])
            .show(ui, |ui| {
                ui.label("Output directory");
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut settings.directory).desired_width(440.0),
                    );
                    if ui.button("…").clicked() {
                        let mut dialog = rfd::FileDialog::new();
                        if Path::new(&settings.directory).is_dir() {
                            dialog = dialog.set_directory(&settings.directory);
                        }
                        if let Some(dir) = dialog.pick_folder() {
                            settings.directory = dir.display().to_string();
                        }
                    }
                });
                ui.end_row();
                ui.label("Output template");
                egui::ComboBox::from_id_salt("export template")
                    .width(320.0)
                    .selected_text(settings.template.label)
                    .show_ui(ui, |ui| {
                        for template in TEMPLATES {
                            if ui
                                .selectable_label(
                                    std::ptr::eq(settings.template, template),
                                    template.label,
                                )
                                .clicked()
                            {
                                settings.template = template;
                            }
                        }
                    });
                ui.end_row();
                ui.label("File type");
                ui.horizontal(|ui| {
                    egui::ComboBox::from_id_salt("export format")
                        .selected_text(settings.format.label())
                        .show_ui(ui, |ui| {
                            for format in FileFormat::ALL {
                                ui.selectable_value(&mut settings.format, format, format.label());
                            }
                        });
                    if !settings.format.bit_depths().contains(&settings.bit_depth) {
                        settings.bit_depth = settings.format.bit_depths()[0];
                    }
                    egui::ComboBox::from_id_salt("export depth")
                        .selected_text(settings.bit_depth.label())
                        .show_ui(ui, |ui| {
                            for &depth in settings.format.bit_depths() {
                                ui.selectable_value(&mut settings.bit_depth, depth, depth.label());
                            }
                        });
                });
                ui.end_row();
                ui.label("Size");
                ui.label("Based on each texture set's size");
                ui.end_row();
                ui.label("Padding");
                ui.horizontal(|ui| {
                    egui::ComboBox::from_id_salt("export padding")
                        .width(260.0)
                        .selected_text(settings.padding.label())
                        .show_ui(ui, |ui| {
                            for padding in Padding::ALL {
                                ui.selectable_value(
                                    &mut settings.padding,
                                    padding,
                                    padding.label(),
                                );
                            }
                        });
                    if settings.padding.has_distance() {
                        ui.label("Dilation distance");
                        ui.add(
                            egui::DragValue::new(&mut settings.dilation)
                                .range(0..=ExportSettings::MAX_DILATION)
                                .suffix(" px"),
                        );
                    }
                });
                ui.end_row();
            });

        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new("TEXTURE SETS").small().strong());
            ui.menu_button("☰", |ui| {
                if ui.button("Check all").clicked() {
                    window.sets.fill(true);
                }
                if ui.button("Uncheck all").clicked() {
                    window.sets.fill(false);
                }
                if ui.button("Invert all").clicked() {
                    for s in &mut window.sets {
                        *s = !*s;
                    }
                }
            });
        });
        let format = window.settings.format;
        let depth = window.settings.bit_depth;
        ui.columns(2, |columns| {
            egui::ScrollArea::vertical()
                .id_salt("export sets")
                .max_height(220.0)
                .show(&mut columns[0], |ui| {
                    for (i, set) in self.model.texture_sets.iter().enumerate() {
                        ui.horizontal(|ui| {
                            ui.checkbox(&mut window.sets[i], "");
                            if ui
                                .selectable_label(window.preview == i, &set.name)
                                .clicked()
                            {
                                window.preview = i;
                            }
                        });
                    }
                });
            let ui = &mut columns[1];
            let set = &self.model.texture_sets[window.preview];
            ui.label(
                RichText::new(format!("OUTPUT MAPS - {}", set.name))
                    .small()
                    .strong(),
            );
            egui::ScrollArea::vertical()
                .id_salt("export maps")
                .max_height(200.0)
                .show(ui, |ui| {
                    for (name, components) in self.output_files(&window.settings, window.preview) {
                        let channels = ["L", "LA", "RGB", "RGBA"][components - 1];
                        ui.horizontal(|ui| {
                            ui.label(name);
                            ui.label(
                                RichText::new(format!(
                                    "{channels} · {} · {}",
                                    format.label(),
                                    depth.label()
                                ))
                                .small()
                                .weak(),
                            );
                        });
                    }
                    ui.label(
                        RichText::new(format!("{0} × {0}", set.resolution))
                            .small()
                            .weak(),
                    );
                });
        });
    }

    /// The file names a texture set gets, with their component counts.
    fn output_files(&self, settings: &ExportSettings, set: usize) -> Vec<(String, usize)> {
        let project = self.project_name();
        let set_name = &self.model.texture_sets[set].name;
        settings
            .template
            .maps
            .iter()
            .map(|map| {
                let name = file_name(map.name, &self.model.name, set_name, &project);
                (
                    format!("{name}.{}", settings.format.extension()),
                    settings.components(map),
                )
            })
            .collect()
    }

    /// The files of the texture sets to export. Names that would come out the same (texture
    /// sets whose names differ only in characters files can't have, or in case, which
    /// Windows ignores) get a number, so no set overwrites another's maps.
    fn export_paths(
        &self,
        settings: &ExportSettings,
        directory: &Path,
        sets: &[bool],
    ) -> Vec<Vec<PathBuf>> {
        let mut taken = std::collections::HashSet::new();
        let extension = settings.format.extension();
        (0..sets.len())
            .map(|set| {
                if !sets[set] {
                    return Vec::new();
                }
                self.output_files(settings, set)
                    .into_iter()
                    .map(|(name, _)| {
                        let stem = name.strip_suffix(&format!(".{extension}")).unwrap_or(&name);
                        let mut name = name.clone();
                        let mut n = 2;
                        while !taken.insert(name.to_lowercase()) {
                            name = format!("{stem}_{n}.{extension}");
                            n += 1;
                        }
                        directory.join(name)
                    })
                    .collect()
            })
            .collect()
    }

    fn start_export(&mut self, window: &mut ExportWindow) {
        let directory = PathBuf::from(window.settings.directory.trim());
        window.log.clear();
        window.tab = Tab::Exports;
        window.directory = Some(directory.clone());
        if let Err(err) = std::fs::create_dir_all(&directory) {
            window.log.push((
                directory.display().to_string(),
                Err(format!("Could not create the folder: {err}")),
            ));
            return;
        }
        let todo: VecDeque<usize> = (0..window.sets.len()).filter(|&i| window.sets[i]).collect();
        let sets = todo.len();
        let files = sets * window.settings.template.maps.len();
        let paths = self.export_paths(&window.settings, &directory, &window.sets);
        let (sender, results) = mpsc::channel();
        window.job = Some(ExportJob {
            directory,
            settings: window.settings.clone(),
            paths,
            todo,
            sets,
            sets_done: 0,
            current: None,
            writers: Vec::new(),
            results,
            sender,
            files,
            done: 0,
            failed: 0,
        });
    }

    /// Composites a few blocks of the current texture set, starts the next texture set
    /// once the last one is packed, and collects what was written.
    fn run_export(&mut self, window: &mut ExportWindow, rs: &RenderState) {
        let Some(job) = &mut window.job else { return };
        let collect = |job: &mut ExportJob, log: &mut Vec<(String, Result<(), String>)>| {
            while let Ok((file, result)) = job.results.try_recv() {
                job.done += 1;
                if result.is_err() {
                    job.failed += 1;
                }
                log.push((file, result));
            }
        };
        collect(job, &mut window.log);
        let mut finished = Vec::new();
        let mut i = 0;
        while i < job.writers.len() {
            if job.writers[i].is_finished() {
                finished.push(job.writers.swap_remove(i));
            } else {
                i += 1;
            }
        }
        for writer in finished {
            if writer.join().is_err() {
                window
                    .log
                    .push(("Export".into(), Err("Packing the maps failed".into())));
                job.done += 1;
                job.failed += 1;
            }
        }
        if job.current.is_some() {
            self.export_blocks(job, rs);
            return;
        }
        if job.writers.len() < SETS_IN_FLIGHT
            && let Some(set) = job.todo.pop_front()
        {
            self.start_set_export(job, set, rs);
            return;
        }
        if !job.writers.is_empty() || !job.todo.is_empty() {
            return;
        }
        // Everything is written: the last worker sent its results before it ended.
        collect(job, &mut window.log);
        let written = job.done - job.failed.min(job.done);
        self.status = if job.failed == 0 {
            format!("Exported {written} files to {}", job.directory.display())
        } else {
            format!("Exported {written} files; {} failed", job.failed)
        };
        window.job = None;
    }

    /// Starts a thread that packs a texture set's texels into its maps as they arrive and
    /// writes the files once all arrived.
    fn start_set_export(&mut self, job: &mut ExportJob, set: usize, rs: &RenderState) {
        let settings = job.settings.clone();
        let template: &'static Template = settings.template;
        let resolution = self.model.texture_sets[set].resolution;
        let paths = std::mem::take(&mut job.paths[set]);
        let files = paths
            .iter()
            .map(|p| {
                p.file_name()
                    .map_or_else(String::new, |n| n.to_string_lossy().into_owned())
            })
            .collect();
        let normal_format: NormalFormat = self.model.normal_format;
        let (packer, receiver) = mpsc::sync_channel::<Packing>(BLOCKS_IN_FLIGHT);
        let results = job.sender.clone();
        let worker = std::thread::spawn(move || {
            let mut maps: Vec<MapImage> = template
                .maps
                .iter()
                .map(|m| MapImage::new(resolution, settings.components(m), settings.bit_depth))
                .collect();
            let mut complete = false;
            for message in receiver {
                match message {
                    Packing::Block(block) => {
                        for (image, map) in maps.iter_mut().zip(template.maps) {
                            image.pack(&block, map, &settings, normal_format);
                        }
                    }
                    Packing::Complete => complete = true,
                }
            }
            // A texture set the GPU could not hand over completely writes nothing.
            if !complete {
                return;
            }
            let files = paths
                .into_iter()
                .zip(maps)
                .map(|(path, image)| MapFile { path, image })
                .collect();
            ink_io::export::write_maps(files, settings.format, settings.bit_depth, |path, r| {
                let name = path
                    .file_name()
                    .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
                let _ = results.send((name, r.map_err(|e| format!("{e:#}"))));
            });
        });
        // The GPU state must match the model before its blocks are counted.
        self.renderer
            .sync_texture_sets(&rs.device, &rs.queue, &self.model, false);
        job.current = Some(SetExport {
            set,
            next: 0,
            blocks: self.renderer.export_blocks(set),
            files,
            packer,
            worker,
        });
    }

    /// Composites the next blocks of the current texture set and hands them to its packer;
    /// after the last one the packer writes the files.
    fn export_blocks(&mut self, job: &mut ExportJob, rs: &RenderState) {
        let Some(current) = &mut job.current else {
            return;
        };
        let end = (current.next + BLOCKS_PER_FRAME).min(current.blocks);
        let packer = &current.packer;
        let result = self.renderer.export_texture_set(
            &rs.device,
            &rs.queue,
            &self.model,
            current.set,
            current.next..end,
            &mut |block| {
                // The packer only stops early if it panicked; the join reports it.
                let _ = packer.send(Packing::Block(block));
            },
        );
        current.next = end;
        let Some(current) = job
            .current
            .take_if(|c| result.is_err() || c.next >= c.blocks)
        else {
            return;
        };
        job.sets_done += 1;
        match result {
            Ok(()) => {
                let _ = current.packer.send(Packing::Complete);
                job.writers.push(current.worker);
            }
            Err(err) => {
                let name = &self.model.texture_sets[current.set].name;
                log::error!("Export of {name} failed: {err}");
                for file in &current.files {
                    let _ = job.sender.send((file.clone(), Err(err.clone())));
                }
                drop(current.packer);
                job.writers.push(current.worker);
            }
        }
    }
}

fn export_list_tab(ui: &mut egui::Ui, window: &ExportWindow) {
    if let Some(dir) = &window.directory {
        ui.label(RichText::new(dir.display().to_string()).small().weak());
    }
    if let Some(job) = &window.job {
        // Half for compositing, half for writing.
        let current = job
            .current
            .as_ref()
            .map_or(0.0, |c| c.next as f32 / c.blocks.max(1) as f32);
        let composited = (job.sets_done as f32 + current) / job.sets.max(1) as f32;
        let written = job.done as f32 / job.files.max(1) as f32;
        let text = match &job.current {
            Some(_) => format!(
                "Texture set {} of {} · {} of {} files",
                job.sets_done + 1,
                job.sets,
                job.done,
                job.files
            ),
            None => format!("{} of {} files", job.done, job.files),
        };
        ui.add(egui::ProgressBar::new(0.5 * (composited + written)).text(text));
    } else if window.log.is_empty() {
        ui.label("Nothing exported yet.");
    }
    egui::ScrollArea::vertical()
        .id_salt("export log")
        .max_height(320.0)
        .stick_to_bottom(true)
        .show(ui, |ui| {
            for (file, result) in &window.log {
                match result {
                    Ok(()) => ui.label(
                        RichText::new(format!("✔ {file}")).color(Color32::from_rgb(140, 200, 140)),
                    ),
                    Err(err) => ui.label(
                        RichText::new(format!("✖ {file}: {err}"))
                            .color(Color32::from_rgb(230, 120, 110)),
                    ),
                };
            }
        });
}
