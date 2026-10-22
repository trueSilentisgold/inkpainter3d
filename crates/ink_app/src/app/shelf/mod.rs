//! The shelf: brushes, alphas, textures and materials in one panel, with the user's own
//! library on disk (`<data dir>/shelf`), a folder importer for PBR maps and the online
//! libraries of ambientCG and Poly Haven. Click an item to use it; drag materials and
//! textures onto the layer stack.

mod content;
mod smart;

use super::InkApp;
use super::presets::user_dir as brush_dir;
use super::project::data_dir;
use crate::theme::{self, icons};
use eframe::egui::{self, Color32, Sense, TextureHandle};
use ink_core::{Alpha, AlphaImage, Fill, TextureImage};
use ink_io::material;
use ink_io::online::{OnlineAsset, Resolution, Source};
use ink_io::project::smart::{Smart, SmartKind};
pub use smart::SmartRef;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};

/// What the shelf shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Category {
    Brushes,
    Alphas,
    Textures,
    Materials,
    SmartMaterials,
    SmartMasks,
    Online,
}

impl Category {
    const ALL: [Self; 7] = [
        Self::Brushes,
        Self::Alphas,
        Self::Textures,
        Self::Materials,
        Self::SmartMaterials,
        Self::SmartMasks,
        Self::Online,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::Brushes => "Brushes",
            Self::Alphas => "Alphas",
            Self::Textures => "Textures",
            Self::Materials => "Materials",
            Self::SmartMaterials => "Smart materials",
            Self::SmartMasks => "Smart masks",
            Self::Online => "Online",
        }
    }

    /// The folder of the user's items in the library.
    fn folder(self) -> Option<&'static str> {
        match self {
            Self::Brushes => Some("brushes"),
            Self::Alphas => Some("alphas"),
            Self::Textures => Some("textures"),
            Self::Materials => Some("materials"),
            Self::SmartMaterials => Some("smart_materials"),
            Self::SmartMasks => Some("smart_masks"),
            Self::Online => None,
        }
    }
}

/// What a dragged shelf item is, for the places it can be dropped on.
#[derive(Clone, Debug)]
pub enum ShelfDrag {
    Material(PathBuf),
    Texture(PathBuf),
    Smart(SmartRef),
}

fn library_dir(category: Category) -> Option<PathBuf> {
    let folder = category.folder()?;
    if category == Category::Brushes {
        return brush_dir();
    }
    data_dir().map(|d| d.join("shelf").join(folder))
}

struct FileItem {
    name: String,
    path: PathBuf,
    thumb: Option<TextureHandle>,
}

struct AlphaItem {
    name: String,
    /// The image, for the alphas made in code; None for files, which load when used.
    image: Option<Arc<AlphaImage>>,
    path: Option<PathBuf>,
    thumb: Option<TextureHandle>,
}

struct OnlineItem {
    asset: OnlineAsset,
    thumb: Option<TextureHandle>,
}

struct Online {
    source: Source,
    query: String,
    resolution: Resolution,
    results: Vec<OnlineItem>,
    /// A search is under way.
    loading: bool,
    /// Whether the last page was full, so there may be more.
    more: bool,
    error: Option<String>,
    /// Searches and previews of older searches are dropped.
    generation: u64,
    started: bool,
}

enum Event {
    Thumb {
        generation: u64,
        category: Category,
        path: PathBuf,
        image: egui::ColorImage,
    },
    Material(anyhow::Result<material::Material>),
    Texture(anyhow::Result<TextureImage>),
    Downloaded {
        name: String,
        result: anyhow::Result<PathBuf>,
        apply: bool,
    },
    OnlineList {
        generation: u64,
        append: bool,
        result: anyhow::Result<Vec<OnlineAsset>>,
    },
    OnlinePreview {
        generation: u64,
        id: String,
        image: egui::ColorImage,
    },
    Imported(anyhow::Result<String>),
    SmartThumb {
        generation: u64,
        path: PathBuf,
        name: String,
        image: egui::ColorImage,
    },
    Smart(anyhow::Result<Smart>),
}

/// Something the user asked for while the panel was drawn.
enum Action {
    ApplyPreset(usize),
    DeletePreset(usize),
    SaveBrush(String),
    UseAlpha(usize),
    UseAlphaAsStencil(usize),
    FillTexture(PathBuf),
    TextureAsAlpha(PathBuf),
    TextureAsStencil(PathBuf),
    ApplyMaterial(PathBuf),
    Delete(Category, PathBuf),
    ImportFiles(Category),
    ImportMaterialFolder,
    OpenFolder(Category),
    Search,
    LoadMore,
    Download(OnlineAsset),
    ApplySmart(SmartRef),
    SaveSmart(SmartKind, String),
}

