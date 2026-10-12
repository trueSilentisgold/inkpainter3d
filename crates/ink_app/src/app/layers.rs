//! The layer stack (Painter's LAYERS panel): layers, folders, masks and their effects, the
//! selection, the edits on them with undo, and the properties of fill layers and effects.

use super::InkApp;
use eframe::egui::{self, Color32, Key, RichText, Sense, Stroke as Line};
use ink_core::{
    BlendMode, Channel, ChannelKind, ChannelSet, Command, Content, Effect, EffectKind, Fill,
    GrayFill, Layer, LayerKind, LayerStack, Levels, Mask, Place, Projection, ProjectionMode,
    TextureImage,
};
use std::sync::Arc;

/// Largest projection scale and offset: typed values like "inf" stay finite.
const PROJECTION_RANGE: f32 = 1000.0;

/// What in a layer is selected.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    /// The layer itself: its paint, fill or children.
    Content,
    Mask,
    /// An effect in the mask, by id.
    Effect(u64),
}

/// The selection in a texture set's layer stack.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Selection {
    pub layer: u64,
    pub target: Target,
    /// Layers added with Ctrl+click, for grouping, deleting and copying several at once.
    pub more: Vec<u64>,
}

impl Selection {
    fn layer(layer: u64) -> Self {
        Self {
            layer,
            target: Target::Content,
            more: Vec::new(),
        }
    }

    /// The selected layers, the main one first.
    fn layers(&self) -> Vec<u64> {
        let mut layers = vec![self.layer];
        layers.extend(self.more.iter().filter(|&&l| l != self.layer));
        layers
    }
}

/// A change to a layer stack that is still going on (a slider being dragged), with the
/// stack as it was before, for undo.
pub struct PendingEdit {
    set: usize,
    before: LayerStack,
    label: String,
}

/// Layer stack state of the app.
pub struct LayersState {
    /// Per texture set (by index).
    pub selections: Vec<Option<Selection>>,
    /// The channel whose blending the rows show and edit (Painter's view mode dropdown).
    pub channel: Channel,
    pub pending: Option<PendingEdit>,
    /// Name of the next recorded edit; set by the action that makes it.
    label: Option<&'static str>,
    clipboard: Clipboard,
    /// Something was copied that the system clipboard doesn't know about yet.
    copied: bool,
    /// Layer being renamed and the name typed so far.
    renaming: Option<(u64, String)>,
    /// Where the effect being dragged would land, found while drawing the rows.
    effect_drop: Option<EffectDrop>,
    /// Images loaded for fills, offered again in every image slot.
    pub images: Vec<Arc<TextureImage>>,
}

impl Default for LayersState {
    fn default() -> Self {
        Self {
            selections: Vec::new(),
            channel: Channel::BaseColor,
            pending: None,
            label: None,
            clipboard: Clipboard::Empty,
            copied: false,
            effect_drop: None,
            renaming: None,
            images: Vec::new(),
        }
    }
}

impl LayersState {
    pub fn reset(&mut self, sets: usize) {
        self.selections = vec![None; sets];
        self.pending = None;
        self.label = None;
        self.renaming = None;
        // Copied strokes belong to the old model's surface.
        self.clipboard = Clipboard::Empty;
    }
}

/// What Ctrl+C copied: layers, or an effect of a mask.
enum Clipboard {
    Empty,
    Layers(Vec<Layer>),
    Effect(Effect),
}

/// Where a dragged layer would land, and the line that shows it.
struct Drop {
    place: Place,
    line: [egui::Pos2; 2],
}

/// The drag-and-drop payload of an effect, which moves within its mask.
#[derive(Clone, Copy)]
struct EffectDrag {
    layer: u64,
    effect: u64,
}

/// Where a dragged effect would land: before index `slot` of its mask's effects (bottom to
/// top), and the line that shows it.
#[derive(Clone, Copy)]
struct EffectDrop {
    slot: usize,
    line: [egui::Pos2; 2],
}

const ROW_HEIGHT: f32 = 34.0;
const INDENT: f32 = 14.0;

impl InkApp {
    pub(super) fn stack(&self) -> Option<&LayerStack> {
        self.model
            .texture_sets
            .get(self.selected_set)
            .map(|s| &s.stack)
    }

    fn stack_mut(&mut self) -> Option<&mut LayerStack> {
        self.model
            .texture_sets
            .get_mut(self.selected_set)
            .map(|s| &mut s.stack)
    }

    pub(super) fn selection(&self) -> Option<&Selection> {
        self.layers.selections.get(self.selected_set)?.as_ref()
    }

    fn select(&mut self, selection: Selection) {
        if let Some(slot) = self.layers.selections.get_mut(self.selected_set) {
            *slot = Some(selection);
        }
    }

    /// Keeps the selection pointing at something that exists: after undo, deletes or a new
    /// model. Falls back to the topmost layer.
    pub(super) fn validate_selection(&mut self) {
        let sets = self.model.texture_sets.len();
        if self.layers.selections.len() != sets {
            self.layers.selections.resize(sets, None);
        }
        for (set, slot) in self.layers.selections.iter_mut().enumerate() {
            let stack = &self.model.texture_sets[set].stack;
            // A mask or effect that went away leaves the selection on what holds it: an
            // effect's mask, a mask's layer. Only a layer that went away moves it elsewhere.
            if let Some(selection) = slot.as_mut()
                && let Some(layer) = stack.find(selection.layer)
            {
                let mask = layer.mask.as_ref();
                if let Target::Effect(e) = selection.target
                    && mask.is_none_or(|m| m.effect(e).is_none())
                {
                    selection.target = Target::Mask;
                }
                if selection.target == Target::Mask && mask.is_none() {
                    selection.target = Target::Content;
                }
            }
            if slot.as_ref().is_none_or(|s| stack.find(s.layer).is_none()) {
                *slot = stack.layers.last().map(|l| Selection::layer(l.id));
            }
            if let Some(selection) = slot {
                selection.more.retain(|&id| stack.find(id).is_some());
            }
        }
    }

    /// The paint layer the brush paints into: the selected paint layer, mask or paint
    /// effect. None for fill layers, folders and other effects, which can't be painted.
    pub(super) fn paint_target(&self) -> Option<(usize, u64)> {
        let selection = self.selection()?;
        let layer = self.stack()?.find(selection.layer)?;
        let paint = match (selection.target, &layer.content) {
            (Target::Content, Content::Paint(paint)) => paint.id,
            (Target::Mask, _) => layer.mask.as_ref()?.paint.id,
            (Target::Effect(id), _) => match &layer.mask.as_ref()?.effect(id)?.kind {
                EffectKind::Paint(paint) => paint.id,
                _ => return None,
            },
            _ => return None,
        };
        Some((self.selected_set, paint))
    }

    /// Whether the brush paints grayscale (into a mask) rather than material channels.
    pub(super) fn paints_grayscale(&self) -> bool {
        self.selection()
            .is_some_and(|s| matches!(s.target, Target::Mask | Target::Effect(_)))
    }

