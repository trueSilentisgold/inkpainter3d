//! The layer stack of a texture set: paint layers, fill layers and folders, each with a
//! blend mode and opacity per channel and an optional mask, as in Painter.
//!
//! Layers are stored bottom to top (index 0 is the lowest); the UI lists them the other
//! way round. Every layer, paint layer and effect has an id that is unique within the
//! model ([`crate::Ids`]), so selections, undo steps and the renderer's caches survive
//! reordering.

use crate::brush::{Channel, ChannelSet};
use crate::model::{ChannelValues, Ids};
use crate::stroke::PaintLayer;
use crate::texture::TextureImage;
use std::sync::Arc;

/// How a layer's channel is combined with what lies below. The order is Painter's (its
/// blending mode list) and the `BLEND_*` codes in the compositor's shader.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum BlendMode {
    #[default]
    Normal,
    /// Folders: the children blend with the layers below the folder as if the folder
    /// weren't there.
    Passthrough,
    /// The layer leaves this channel alone.
    Disable,
    /// The layer's value and coverage replace what lies below, also where the layer is
    /// empty.
    Replace,
    Multiply,
    Divide,
    InverseDivide,
    Darken,
    Lighten,
    LinearDodge,
    Subtract,
    InverseSubtract,
    Difference,
    Exclusion,
    SignedAddition,
    Overlay,
    Screen,
    LinearBurn,
    ColorBurn,
    ColorDodge,
    SoftLight,
    HardLight,
    VividLight,
    LinearLight,
    PinLight,
    Tint,
    Saturation,
    Color,
    Value,
    NormalMapCombine,
    NormalMapDetail,
    NormalMapInverseDetail,
}

impl BlendMode {
    pub const ALL: [Self; 32] = [
        Self::Normal,
        Self::Passthrough,
        Self::Disable,
        Self::Replace,
        Self::Multiply,
        Self::Divide,
        Self::InverseDivide,
        Self::Darken,
        Self::Lighten,
        Self::LinearDodge,
        Self::Subtract,
        Self::InverseSubtract,
        Self::Difference,
        Self::Exclusion,
        Self::SignedAddition,
        Self::Overlay,
        Self::Screen,
        Self::LinearBurn,
        Self::ColorBurn,
        Self::ColorDodge,
        Self::SoftLight,
        Self::HardLight,
        Self::VividLight,
        Self::LinearLight,
        Self::PinLight,
        Self::Tint,
        Self::Saturation,
        Self::Color,
        Self::Value,
        Self::NormalMapCombine,
        Self::NormalMapDetail,
        Self::NormalMapInverseDetail,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Normal => "Normal",
            Self::Passthrough => "Passthrough",
            Self::Disable => "Disable",
            Self::Replace => "Replace",
            Self::Multiply => "Multiply",
            Self::Divide => "Divide",
            Self::InverseDivide => "Inverse Divide",
            Self::Darken => "Darken (Min)",
            Self::Lighten => "Lighten (Max)",
            Self::LinearDodge => "Linear Dodge (Add)",
            Self::Subtract => "Subtract",
            Self::InverseSubtract => "Inverse Subtract",
            Self::Difference => "Difference",
            Self::Exclusion => "Exclusion",
            Self::SignedAddition => "Signed Addition (AddSub)",
            Self::Overlay => "Overlay",
            Self::Screen => "Screen",
            Self::LinearBurn => "Linear Burn",
            Self::ColorBurn => "Color Burn",
            Self::ColorDodge => "Color Dodge",
            Self::SoftLight => "Soft Light",
            Self::HardLight => "Hard Light",
            Self::VividLight => "Vivid Light",
            Self::LinearLight => "Linear Light",
            Self::PinLight => "Pin Light",
            Self::Tint => "Tint",
            Self::Saturation => "Saturation",
            Self::Color => "Color",
            Self::Value => "Value",
            Self::NormalMapCombine => "Normal Map Combine",
            Self::NormalMapDetail => "Normal Map Detail",
            Self::NormalMapInverseDetail => "Normal Map Inverse Detail",
        }
    }

    /// The abbreviation on a layer row, above the opacity ("Norm", "Ldge", "NMdt" as in
    /// Painter).
    pub fn short_label(self) -> &'static str {
        match self {
            Self::Normal => "Norm",
            Self::Passthrough => "Pthr",
            Self::Disable => "Dsbl",
            Self::Replace => "Repl",
            Self::Multiply => "Mul",
            Self::Divide => "Div",
            Self::InverseDivide => "IDiv",
            Self::Darken => "Dark",
            Self::Lighten => "Ligh",
            Self::LinearDodge => "Ldge",
            Self::Subtract => "Sub",
            Self::InverseSubtract => "ISub",
            Self::Difference => "Diff",
            Self::Exclusion => "Excl",
            Self::SignedAddition => "AdSb",
            Self::Overlay => "Ovrl",
            Self::Screen => "Scrn",
            Self::LinearBurn => "Lbrn",
            Self::ColorBurn => "Cbrn",
            Self::ColorDodge => "Cdge",
            Self::SoftLight => "SftL",
            Self::HardLight => "HrdL",
            Self::VividLight => "VvdL",
            Self::LinearLight => "LinL",
            Self::PinLight => "PinL",
            Self::Tint => "Tint",
            Self::Saturation => "Satu",
            Self::Color => "Colr",
            Self::Value => "Valu",
            Self::NormalMapCombine => "NMcb",
            Self::NormalMapDetail => "NMdt",
            Self::NormalMapInverseDetail => "NMid",
        }
    }

    /// Must match the `BLEND_*` constants in the compositor's shader.
    pub fn code(self) -> u32 {
        Self::ALL.iter().position(|&m| m == self).unwrap_or(0) as u32
    }

    /// Whether the mode is offered for layers that are not folders. Painter's Passthrough
    /// on layers (cloning what lies below) comes with the filters.
    pub fn for_layers(self) -> bool {
        self != Self::Passthrough
    }

    /// Modes that work on grayscale values, offered for mask effects.
    pub fn for_masks(self) -> bool {
        !matches!(
            self,
            Self::Passthrough
                | Self::Tint
                | Self::Saturation
                | Self::Color
                | Self::Value
                | Self::NormalMapCombine
                | Self::NormalMapDetail
                | Self::NormalMapInverseDetail
        )
    }
}