pub struct ShelfState {
    category: Category,
    search: String,
    /// Name typed for saving the current brush.
    brush_name: String,
    alphas: Vec<AlphaItem>,
    textures: Vec<FileItem>,
    materials: Vec<FileItem>,
    brush_pictures: HashMap<String, TextureHandle>,
    smart: smart::SmartShelf,
    scanned: bool,
    /// Thumbnails from older scans are dropped.
    generation: u64,
    online: Online,
    /// Background jobs, shown as a hint.
    jobs: Vec<String>,
    tx: Sender<Event>,
    rx: Receiver<Event>,
}

impl Default for ShelfState {
    fn default() -> Self {
        let (tx, rx) = channel();
        Self {
            category: Category::Brushes,
            search: String::new(),
            brush_name: String::new(),
            alphas: Vec::new(),
            textures: Vec::new(),
            materials: Vec::new(),
            brush_pictures: HashMap::new(),
            smart: Default::default(),
            scanned: false,
            generation: 0,
            online: Online {
                source: Source::AmbientCg,
                query: String::new(),
                resolution: Resolution::K1,
                results: Vec::new(),
                loading: false,
                more: false,
                error: None,
                generation: 0,
                started: false,
            },
            jobs: Vec::new(),
            tx,
            rx,
        }
    }
}

/// Size of a cell of the grid.
const CELL: egui::Vec2 = egui::vec2(104.0, 86.0);
/// Height the picture of a cell takes.
const PICTURE_HEIGHT: f32 = 56.0;
/// The narrowest a cell gets when the panel is narrower than one cell.
const MIN_CELL_WIDTH: f32 = 64.0;
/// The narrowest the shelf's text fields get.
const MIN_FIELD_WIDTH: f32 = 60.0;

/// Width for a text field that wants `wanted` but gives way in a narrow panel.
fn field_width(ui: &egui::Ui, wanted: f32) -> f32 {
    wanted.min(panel_width(ui) - 8.0).max(MIN_FIELD_WIDTH)
}

fn panel_width_id() -> egui::Id {
    egui::Id::new("shelf panel width")
}

/// The width of the whole shelf panel, noted at the top of each frame: inside a wrapping row
/// `available_width` only tells what is left of the current row.
fn panel_width(ui: &egui::Ui) -> f32 {
    ui.data(|d| d.get_temp::<f32>(panel_width_id()))
        .unwrap_or(f32::INFINITY)
}
/// Items per online page.
const PAGE: usize = 36;

fn file_stem(path: &Path) -> String {
    path.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn list_files(dir: &Path, extensions: &[&str]) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = entries
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.extension()
                .is_some_and(|e| extensions.iter().any(|x| e.eq_ignore_ascii_case(x)))
        })
        .collect();
    files.sort_by_key(|p| file_stem(p).to_lowercase());
    files
}

fn to_color_image(image: &image::RgbaImage) -> egui::ColorImage {
    egui::ColorImage::from_rgba_unmultiplied(
        [image.width() as usize, image.height() as usize],
        image.as_raw(),
    )
}

impl InkApp {
    /// Reads the library folders and starts making thumbnails of what has none.
    fn shelf_rescan(&mut self, ctx: &egui::Context) {
        let shelf = &mut self.shelf;
        shelf.scanned = true;
        shelf.generation += 1;
        let generation = shelf.generation;

        // Alphas: the ones made in code, then the user's files.
        shelf.alphas = content::builtin_alphas()
            .into_iter()
            .map(|image| AlphaItem {
                name: image.name.clone(),
                thumb: Some(ctx.load_texture(
                    format!("alpha {}", image.name),
                    content::alpha_picture(&image, 64),
                    egui::TextureOptions::LINEAR,
                )),
                image: Some(image),
                path: None,
            })
            .collect();
        let mut jobs: Vec<(Category, PathBuf)> = Vec::new();
        if let Some(dir) = library_dir(Category::Alphas) {
            for path in list_files(&dir, ink_io::ALPHA_EXTENSIONS) {
                jobs.push((Category::Alphas, path.clone()));
                shelf.alphas.push(AlphaItem {
                    name: file_stem(&path),
                    image: None,
                    path: Some(path),
                    thumb: None,
                });
            }
        }
        let files = |category, extensions: &[&str], jobs: &mut Vec<(Category, PathBuf)>| {
            let Some(dir) = library_dir(category) else {
                return Vec::new();
            };
            list_files(&dir, extensions)
                .into_iter()
                .map(|path| {
                    jobs.push((category, path.clone()));
                    FileItem {
                        name: file_stem(&path),
                        path,
                        thumb: None,
                    }
                })
                .collect::<Vec<_>>()
        };
        shelf.textures = files(Category::Textures, ink_io::TEXTURE_EXTENSIONS, &mut jobs);
        shelf.materials = files(Category::Materials, &[material::EXTENSION], &mut jobs);
        self.smart_rescan(ctx, generation);
        let shelf = &mut self.shelf;

        let (tx, ctx) = (shelf.tx.clone(), ctx.clone());
        std::thread::spawn(move || {
            for (category, path) in jobs {
                let image = match category {
                    Category::Materials => material::preview(&path)
                        .ok()
                        .map(|(_, image)| to_color_image(&image)),
                    Category::Alphas => ink_io::load_alpha(&path)
                        .ok()
                        .map(|image| content::alpha_picture(&image, 64)),
                    _ => image::open(&path)
                        .ok()
                        .map(|image| to_color_image(&image.thumbnail(128, 128).to_rgba8())),
                };
                if let Some(image) = image {
                    let _ = tx.send(Event::Thumb {
                        generation,
                        category,
                        path,
                        image,
                    });
                    ctx.request_repaint();
                }
            }
        });
    }