    /// Why nothing can be painted now, for the status bar.
    pub(super) fn not_paintable_reason(&self) -> &'static str {
        match self
            .selection()
            .and_then(|s| Some((s, self.stack()?.find(s.layer)?)))
        {
            Some((s, layer)) if s.target == Target::Content => match layer.content {
                Content::Fill(_) => {
                    "A fill layer can't be painted: paint into its mask, or select a paint layer"
                }
                Content::Folder(_) => "A folder can't be painted: select a paint layer or a mask",
                Content::Paint(_) => "",
            },
            Some(_) => "This effect can't be painted: select the mask or a paint effect",
            None => "Add a paint layer to paint",
        }
    }

    // ---- Undo of stack edits ----

    /// Remembers the selected set's stack before the panels that edit it run.
    pub(super) fn begin_stack_edits(&mut self, ui: &egui::Ui) -> Option<(usize, LayerStack)> {
        // A value still being typed is done once the user clicks elsewhere: the click's own
        // edit becomes a step of its own.
        let typed_then_clicked =
            ui.ctx().text_edit_focused() && ui.input(|i| i.pointer.any_pressed());
        if self
            .layers
            .pending
            .as_ref()
            .is_some_and(|p| p.set != self.selected_set || typed_then_clicked)
        {
            self.commit_stack_edit();
        }
        if self.layers.pending.is_some() {
            return None;
        }
        Some((self.selected_set, self.stack()?.clone()))
    }

    /// Records what the panels changed as one undo step, once the interaction that made
    /// the change is over (no button held, no text being typed).
    pub(super) fn end_stack_edits(&mut self, ui: &egui::Ui, before: Option<(usize, LayerStack)>) {
        // Compared with the same set: the panels may have selected another one meanwhile.
        if let Some((set, before)) = before
            && self
                .model
                .texture_sets
                .get(set)
                .is_some_and(|s| s.stack != before)
        {
            self.layers.pending = Some(PendingEdit {
                set,
                before,
                label: self.layers.label.unwrap_or("Edit layers").to_owned(),
            });
        }
        self.layers.label = None;
        let busy = ui.input(|i| i.pointer.any_down()) || ui.ctx().text_edit_focused();
        if !busy {
            self.commit_stack_edit();
        }
    }

    pub(super) fn commit_stack_edit(&mut self) {
        if let Some(edit) = self.layers.pending.take() {
            let changed = self
                .model
                .texture_sets
                .get(edit.set)
                .is_some_and(|s| s.stack != edit.before);
            if changed {
                self.history
                    .record(Command::edit_stack(edit.set, edit.label, edit.before));
            }
        }
    }

    /// Names the undo step of the edit being made now.
    fn label(&mut self, label: &'static str) {
        self.layers.label.get_or_insert(label);
    }

    // ---- Edits ----

    fn add_layer(&mut self, kind: LayerKind) {
        let anchor = self.selection().map(|s| s.layer);
        let Some(texture_set) = self.model.texture_sets.get_mut(self.selected_set) else {
            return;
        };
        let stack = &mut texture_set.stack;
        let layer = Layer::new(kind, stack.new_name(kind), &mut self.model.ids);
        let id = layer.id;
        let place = stack.place_above(anchor);
        if stack.insert(place, layer).is_ok() {
            self.label(match kind {
                LayerKind::Paint => "Add paint layer",
                LayerKind::Fill => "Add fill layer",
                LayerKind::Folder => "Add folder",
            });
            self.select(Selection::layer(id));
        }
    }

    fn delete_selected(&mut self) {
        let Some(selection) = self.selection().cloned() else {
            return;
        };
        if let Target::Effect(effect) = selection.target {
            if let Some(mask) = self
                .stack_mut()
                .and_then(|s| s.find_mut(selection.layer))
                .and_then(|l| l.mask.as_mut())
            {
                mask.effects.retain(|e| e.id != effect);
                self.label("Delete effect");
                self.select(Selection {
                    target: Target::Mask,
                    ..Selection::layer(selection.layer)
                });
            }
            return;
        }
        let Some(stack) = self.stack_mut() else {
            return;
        };
        // The layer below the deleted one gets the selection, or the one above.
        let neighbour = stack.place(selection.layer).and_then(|place| {
            let list = stack.list(place.parent)?;
            let ids: Vec<u64> = list.iter().map(|l| l.id).collect();
            let gone = selection.layers();
            ids[..place.index]
                .iter()
                .rev()
                .chain(ids[place.index..].iter())
                .find(|id| !gone.contains(id))
                .copied()
                .or(place.parent)
        });
        for id in selection.layers() {
            stack.remove(id);
        }
        self.label("Delete layer");
        if let Some(slot) = self.layers.selections.get_mut(self.selected_set) {
            *slot = neighbour.map(Selection::layer);
        }
    }

    fn duplicate_selected(&mut self) {
        let Some(selection) = self.selection().cloned() else {
            return;
        };
        let Some(texture_set) = self.model.texture_sets.get_mut(self.selected_set) else {
            return;
        };
        let stack = &mut texture_set.stack;
        let mut last = None;
        for id in selection.layers() {
            let (Some(layer), Some(place)) = (stack.find(id), stack.place(id)) else {
                continue;
            };
            let mut copy = layer.duplicate(&mut self.model.ids);
            copy.name = copy_name(stack, &layer.name);
            last = Some(copy.id);
            let _ = stack.insert(
                Place {
                    parent: place.parent,
                    index: place.index + 1,
                },
                copy,
            );
        }
        if let Some(id) = last {
            self.label("Duplicate layer");
            self.select(Selection::layer(id));
        }
    }

    /// Copies the selected layers, or the selected effect.
    fn copy_selected(&mut self) {
        let (Some(selection), Some(stack)) = (self.selection(), self.stack()) else {
            return;
        };
        if let Target::Effect(id) = selection.target {
            if let Some(effect) = stack
                .find(selection.layer)
                .and_then(|l| l.mask.as_ref()?.effect(id))
            {
                self.layers.clipboard = Clipboard::Effect(effect.clone());
                self.layers.copied = true;
            }
            return;
        }
        let mut layers: Vec<Layer> = selection
            .layers()
            .into_iter()
            .filter_map(|id| stack.find(id).cloned())
            .collect();
        // Bottom to top, so pasting keeps their order.
        let order: Vec<u64> = stack.iter().map(|l| l.id).collect();
        layers.sort_by_key(|l| order.iter().position(|&id| id == l.id));
        self.layers.clipboard = Clipboard::Layers(layers);
        self.layers.copied = true;
    }

    /// Puts the names of what was copied on the system clipboard: Ctrl+V only arrives as a
    /// paste while it holds text.
    pub(super) fn sync_system_clipboard(&mut self, ui: &egui::Ui) {
        if !std::mem::take(&mut self.layers.copied) {
            return;
        }
        let names: Vec<&str> = match &self.layers.clipboard {
            Clipboard::Empty => return,
            Clipboard::Layers(layers) => layers.iter().map(|l| l.name.as_str()).collect(),
            Clipboard::Effect(effect) => vec![effect.label()],
        };
        ui.ctx().copy_text(names.join("\n"));
    }

    fn cut_selected(&mut self) {
        let effect = self
            .selection()
            .is_some_and(|s| matches!(s.target, Target::Effect(_)));
        self.label(if effect { "Cut effect" } else { "Cut layer" });
        self.copy_selected();
        self.delete_selected();
    }

    /// Pastes copied layers above the selected layer, or a copied effect into the selected
    /// layer's mask (above the selected effect).
    fn paste(&mut self) {
        let layers = match &self.layers.clipboard {
            Clipboard::Empty => return,
            Clipboard::Effect(effect) => {
                let effect = effect.clone();
                self.paste_effect(&effect);
                return;
            }
            Clipboard::Layers(layers) => layers.clone(),
        };
        let anchor = self.selection().map(|s| s.layer);
        let Some(texture_set) = self.model.texture_sets.get_mut(self.selected_set) else {
            return;
        };
        let stack = &mut texture_set.stack;
        let mut place = stack.place_above(anchor);
        let mut last = None;
        for layer in &layers {
            let copy = layer.duplicate(&mut self.model.ids);
            last = Some(copy.id);
            if stack.insert(place, copy).is_ok() {
                place.index += 1;
            }
        }
        if let Some(id) = last {
            self.label("Paste layer");
            self.select(Selection::layer(id));
        }
    }

    fn paste_effect(&mut self, effect: &Effect) {
        let Some(selection) = self.selection().cloned() else {
            return;
        };
        self.label("Paste effect");
        let has_mask = self
            .stack()
            .and_then(|s| s.find(selection.layer))
            .is_some_and(|l| l.mask.is_some());
        if !has_mask {
            self.add_mask(selection.layer, 0.0);
        }
        let copy = effect.duplicate(&mut self.model.ids);
        let id = copy.id;
        let Some(mask) = self.mask_mut(selection.layer) else {
            return;
        };
        let index = match selection.target {
            Target::Effect(e) => mask
                .effects
                .iter()
                .position(|x| x.id == e)
                .map_or(mask.effects.len(), |i| i + 1),
            _ => mask.effects.len(),
        };
        mask.effects.insert(index, copy);
        self.select(Selection {
            target: Target::Effect(id),
            ..Selection::layer(selection.layer)
        });
    }

    /// Points the selection at the selected layers themselves, not at a mask or effect of
    /// theirs: the layer context menu acts on layers.
    fn select_layers_themselves(&mut self) {
        if let Some(Some(selection)) = self.layers.selections.get_mut(self.selected_set) {
            selection.target = Target::Content;
        }
    }

    fn group_selected(&mut self) {
        let Some(selection) = self.selection().cloned() else {
            return;
        };
        let Some(texture_set) = self.model.texture_sets.get_mut(self.selected_set) else {
            return;
        };
        if let Some(folder) = texture_set
            .stack
            .group(&selection.layers(), &mut self.model.ids)
        {
            self.label("Group layers");
            self.select(Selection::layer(folder));
        }
    }

    fn add_mask(&mut self, layer: u64, background: f32) {
        let paint = self.model.next_id();
        let Some(target) = self.stack_mut().and_then(|s| s.find_mut(layer)) else {
            return;
        };
        target.mask = Some(Mask::new(background, paint));
        self.label(if background > 0.5 {
            "Add white mask"
        } else {
            "Add black mask"
        });
        self.select(Selection {
            target: Target::Mask,
            ..Selection::layer(layer)
        });
        // As in Painter, the brush paints the opposite of the new mask.
        self.paint.paint_brush.material.grayscale = 1.0 - background;
    }

    fn add_effect(&mut self, layer: u64, kind: EffectKind) {
        // Named first: a mask added on the way is part of the same step.
        self.label(match kind {
            EffectKind::Paint(_) => "Add paint effect",
            EffectKind::Fill(_) => "Add fill effect",
            EffectKind::Levels(_) => "Add levels",
        });
        let has_mask = self
            .stack()
            .and_then(|s| s.find(layer))
            .is_some_and(|l| l.mask.is_some());
        if !has_mask {
            self.add_mask(layer, 0.0);
        }
        let id = self.model.next_id();
        let kind = match kind {
            EffectKind::Paint(_) => {
                EffectKind::Paint(ink_core::PaintLayer::new(self.model.next_id()))
            }
            other => other,
        };
        if let Some(mask) = self
            .stack_mut()
            .and_then(|s| s.find_mut(layer))
            .and_then(|l| l.mask.as_mut())
        {
            mask.effects.push(Effect::new(id, kind));
            self.select(Selection {
                target: Target::Effect(id),
                ..Selection::layer(layer)
            });
        }
    }

    /// Layer stack shortcuts: Ctrl+D duplicate, Ctrl+G group; with the pointer over the
    /// layer stack also Ctrl+C / X / V and Delete, as in Painter.
    pub(super) fn layer_shortcuts(&mut self, ui: &egui::Ui, over_stack: bool) {
        if ui.ctx().text_edit_focused() || self.paint.is_busy() {
            return;
        }
        let (command, shift) = ui.input(|i| (i.modifiers.command, i.modifiers.shift));
        let pressed = |key| ui.input(|i| i.key_pressed(key));
        if command && !shift {
            if pressed(Key::D) {
                self.duplicate_selected();
            }
            if pressed(Key::G) {
                self.group_selected();
            }
        }
        if over_stack {
            // Ctrl+C / X / V arrive as clipboard events, not as key presses.
            let events = ui.input(|i| i.events.clone());
            for event in events {
                match event {
                    egui::Event::Copy => self.copy_selected(),
                    egui::Event::Cut => self.cut_selected(),
                    egui::Event::Paste(_) => self.paste(),
                    _ => {}
                }
            }
        }
        if over_stack && !command && pressed(Key::Delete) {
            self.delete_selected();
        }
    }

    // ---- The panel ----

    /// Painter's LAYERS panel.
    pub(super) fn layer_stack_panel(&mut self, ui: &mut egui::Ui) {
        self.layer_toolbar(ui);
        ui.separator();
        let Some(stack) = self.stack().cloned() else {
            return;
        };
        let mut drop: Option<Drop> = None;
        let dragged = egui::DragAndDrop::payload::<u64>(ui.ctx()).map(|p| *p);
        egui::ScrollArea::vertical()
            .id_salt("layer stack")
            .max_height(320.0)
            .show(ui, |ui| {
                if stack.layers.is_empty() {
                    ui.label(RichText::new("No layers. Add one with the buttons above.").weak());
                }
                self.layers.effect_drop = None;
                self.layer_rows(ui, &stack, &stack.layers, None, 0, dragged, &mut drop);
            });
        self.drop_effect(ui);
        if let Some(id) = dragged {
            if let Some(drop) = &drop {
                ui.painter()
                    .line_segment(drop.line, Line::new(2.0, ui.visuals().selection.bg_fill));
            }
            if ui.input(|i| i.pointer.any_released()) {
                if let Some(drop) = drop
                    && let Some(stack) = self.stack_mut()
                    && stack.move_to(id, drop.place)
                {
                    if let Some(parent) = drop.place.parent
                        && let Some(Content::Folder(folder)) =
                            stack.find_mut(parent).map(|l| &mut l.content)
                    {
                        folder.expanded = true;
                    }
                    self.label("Move layer");
                }
                egui::DragAndDrop::clear_payload(ui.ctx());
            }
        }
    }

    fn layer_toolbar(&mut self, ui: &mut egui::Ui) {
        let selected = self.selection().map(|s| s.layer);
        ui.horizontal(|ui| {
            egui::ComboBox::from_id_salt("stack channel")
                .selected_text(self.layers.channel.label())
                .width(110.0)
                .show_ui(ui, |ui| {
                    for channel in Channel::ALL {
                        ui.selectable_value(&mut self.layers.channel, channel, channel.label());
                    }
                })
                .response
                .on_hover_text("The channel whose blending mode and opacity the layers show");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let enabled = selected.is_some();
                if ui
                    .add_enabled(enabled, egui::Button::new("🗑"))
                    .on_hover_text("Delete the selected layer or effect (Delete)")
                    .clicked()
                {
                    self.delete_selected();
                }
                if ui
                    .button("🗀")
                    .on_hover_text("Add a folder above the selected layer")
                    .clicked()
                {
                    self.add_layer(LayerKind::Folder);
                }
                if ui
                    .button("🎨")
                    .on_hover_text("Add a fill layer above the selected layer")
                    .clicked()
                {
                    self.add_layer(LayerKind::Fill);
                }
                if ui
                    .button("🖊")
                    .on_hover_text("Add a paint layer above the selected layer")
                    .clicked()
                {
                    self.add_layer(LayerKind::Paint);
                }
                ui.add_enabled_ui(enabled, |ui| {
                    ui.menu_button("◑", |ui| {
                        if let Some(layer) = selected {
                            self.mask_menu(ui, layer);
                        }
                    })
                    .response
                    .on_hover_text("Add a mask to the selected layer");
                    ui.menu_button("✨", |ui| {
                        if let Some(layer) = selected {
                            self.effect_menu(ui, layer);
                        }
                    })
                    .response
                    .on_hover_text("Add an effect to the selected layer's mask");
                });
            });
        });
    }

    fn mask_menu(&mut self, ui: &mut egui::Ui, layer: u64) {
        let has_mask = self
            .stack()
            .and_then(|s| s.find(layer))
            .is_some_and(|l| l.mask.is_some());
        if ui.button("Add white mask").clicked() {
            self.add_mask(layer, 1.0);
            ui.close();
        }
        if ui.button("Add black mask").clicked() {
            self.add_mask(layer, 0.0);
            ui.close();
        }
        if has_mask {
            ui.separator();
            if ui.button("Toggle mask (Shift+click)").clicked() {
                if let Some(mask) = self.mask_mut(layer) {
                    mask.enabled = !mask.enabled;
                    self.label("Toggle mask");
                }
                ui.close();
            }
            if ui.button("Invert mask background").clicked() {
                if let Some(mask) = self.mask_mut(layer) {
                    mask.background = 1.0 - mask.background;
                    self.label("Invert mask background");
                }
                ui.close();
            }
            if ui.button("Remove mask").clicked() {
                if let Some(target) = self.stack_mut().and_then(|s| s.find_mut(layer)) {
                    target.mask = None;
                    self.label("Remove mask");
                    self.select(Selection::layer(layer));
                }
                ui.close();
            }
        }
    }

    fn effect_menu(&mut self, ui: &mut egui::Ui, layer: u64) {
        if ui.button("Add paint").clicked() {
            self.add_effect(layer, EffectKind::Paint(ink_core::PaintLayer::new(0)));
            ui.close();
        }
        if ui.button("Add fill").clicked() {
            self.add_effect(layer, EffectKind::Fill(GrayFill::default()));
            ui.close();
        }
        if ui.button("Add levels").clicked() {
            self.add_effect(layer, EffectKind::Levels(Levels::default()));
            ui.close();
        }
        ui.label(
            RichText::new("Effects go into the mask; one is added if needed.")
                .small()
                .weak(),
        );
    }

    fn mask_mut(&mut self, layer: u64) -> Option<&mut Mask> {
        self.stack_mut()?.find_mut(layer)?.mask.as_mut()
    }

    /// Rows of a layer list, top to bottom (the list is stored bottom to top).
    #[allow(clippy::too_many_arguments)]
    fn layer_rows(
        &mut self,
        ui: &mut egui::Ui,
        stack: &LayerStack,
        layers: &[Layer],
        parent: Option<u64>,
        depth: usize,
        dragged: Option<u64>,
        drop: &mut Option<Drop>,
    ) {
        for (index, layer) in layers.iter().enumerate().rev() {
            let row = self.layer_row(ui, layer, depth);
            if let Some(id) = dragged
                && id != layer.id
                && let Some(pointer) = ui.input(|i| i.pointer.interact_pos())
                && row.interact_rect.contains(pointer)
            {
                // Upper part: above this layer; lower part: below it; the middle of a
                // folder: into it, on top.
                let y = (pointer.y - row.rect.top()) / row.rect.height();
                let folder = matches!(layer.content, Content::Folder(_));
                let into = folder && (0.3..0.7).contains(&y) && !stack.is_within(layer.id, id);
                let (place, line_y) = if into {
                    (
                        Place {
                            parent: Some(layer.id),
                            index: layer.children().map_or(0, Vec::len),
                        },
                        row.rect.center().y,
                    )
                } else if y < 0.5 {
                    (
                        Place {
                            parent,
                            index: index + 1,
                        },
                        row.rect.top(),
                    )
                } else {
                    (Place { parent, index }, row.rect.bottom())
                };
                let left =
                    row.rect.left() + depth as f32 * INDENT + if into { INDENT } else { 0.0 };
                *drop = Some(Drop {
                    place,
                    line: [
                        egui::pos2(left, line_y),
                        egui::pos2(row.rect.right(), line_y),
                    ],
                });
            }
            // The mask's effects, listed under the layer as in Painter.
            if let Some(mask) = &layer.mask {
                for (index, effect) in mask.effects.iter().enumerate().rev() {
                    self.effect_row(ui, layer.id, index, effect, depth + 1);
                }
            }
            if let Content::Folder(folder) = &layer.content
                && folder.expanded
            {
                self.layer_rows(
                    ui,
                    stack,
                    &folder.layers,
                    Some(layer.id),
                    depth + 1,
                    dragged,
                    drop,
                );
            }
        }
    }

    fn layer_row(&mut self, ui: &mut egui::Ui, layer: &Layer, depth: usize) -> egui::Response {
        let selection = self.selection().cloned();
        let main = selection.as_ref().is_some_and(|s| s.layer == layer.id);
        let extra = selection
            .as_ref()
            .is_some_and(|s| s.more.contains(&layer.id));
        let width = ui.available_width();
        let (rect, row) =
            ui.allocate_exact_size(egui::vec2(width, ROW_HEIGHT), Sense::click_and_drag());
        let visuals = ui.visuals().clone();
        if main || extra {
            ui.painter()
                .rect_filled(rect, 3.0, visuals.selection.bg_fill.gamma_multiply(0.35));
        } else if row.hovered() {
            ui.painter()
                .rect_filled(rect, 3.0, visuals.widgets.hovered.weak_bg_fill);
        }
        if row.drag_started_by(egui::PointerButton::Primary) {
            egui::DragAndDrop::set_payload(ui.ctx(), layer.id);
            if !main && !extra {
                self.select(Selection::layer(layer.id));
            }
        }
        if row.dragged()
            && let Some(pointer) = ui.input(|i| i.pointer.interact_pos())
        {
            ui.painter().text(
                pointer + egui::vec2(14.0, 0.0),
                egui::Align2::LEFT_CENTER,
                &layer.name,
                egui::FontId::proportional(13.0),
                visuals.strong_text_color(),
            );
        }

        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(rect.shrink2(egui::vec2(4.0, 2.0)))
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        let ui = &mut child;
        ui.add_space(depth as f32 * INDENT);
        // Eye.
        if super::eye(ui, layer.visible).clicked() {
            if let Some(l) = self.stack_mut().and_then(|s| s.find_mut(layer.id)) {
                l.visible = !l.visible;
            }
            self.label("Toggle layer visibility");
        }
        // Folder arrow and icon or thumbnail.
        let content_selected = main
            && selection
                .as_ref()
                .is_some_and(|s| s.target == Target::Content);
        let content_thumb = match &layer.content {
            Content::Folder(folder) => {
                let arrow = if folder.expanded { "⏷" } else { "⏵" };
                if ui.add(egui::Button::new(arrow).frame(false)).clicked()
                    && let Some(Content::Folder(f)) = self
                        .stack_mut()
                        .and_then(|s| s.find_mut(layer.id))
                        .map(|l| &mut l.content)
                {
                    f.expanded = !f.expanded;
                }
                thumbnail(ui, content_selected, |painter, r| {
                    painter.text(
                        r.center(),
                        egui::Align2::CENTER_CENTER,
                        "🗀",
                        egui::FontId::proportional(14.0),
                        Color32::from_gray(210),
                    );
                })
            }
            Content::Paint(_) => thumbnail(ui, content_selected, |painter, r| {
                painter.text(
                    r.center(),
                    egui::Align2::CENTER_CENTER,
                    "🖊",
                    egui::FontId::proportional(14.0),
                    Color32::from_gray(210),
                );
            }),
            Content::Fill(fill) => {
                let [r, g, b] = fill
                    .values
                    .base_color
                    .map(|c| (c.clamp(0.0, 1.0) * 255.0) as u8);
                let has_color = fill.channels.contains(Channel::BaseColor);
                thumbnail(ui, content_selected, |painter, rect| {
                    if has_color {
                        painter.rect_filled(rect.shrink(3.0), 2.0, Color32::from_rgb(r, g, b));
                    }
                    painter.text(
                        rect.right_bottom() - egui::vec2(5.0, 5.0),
                        egui::Align2::CENTER_CENTER,
                        "🎨",
                        egui::FontId::proportional(9.0),
                        Color32::WHITE,
                    );
                })
            }
        };
        // Mask thumbnail.
        let mut mask_response = None;
        if let Some(mask) = &layer.mask {
            let mask_selected = main
                && selection
                    .as_ref()
                    .is_some_and(|s| s.target != Target::Content);
            let response = thumbnail(ui, mask_selected, |painter, rect| {
                let v = (mask.background * 255.0) as u8;
                painter.rect_filled(rect.shrink(3.0), 1.0, Color32::from_gray(v));
                if !mask.paint.strokes.is_empty() || !mask.effects.is_empty() {
                    painter.circle_filled(rect.center(), 3.0, Color32::from_gray(255 - v));
                }
                if !mask.enabled {
                    let r = rect.shrink(3.0);
                    painter.line_segment(
                        [r.left_top(), r.right_bottom()],
                        Line::new(2.0, Color32::RED),
                    );
                    painter.line_segment(
                        [r.right_top(), r.left_bottom()],
                        Line::new(2.0, Color32::RED),
                    );
                }
            })
            .on_hover_text("Mask: click to paint it, Shift+click turns it off");
            mask_response = Some(response);
        }
        // Name, or the rename field.
        let renaming = self
            .layers
            .renaming
            .as_ref()
            .is_some_and(|(id, _)| *id == layer.id);
        ui.add_space(4.0);
        if renaming {
            let (_, text) = self.layers.renaming.as_mut().expect("checked");
            let output = egui::TextEdit::singleline(text)
                .desired_width(110.0)
                .show(ui);
            let edit = output.response.response;
            if !edit.has_focus() && !edit.lost_focus() {
                // Just opened: focus it with the whole name selected, as in Painter.
                edit.request_focus();
                let mut state = output.state;
                state
                    .cursor
                    .set_char_range(Some(egui::text::CCursorRange::select_all(&output.galley)));
                state.store(ui.ctx(), edit.id);
            }
            let done = edit.lost_focus() || ui.input(|i| i.key_pressed(Key::Enter));
            if ui.input(|i| i.key_pressed(Key::Escape)) {
                self.layers.renaming = None;
            } else if done {
                let (id, name) = self.layers.renaming.take().expect("checked");
                let name = name.trim().to_owned();
                if !name.is_empty()
                    && let Some(l) = self.stack_mut().and_then(|s| s.find_mut(id))
                {
                    l.name = name;
                    self.label("Rename layer");
                }
            }
        } else {
            let mut name = RichText::new(&layer.name);
            if !layer.visible {
                name = name.weak();
            }
            if extra {
                name = name.color(visuals.selection.stroke.color);
            }
            ui.add(egui::Label::new(name).truncate().selectable(false));
        }
        // Blending of the viewed channel.
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            self.blend_controls(ui, layer);
        });

        // Clicks on the row or the content thumbnail select the layer; on the mask
        // thumbnail they select the mask.
        let (shift, command) = ui.input(|i| (i.modifiers.shift, i.modifiers.command));
        if let Some(mask) = mask_response
            && mask.clicked()
        {
            if shift {
                if let Some(m) = self.mask_mut(layer.id) {
                    m.enabled = !m.enabled;
                    self.label("Toggle mask");
                }
            } else {
                self.select(Selection {
                    target: Target::Mask,
                    ..Selection::layer(layer.id)
                });
            }
        } else if row.clicked() || content_thumb.clicked() {
            if command {
                if let Some(Some(selection)) = self.layers.selections.get_mut(self.selected_set) {
                    if let Some(i) = selection.more.iter().position(|&l| l == layer.id) {
                        selection.more.remove(i);
                    } else if selection.layer != layer.id {
                        selection.more.push(layer.id);
                    }
                }
            } else {
                self.select(Selection::layer(layer.id));
            }
        }
        if row.double_clicked() {
            self.layers.renaming = Some((layer.id, layer.name.clone()));
        }
        row.context_menu(|ui| self.layer_context_menu(ui, layer.id));
        row
    }

    /// Blend mode and opacity of the viewed channel, as on Painter's layer rows.
    fn blend_controls(&mut self, ui: &mut egui::Ui, layer: &Layer) {
        let channel = self.layers.channel;
        let blend = layer.blending.get(channel);
        let folder = matches!(layer.content, Content::Folder(_));
        // Right-aligned: the parent lays out right to left, a plain vertical would start
        // right after the name.
        ui.with_layout(egui::Layout::top_down(egui::Align::Max), |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            let mut mode = blend.mode;
            ui.menu_button(
                RichText::new(format!("{} ⏷", mode.short_label())).small(),
                |ui| {
                    for m in BlendMode::ALL {
                        if !folder && !m.for_layers() {
                            continue;
                        }
                        if m == BlendMode::Multiply
                            || m == BlendMode::Tint
                            || m == BlendMode::NormalMapCombine
                        {
                            ui.separator();
                        }
                        if ui.selectable_label(m == mode, m.label()).clicked() {
                            mode = m;
                            ui.close();
                        }
                    }
                    // (Not a context menu on the button: it would share the menu's popup.)
                    ui.separator();
                    if ui
                        .button("Apply to all channels")
                        .on_hover_text(format!(
                            "Use this mode and opacity of {} for every channel",
                            channel.label()
                        ))
                        .clicked()
                    {
                        self.apply_blend_to_all(layer.id, blend.mode, blend.opacity);
                        ui.close();
                    }
                },
            )
            .response
            .on_hover_text(format!("{}: {}", channel.label(), blend.mode.label()));
            let mut opacity = blend.opacity * 100.0;
            ui.add(
                egui::DragValue::new(&mut opacity)
                    .range(0.0..=100.0)
                    .speed(0.5)
                    .max_decimals(0),
            )
            .on_hover_text(format!("{} opacity", channel.label()));
            if mode != blend.mode || (opacity / 100.0 - blend.opacity).abs() > f32::EPSILON {
                if let Some(l) = self.stack_mut().and_then(|s| s.find_mut(layer.id)) {
                    let b = l.blending.get_mut(channel);
                    b.mode = mode;
                    b.opacity = (opacity / 100.0).clamp(0.0, 1.0);
                }
                self.label(if mode != blend.mode {
                    "Change blending mode"
                } else {
                    "Change opacity"
                });
            }
        });
    }

    fn apply_blend_to_all(&mut self, layer: u64, mode: BlendMode, opacity: f32) {
        if let Some(l) = self.stack_mut().and_then(|s| s.find_mut(layer)) {
            let folder = matches!(l.content, Content::Folder(_));
            for channel in Channel::ALL {
                let b = l.blending.get_mut(channel);
                // Passthrough exists only for folders.
                if folder || mode.for_layers() {
                    b.mode = mode;
                }
                b.opacity = opacity;
            }
            self.label("Apply blending to all channels");
        }
    }

    /// Moves a dragged effect within its mask when the button is released over the list.
    fn drop_effect(&mut self, ui: &egui::Ui) {
        let Some(drag) = egui::DragAndDrop::payload::<EffectDrag>(ui.ctx()).map(|p| *p) else {
            return;
        };
        if let Some(drop) = self.layers.effect_drop {
            ui.painter()
                .line_segment(drop.line, Line::new(2.0, ui.visuals().selection.bg_fill));
        }
        if !ui.input(|i| i.pointer.any_released()) {
            return;
        }
        egui::DragAndDrop::clear_payload(ui.ctx());
        let Some(drop) = self.layers.effect_drop.take() else {
            return;
        };
        if let Some(mask) = self.mask_mut(drag.layer)
            && let Some(from) = mask.effects.iter().position(|e| e.id == drag.effect)
        {
            let to = if from < drop.slot {
                drop.slot - 1
            } else {
                drop.slot
            };
            if to != from {
                let effect = mask.effects.remove(from);
                mask.effects.insert(to, effect);
                self.label("Move effect");
            }
        }
    }

    fn effect_row(
        &mut self,
        ui: &mut egui::Ui,
        layer: u64,
        index: usize,
        effect: &Effect,
        depth: usize,
    ) {
        let selected = self
            .selection()
            .is_some_and(|s| s.layer == layer && s.target == Target::Effect(effect.id));
        let width = ui.available_width();
        let (rect, row) = ui.allocate_exact_size(egui::vec2(width, 24.0), Sense::click_and_drag());
        // Effects are reordered by dragging them within their mask, as in Painter.
        if row.drag_started_by(egui::PointerButton::Primary) {
            egui::DragAndDrop::set_payload(
                ui.ctx(),
                EffectDrag {
                    layer,
                    effect: effect.id,
                },
            );
            self.select(Selection {
                target: Target::Effect(effect.id),
                ..Selection::layer(layer)
            });
        }
        if let Some(drag) = egui::DragAndDrop::payload::<EffectDrag>(ui.ctx()).map(|p| *p)
            && drag.layer == layer
            && drag.effect != effect.id
            && let Some(pointer) = ui.input(|i| i.pointer.interact_pos())
            && row.interact_rect.contains(pointer)
        {
            // The list shows the topmost effect first: the upper half drops above.
            let above = pointer.y < rect.center().y;
            let y = if above { rect.top() } else { rect.bottom() };
            let left = rect.left() + depth as f32 * INDENT;
            self.layers.effect_drop = Some(EffectDrop {
                slot: if above { index + 1 } else { index },
                line: [egui::pos2(left, y), egui::pos2(rect.right(), y)],
            });
        }
        let visuals = ui.visuals().clone();
        if selected {
            ui.painter()
                .rect_filled(rect, 3.0, visuals.selection.bg_fill.gamma_multiply(0.35));
        } else if row.hovered() {
            ui.painter()
                .rect_filled(rect, 3.0, visuals.widgets.hovered.weak_bg_fill);
        }
        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(rect.shrink2(egui::vec2(4.0, 1.0)))
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        let ui = &mut child;
        ui.add_space(depth as f32 * INDENT);
        let mut changed: Option<Effect> = None;
        if super::eye(ui, effect.visible).clicked() {
            let mut e = effect.clone();
            e.visible = !e.visible;
            changed = Some(e);
        }
        let icon = match effect.kind {
            EffectKind::Paint(_) => "🖊",
            EffectKind::Fill(_) => "🎨",
            EffectKind::Levels(_) => "📊",
        };
        ui.label(format!("{icon} {}", effect.label()));
        let mut remove = false;
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            remove = ui
                .add(egui::Button::new("✖").frame(false))
                .on_hover_text("Remove the effect")
                .clicked();
            // Levels has no blending, as in Painter.
            if !matches!(effect.kind, EffectKind::Levels(_)) {
                let mut opacity = effect.opacity * 100.0;
                ui.add(
                    egui::DragValue::new(&mut opacity)
                        .range(0.0..=100.0)
                        .speed(0.5)
                        .max_decimals(0),
                );
                let mut mode = effect.blend;
                ui.menu_button(
                    RichText::new(format!("{} ⏷", mode.short_label())).small(),
                    |ui| {
                        for m in BlendMode::ALL.into_iter().filter(|m| m.for_masks()) {
                            if ui.selectable_label(m == mode, m.label()).clicked() {
                                mode = m;
                                ui.close();
                            }
                        }
                    },
                );
                if mode != effect.blend || (opacity / 100.0 - effect.opacity).abs() > f32::EPSILON {
                    let mut e = changed.take().unwrap_or_else(|| effect.clone());
                    e.blend = mode;
                    e.opacity = (opacity / 100.0).clamp(0.0, 1.0);
                    changed = Some(e);
                }
            }
        });
        if remove {
            if let Some(mask) = self.mask_mut(layer) {
                mask.effects.retain(|e| e.id != effect.id);
                self.label("Delete effect");
            }
        } else if let Some(e) = changed
            && let Some(slot) = self.mask_mut(layer).and_then(|m| m.effect_mut(effect.id))
        {
            *slot = e;
            self.label("Edit effect");
        }
        if row.clicked() {
            self.select(Selection {
                target: Target::Effect(effect.id),
                ..Selection::layer(layer)
            });
        }
    }

    fn layer_context_menu(&mut self, ui: &mut egui::Ui, layer: u64) {
        if self
            .selection()
            .is_none_or(|s| s.layer != layer && !s.more.contains(&layer))
        {
            self.select(Selection::layer(layer));
        }
        self.mask_menu(ui, layer);
        ui.separator();
        ui.menu_button("Add effect", |ui| self.effect_menu(ui, layer));
        ui.separator();
        let item = |ui: &mut egui::Ui, text: &str, shortcut: &str| {
            ui.add(egui::Button::new(text).shortcut_text(shortcut))
                .clicked()
        };
        if item(ui, "Cut layer", "Ctrl+X") {
            self.select_layers_themselves();
            self.cut_selected();
            ui.close();
        }
        if item(ui, "Copy layer", "Ctrl+C") {
            self.select_layers_themselves();
            self.copy_selected();
            ui.close();
        }
        let paste = match self.layers.clipboard {
            Clipboard::Effect(_) => "Paste effect",
            _ => "Paste layer",
        };
        if ui
            .add_enabled(
                !matches!(self.layers.clipboard, Clipboard::Empty),
                egui::Button::new(paste).shortcut_text("Ctrl+V"),
            )
            .clicked()
        {
            self.paste();
            ui.close();
        }
        if item(ui, "Duplicate layer", "Ctrl+D") {
            self.duplicate_selected();
            ui.close();
        }
        if item(ui, "Remove layer", "Del") {
            self.select_layers_themselves();
            self.delete_selected();
            ui.close();
        }
        if item(ui, "Group layers", "Ctrl+G") {
            self.group_selected();
            ui.close();
        }
        if item(ui, "Rename", "Double-click") {
            let name = self
                .stack()
                .and_then(|s| s.find(layer))
                .map(|l| l.name.clone())
                .unwrap_or_default();
            self.layers.renaming = Some((layer, name));
            ui.close();
        }
    }

    // ---- Properties ----

    /// Properties of the selected fill layer or effect. Returns false if the selection has
    /// none, so the tool's properties show instead.
    pub(super) fn selection_properties(&mut self, ui: &mut egui::Ui) -> bool {
        let Some(selection) = self.selection().cloned() else {
            return false;
        };
        let Some(layer) = self.stack().and_then(|s| s.find(selection.layer)).cloned() else {
            return false;
        };
        let mut load: Option<LoadRequest> = None;
        let images = self.layers.images.clone();
        match selection.target {
            Target::Content => {
                let Content::Fill(mut fill) = layer.content.clone() else {
                    return false;
                };
                fill_properties(ui, &mut fill, &images, &mut load);
                if Content::Fill(fill.clone()) != layer.content {
                    if let Some(l) = self.stack_mut().and_then(|s| s.find_mut(layer.id)) {
                        l.content = Content::Fill(fill);
                    }
                    self.label("Edit fill");
                }
            }
            Target::Effect(id) => {
                let Some(effect) = layer.mask.as_ref().and_then(|m| m.effect(id)).cloned() else {
                    return false;
                };
                let mut edited = effect.clone();
                match &mut edited.kind {
                    EffectKind::Fill(gray) => gray_fill_properties(ui, gray, &images, &mut load),
                    EffectKind::Levels(levels) => levels_properties(ui, levels),
                    EffectKind::Paint(_) => return false,
                }
                if edited != effect {
                    if let Some(slot) = self.mask_mut(layer.id).and_then(|m| m.effect_mut(id)) {
                        *slot = edited;
                    }
                    self.label("Edit effect");
                }
            }
            Target::Mask => return false,
        }
        if let Some(request) = load {
            self.load_fill_image(selection.layer, selection.target, request);
        }
        true
    }

    /// Title of the Properties panel for the selection, if it has its own properties.
    pub(super) fn selection_properties_title(&self) -> Option<&'static str> {
        let selection = self.selection()?;
        let layer = self.stack()?.find(selection.layer)?;
        match selection.target {
            Target::Content => {
                matches!(layer.content, Content::Fill(_)).then_some("PROPERTIES - FILL")
            }
            Target::Effect(id) => match layer.mask.as_ref()?.effect(id)?.kind {
                EffectKind::Fill(_) => Some("PROPERTIES - FILL"),
                EffectKind::Levels(_) => Some("PROPERTIES - LEVELS"),
                EffectKind::Paint(_) => None,
            },
            Target::Mask => None,
        }
    }

    /// Opens a file dialog (or takes an already loaded image) for an image slot.
    fn load_fill_image(&mut self, layer: u64, target: Target, request: LoadRequest) {
        let image = match request.image {
            Some(image) => image,
            None => {
                let Some(path) = rfd::FileDialog::new()
                    .add_filter("Images", ink_io::TEXTURE_EXTENSIONS)
                    .pick_file()
                else {
                    return;
                };
                match ink_io::load_texture(&path) {
                    Ok(image) => {
                        let image = Arc::new(image);
                        self.layers.images.push(image.clone());
                        self.status = format!("Image: {}", path.display());
                        image
                    }
                    Err(err) => {
                        log::error!("{err:#}");
                        self.status = format!("Error: {err:#}");
                        return;
                    }
                }
            }
        };
        let Some(l) = self.stack_mut().and_then(|s| s.find_mut(layer)) else {
            return;
        };
        match (target, request.channel) {
            (Target::Content, Some(channel)) => {
                if let Content::Fill(fill) = &mut l.content {
                    fill.images[channel.index()] = Some(image);
                    fill.channels.set(channel, true);
                }
            }
            (Target::Effect(id), None) => {
                if let Some(EffectKind::Fill(gray)) = l
                    .mask
                    .as_mut()
                    .and_then(|m| m.effect_mut(id))
                    .map(|e| &mut e.kind)
                {
                    gray.image = Some(image);
                }
            }
            _ => return,
        }
        self.label("Set image");
    }
}