/// Blend mode and opacity of one channel of a layer.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ChannelBlend {
    pub mode: BlendMode,
    /// 0 to 1.
    pub opacity: f32,
}

impl ChannelBlend {
    pub const NORMAL: Self = Self {
        mode: BlendMode::Normal,
        opacity: 1.0,
    };
}

/// Blend mode and opacity per channel.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Blending([ChannelBlend; Channel::COUNT]);

impl Blending {
    /// Painter's defaults for new layers and folders: Normal, but Linear Dodge (Add) for
    /// height and Normal Map Detail for normals, all at full opacity.
    pub fn new_layer() -> Self {
        let mut blending = Self::uniform(BlendMode::Normal);
        blending.get_mut(Channel::Height).mode = BlendMode::LinearDodge;
        blending.get_mut(Channel::Normal).mode = BlendMode::NormalMapDetail;
        blending
    }

    /// The same mode at full opacity for every channel.
    pub fn uniform(mode: BlendMode) -> Self {
        Self([ChannelBlend { mode, opacity: 1.0 }; Channel::COUNT])
    }

    pub fn get(&self, channel: Channel) -> ChannelBlend {
        self.0[channel.index()]
    }

    pub fn get_mut(&mut self, channel: Channel) -> &mut ChannelBlend {
        &mut self.0[channel.index()]
    }
}

/// How a fill's image is laid onto the model.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ProjectionMode {
    /// Along the UVs, tiled.
    #[default]
    Uv,
    /// From the three axes, blended by the surface normal: no seams at UV borders.
    TriPlanar,
}

/// Placement of a fill's images. Uniform values ignore it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Projection {
    pub mode: ProjectionMode,
    /// Repetitions across the UV square, or across the model for tri-planar projection.
    pub scale: [f32; 2],
    /// Degrees, counter-clockwise.
    pub rotation: f32,
    /// Shift in tiles.
    pub offset: [f32; 2],
    /// Tri-planar only: 0 blends the three projections smoothly, 1 cuts hard between them.
    pub hardness: f32,
}