    /// Takes what the background jobs have finished.
    fn shelf_events(&mut self, ctx: &egui::Context) {
        while let Ok(event) = self.shelf.rx.try_recv() {
            match event {
                Event::Thumb {
                    generation,
                    category,
                    path,
                    image,
                } => {
                    if generation != self.shelf.generation {
                        continue;
                    }
                    let texture = ctx.load_texture(
                        path.to_string_lossy(),
                        image,
                        egui::TextureOptions::LINEAR,
                    );
                    match category {
                        Category::Alphas => {
                            if let Some(item) = self
                                .shelf
                                .alphas
                                .iter_mut()
                                .find(|i| i.path.as_ref() == Some(&path))
                            {
                                item.thumb = Some(texture);
                            }
                        }
                        Category::Textures | Category::Materials => {
                            let list = if category == Category::Textures {
                                &mut self.shelf.textures
                            } else {
                                &mut self.shelf.materials
                            };
                            if let Some(item) = list.iter_mut().find(|i| i.path == path) {
                                item.thumb = Some(texture);
                            }
                        }
                        _ => {}
                    }
                }
                Event::Material(result) => {
                    self.shelf.jobs.retain(|j| !j.starts_with("Loading"));
                    match result {
                        Ok(material) => self.apply_material(material),
                        Err(error) => self.status = format!("Error: {error:#}"),
                    }
                }
                Event::Texture(result) => {
                    self.shelf.jobs.retain(|j| !j.starts_with("Loading"));
                    match result {
                        Ok(image) => {
                            let name = image.name.clone();
                            let mut fill = Fill::default();
                            fill.images[ink_core::Channel::BaseColor.index()] =
                                Some(Arc::new(image));
                            self.add_fill_layer_with(name, fill);
                        }
                        Err(error) => self.status = format!("Error: {error:#}"),
                    }
                }
                Event::Downloaded {
                    name,
                    result,
                    apply,
                } => {
                    self.shelf
                        .jobs
                        .retain(|j| j != &format!("Downloading {name}"));
                    match result {
                        Ok(path) => {
                            self.status = format!("Downloaded {name} into the shelf");
                            self.shelf_rescan(ctx);
                            if apply {
                                self.shelf_load_material(ctx, path);
                            }
                        }
                        Err(error) => self.status = format!("Error: {error:#}"),
                    }
                }
                Event::OnlineList {
                    generation,
                    append,
                    result,
                } => {
                    let online = &mut self.shelf.online;
                    if generation != online.generation {
                        continue;
                    }
                    online.loading = false;
                    match result {
                        Ok(assets) => {
                            online.error = None;
                            online.more = assets.len() >= PAGE;
                            if !append {
                                online.results.clear();
                            }
                            let tx = self.shelf.tx.clone();
                            let ctx = ctx.clone();
                            for asset in &assets {
                                let (tx, ctx, asset) = (tx.clone(), ctx.clone(), asset.clone());
                                std::thread::spawn(move || {
                                    if let Ok(bytes) = ink_io::online::fetch(&asset.preview_url)
                                        && let Ok(image) = image::load_from_memory(&bytes)
                                    {
                                        let small = image.thumbnail(160, 160).to_rgba8();
                                        let _ = tx.send(Event::OnlinePreview {
                                            generation,
                                            id: asset.id,
                                            image: to_color_image(&small),
                                        });
                                        ctx.request_repaint();
                                    }
                                });
                            }
                            online.results.extend(
                                assets
                                    .into_iter()
                                    .map(|asset| OnlineItem { asset, thumb: None }),
                            );
                        }
                        Err(error) => online.error = Some(format!("{error:#}")),
                    }
                }
                Event::OnlinePreview {
                    generation,
                    id,
                    image,
                } => {
                    if generation == self.shelf.online.generation
                        && let Some(item) = self
                            .shelf
                            .online
                            .results
                            .iter_mut()
                            .find(|i| i.asset.id == id)
                    {
                        item.thumb = Some(ctx.load_texture(
                            format!("online {id}"),
                            image,
                            egui::TextureOptions::LINEAR,
                        ));
                    }
                }
                Event::SmartThumb {
                    generation,
                    path,
                    name,
                    image,
                } => {
                    if generation == self.shelf.generation
                        && let Some(item) = self
                            .shelf
                            .smart
                            .items
                            .iter_mut()
                            .find(|i| matches!(&i.source, SmartRef::File(p) if *p == path))
                    {
                        item.name = name;
                        item.thumb = Some(ctx.load_texture(
                            path.to_string_lossy(),
                            image,
                            egui::TextureOptions::LINEAR,
                        ));
                    }
                }
                Event::Smart(result) => {
                    self.shelf.jobs.retain(|j| !j.starts_with("Loading"));
                    match result {
                        Ok(smart) => self.apply_smart(&smart),
                        Err(error) => self.status = format!("Error: {error:#}"),
                    }
                }
                Event::Imported(result) => {
                    self.shelf.jobs.retain(|j| !j.starts_with("Importing"));
                    match result {
                        Ok(name) => {
                            self.status = format!("Imported {name}");
                            self.shelf_rescan(ctx);
                        }
                        Err(error) => self.status = format!("Error: {error:#}"),
                    }
                }
            }
        }
    }