/// An image slot asked for an image: a file to pick (`image` None) or one already loaded.
struct LoadRequest {
    channel: Option<Channel>,
    image: Option<Arc<TextureImage>>,
}

/// A small square like Painter's thumbnails, outlined when selected.
fn thumbnail(
    ui: &mut egui::Ui,
    selected: bool,
    paint: impl FnOnce(&egui::Painter, egui::Rect),
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(26.0, 26.0), Sense::click());
    let painter = ui.painter();
    painter.rect_filled(rect, 3.0, Color32::from_gray(45));
    paint(painter, rect);
    if selected {
        painter.rect_stroke(
            rect,
            3.0,
            Line::new(2.0, ui.visuals().selection.stroke.color),
            egui::StrokeKind::Inside,
        );
    }
    response
}

/// "Layer 1 copy 1", "Layer 1 copy 2", …
fn copy_name(stack: &LayerStack, name: &str) -> String {
    let base = name
        .rsplit_once(" copy ")
        .filter(|(_, n)| n.parse::<u32>().is_ok())
        .map_or(name, |(b, _)| b);
    let next = (1..)
        .find(|n| stack.iter().all(|l| l.name != format!("{base} copy {n}")))
        .unwrap_or(1);
    format!("{base} copy {next}")
}

fn projection_ui(ui: &mut egui::Ui, projection: &mut Projection) {
    egui::Grid::new("projection")
        .num_columns(2)
        .spacing([12.0, 6.0])
        .show(ui, |ui| {
            ui.label("Projection");
            egui::ComboBox::from_id_salt("projection mode")
                .selected_text(match projection.mode {
                    ProjectionMode::Uv => "UV projection",
                    ProjectionMode::TriPlanar => "Tri-planar projection",
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut projection.mode, ProjectionMode::Uv, "UV projection");
                    ui.selectable_value(
                        &mut projection.mode,
                        ProjectionMode::TriPlanar,
                        "Tri-planar projection",
                    );
                });
            ui.end_row();
            if projection.mode == ProjectionMode::TriPlanar {
                ui.label("Hardness");
                ui.add(egui::Slider::new(&mut projection.hardness, 0.0..=1.0));
                ui.end_row();
            }
            ui.label("Scale");
            ui.horizontal(|ui| {
                let speed = 0.01 * projection.scale[0].abs().max(0.1);
                let changed = ui
                    .add(
                        egui::DragValue::new(&mut projection.scale[0])
                            .speed(speed)
                            .range(-PROJECTION_RANGE..=PROJECTION_RANGE)
                            .max_decimals(3),
                    )
                    .changed();
                let locked = ui.id().with("scale lock");
                let mut lock = ui.data(|d| d.get_temp::<bool>(locked)).unwrap_or(true);
                if lock {
                    if changed {
                        projection.scale[1] = projection.scale[0];
                    }
                } else {
                    let speed = 0.01 * projection.scale[1].abs().max(0.1);
                    ui.add(
                        egui::DragValue::new(&mut projection.scale[1])
                            .speed(speed)
                            .range(-PROJECTION_RANGE..=PROJECTION_RANGE)
                            .max_decimals(3),
                    );
                }
                if ui
                    .toggle_value(&mut lock, "🔒")
                    .on_hover_text("Same scale for U and V")
                    .changed()
                    && lock
                {
                    projection.scale[1] = projection.scale[0];
                }
                ui.data_mut(|d| d.insert_temp(locked, lock));
            });
            ui.end_row();
            ui.label("Rotation");
            ui.add(egui::Slider::new(&mut projection.rotation, 0.0..=360.0).suffix("°"));
            ui.end_row();
            // Painter has no UV offset for tri-planar projections.
            if projection.mode == ProjectionMode::Uv {
                ui.label("Offset");
                ui.horizontal(|ui| {
                    ui.add(
                        egui::DragValue::new(&mut projection.offset[0])
                            .speed(0.005)
                            .range(-PROJECTION_RANGE..=PROJECTION_RANGE)
                            .max_decimals(3),
                    );
                    ui.add(
                        egui::DragValue::new(&mut projection.offset[1])
                            .speed(0.005)
                            .range(-PROJECTION_RANGE..=PROJECTION_RANGE)
                            .max_decimals(3),
                    );
                });
                ui.end_row();
            }
        });
}