impl Default for Projection {
    fn default() -> Self {
        Self {
            mode: ProjectionMode::Uv,
            scale: [1.0, 1.0],
            rotation: 0.0,
            offset: [0.0, 0.0],
            hardness: 0.5,
        }
    }
}

/// The content of a fill layer: a uniform value or an image per channel.
#[derive(Clone, Debug, PartialEq)]
pub struct Fill {
    /// The channels the fill sets (Painter's material buttons); the layer leaves the others
    /// alone.
    pub channels: ChannelSet,
    /// Values of the channels without an image. Colors are sRGB.
    pub values: ChannelValues,
    /// Per channel, in [`Channel::ALL`] order.
    pub images: [Option<Arc<TextureImage>>; Channel::COUNT],
    /// Whether the normal channel's image is a DirectX normal map (green points down), as
    /// Painter assumes for loaded normal maps; OpenGL otherwise.
    pub normal_directx: bool,
    pub projection: Projection,
}

impl Default for Fill {
    /// As in Painter since version 11, a new fill sets only the base color.
    fn default() -> Self {
        let mut channels = ChannelSet::NONE;
        channels.set(Channel::BaseColor, true);
        Self {
            channels,
            values: ChannelValues::default(),
            images: Default::default(),
            normal_directx: true,
            projection: Projection::default(),
        }
    }
}

impl Fill {
    pub fn image(&self, channel: Channel) -> Option<&Arc<TextureImage>> {
        self.images[channel.index()].as_ref()
    }
}

/// A grayscale fill, e.g. a fill effect in a mask.
#[derive(Clone, Debug, PartialEq)]
pub struct GrayFill {
    /// 0 (black) to 1 (white); used without an image.
    pub value: f32,
    /// Its red channel is used.
    pub image: Option<Arc<TextureImage>>,
    pub projection: Projection,
}

impl Default for GrayFill {
    fn default() -> Self {
        Self {
            value: 1.0,
            image: None,
            projection: Projection::default(),
        }
    }
}

/// Remaps grayscale values: input range, gamma and output range, all 0 to 1.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Levels {
    pub input: [f32; 2],
    pub gamma: f32,
    pub output: [f32; 2],
}

impl Default for Levels {
    fn default() -> Self {
        Self {
            input: [0.0, 1.0],
            gamma: 1.0,
            output: [0.0, 1.0],
        }
    }
}

impl Levels {
    pub fn apply(&self, v: f32) -> f32 {
        let span = self.input[1] - self.input[0];
        let t = if span.abs() < 1e-6 {
            f32::from(v >= self.input[0])
        } else {
            ((v - self.input[0]) / span).clamp(0.0, 1.0)
        };
        let t = t.powf(1.0 / self.gamma.max(1e-3));
        self.output[0] + (self.output[1] - self.output[0]) * t
    }
}

/// What an effect does.
#[derive(Clone, Debug, PartialEq)]
pub enum EffectKind {
    /// Painted grayscale.
    Paint(PaintLayer),
    Fill(GrayFill),
    /// Changes what lies below instead of adding to it.
    Levels(Levels),
}

/// A step in a mask's effect stack, blended onto the mask below it.
#[derive(Clone, Debug, PartialEq)]
pub struct Effect {
    pub id: u64,
    pub visible: bool,
    pub blend: BlendMode,
    /// 0 to 1.
    pub opacity: f32,
    pub kind: EffectKind,
}

impl Effect {
    pub fn new(id: u64, kind: EffectKind) -> Self {
        Self {
            id,
            visible: true,
            blend: BlendMode::Normal,
            opacity: 1.0,
            kind,
        }
    }

    /// A copy with new ids for the effect and its paint layer.
    pub fn duplicate(&self, ids: &mut Ids) -> Self {
        let mut copy = self.clone();
        copy.renew_ids(ids);
        copy
    }

    fn renew_ids(&mut self, ids: &mut Ids) {
        self.id = ids.next();
        if let EffectKind::Paint(paint) = &mut self.kind {
            paint.id = ids.next();
        }
    }

