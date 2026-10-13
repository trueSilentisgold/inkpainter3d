//! The look: colors, fonts, spacing, icons, and helpers for the widgets that
//! recur across panels (headers, icon buttons, rows, property grids), so that panels don't
//! each pick their own.
//!
//! Surfaces are neutral dark grays with only a hint of olive, so that they don't tint how
//! colors on the model are judged (Painter is gray for the same reason). The brand colors
//! mark state: olive for selection and active controls, copper for highlights.

use eframe::egui::{
    self, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Image, ImageSource,
    Margin, Response, RichText, Shadow, Stroke, TextStyle, Theme, ThemePreference, Ui, Vec2,
    WidgetText, vec2,
};
use std::sync::Arc;

// ---- Colors (sRGB) ----

/// Text fields, wells, and the gap between the views.
pub const BG_DEEP: Color32 = Color32::from_rgb(0x1E, 0x1E, 0x1E);
/// Panels.
pub const PANEL: Color32 = Color32::from_rgb(0x2B, 0x2B, 0x2B);
/// Dialogs, menus and popups.
pub const WINDOW: Color32 = Color32::from_rgb(0x30, 0x30, 0x30);
/// Hovered rows, headers, thumbnail backgrounds.
pub const RAISED: Color32 = Color32::from_rgb(0x35, 0x35, 0x35);
/// Buttons and other controls.
pub const WIDGET: Color32 = Color32::from_rgb(0x3C, 0x3C, 0x3C);
pub const WIDGET_HOVER: Color32 = Color32::from_rgb(0x48, 0x48, 0x48);
pub const WIDGET_ACTIVE: Color32 = Color32::from_rgb(0x54, 0x54, 0x54);
/// Separators and frames.
pub const BORDER: Color32 = Color32::from_rgb(0x1F, 0x1F, 0x1F);
pub const BORDER_STRONG: Color32 = Color32::from_rgb(0x4A, 0x4A, 0x4A);

pub const TEXT: Color32 = Color32::from_rgb(0xD2, 0xD2, 0xD2);
pub const TEXT_STRONG: Color32 = Color32::from_rgb(0xF2, 0xF2, 0xF2);
/// Secondary text: hints, sizes, units. About 5:1 on [`PANEL`].
pub const TEXT_WEAK: Color32 = Color32::from_rgb(0x98, 0x98, 0x98);
/// Hidden or unavailable things.
pub const TEXT_DISABLED: Color32 = Color32::from_rgb(0x7A, 0x7A, 0x7A);

/// The brand olive: selection outlines, active toggles, success.
pub const OLIVE: Color32 = Color32::from_rgb(0x7A, 0xB8, 0x30);
/// Olive as a fill behind light text: selected buttons, slider fills, primary buttons.
pub const OLIVE_FILL: Color32 = Color32::from_rgb(0x4E, 0x6B, 0x24);
/// Background of a selected row.
pub const OLIVE_ROW: Color32 = Color32::from_rgb(0x2F, 0x3B, 0x22);
/// The brand copper: highlights such as drop targets, links and the text cursor.
pub const COPPER: Color32 = Color32::from_rgb(0xC8, 0x88, 0x40);

pub const WARN: Color32 = Color32::from_rgb(0xDC, 0x86, 0x05);
pub const DANGER: Color32 = Color32::from_rgb(0xE8, 0x65, 0x5F);

// ---- Fonts and text styles ----

const SANS: &str = "Space Grotesk";
const SANS_MEDIUM: &str = "Space Grotesk Medium";
const MONO: &str = "JetBrains Mono NL";
const MONO_MEDIUM: &str = "JetBrains Mono NL Medium";
/// Text style of panel and section headers: small monospace capitals, as on inkrabbit.de.
pub const HEADER: &str = "header";

fn header_style() -> TextStyle {
    TextStyle::Name(HEADER.into())
}