    /// A material becomes a fill layer on the selected texture set.
    fn apply_material(&mut self, material: material::Material) {
        let mut fill = Fill {
            channels: ink_core::ChannelSet::NONE,
            ..Fill::default()
        };
        for (channel, image) in material.images {
            fill.channels.set(channel, true);
            fill.images[channel.index()] = Some(Arc::new(image));
        }
        fill.normal_directx = material.normal_directx;
        self.add_fill_layer_with(material.name.clone(), fill);
        self.status = format!("Added the material {}", material.name);
    }

    fn shelf_load_material(&mut self, ctx: &egui::Context, path: PathBuf) {
        self.shelf
            .jobs
            .push(format!("Loading {}", file_stem(&path)));
        let (tx, ctx) = (self.shelf.tx.clone(), ctx.clone());
        std::thread::spawn(move || {
            let _ = tx.send(Event::Material(material::load(&path)));
            ctx.request_repaint();
        });
    }

    fn shelf_load_texture(&mut self, ctx: &egui::Context, path: PathBuf) {
        self.shelf
            .jobs
            .push(format!("Loading {}", file_stem(&path)));
        let (tx, ctx) = (self.shelf.tx.clone(), ctx.clone());
        std::thread::spawn(move || {
            let _ = tx.send(Event::Texture(ink_io::load_texture(&path)));
            ctx.request_repaint();
        });
    }

    /// Makes the alpha the brush's tip and keeps it in the list of loaded alphas.
    fn use_alpha(&mut self, image: Arc<AlphaImage>) {
        if !self.paint.alphas.iter().any(|a| a.id == image.id) {
            self.paint.alphas.push(image.clone());
        }
        self.paint.brush_mut().alpha = Alpha::Image(image);
        self.paint.fill_tool = false;
    }

    fn shelf_alpha_image(&mut self, index: usize) -> Option<Arc<AlphaImage>> {
        let item = self.shelf.alphas.get_mut(index)?;
        if item.image.is_none() {
            let path = item.path.clone()?;
            match ink_io::load_alpha(&path) {
                Ok(image) => item.image = Some(Arc::new(image)),
                Err(error) => {
                    self.status = format!("Error: {error:#}");
                    return None;
                }
            }
        }
        self.shelf.alphas.get(index)?.image.clone()
    }

    fn shelf_alpha_from_file(&mut self, path: &Path) -> Option<Arc<AlphaImage>> {
        match ink_io::load_alpha(path) {
            Ok(image) => Some(Arc::new(image)),
            Err(error) => {
                self.status = format!("Error: {error:#}");
                None
            }
        }
    }

    fn do_action(&mut self, ctx: &egui::Context, action: Action) {
        match action {
            Action::ApplyPreset(i) => self.apply_preset(i),
            Action::DeletePreset(i) => self.delete_preset(i),
            Action::SaveBrush(name) => {
                self.preset_draft = name;
                self.save_preset();
                self.shelf.brush_pictures.clear();
            }
            Action::UseAlpha(i) => {
                if let Some(image) = self.shelf_alpha_image(i) {
                    self.use_alpha(image);
                }
            }
            Action::UseAlphaAsStencil(i) => {
                if let Some(image) = self.shelf_alpha_image(i) {
                    self.set_stencil_image(image);
                }
            }
            Action::FillTexture(path) => self.shelf_load_texture(ctx, path),
            Action::TextureAsAlpha(path) => {
                if let Some(image) = self.shelf_alpha_from_file(&path) {
                    self.use_alpha(image);
                }
            }
            Action::TextureAsStencil(path) => {
                if let Some(image) = self.shelf_alpha_from_file(&path) {
                    self.set_stencil_image(image);
                }
            }
            Action::ApplyMaterial(path) => self.shelf_load_material(ctx, path),
            Action::Delete(category, path) => {
                if let Err(error) = std::fs::remove_file(&path) {
                    self.status = format!("Could not delete {}: {error}", path.display());
                } else {
                    self.shelf_rescan(ctx);
                    if category == Category::Brushes {
                        self.reload_presets();
                    }
                }
            }
            Action::ImportFiles(category) => self.shelf_import_files(ctx, category),
            Action::ImportMaterialFolder => {
                if let Some(dir) = rfd::FileDialog::new().pick_folder() {
                    self.shelf_import_folder(ctx, dir);
                }
            }
            Action::OpenFolder(category) => {
                if let Some(dir) = library_dir(category) {
                    let _ = std::fs::create_dir_all(&dir);
                    crate::report::open_folder(&dir);
                }
            }
            Action::Search => self.shelf_search(ctx, false),
            Action::LoadMore => self.shelf_search(ctx, true),
            Action::Download(asset) => self.shelf_download(ctx, asset, true),
            Action::ApplySmart(source) => self.smart_apply(ctx, source),
            Action::SaveSmart(kind, name) => match kind {
                SmartKind::Material => self.save_smart_material(ctx, name),
                SmartKind::Mask => self.save_smart_mask(ctx, name),
            },
        }
    }