    pub fn label(&self) -> &'static str {
        match self.kind {
            EffectKind::Paint(_) => "Paint",
            EffectKind::Fill(_) => "Fill",
            EffectKind::Levels(_) => "Levels",
        }
    }
}

/// A grayscale mask: white shows the layer, black hides it. It starts black or white, is
/// painted directly, and can stack effects on top.
#[derive(Clone, Debug, PartialEq)]
pub struct Mask {
    /// Shift+click on the mask turns it off; the layer then shows everywhere.
    pub enabled: bool,
    /// The value where nothing is painted: 0 for a black mask, 1 for a white one.
    pub background: f32,
    /// Painted directly into the mask, below the effects.
    pub paint: PaintLayer,
    /// Bottom to top.
    pub effects: Vec<Effect>,
}

impl Mask {
    pub fn new(background: f32, paint_id: u64) -> Self {
        Self {
            enabled: true,
            background,
            paint: PaintLayer::new(paint_id),
            effects: Vec::new(),
        }
    }

    pub fn effect(&self, id: u64) -> Option<&Effect> {
        self.effects.iter().find(|e| e.id == id)
    }

    pub fn effect_mut(&mut self, id: u64) -> Option<&mut Effect> {
        self.effects.iter_mut().find(|e| e.id == id)
    }
}

/// A group of layers.
#[derive(Clone, Debug, Default)]
pub struct Folder {
    /// Bottom to top.
    pub layers: Vec<Layer>,
    /// Whether the layer stack shows the children.
    pub expanded: bool,
}

/// Compares the children only: opening or closing a folder in the panel changes nothing
/// that undo should record, as in Painter.
impl PartialEq for Folder {
    fn eq(&self, other: &Self) -> bool {
        self.layers == other.layers
    }
}

/// What a layer holds.
#[derive(Clone, Debug, PartialEq)]
pub enum Content {
    Paint(PaintLayer),
    Fill(Fill),
    Folder(Folder),
}

/// The kinds of layers, e.g. for naming new ones.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LayerKind {
    Paint,
    Fill,
    Folder,
}

impl LayerKind {
    /// Painter's names for new layers: "Layer 1", "Fill layer 1", "Folder 1".
    pub fn base_name(self) -> &'static str {
        match self {
            Self::Paint => "Layer",
            Self::Fill => "Fill layer",
            Self::Folder => "Folder",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Layer {
    pub id: u64,
    pub name: String,
    pub visible: bool,
    pub blending: Blending,
    pub content: Content,
    pub mask: Option<Mask>,
}

impl Layer {
    /// An empty layer of the kind, with Painter's default blending (see
    /// [`Blending::new_layer`]); folders blend like layers too.
    pub fn new(kind: LayerKind, name: String, ids: &mut Ids) -> Self {
        let id = ids.next();
        let content = match kind {
            LayerKind::Paint => Content::Paint(PaintLayer::new(ids.next())),
            LayerKind::Fill => Content::Fill(Fill::default()),
            LayerKind::Folder => Content::Folder(Folder {
                layers: Vec::new(),
                expanded: true,
            }),
        };
        Self {
            id,
            name,
            visible: true,
            blending: Blending::new_layer(),
            content,
            mask: None,
        }
    }

    pub fn kind(&self) -> LayerKind {
        match self.content {
            Content::Paint(_) => LayerKind::Paint,
            Content::Fill(_) => LayerKind::Fill,
            Content::Folder(_) => LayerKind::Folder,
        }
    }

    pub fn children(&self) -> Option<&Vec<Layer>> {
        match &self.content {
            Content::Folder(folder) => Some(&folder.layers),
            _ => None,
        }
    }

    pub fn children_mut(&mut self) -> Option<&mut Vec<Layer>> {
        match &mut self.content {
            Content::Folder(folder) => Some(&mut folder.layers),
            _ => None,
        }
    }

    /// A copy with new ids for the layer, its paint layers and effects, its children's too.
    /// Strokes keep their ids, which only need to be unique within a paint layer's history.
    pub fn duplicate(&self, ids: &mut Ids) -> Self {
        let mut copy = self.clone();
        copy.renew_ids(ids);
        copy
    }

    fn renew_ids(&mut self, ids: &mut Ids) {
        self.id = ids.next();
        match &mut self.content {
            Content::Paint(paint) => paint.id = ids.next(),
            Content::Fill(_) => {}
            Content::Folder(folder) => {
                for child in &mut folder.layers {
                    child.renew_ids(ids);
                }
            }
        }
        if let Some(mask) = &mut self.mask {
            mask.paint.id = ids.next();
            for effect in &mut mask.effects {
                effect.renew_ids(ids);
            }
        }
    }

    /// The paint layers of this layer itself (content, mask, paint effects), not of its
    /// children.
    pub fn own_paint(&self) -> impl Iterator<Item = &PaintLayer> {
        let content = match &self.content {
            Content::Paint(paint) => Some(paint),
            _ => None,
        };
        let mask = self.mask.iter().flat_map(|m| {
            std::iter::once(&m.paint).chain(m.effects.iter().filter_map(|e| match &e.kind {
                EffectKind::Paint(paint) => Some(paint),
                _ => None,
            }))
        });
        content.into_iter().chain(mask)
    }

    fn own_paint_mut(&mut self, id: u64) -> Option<&mut PaintLayer> {
        if let Content::Paint(paint) = &mut self.content
            && paint.id == id
        {
            return Some(paint);
        }
        let mask = self.mask.as_mut()?;
        if mask.paint.id == id {
            return Some(&mut mask.paint);
        }
        mask.effects.iter_mut().find_map(|e| match &mut e.kind {
            EffectKind::Paint(paint) if paint.id == id => Some(paint),
            _ => None,
        })
    }
}

/// Where a layer sits: in a folder (by id) or at the top level, at an index counted from
/// the bottom.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Place {
    pub parent: Option<u64>,
    pub index: usize,
}

/// The layers of a texture set, bottom to top.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LayerStack {
    pub layers: Vec<Layer>,
}