/// Space Grotesk for the interface, JetBrains Mono (the variant without ligatures, so that
/// numbers and ranges show as typed) for numbers and headers. egui's fonts stay behind them
/// for the glyphs these lack.
fn fonts() -> FontDefinitions {
    let mut fonts = FontDefinitions::default();
    let files: [(&str, &'static [u8]); 4] = [
        (
            SANS,
            include_bytes!("../../../assets/fonts/space-grotesk/SpaceGrotesk-Regular.ttf"),
        ),
        (
            SANS_MEDIUM,
            include_bytes!("../../../assets/fonts/space-grotesk/SpaceGrotesk-Medium.ttf"),
        ),
        (
            MONO,
            include_bytes!("../../../assets/fonts/jetbrains-mono/JetBrainsMonoNL-Regular.ttf"),
        ),
        (
            MONO_MEDIUM,
            include_bytes!("../../../assets/fonts/jetbrains-mono/JetBrainsMonoNL-Medium.ttf"),
        ),
    ];
    for (name, bytes) in files {
        fonts
            .font_data
            .insert(name.to_owned(), Arc::new(FontData::from_static(bytes)));
    }
    let fallbacks = |family: FontFamily| fonts.families.get(&family).cloned().unwrap_or_default();
    let (sans_fallbacks, mono_fallbacks) = (
        fallbacks(FontFamily::Proportional),
        fallbacks(FontFamily::Monospace),
    );
    let chain = |first: &str, rest: &[String]| {
        std::iter::once(first.to_owned())
            .chain(rest.iter().cloned())
            .collect::<Vec<_>>()
    };
    fonts
        .families
        .insert(FontFamily::Proportional, chain(SANS, &sans_fallbacks));
    fonts.families.insert(
        FontFamily::Name(SANS_MEDIUM.into()),
        chain(SANS_MEDIUM, &sans_fallbacks),
    );
    fonts
        .families
        .insert(FontFamily::Monospace, chain(MONO, &mono_fallbacks));
    fonts.families.insert(
        FontFamily::Name(MONO_MEDIUM.into()),
        chain(MONO_MEDIUM, &mono_fallbacks),
    );
    fonts
}

/// The medium weight of the interface font, for emphasis (the tool name, dialog titles).
pub fn medium(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(SANS_MEDIUM.into()))
}

// ---- Style ----

/// Installs the theme, the fonts and the SVG loader. Call once at startup.
pub fn install(ctx: &egui::Context) {
    egui_extras::install_image_loaders(ctx);
    ctx.set_fonts(fonts());
    ctx.set_theme(ThemePreference::Dark);
    ctx.set_style_of(Theme::Dark, style());
}