/// An image slot: the image's name with a menu to load or pick one, and a button to clear.
fn image_slot(
    ui: &mut egui::Ui,
    image: &mut Option<Arc<TextureImage>>,
    images: &[Arc<TextureImage>],
    channel: Option<Channel>,
    load: &mut Option<LoadRequest>,
) {
    let text = image
        .as_ref()
        .map_or("uniform".to_owned(), |i| i.name.clone());
    ui.menu_button(RichText::new(format!("🖼 {text}")).small(), |ui| {
        if ui.button("Load image…").clicked() {
            *load = Some(LoadRequest {
                channel,
                image: None,
            });
            ui.close();
        }
        if !images.is_empty() {
            ui.separator();
            for i in images {
                if ui.button(&i.name).clicked() {
                    *load = Some(LoadRequest {
                        channel,
                        image: Some(i.clone()),
                    });
                    ui.close();
                }
            }
        }
    })
    .response
    .on_hover_text("Use an image instead of the uniform value");
    if image.is_some()
        && ui
            .add(egui::Button::new("✖").frame(false))
            .on_hover_text("Back to the uniform value")
            .clicked()
    {
        *image = None;
    }
}

fn fill_properties(
    ui: &mut egui::Ui,
    fill: &mut Fill,
    images: &[Arc<TextureImage>],
    load: &mut Option<LoadRequest>,
) {
    section(ui, "FILL");
    projection_ui(ui, &mut fill.projection);

    section(ui, "MATERIAL");
    // Painter's channel buttons; Alt+click on base color turns every channel on.
    ui.horizontal_wrapped(|ui| {
        for channel in Channel::ALL {
            let mut on = fill.channels.contains(channel);
            let response = ui
                .toggle_value(&mut on, channel.short_label())
                .on_hover_text(match channel {
                    Channel::BaseColor => "Set the base color (Alt+click: every channel)",
                    _ => "Set this channel",
                });
            if response.clicked() {
                if channel == Channel::BaseColor && ui.input(|i| i.modifiers.alt) {
                    fill.channels = ChannelSet::ALL;
                } else {
                    fill.channels.set(channel, on);
                }
            }
        }
    });
    ui.add_space(4.0);
    egui::Grid::new("fill material")
        .num_columns(2)
        .spacing([12.0, 6.0])
        .show(ui, |ui| {
            for channel in Channel::ALL
                .into_iter()
                .filter(|&c| fill.channels.contains(c))
            {
                ui.label(channel.label());
                ui.vertical(|ui| {
                    ui.horizontal(|ui| {
                        if fill.images[channel.index()].is_none() {
                            super::channel_value_edit(ui, &mut fill.values, channel);
                        }
                        image_slot(
                            ui,
                            &mut fill.images[channel.index()],
                            images,
                            Some(channel),
                            load,
                        );
                    });
                    if channel.kind() == ChannelKind::Normal
                        && fill.images[channel.index()].is_some()
                    {
                        ui.horizontal(|ui| {
                            ui.selectable_value(&mut fill.normal_directx, true, "DirectX");
                            ui.selectable_value(&mut fill.normal_directx, false, "OpenGL");
                        })
                        .response
                        .on_hover_text("Normal map format of the image; Painter assumes DirectX");
                    }
                });
                ui.end_row();
            }
        });
}