impl LayerStack {
    /// A new texture set's stack: one empty paint layer, as in Painter.
    pub fn new_default(ids: &mut Ids) -> Self {
        Self {
            layers: vec![Layer::new(LayerKind::Paint, "Layer 1".into(), ids)],
        }
    }

    /// Every layer, depth first, children before the folder's next sibling, bottom to top
    /// within each list.
    pub fn iter(&self) -> impl Iterator<Item = &Layer> {
        let mut todo: Vec<&Layer> = self.layers.iter().rev().collect();
        std::iter::from_fn(move || {
            let layer = todo.pop()?;
            if let Some(children) = layer.children() {
                todo.extend(children.iter().rev());
            }
            Some(layer)
        })
    }

    pub fn find(&self, id: u64) -> Option<&Layer> {
        self.iter().find(|l| l.id == id)
    }

    pub fn find_mut(&mut self, id: u64) -> Option<&mut Layer> {
        fn search(layers: &mut [Layer], id: u64) -> Option<&mut Layer> {
            for layer in layers {
                if layer.id == id {
                    return Some(layer);
                }
                if let Some(children) = layer.children_mut()
                    && let Some(found) = search(children, id)
                {
                    return Some(found);
                }
            }
            None
        }
        search(&mut self.layers, id)
    }

    /// Where the layer is.
    pub fn place(&self, id: u64) -> Option<Place> {
        fn search(layers: &[Layer], parent: Option<u64>, id: u64) -> Option<Place> {
            for (index, layer) in layers.iter().enumerate() {
                if layer.id == id {
                    return Some(Place { parent, index });
                }
                if let Some(children) = layer.children()
                    && let Some(found) = search(children, Some(layer.id), id)
                {
                    return Some(found);
                }
            }
            None
        }
        search(&self.layers, None, id)
    }

    /// The layer list of a folder, or the top level for None.
    pub fn list(&self, parent: Option<u64>) -> Option<&Vec<Layer>> {
        match parent {
            None => Some(&self.layers),
            Some(id) => self.find(id)?.children(),
        }
    }

    pub fn list_mut(&mut self, parent: Option<u64>) -> Option<&mut Vec<Layer>> {
        match parent {
            None => Some(&mut self.layers),
            Some(id) => self.find_mut(id)?.children_mut(),
        }
    }