fn style() -> egui::Style {
    let mut style = Theme::Dark.default_style();
    style.text_styles = [
        (
            TextStyle::Small,
            FontId::new(11.0, FontFamily::Proportional),
        ),
        (TextStyle::Body, FontId::new(13.0, FontFamily::Proportional)),
        (
            TextStyle::Button,
            FontId::new(13.0, FontFamily::Proportional),
        ),
        (TextStyle::Heading, medium(17.0)),
        (
            TextStyle::Monospace,
            FontId::new(12.0, FontFamily::Monospace),
        ),
        (
            header_style(),
            FontId::new(11.0, FontFamily::Name(MONO_MEDIUM.into())),
        ),
    ]
    .into();
    // Numbers in monospace, so that columns of values line up.
    style.drag_value_text_style = TextStyle::Monospace;

    let s = &mut style.spacing;
    s.item_spacing = vec2(6.0, 4.0);
    s.button_padding = vec2(6.0, 3.0);
    s.interact_size = vec2(36.0, 20.0);
    s.window_margin = Margin::same(14);
    s.menu_margin = Margin::same(5);
    s.indent = 14.0;
    s.slider_width = 120.0;
    s.slider_rail_height = 4.0;
    s.combo_width = 120.0;

    let v = &mut style.visuals;
    v.panel_fill = PANEL;
    v.window_fill = WINDOW;
    v.extreme_bg_color = BG_DEEP;
    v.faint_bg_color = RAISED;
    v.code_bg_color = RAISED;
    v.hyperlink_color = COPPER;
    v.warn_fg_color = WARN;
    v.error_fg_color = DANGER;
    v.weak_text_color = Some(TEXT_WEAK);
    v.window_corner_radius = CornerRadius::same(4);
    v.menu_corner_radius = CornerRadius::same(3);
    v.window_stroke = Stroke::new(1.0, BORDER_STRONG);
    v.window_shadow = Shadow {
        offset: [0, 8],
        blur: 24,
        spread: 0,
        color: Color32::from_black_alpha(150),
    };
    v.popup_shadow = Shadow {
        offset: [0, 4],
        blur: 12,
        spread: 0,
        color: Color32::from_black_alpha(130),
    };
    v.selection.bg_fill = OLIVE_FILL;
    v.selection.stroke = Stroke::new(1.0, TEXT_STRONG);
    v.text_cursor.stroke = Stroke::new(2.0, COPPER);
    v.slider_trailing_fill = true;
    v.collapsing_header_frame = true;
    v.handle_shape = egui::style::HandleShape::Circle;

    let radius = CornerRadius::same(2);
    let w = &mut v.widgets;
    w.noninteractive.bg_fill = PANEL;
    w.noninteractive.weak_bg_fill = PANEL;
    w.noninteractive.bg_stroke = Stroke::new(1.0, BORDER);
    w.noninteractive.fg_stroke = Stroke::new(1.0, TEXT);
    w.noninteractive.corner_radius = radius;

    w.inactive.bg_fill = WIDGET;
    w.inactive.weak_bg_fill = WIDGET;
    w.inactive.bg_stroke = Stroke::NONE;
    w.inactive.fg_stroke = Stroke::new(1.0, TEXT);
    w.inactive.corner_radius = radius;

    w.hovered.bg_fill = WIDGET_HOVER;
    w.hovered.weak_bg_fill = WIDGET_HOVER;
    w.hovered.bg_stroke = Stroke::new(1.0, BORDER_STRONG);
    w.hovered.fg_stroke = Stroke::new(1.5, TEXT_STRONG);
    w.hovered.corner_radius = radius;
    w.hovered.expansion = 0.0;

    w.active.bg_fill = WIDGET_ACTIVE;
    w.active.weak_bg_fill = WIDGET_ACTIVE;
    w.active.bg_stroke = Stroke::new(1.0, OLIVE);
    w.active.fg_stroke = Stroke::new(2.0, TEXT_STRONG);
    w.active.corner_radius = radius;
    w.active.expansion = 0.0;

    w.open.bg_fill = PANEL;
    w.open.weak_bg_fill = WIDGET_HOVER;
    w.open.bg_stroke = Stroke::new(1.0, BORDER_STRONG);
    w.open.fg_stroke = Stroke::new(1.0, TEXT_STRONG);
    w.open.corner_radius = radius;
    style
}

// ---- Icons ----

/// The icon set (`assets/icons`): white line drawings on a 24 × 24 grid, recolored with a
/// tint where they are drawn.
pub mod icons {
    use eframe::egui::{ImageSource, include_image};

    macro_rules! icons {
        ($($name:ident = $file:literal,)*) => {
            $(#[allow(dead_code)]
            pub const $name: ImageSource<'static> =
                include_image!(concat!("../../../assets/icons/", $file, ".svg"));)*
        };
    }