fn gray_fill_properties(
    ui: &mut egui::Ui,
    fill: &mut GrayFill,
    images: &[Arc<TextureImage>],
    load: &mut Option<LoadRequest>,
) {
    section(ui, "FILL");
    projection_ui(ui, &mut fill.projection);
    section(ui, "GRAYSCALE");
    ui.horizontal(|ui| {
        if fill.image.is_none() {
            ui.add(egui::Slider::new(&mut fill.value, 0.0..=1.0));
        }
        image_slot(ui, &mut fill.image, images, None, load);
    });
}

fn levels_properties(ui: &mut egui::Ui, levels: &mut Levels) {
    section(ui, "LEVELS");
    egui::Grid::new("levels")
        .num_columns(2)
        .spacing([12.0, 6.0])
        .show(ui, |ui| {
            ui.label("Input");
            ui.horizontal(|ui| {
                ui.add(
                    egui::DragValue::new(&mut levels.input[0])
                        .range(0.0..=1.0)
                        .speed(0.005),
                );
                ui.add(
                    egui::DragValue::new(&mut levels.input[1])
                        .range(0.0..=1.0)
                        .speed(0.005),
                );
            });
            ui.end_row();
            ui.label("Gamma");
            ui.add(egui::Slider::new(&mut levels.gamma, 0.1..=10.0).logarithmic(true));
            ui.end_row();
            ui.label("Output");
            ui.horizontal(|ui| {
                ui.add(
                    egui::DragValue::new(&mut levels.output[0])
                        .range(0.0..=1.0)
                        .speed(0.005),
                );
                ui.add(
                    egui::DragValue::new(&mut levels.output[1])
                        .range(0.0..=1.0)
                        .speed(0.005),
                );
            });
            ui.end_row();
            ui.label("");
            if ui.button("Invert").clicked() {
                levels.output.swap(0, 1);
            }
            ui.end_row();
        });
}

fn section(ui: &mut egui::Ui, title: &str) {
    ui.add_space(6.0);
    ui.label(RichText::new(title).small().weak());
}

#[cfg(test)]
mod tests {
    use super::*;
    use ink_core::Ids;

    #[test]
    fn copies_count_up() {
        let mut ids = Ids::default();
        let mut stack = LayerStack::new_default(&mut ids);
        assert_eq!(copy_name(&stack, "Layer 1"), "Layer 1 copy 1");
        stack.layers.push(Layer::new(
            LayerKind::Paint,
            "Layer 1 copy 1".into(),
            &mut ids,
        ));
        assert_eq!(copy_name(&stack, "Layer 1"), "Layer 1 copy 2");
        assert_eq!(copy_name(&stack, "Layer 1 copy 1"), "Layer 1 copy 2");
    }
}