    /// Whether `id` is `ancestor` or lies inside it.
    pub fn is_within(&self, id: u64, ancestor: u64) -> bool {
        self.find(ancestor)
            .is_some_and(|a| a.id == id || a.children().is_some_and(|c| contains(c, id)))
    }

    /// Takes a layer out of the stack.
    pub fn remove(&mut self, id: u64) -> Option<Layer> {
        let place = self.place(id)?;
        Some(self.list_mut(place.parent)?.remove(place.index))
    }

    /// Puts a layer at a place; the index is clamped to the list. Returns the layer back if
    /// the parent is not a folder of this stack.
    pub fn insert(&mut self, place: Place, layer: Layer) -> Result<(), Box<Layer>> {
        match self.list_mut(place.parent) {
            Some(list) => {
                let index = place.index.min(list.len());
                list.insert(index, layer);
                Ok(())
            }
            None => Err(Box::new(layer)),
        }
    }

    /// Where a new layer goes so that it lies right above `anchor` (Painter adds layers
    /// above the selected one), or on top without an anchor.
    pub fn place_above(&self, anchor: Option<u64>) -> Place {
        match anchor.and_then(|id| self.place(id)) {
            Some(place) => Place {
                parent: place.parent,
                index: place.index + 1,
            },
            None => Place {
                parent: None,
                index: self.layers.len(),
            },
        }
    }

    /// Moves a layer to a place, counted in the list as it is before the move. Refuses to
    /// move a folder into itself.
    pub fn move_to(&mut self, id: u64, target: Place) -> bool {
        let Some(from) = self.place(id) else {
            return false;
        };
        if target.parent.is_some_and(|p| self.is_within(p, id)) {
            return false;
        }
        let Some(layer) = self.remove(id) else {
            return false;
        };
        let mut target = target;
        if target.parent == from.parent && target.index > from.index {
            target.index -= 1;
        }
        match self.insert(target, layer) {
            Ok(()) => true,
            Err(layer) => {
                // Unreachable after the checks above; put it back rather than lose it.
                let _ = self.insert(from, *layer);
                false
            }
        }
    }

    /// Every paint layer: contents, masks and paint effects.
    pub fn paint_layers(&self) -> impl Iterator<Item = &PaintLayer> {
        self.iter().flat_map(Layer::own_paint)
    }

    pub fn paint(&self, id: u64) -> Option<&PaintLayer> {
        self.paint_layers().find(|p| p.id == id)
    }

    pub fn paint_mut(&mut self, id: u64) -> Option<&mut PaintLayer> {
        fn search(layers: &mut [Layer], id: u64) -> Option<&mut PaintLayer> {
            for layer in layers {
                // Checked first so the borrow of the layer's own paint ends before the
                // children are searched.
                let own = layer.own_paint().any(|p| p.id == id);
                if own {
                    return layer.own_paint_mut(id);
                }
                if let Some(children) = layer.children_mut()
                    && let Some(found) = search(children, id)
                {
                    return Some(found);
                }
            }
            None
        }
        search(&mut self.layers, id)
    }

    /// The layer whose content, mask or effects hold the paint layer.
    pub fn owner_of_paint(&self, paint: u64) -> Option<&Layer> {
        self.iter().find(|l| l.own_paint().any(|p| p.id == paint))
    }

    /// A name for a new layer of the kind: the base name with the next free number.
    pub fn new_name(&self, kind: LayerKind) -> String {
        let base = kind.base_name();
        let highest = self
            .iter()
            .filter_map(|l| {
                l.name
                    .strip_prefix(base)?
                    .strip_prefix(' ')?
                    .parse::<u32>()
                    .ok()
            })
            .max()
            .unwrap_or(0);
        format!("{base} {}", highest + 1)
    }