    icons! {
        INKRABBIT = "inkrabbit",
        TOOL_PAINT = "tool-paint",
        TOOL_ERASER = "tool-eraser",
        VISIBLE = "visible",
        HIDDEN = "hidden",
        CHEVRON_RIGHT = "chevron-right",
        CHEVRON_DOWN = "chevron-down",
        LAYER_PAINT = "layer-paint",
        LAYER_FILL = "layer-fill",
        FOLDER = "folder",
        FOLDER_OPEN = "folder-open",
        ADD_EFFECT = "add-effect",
        ADD_MASK = "add-mask",
        ADD_PAINT_LAYER = "add-paint-layer",
        ADD_FILL_LAYER = "add-fill-layer",
        ADD_FOLDER = "add-folder",
        DELETE = "delete",
        EFFECT_LEVELS = "effect-levels",
        REMOVE = "remove",
        INVERT = "invert",
        LINK = "link",
        UNLINK = "unlink",
        IMAGE = "image",
        MENU = "menu",
        FOCUS = "focus",
        MULTI_MATERIAL = "multi-material",
        PEN_PRESSURE = "pen-pressure",
        CHECK = "check",
        ERROR = "error",
        WARNING = "warning",
        INFO = "info",
    }
}

/// Size of icons in rows and small buttons.
pub const ICON: f32 = 16.0;
/// Size of icons in the tool bar.
pub const TOOL_ICON: f32 = 22.0;

/// An icon at a size, tinted.
pub fn icon(source: ImageSource<'static>, size: f32, tint: Color32) -> Image<'static> {
    Image::new(source)
        .fit_to_exact_size(Vec2::splat(size))
        .tint(tint)
}

/// A frameless icon button that shows its frame on hover, like the buttons of Painter's
/// panel tool bars.
pub fn icon_button(ui: &mut Ui, source: ImageSource<'static>, tip: &str) -> Response {
    ui.add(
        egui::Button::new(Image::new(source).fit_to_exact_size(Vec2::splat(ICON)))
            .image_tint_follows_text_color(true)
            .frame_when_inactive(false)
            .min_size(Vec2::splat(22.0)),
    )
    .on_hover_text(tip)
}

/// An icon button that toggles a state; olive while on.
pub fn icon_toggle(
    ui: &mut Ui,
    source: ImageSource<'static>,
    on: &mut bool,
    tip: &str,
) -> Response {
    let mut response = ui
        .add(
            egui::Button::selectable(*on, Image::new(source).fit_to_exact_size(Vec2::splat(ICON)))
                .image_tint_follows_text_color(true)
                .min_size(Vec2::splat(22.0)),
        )
        .on_hover_text(tip);
    if response.clicked() {
        *on = !*on;
        response.mark_changed();
    }
    response
}

/// An icon button that opens a menu.
pub fn icon_menu<R>(
    ui: &mut Ui,
    source: ImageSource<'static>,
    tip: &str,
    add_contents: impl FnOnce(&mut Ui) -> R,
) -> Response {
    let button = egui::Button::new(Image::new(source).fit_to_exact_size(Vec2::splat(ICON)))
        .image_tint_follows_text_color(true)
        .frame_when_inactive(false)
        .min_size(Vec2::splat(22.0));
    let (response, _) =
        egui::containers::menu::MenuButton::from_button(button).ui(ui, add_contents);
    response.on_hover_text(tip)
}

/// The visibility toggle of texture sets, objects, layers and effects.
pub fn eye(ui: &mut Ui, visible: bool) -> Response {
    let (source, tint, tip) = if visible {
        (icons::VISIBLE, TEXT, "Hide")
    } else {
        (icons::HIDDEN, TEXT_DISABLED, "Show")
    };
    ui.add(
        egui::Button::new(icon(source, ICON, tint))
            .frame_when_inactive(false)
            .min_size(Vec2::splat(20.0)),
    )
    .on_hover_text(tip)
}

/// The expand / collapse arrow of texture sets and folders.
pub fn disclosure(ui: &mut Ui, open: bool) -> Response {
    let source = if open {
        icons::CHEVRON_DOWN
    } else {
        icons::CHEVRON_RIGHT
    };
    ui.add(
        egui::Button::new(icon(source, 14.0, TEXT_WEAK))
            .frame(false)
            .min_size(vec2(14.0, 20.0)),
    )
}

// ---- Headers and labels ----

/// A panel title such as `LAYERS`: small capitals, as in the title bars of Painter's panels.
pub fn header(title: &str) -> WidgetText {
    let format = egui::TextFormat {
        font_id: FontId::new(11.0, FontFamily::Name(SANS_MEDIUM.into())),
        color: TEXT,
        extra_letter_spacing: 0.8,
        valign: egui::Align::Center,
        ..Default::default()
    };
    let mut job = egui::text::LayoutJob::default();
    job.append(&title.to_uppercase(), 0.0, format);
    job.into()
}

/// A collapsible panel section with a [`header`] title.
pub fn section(title: &str) -> egui::CollapsingHeader {
    egui::CollapsingHeader::new(header(title))
        .id_salt(title)
        .default_open(true)
}

/// A group label inside a panel (BRUSH, ALPHA, MATERIAL …).
pub fn group_label(ui: &mut Ui, text: &str) {
    ui.add_space(4.0);
    ui.label(
        RichText::new(text.to_uppercase())
            .text_style(header_style())
            .color(TEXT_WEAK),
    );
}

/// Width of the label column of property grids, so that controls line up across sections.
pub const LABEL_WIDTH: f32 = 108.0;

/// A two-column grid of labels and controls.
pub fn property_grid(id: impl egui::AsIdSalt) -> egui::Grid {
    egui::Grid::new(id)
        .num_columns(2)
        .spacing([10.0, 6.0])
        .min_col_width(LABEL_WIDTH)
}

/// A toggle chip, such as the channels a brush or fill layer sets (color, height, rough …):
/// outlined in olive while on, as Painter outlines enabled channels.
pub fn chip(ui: &mut Ui, on: bool, text: &str) -> Response {
    let (fill, stroke, color) = if on {
        (OLIVE_ROW, Stroke::new(1.0, OLIVE), TEXT_STRONG)
    } else {
        (WIDGET, Stroke::new(1.0, WIDGET), TEXT_WEAK)
    };
    ui.add(
        egui::Button::new(RichText::new(text).color(color))
            .fill(fill)
            .stroke(stroke)
            .corner_radius(10.0),
    )
}

/// The confirming button of a dialog.
pub fn primary_button(text: &str) -> egui::Button<'static> {
    egui::Button::new(RichText::new(text).color(TEXT_STRONG)).fill(OLIVE_FILL)
}