    fn shelf_import_files(&mut self, ctx: &egui::Context, category: Category) {
        let (title, extensions): (&str, &[&str]) = match category {
            Category::Brushes => ("Brush presets", &[ink_io::preset::EXTENSION]),
            Category::Alphas => ("Images", ink_io::ALPHA_EXTENSIONS),
            Category::Textures => ("Images", ink_io::TEXTURE_EXTENSIONS),
            Category::Materials => ("Materials", &[material::EXTENSION]),
            Category::SmartMaterials | Category::SmartMasks => {
                ("Smart presets", &[ink_io::project::smart::EXTENSION])
            }
            Category::Online => return,
        };
        let Some(files) = rfd::FileDialog::new()
            .add_filter(title, extensions)
            .pick_files()
        else {
            return;
        };
        let Some(dir) = library_dir(category) else {
            return;
        };
        let _ = std::fs::create_dir_all(&dir);
        let mut copied = 0;
        for file in files {
            let Some(name) = file.file_name() else {
                continue;
            };
            if std::fs::copy(&file, dir.join(name)).is_ok() {
                copied += 1;
            }
        }
        self.status = format!("Added {copied} file(s) to the shelf");
        self.shelf_rescan(ctx);
        if category == Category::Brushes {
            self.reload_presets();
        }
    }

    /// Imports a folder of PBR maps as a material, in the background.
    fn shelf_import_folder(&mut self, ctx: &egui::Context, dir: PathBuf) {
        let Some(target) = library_dir(Category::Materials) else {
            return;
        };
        self.shelf
            .jobs
            .push(format!("Importing {}", file_stem(&dir)));
        let (tx, ctx) = (self.shelf.tx.clone(), ctx.clone());
        std::thread::spawn(move || {
            let result = (|| {
                let files = material::from_folder(&dir)?;
                std::fs::create_dir_all(&target)?;
                let name = files.name.clone();
                material::save(
                    &target.join(format!("{}.{}", safe_name(&name), material::EXTENSION)),
                    &files,
                )?;
                Ok(name)
            })();
            let _ = tx.send(Event::Imported(result));
            ctx.request_repaint();
        });
    }

    fn shelf_search(&mut self, ctx: &egui::Context, append: bool) {
        let online = &mut self.shelf.online;
        if !append {
            online.generation += 1;
            online.results.clear();
        }
        online.loading = true;
        online.started = true;
        online.error = None;
        let (source, query, generation) = (online.source, online.query.clone(), online.generation);
        let offset = if append { online.results.len() } else { 0 };
        let (tx, ctx) = (self.shelf.tx.clone(), ctx.clone());
        std::thread::spawn(move || {
            let result = ink_io::online::search(source, &query, offset, PAGE);
            let _ = tx.send(Event::OnlineList {
                generation,
                append,
                result,
            });
            ctx.request_repaint();
        });
    }

    fn shelf_download(&mut self, ctx: &egui::Context, asset: OnlineAsset, apply: bool) {
        let Some(dir) = library_dir(Category::Materials) else {
            return;
        };
        let path = dir.join(format!("{}.{}", safe_name(&asset.id), material::EXTENSION));
        if path.exists() {
            if apply {
                self.shelf_load_material(ctx, path);
            }
            return;
        }
        let name = asset.name.clone();
        let job = format!("Downloading {name}");
        if self.shelf.jobs.contains(&job) {
            return;
        }
        self.shelf.jobs.push(job);
        let resolution = self.shelf.online.resolution;
        let (tx, ctx) = (self.shelf.tx.clone(), ctx.clone());
        std::thread::spawn(move || {
            let result = (|| {
                let files = ink_io::online::download(&asset, resolution)?;
                std::fs::create_dir_all(&dir)?;
                material::save(&path, &files)?;
                Ok(path)
            })();
            let _ = tx.send(Event::Downloaded {
                name,
                result,
                apply,
            });
            ctx.request_repaint();
        });
    }