    /// Wraps layers that share a list in a new folder placed where the topmost of them was.
    /// Layers from other lists than the first one's are ignored. Returns the folder's id.
    pub fn group(&mut self, ids_to_group: &[u64], ids: &mut Ids) -> Option<u64> {
        let first = self.place(*ids_to_group.first()?)?;
        let mut members: Vec<Place> = ids_to_group
            .iter()
            .filter_map(|&id| self.place(id))
            .filter(|p| p.parent == first.parent)
            .collect();
        members.sort_by_key(|p| p.index);
        members.dedup();
        let top = members.last()?.index;
        let mut folder = Layer::new(LayerKind::Folder, self.new_name(LayerKind::Folder), ids);
        let list = self.list_mut(first.parent)?;
        let mut children = Vec::new();
        for place in members.iter().rev() {
            children.push(list.remove(place.index));
        }
        children.reverse();
        let index = top + 1 - members.len();
        let id = folder.id;
        if let Some(c) = folder.children_mut() {
            *c = children;
        }
        list.insert(index, folder);
        Some(id)
    }
}

fn contains(layers: &[Layer], id: u64) -> bool {
    layers
        .iter()
        .any(|l| l.id == id || l.children().is_some_and(|c| contains(c, id)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(layers: &[Layer]) -> Vec<&str> {
        layers.iter().map(|l| l.name.as_str()).collect()
    }

    fn stack(ids: &mut Ids) -> LayerStack {
        // Bottom to top: A, Folder [B, C], D.
        let mut s = LayerStack::default();
        for name in ["A", "Folder", "D"] {
            let kind = if name == "Folder" {
                LayerKind::Folder
            } else {
                LayerKind::Paint
            };
            s.layers.push(Layer::new(kind, name.into(), ids));
        }
        let folder = s.layers[1].children_mut().unwrap();
        folder.push(Layer::new(LayerKind::Fill, "B".into(), ids));
        folder.push(Layer::new(LayerKind::Paint, "C".into(), ids));
        s
    }

    fn id_of(s: &LayerStack, name: &str) -> u64 {
        s.iter().find(|l| l.name == name).unwrap().id
    }

    #[test]
    fn iterates_depth_first_bottom_to_top() {
        let mut ids = Ids::default();
        let s = stack(&mut ids);
        let order: Vec<&str> = s.iter().map(|l| l.name.as_str()).collect();
        assert_eq!(order, ["A", "Folder", "B", "C", "D"]);
        let c = id_of(&s, "C");
        let folder = id_of(&s, "Folder");
        assert_eq!(
            s.place(c),
            Some(Place {
                parent: Some(folder),
                index: 1
            })
        );
        assert!(s.is_within(c, folder));
        assert!(!s.is_within(folder, c));
    }

    #[test]
    fn moves_layers_between_lists() {
        let mut ids = Ids::default();
        let mut s = stack(&mut ids);
        let (a, d, folder) = (id_of(&s, "A"), id_of(&s, "D"), id_of(&s, "Folder"));
        // D to the bottom.
        assert!(s.move_to(
            d,
            Place {
                parent: None,
                index: 0
            }
        ));
        assert_eq!(names(&s.layers), ["D", "A", "Folder"]);
        // A on top of the folder's children.
        assert!(s.move_to(
            a,
            Place {
                parent: Some(folder),
                index: 2
            }
        ));
        assert_eq!(names(s.list(Some(folder)).unwrap()), ["B", "C", "A"]);
        // Moving up within a list counts places before the move.
        let b = id_of(&s, "B");
        assert!(s.move_to(
            b,
            Place {
                parent: Some(folder),
                index: 3
            }
        ));
        assert_eq!(names(s.list(Some(folder)).unwrap()), ["C", "A", "B"]);
        // A folder can't go into itself.
        assert!(!s.move_to(
            folder,
            Place {
                parent: Some(folder),
                index: 0
            }
        ));
        assert_eq!(s.iter().count(), 5);
    }

    #[test]
    fn finds_paint_in_contents_and_masks() {
        let mut ids = Ids::default();
        let mut s = stack(&mut ids);
        let c = id_of(&s, "C");
        let mask_paint = ids.next();
        s.find_mut(c).unwrap().mask = Some(Mask::new(0.0, mask_paint));
        let effect_paint = ids.next();
        let effect = Effect::new(ids.next(), EffectKind::Paint(PaintLayer::new(effect_paint)));
        s.find_mut(c)
            .unwrap()
            .mask
            .as_mut()
            .unwrap()
            .effects
            .push(effect);
        assert_eq!(s.paint_layers().count(), 5, "A, C, D, C's mask and effect");
        assert!(s.paint_mut(mask_paint).is_some());
        assert!(s.paint_mut(effect_paint).is_some());
        assert_eq!(s.owner_of_paint(effect_paint).unwrap().id, c);
        assert!(s.paint_mut(9999).is_none());
    }

    #[test]
    fn duplicates_get_new_ids() {
        let mut ids = Ids::default();
        let s = stack(&mut ids);
        let folder = s.find(id_of(&s, "Folder")).unwrap();
        let copy = folder.duplicate(&mut ids);
        let old: Vec<u64> = std::iter::once(folder.id)
            .chain(folder.children().unwrap().iter().map(|l| l.id))
            .collect();
        assert!(!old.contains(&copy.id));
        for child in copy.children().unwrap() {
            assert!(!old.contains(&child.id));
        }
        assert_eq!(copy, {
            let mut same = copy.clone();
            same.name = copy.name.clone();
            same
        });
    }

    #[test]
    fn names_count_up() {
        let mut ids = Ids::default();
        let mut s = LayerStack::new_default(&mut ids);
        assert_eq!(s.new_name(LayerKind::Paint), "Layer 2");
        assert_eq!(s.new_name(LayerKind::Fill), "Fill layer 1");
        s.layers
            .push(Layer::new(LayerKind::Fill, "Fill layer 7".into(), &mut ids));
        assert_eq!(s.new_name(LayerKind::Fill), "Fill layer 8");
    }

    #[test]
    fn groups_layers_in_place() {
        let mut ids = Ids::default();
        let mut s = LayerStack::default();
        for name in ["A", "B", "C", "D"] {
            s.layers
                .push(Layer::new(LayerKind::Paint, name.into(), &mut ids));
        }
        let (b, c) = (id_of(&s, "B"), id_of(&s, "C"));
        let folder = s.group(&[c, b], &mut ids).unwrap();
        assert_eq!(names(&s.layers), ["A", "Folder 1", "D"]);
        assert_eq!(names(s.list(Some(folder)).unwrap()), ["B", "C"]);
    }

    #[test]
    fn levels_remap() {
        let levels = Levels {
            input: [0.2, 0.6],
            gamma: 1.0,
            output: [0.0, 1.0],
        };
        assert_eq!(levels.apply(0.1), 0.0);
        assert!((levels.apply(0.4) - 0.5).abs() < 1e-6);
        assert_eq!(levels.apply(0.9), 1.0);
        assert_eq!(Levels::default().apply(0.3), 0.3);
    }

    #[test]
    fn opening_a_folder_is_no_edit() {
        let mut ids = Ids::default();
        let before = stack(&mut ids);
        let mut after = before.clone();
        let folder = id_of(&after, "Folder");
        if let Some(Content::Folder(f)) = after.find_mut(folder).map(|l| &mut l.content) {
            f.expanded = !f.expanded;
        }
        assert_eq!(before, after);
    }

    #[test]
    fn new_layers_blend_like_painter() {
        let mut ids = Ids::default();
        for kind in [LayerKind::Paint, LayerKind::Fill, LayerKind::Folder] {
            let layer = Layer::new(kind, String::new(), &mut ids);
            assert_eq!(layer.blending.get(Channel::BaseColor), ChannelBlend::NORMAL);
            assert_eq!(
                layer.blending.get(Channel::Height).mode,
                BlendMode::LinearDodge
            );
            assert_eq!(
                layer.blending.get(Channel::Normal).mode,
                BlendMode::NormalMapDetail
            );
        }
        let Content::Fill(fill) = Layer::new(LayerKind::Fill, String::new(), &mut ids).content
        else {
            unreachable!()
        };
        assert_eq!(
            fill.channels.iter().collect::<Vec<_>>(),
            [Channel::BaseColor]
        );
    }

    #[test]
    fn blend_codes_follow_the_list() {
        for (i, mode) in BlendMode::ALL.iter().enumerate() {
            assert_eq!(mode.code(), i as u32);
        }
        assert!(!BlendMode::Passthrough.for_layers());
        assert!(BlendMode::Multiply.for_masks());
    }
}