// ---- Rows ----

/// Background of list rows (texture sets, layers, effects): olive with an outline when
/// selected, a lighter gray when hovered.
pub fn paint_row(ui: &Ui, rect: egui::Rect, selected: bool, hovered: bool) {
    let painter = ui.painter();
    if selected {
        painter.rect(
            rect,
            3.0,
            OLIVE_ROW,
            Stroke::new(1.0, OLIVE),
            egui::StrokeKind::Inside,
        );
    } else if hovered {
        painter.rect_filled(rect, 3.0, RAISED);
    }
}

/// Where a dragged layer will land.
pub const DROP_LINE: Stroke = Stroke {
    width: 2.0,
    color: COPPER,
};

// ---- Viewport overlays ----

/// Text over the rendered views, on a translucent plate so that it reads over any HDRI.
pub fn overlay_text(
    painter: &egui::Painter,
    pos: egui::Pos2,
    anchor: egui::Align2,
    text: &str,
) -> egui::Rect {
    let galley = painter.layout_no_wrap(
        text.to_owned(),
        FontId::new(12.0, FontFamily::Proportional),
        TEXT,
    );
    let rect = anchor
        .anchor_size(pos, galley.size())
        .expand2(vec2(6.0, 3.0));
    painter.rect_filled(rect, 4.0, Color32::from_black_alpha(140));
    painter.galley(rect.min + vec2(6.0, 3.0), galley, TEXT);
    rect
}

/// The title bar of a panel that is not collapsible: the title on the left, `right` (icon
/// buttons) on the right, on the same bar as [`section`] headers.
pub fn title_bar(ui: &mut Ui, title: &str, right: impl FnOnce(&mut Ui)) {
    egui::Frame::new()
        .fill(RAISED)
        .corner_radius(2.0)
        .inner_margin(Margin::symmetric(4, 2))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(header(title));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), right);
            });
        });
}