    /// The shelf panel.
    pub(super) fn shelf_panel(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        if !self.shelf.scanned {
            self.shelf_rescan(&ctx);
        }
        self.shelf_events(&ctx);
        if !self.shelf.jobs.is_empty() {
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
        }

        let mut actions: Vec<Action> = Vec::new();
        ui.data_mut(|d| d.insert_temp(panel_width_id(), ui.available_width()));
        // The categories as a row of tabs above everything, so the panel can stay low.
        ui.horizontal_wrapped(|ui| {
            for category in Category::ALL {
                let selected = self.shelf.category == category;
                if ui
                    .add(egui::Button::selectable(selected, category.label()))
                    .clicked()
                {
                    self.shelf.category = category;
                }
            }
        });
        ui.add_space(2.0);
        self.shelf_toolbar(ui, &mut actions);
        ui.add_space(4.0);
        egui::ScrollArea::vertical()
            .id_salt(("shelf", self.shelf.category))
            .auto_shrink([false, false])
            .show(ui, |ui| self.shelf_grid(ui, &mut actions));
        for action in actions {
            self.do_action(&ctx, action);
        }
    }

    fn shelf_toolbar(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        let category = self.shelf.category;
        ui.horizontal_wrapped(|ui| {
            if category == Category::Online {
                let online = &mut self.shelf.online;
                egui::ComboBox::from_id_salt("online source")
                    .selected_text(online.source.label())
                    .width(100.0)
                    .show_ui(ui, |ui| {
                        for source in Source::ALL {
                            if ui
                                .selectable_value(&mut online.source, source, source.label())
                                .clicked()
                            {
                                online.started = false;
                            }
                        }
                    });
                let response = ui.add(
                    egui::TextEdit::singleline(&mut online.query)
                        .hint_text("Search materials")
                        .desired_width(field_width(ui, 180.0)),
                );
                let enter = response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if ui.button("Search").clicked() || enter || !online.started {
                    actions.push(Action::Search);
                }
                egui::ComboBox::from_id_salt("online resolution")
                    .selected_text(online.resolution.label())
                    .width(56.0)
                    .show_ui(ui, |ui| {
                        for resolution in Resolution::ALL {
                            ui.selectable_value(
                                &mut online.resolution,
                                resolution,
                                resolution.label(),
                            );
                        }
                    })
                    .response
                    .on_hover_text("Size of the maps to download");
                ui.label(
                    egui::RichText::new(format!("CC0, from {}", online.source.url()))
                        .color(theme::TEXT_WEAK),
                );
            } else {
                ui.add(
                    egui::TextEdit::singleline(&mut self.shelf.search)
                        .hint_text("Filter")
                        .desired_width(field_width(ui, 160.0)),
                );
                if category == Category::Brushes {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.shelf.brush_name)
                            .hint_text("Name for the current brush")
                            .desired_width(field_width(ui, 160.0)),
                    );
                    let ready = !self.shelf.brush_name.trim().is_empty();
                    if ui
                        .add_enabled(ready, egui::Button::new("Save brush"))
                        .clicked()
                    {
                        actions.push(Action::SaveBrush(self.shelf.brush_name.trim().to_owned()));
                        self.shelf.brush_name.clear();
                    }
                }
                if let Some(kind) = category.smart_kind() {
                    self.smart_toolbar(ui, kind, actions);
                }
                if category == Category::Materials
                    && ui
                        .button("Import folder…")
                        .on_hover_text(
                            "A folder of PBR maps, as Poly Haven and ambientCG ship them",
                        )
                        .clicked()
                {
                    actions.push(Action::ImportMaterialFolder);
                }
                if ui.button("Import…").clicked() {
                    actions.push(Action::ImportFiles(category));
                }
                if theme::icon_button(ui, icons::FOLDER_OPEN, "Show the library folder").clicked() {
                    actions.push(Action::OpenFolder(category));
                }
            }
            if let Some(job) = self.shelf.jobs.last() {
                ui.label(egui::RichText::new(format!("{job}…")).color(theme::COPPER));
            }
        });
    }

    fn shelf_grid(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        let filter = self.shelf.search.trim().to_lowercase();
        let matches = |name: &str| filter.is_empty() || name.to_lowercase().contains(&filter);
        ui.spacing_mut().item_spacing = egui::vec2(6.0, 6.0);
        match self.shelf.category {
            Category::Brushes => {
                let current = self.preset_name.clone();
                ui.horizontal_wrapped(|ui| {
                    for i in 0..self.presets.len() {
                        let preset = &self.presets[i];
                        if !matches(&preset.name) {
                            continue;
                        }
                        let name = preset.name.clone();
                        let user = preset.file.is_some();
                        let texture = self
                            .shelf
                            .brush_pictures
                            .entry(name.clone())
                            .or_insert_with(|| {
                                ui.ctx().load_texture(
                                    format!("brush {name}"),
                                    content::brush_picture(&preset.brush),
                                    egui::TextureOptions::LINEAR,
                                )
                            })
                            .clone();
                        let response = cell(ui, &name, Some(&texture), name == current, false);
                        if response.clicked() {
                            actions.push(Action::ApplyPreset(i));
                        }
                        if user {
                            response.context_menu(|ui| {
                                if ui.button("Delete").clicked() {
                                    actions.push(Action::DeletePreset(i));
                                    ui.close();
                                }
                            });
                        }
                    }
                });
            }
            Category::Alphas => {
                let current = match &self.paint.brush().alpha {
                    Alpha::Image(image) => Some(image.name.clone()),
                    _ => None,
                };
                ui.horizontal_wrapped(|ui| {
                    for (i, item) in self.shelf.alphas.iter().enumerate() {
                        if !matches(&item.name) {
                            continue;
                        }
                        let selected = current.as_deref() == Some(item.name.as_str());
                        let response = cell(ui, &item.name, item.thumb.as_ref(), selected, true);
                        if response.clicked() {
                            actions.push(Action::UseAlpha(i));
                        }
                        response.context_menu(|ui| {
                            if ui.button("Use as brush alpha").clicked() {
                                actions.push(Action::UseAlpha(i));
                                ui.close();
                            }
                            if ui.button("Use as stencil").clicked() {
                                actions.push(Action::UseAlphaAsStencil(i));
                                ui.close();
                            }
                            if let Some(path) = &item.path
                                && ui.button("Delete").clicked()
                            {
                                actions.push(Action::Delete(Category::Alphas, path.clone()));
                                ui.close();
                            }
                        });
                    }
                });
            }
            Category::Textures => {
                if self.shelf.textures.is_empty() {
                    empty_hint(ui, "No textures yet. Use Import… to add images.");
                }
                ui.horizontal_wrapped(|ui| {
                    for item in &self.shelf.textures {
                        if !matches(&item.name) {
                            continue;
                        }
                        let response = cell(ui, &item.name, item.thumb.as_ref(), false, true);
                        response.dnd_set_drag_payload(ShelfDrag::Texture(item.path.clone()));
                        if response.clicked() {
                            actions.push(Action::FillTexture(item.path.clone()));
                        }
                        response.context_menu(|ui| {
                            if ui.button("New fill layer").clicked() {
                                actions.push(Action::FillTexture(item.path.clone()));
                                ui.close();
                            }
                            if ui.button("Use as brush alpha").clicked() {
                                actions.push(Action::TextureAsAlpha(item.path.clone()));
                                ui.close();
                            }
                            if ui.button("Use as stencil").clicked() {
                                actions.push(Action::TextureAsStencil(item.path.clone()));
                                ui.close();
                            }
                            if ui.button("Delete").clicked() {
                                actions.push(Action::Delete(Category::Textures, item.path.clone()));
                                ui.close();
                            }
                        });
                    }
                });
            }
            Category::Materials => {
                if self.shelf.materials.is_empty() {
                    empty_hint(
                        ui,
                        "No materials yet. Import a folder of PBR maps, or download some from Online.",
                    );
                }
                ui.horizontal_wrapped(|ui| {
                    for item in &self.shelf.materials {
                        if !matches(&item.name) {
                            continue;
                        }
                        let response = cell(ui, &item.name, item.thumb.as_ref(), false, true);
                        response.dnd_set_drag_payload(ShelfDrag::Material(item.path.clone()));
                        if response.clicked() {
                            actions.push(Action::ApplyMaterial(item.path.clone()));
                        }
                        response.context_menu(|ui| {
                            if ui.button("New fill layer").clicked() {
                                actions.push(Action::ApplyMaterial(item.path.clone()));
                                ui.close();
                            }
                            if ui.button("Delete").clicked() {
                                actions
                                    .push(Action::Delete(Category::Materials, item.path.clone()));
                                ui.close();
                            }
                        });
                    }
                });
            }
            Category::SmartMaterials => {
                self.smart_grid(ui, SmartKind::Material, &filter, actions);
            }
            Category::SmartMasks => self.smart_grid(ui, SmartKind::Mask, &filter, actions),
            Category::Online => {
                let have: std::collections::HashSet<String> = library_dir(Category::Materials)
                    .map(|dir| {
                        list_files(&dir, &[material::EXTENSION])
                            .iter()
                            .map(|p| file_stem(p))
                            .collect()
                    })
                    .unwrap_or_default();
                if let Some(error) = &self.shelf.online.error {
                    ui.label(
                        egui::RichText::new(format!("Could not reach the library: {error}"))
                            .color(theme::DANGER),
                    );
                }
                if self.shelf.online.loading && self.shelf.online.results.is_empty() {
                    ui.label(egui::RichText::new("Searching…").color(theme::TEXT_WEAK));
                }
                ui.horizontal_wrapped(|ui| {
                    for item in &self.shelf.online.results {
                        let downloaded = have.contains(&safe_name(&item.asset.id));
                        let response = cell(ui, &item.asset.name, item.thumb.as_ref(), false, true);
                        let response = response.on_hover_text(if downloaded {
                            format!(
                                "{} ({}). Click to add it as a fill layer.",
                                item.asset.name, item.asset.category
                            )
                        } else {
                            format!(
                                "{} ({}). Click to download and add it as a fill layer.",
                                item.asset.name, item.asset.category
                            )
                        });
                        if downloaded {
                            badge(ui, &response);
                        }
                        if response.clicked() {
                            actions.push(Action::Download(item.asset.clone()));
                        }
                    }
                });
                if self.shelf.online.more && !self.shelf.online.loading {
                    ui.add_space(4.0);
                    if ui.button("Load more").clicked() {
                        actions.push(Action::LoadMore);
                    }
                }
            }
        }
    }

    /// Drops of shelf items on a panel: materials and textures become fill layers.
    pub(super) fn shelf_drop_on_layers(&mut self, ui: &egui::Ui) {
        if !egui::DragAndDrop::has_payload_of_type::<ShelfDrag>(ui.ctx()) {
            return;
        }
        let rect = ui.max_rect();
        if !ui.rect_contains_pointer(rect) {
            return;
        }
        ui.painter().rect_stroke(
            rect.shrink(1.0),
            2.0,
            egui::Stroke::new(2.0, theme::OLIVE),
            egui::StrokeKind::Inside,
        );
        if ui.input(|i| i.pointer.any_released())
            && let Some(drag) = egui::DragAndDrop::take_payload::<ShelfDrag>(ui.ctx())
        {
            let ctx = ui.ctx().clone();
            match &*drag {
                ShelfDrag::Material(path) => self.shelf_load_material(&ctx, path.clone()),
                ShelfDrag::Texture(path) => self.shelf_load_texture(&ctx, path.clone()),
                ShelfDrag::Smart(source) => self.smart_apply(&ctx, source.clone()),
            }
        }
    }
}

fn empty_hint(ui: &mut egui::Ui, text: &str) {
    ui.label(egui::RichText::new(text).color(theme::TEXT_WEAK));
    ui.add_space(4.0);
}

/// A file name for something called `name`.
fn safe_name(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// A small check mark in the corner of a cell: already in the library.
fn badge(ui: &egui::Ui, response: &egui::Response) {
    let rect = egui::Rect::from_min_size(
        response.rect.right_top() + egui::vec2(-18.0, 4.0),
        egui::vec2(14.0, 14.0),
    );
    ui.painter()
        .circle_filled(rect.center(), 8.0, theme::OLIVE_FILL);
    theme::icon(icons::CHECK, 12.0, theme::TEXT_STRONG).paint_at(ui, rect);
}

/// One item of the grid: its picture and name.
fn cell(
    ui: &mut egui::Ui,
    name: &str,
    texture: Option<&TextureHandle>,
    selected: bool,
    square: bool,
) -> egui::Response {
    // One cell per row when the panel is narrower than a cell.
    let cell_size = egui::vec2(
        CELL.x.min(panel_width(ui) - 14.0).max(MIN_CELL_WIDTH),
        CELL.y,
    );
    let picture_size = egui::vec2(cell_size.x - 8.0, PICTURE_HEIGHT);
    let (rect, response) = ui.allocate_exact_size(cell_size, Sense::click_and_drag());
    let hovered = response.hovered();
    let painter = ui.painter_at(rect);
    let fill = if hovered {
        theme::WIDGET_HOVER
    } else {
        theme::RAISED
    };
    painter.rect_filled(rect, 2.0, fill);
    let picture = egui::Rect::from_center_size(
        rect.center_top() + egui::vec2(0.0, 4.0 + picture_size.y * 0.5),
        picture_size,
    );
    match texture {
        Some(texture) => {
            let size = texture.size_vec2();
            let scale = (picture_size.x / size.x).min(picture_size.y / size.y);
            let shown = if square {
                // Pictures are square and drawn as large as the box allows.
                egui::Rect::from_center_size(picture.center(), size * scale)
            } else {
                picture
            };
            painter.image(
                texture.id(),
                shown,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                Color32::WHITE,
            );
        }
        None => {
            painter.rect_filled(picture, 2.0, theme::BG_DEEP);
        }
    }
    let label = fit_text(ui, name, cell_size.x - 8.0);
    painter.text(
        egui::pos2(rect.center().x, rect.bottom() - 6.0),
        egui::Align2::CENTER_BOTTOM,
        label,
        egui::FontId::proportional(11.0),
        if selected {
            theme::TEXT_STRONG
        } else {
            theme::TEXT
        },
    );
    if selected {
        painter.rect_stroke(
            rect,
            2.0,
            egui::Stroke::new(2.0, theme::OLIVE),
            egui::StrokeKind::Inside,
        );
    }
    response
}

/// The text cut to fit a width, with an ellipsis.
fn fit_text(ui: &egui::Ui, text: &str, width: f32) -> String {
    let font = egui::FontId::proportional(11.0);
    let measure = |s: &str| {
        ui.painter()
            .layout_no_wrap(s.to_owned(), font.clone(), Color32::WHITE)
            .size()
            .x
    };
    if measure(text) <= width {
        return text.to_owned();
    }
    let mut chars: Vec<char> = text.chars().collect();
    while !chars.is_empty() {
        chars.pop();
        let candidate: String = chars.iter().collect::<String>() + "…";
        if measure(&candidate) <= width {
            return candidate;
        }
    }
    "…".to_owned()
}
