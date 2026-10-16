//! Smart materials and masks that ship with the app, built in code from our own fills,
//! noise and generators: no outside images. They use the mesh maps, so bake the model first
//! (Ctrl+Shift+B) to see edges and crevices; without maps they fall back to noise.

use crate::project::smart::Smart;
use ink_core::{
    BlendMode, Channel, ChannelSet, ChannelValues, Content, Effect, EffectKind, Fill, Generator,
    GeneratorKind, Ids, Layer, LayerKind, Levels, Mask,
};

/// The presets, materials first.
pub fn all() -> Vec<Smart> {
    let mut ids = Ids::default();
    let ids = &mut ids;
    let mut presets = vec![
        steel(ids),
        painted_metal(ids),
        rusty_metal(ids),
        gold(ids),
        copper_patina(ids),
        glossy_plastic(ids),
        rubber(ids),
        dry_wood(ids),
        leather(ids),
        rough_stone(ids),
        concrete(ids),
        fabric(ids),
    ];
    presets.extend(masks(ids));
    presets
}

// ---- Building blocks ----

fn generator(kind: GeneratorKind, params: &[(&str, f32)]) -> Generator {
    let mut g = Generator::new(kind);
    for (key, value) in params {
        g.set(key, *value);
    }
    g
}

/// A fill layer that sets the base color (sRGB), roughness and metallic, and nothing else.
fn surface(ids: &mut Ids, name: &str, color: [f32; 3], roughness: f32, metallic: f32) -> Layer {
    let mut layer = Layer::new(LayerKind::Fill, name.to_owned(), ids);
    if let Content::Fill(fill) = &mut layer.content {
        let mut channels = ChannelSet::NONE;
        for channel in [Channel::BaseColor, Channel::Roughness, Channel::Metallic] {
            channels.set(channel, true);
        }
        *fill = Fill {
            channels,
            values: ChannelValues {
                base_color: color,
                roughness,
                metallic,
                ..ChannelValues::default()
            },
            ..Fill::default()
        };
    }
    layer
}

/// Gives a fill layer a height of its own, so where its mask shows it, it stands out from
/// (or sinks into) the layers below: paint over bare metal, rust crust, worn-through edges.
fn with_height(mut layer: Layer, height: f32) -> Layer {
    if let Content::Fill(fill) = &mut layer.content {
        fill.channels.set(Channel::Height, true);
        fill.values.height = height;
    }
    layer
}

/// A fill layer that only shapes the surface: `height` where its mask is white, at
/// `opacity`. Pores, grain, bumps and cracks.
fn relief(
    ids: &mut Ids,
    name: &str,
    height: f32,
    opacity: f32,
    effects: Vec<(Generator, BlendMode)>,
) -> Layer {
    let mut layer = Layer::new(LayerKind::Fill, name.to_owned(), ids);
    if let Content::Fill(fill) = &mut layer.content {
        let mut channels = ChannelSet::NONE;
        channels.set(Channel::Height, true);
        *fill = Fill {
            channels,
            values: ChannelValues {
                height,
                ..ChannelValues::default()
            },
            ..Fill::default()
        };
    }
    layer.blending.get_mut(Channel::Height).opacity = opacity;
    masked(ids, layer, effects)
}

/// Puts a black mask on the layer; the generators in it say where the layer shows. Each is
/// `(generator, blend mode)` and the modes combine them in order.
fn masked(ids: &mut Ids, mut layer: Layer, effects: Vec<(Generator, BlendMode)>) -> Layer {
    let mut mask = Mask::new(0.0, ids.next());
    for (generator, blend) in effects {
        let mut effect = Effect::new(ids.next(), EffectKind::Generator(generator));
        effect.blend = blend;
        mask.effects.push(effect);
    }
    layer.mask = Some(mask);
    layer
}

/// Swaps black and white in the layer's mask: its generators then pick what they left out.
fn invert_mask(mut layer: Layer) -> Layer {
    if let Some(mask) = &mut layer.mask {
        for effect in &mut mask.effects {
            if let EffectKind::Generator(g) = &mut effect.kind {
                g.set("invert", 1.0);
            }
        }
    }
    layer
}

/// Thin lines along the borders of noise cells: white where a texel is closer than `width`
/// (in cells) to a border. Cracks, creases, seams.
fn lines(ids: &mut Ids, layer: Layer, scale: f32, width: f32, seed: f32) -> Layer {
    let mut layer = masked(
        ids,
        layer,
        vec![(noise(4.0, scale, 1.0, 0.0, seed), Normal)],
    );
    if let Some(mask) = &mut layer.mask {
        let levels = Levels {
            input: [0.0, width],
            output: [1.0, 0.0],
            ..Levels::default()
        };
        mask.effects
            .push(Effect::new(ids.next(), EffectKind::Levels(levels)));
    }
    layer
}

/// Limits the layer's mask to where `generator` is white (multiplied on top).
fn only_where(ids: &mut Ids, mut layer: Layer, generator: Generator) -> Layer {
    if let Some(mask) = &mut layer.mask {
        let mut effect = Effect::new(ids.next(), EffectKind::Generator(generator));
        effect.blend = Multiply;
        mask.effects.push(effect);
    }
    layer
}

fn folder(ids: &mut Ids, name: &str, layers: Vec<Layer>) -> Layer {
    let mut folder = Layer::new(LayerKind::Folder, name.to_owned(), ids);
    if let Content::Folder(f) = &mut folder.content {
        f.layers = layers;
    }
    folder
}

/// A material preset: the layers in a folder, shown with the color of its main layer.
fn material(ids: &mut Ids, name: &str, layers: Vec<Layer>, swatch: [f32; 3]) -> Smart {
    let folder = folder(ids, name, layers);
    Smart::material(name.to_owned(), vec![folder]).with_swatch(swatch)
}

use BlendMode::{Multiply, Normal, Subtract};
use GeneratorKind::{Dirt, EdgeWear, MaskBuilder, Noise};

fn edge_wear(amount: f32, noise_scale: f32, seed: f32) -> Generator {
    generator(
        EdgeWear,
        &[
            ("amount", amount),
            ("noise_scale", noise_scale),
            ("seed", seed),
        ],
    )
}

fn dirt(amount: f32, seed: f32) -> Generator {
    generator(Dirt, &[("amount", amount), ("seed", seed)])
}

fn patches(scale: f32, contrast: f32, balance: f32, seed: f32) -> Generator {
    noise(3.0, scale, contrast, balance, seed)
}

/// A noise of `kind`: 0 Perlin, 1 fractal, 2 cells, 3 clouds, 4 cell edges.
fn noise(kind: f32, scale: f32, contrast: f32, balance: f32, seed: f32) -> Generator {
    generator(
        Noise,
        &[
            ("type", kind),
            ("scale", scale),
            ("contrast", contrast),
            ("balance", balance),
            ("seed", seed),
        ],
    )
}

/// Fine, even grain over the whole surface, for the micro relief of a material.
fn grain(scale: f32, seed: f32) -> Generator {
    noise(1.0, scale, 1.0, 0.0, seed)
}

// ---- Materials ----

fn steel(ids: &mut Ids) -> Smart {
    let base = surface(ids, "Steel", [0.62, 0.63, 0.64], 0.32, 1.0);
    let grain = relief(
        ids,
        "Fine grain",
        0.15,
        0.4,
        vec![(grain(160.0, 2.0), Normal)],
    );
    let scuffs = with_height(surface(ids, "Scuffs", [0.5, 0.5, 0.52], 0.6, 1.0), -0.1);
    let scuffs = masked(ids, scuffs, vec![(patches(48.0, 3.0, -0.2, 3.0), Normal)]);
    material(ids, "Steel", vec![base, grain, scuffs], [0.62, 0.63, 0.64])
}

fn painted_metal(ids: &mut Ids) -> Smart {
    let steel = surface(ids, "Bare steel", [0.6, 0.6, 0.62], 0.4, 1.0);
    // The coat has a thickness: where the edges wear through, it steps down to the steel.
    let paint = with_height(surface(ids, "Paint", [0.62, 0.07, 0.06], 0.42, 0.0), 0.3);
    // The paint is everywhere except where edges are worn through.
    let paint = masked(
        ids,
        paint,
        vec![
            (
                generator(
                    Noise,
                    &[
                        ("type", 0.0),
                        ("scale", 1.0),
                        ("contrast", 0.0),
                        ("balance", 0.5),
                    ],
                ),
                Normal,
            ),
            (edge_wear(0.55, 26.0, 1.0), Subtract),
        ],
    );
    let grime = surface(ids, "Grime", [0.06, 0.05, 0.04], 0.9, 0.0);
    let grime = masked(ids, grime, vec![(dirt(0.5, 2.0), Normal)]);
    let peel = relief(
        ids,
        "Orange peel",
        0.1,
        0.3,
        vec![(grain(200.0, 32.0), Normal)],
    );
    material(
        ids,
        "Painted metal",
        vec![steel, paint, peel, grime],
        [0.62, 0.07, 0.06],
    )
}

fn rusty_metal(ids: &mut Ids) -> Smart {
    let steel = surface(ids, "Dark steel", [0.32, 0.33, 0.34], 0.5, 1.0);
    // Rust grows as a crust over the steel, rough with its own bumps.
    let rust = with_height(surface(ids, "Rust", [0.42, 0.18, 0.07], 0.85, 0.0), 0.35);
    let rust = masked(
        ids,
        rust,
        vec![(
            generator(
                MaskBuilder,
                &[
                    ("curvature", 0.25),
                    ("occlusion", -0.35),
                    ("noise", 0.9),
                    ("noise_scale", 7.0),
                    ("contrast", 3.0),
                    ("seed", 4.0),
                ],
            ),
            Normal,
        )],
    );
    let fine = surface(ids, "Orange dust", [0.7, 0.32, 0.1], 0.95, 0.0);
    let fine = masked(
        ids,
        fine,
        vec![
            (patches(30.0, 4.0, -0.35, 9.0), Normal),
            (dirt(0.4, 5.0), Multiply),
        ],
    );
    // Pits eaten into the steel, in patches rather than everywhere.
    let pitting = relief(
        ids,
        "Pitting",
        -0.3,
        0.35,
        vec![(noise(2.0, 45.0, 6.0, -0.6, 10.0), Normal)],
    );
    let pitting = only_where(ids, pitting, patches(6.0, 2.5, -0.1, 31.0));
    material(
        ids,
        "Rusty metal",
        vec![steel, pitting, rust, fine],
        [0.42, 0.18, 0.07],
    )
}

fn gold(ids: &mut Ids) -> Smart {
    let base = surface(ids, "Gold", [1.0, 0.77, 0.34], 0.22, 1.0);
    let dents = relief(
        ids,
        "Soft dents",
        -0.25,
        0.5,
        vec![(patches(14.0, 1.5, -0.2, 7.0), Normal)],
    );
    let worn = surface(ids, "Worn", [0.9, 0.62, 0.22], 0.45, 1.0);
    let worn = masked(ids, worn, vec![(edge_wear(0.7, 40.0, 6.0), Normal)]);
    material(ids, "Gold", vec![base, dents, worn], [1.0, 0.77, 0.34])
}

fn copper_patina(ids: &mut Ids) -> Smart {
    let copper = surface(ids, "Copper", [0.95, 0.5, 0.36], 0.3, 1.0);
    let patina = with_height(surface(ids, "Patina", [0.2, 0.58, 0.48], 0.72, 0.0), 0.2);
    let patina = masked(
        ids,
        patina,
        vec![(
            generator(
                MaskBuilder,
                &[
                    ("curvature", -0.2),
                    ("occlusion", -0.55),
                    ("noise", 0.7),
                    ("noise_scale", 9.0),
                    ("contrast", 2.6),
                    ("seed", 8.0),
                ],
            ),
            Normal,
        )],
    );
    material(
        ids,
        "Copper patina",
        vec![copper, patina],
        [0.2, 0.58, 0.48],
    )
}

fn glossy_plastic(ids: &mut Ids) -> Smart {
    let base = surface(ids, "Plastic", [0.88, 0.88, 0.85], 0.16, 0.0);
    // Molded plastic is never quite flat: a faint texture from the mold.
    let mold = relief(
        ids,
        "Mold texture",
        0.1,
        0.25,
        vec![(grain(180.0, 33.0), Normal)],
    );
    let grime = surface(ids, "Yellowing", [0.45, 0.4, 0.26], 0.5, 0.0);
    let grime = masked(ids, grime, vec![(dirt(0.35, 7.0), Normal)]);
    material(
        ids,
        "Glossy plastic",
        vec![base, mold, grime],
        [0.88, 0.88, 0.85],
    )
}

fn rubber(ids: &mut Ids) -> Smart {
    let base = surface(ids, "Rubber", [0.035, 0.035, 0.04], 0.82, 0.0);
    let grain = relief(ids, "Grain", 0.3, 0.5, vec![(grain(120.0, 23.0), Normal)]);
    let dust = surface(ids, "Dust", [0.22, 0.22, 0.22], 0.96, 0.0);
    let dust = masked(ids, dust, vec![(patches(20.0, 3.0, -0.3, 11.0), Normal)]);
    material(ids, "Rubber", vec![base, grain, dust], [0.07, 0.07, 0.08])
}

fn dry_wood(ids: &mut Ids) -> Smart {
    let base = surface(ids, "Wood", [0.4, 0.26, 0.14], 0.62, 0.0);
    let fibers = relief(ids, "Fibers", 0.3, 0.6, vec![(grain(90.0, 24.0), Normal)]);
    let dark = with_height(
        surface(ids, "Dark patches", [0.22, 0.13, 0.07], 0.7, 0.0),
        -0.15,
    );
    let dark = masked(ids, dark, vec![(patches(5.0, 2.2, -0.1, 12.0), Normal)]);
    let edges = with_height(
        surface(ids, "Worn edges", [0.58, 0.42, 0.26], 0.5, 0.0),
        -0.2,
    );
    let edges = masked(ids, edges, vec![(edge_wear(0.5, 18.0, 13.0), Normal)]);
    material(
        ids,
        "Dry wood",
        vec![base, fibers, dark, edges],
        [0.4, 0.26, 0.14],
    )
}

fn leather(ids: &mut Ids) -> Smart {
    let base = surface(ids, "Leather", [0.2, 0.09, 0.045], 0.55, 0.0);
    // Pebbled grain: small raised cells with creases between them.
    let pebbles = relief(
        ids,
        "Pebbles",
        0.35,
        0.7,
        vec![(noise(2.0, 60.0, 2.0, 0.3, 26.0), Normal)],
    );
    let pebbles = invert_mask(pebbles);
    let creases = with_height(surface(ids, "Creases", [0.08, 0.035, 0.02], 0.7, 0.0), -0.4);
    let creases = lines(ids, creases, 24.0, 0.06, 14.0);
    let wear = surface(ids, "Worn", [0.38, 0.21, 0.11], 0.4, 0.0);
    let wear = masked(ids, wear, vec![(edge_wear(0.6, 22.0, 15.0), Normal)]);
    material(
        ids,
        "Leather",
        vec![base, pebbles, creases, wear],
        [0.2, 0.09, 0.045],
    )
}

fn rough_stone(ids: &mut Ids) -> Smart {
    let base = surface(ids, "Stone", [0.42, 0.41, 0.4], 0.9, 0.0);
    let bumps = relief(
        ids,
        "Bumps",
        1.0,
        1.0,
        vec![(noise(1.0, 22.0, 1.6, 0.0, 27.0), Normal)],
    );
    let grit = relief(ids, "Grit", 0.3, 0.5, vec![(grain(110.0, 28.0), Normal)]);
    let dark = surface(ids, "Dark patches", [0.22, 0.22, 0.22], 0.95, 0.0);
    let dark = masked(ids, dark, vec![(patches(13.0, 2.5, -0.15, 16.0), Normal)]);
    let dust = surface(ids, "Dust", [0.6, 0.58, 0.54], 1.0, 0.0);
    let dust = masked(ids, dust, vec![(edge_wear(0.45, 30.0, 17.0), Normal)]);
    material(
        ids,
        "Rough stone",
        vec![base, bumps, grit, dark, dust],
        [0.42, 0.41, 0.4],
    )
}

fn concrete(ids: &mut Ids) -> Smart {
    let base = surface(ids, "Concrete", [0.5, 0.5, 0.48], 0.95, 0.0);
    let stains = surface(ids, "Stains", [0.28, 0.28, 0.27], 0.98, 0.0);
    let stains = masked(ids, stains, vec![(dirt(0.6, 18.0), Normal)]);
    let pores = relief(ids, "Pores", -0.4, 0.5, vec![(grain(90.0, 22.0), Normal)]);
    // A few cracks, not a tiling: the lines only show where clouds of noise allow.
    let cracks = with_height(surface(ids, "Cracks", [0.13, 0.13, 0.13], 1.0, 0.0), -0.4);
    let cracks = lines(ids, cracks, 2.5, 0.012, 19.0);
    let cracks = only_where(ids, cracks, patches(3.0, 3.0, -0.35, 30.0));
    material(
        ids,
        "Concrete",
        vec![base, pores, stains, cracks],
        [0.5, 0.5, 0.48],
    )
}

fn fabric(ids: &mut Ids) -> Smart {
    let base = surface(ids, "Fabric", [0.28, 0.38, 0.55], 0.95, 0.0);
    let fuzz = relief(ids, "Fibers", 0.25, 0.5, vec![(grain(140.0, 29.0), Normal)]);
    let wear = surface(ids, "Faded", [0.5, 0.58, 0.72], 0.9, 0.0);
    let wear = masked(
        ids,
        wear,
        vec![
            (edge_wear(0.6, 30.0, 20.0), Normal),
            (patches(10.0, 2.0, -0.4, 21.0), Multiply),
        ],
    );
    material(ids, "Fabric", vec![base, fuzz, wear], [0.28, 0.38, 0.55])
}

// ---- Masks ----

fn mask_of(ids: &mut Ids, name: &str, effects: Vec<(Generator, BlendMode)>) -> Smart {
    let mut mask = Mask::new(0.0, ids.next());
    for (generator, blend) in effects {
        let mut effect = Effect::new(ids.next(), EffectKind::Generator(generator));
        effect.blend = blend;
        mask.effects.push(effect);
    }
    Smart::mask(name.to_owned(), mask, ids)
}

fn masks(ids: &mut Ids) -> Vec<Smart> {
    vec![
        mask_of(ids, "Edge wear", vec![(edge_wear(0.55, 24.0, 1.0), Normal)]),
        mask_of(ids, "Dirt in crevices", vec![(dirt(0.55, 2.0), Normal)]),
        mask_of(
            ids,
            "Worn surface",
            vec![(
                generator(
                    MaskBuilder,
                    &[
                        ("curvature", 0.5),
                        ("occlusion", -0.2),
                        ("noise", 0.55),
                        ("noise_scale", 14.0),
                        ("contrast", 2.2),
                    ],
                ),
                Normal,
            )],
        ),
        mask_of(
            ids,
            "Noise patches",
            vec![(patches(8.0, 2.5, -0.1, 3.0), Normal)],
        ),
        mask_of(
            ids,
            "Gradient from the bottom",
            vec![(
                generator(
                    GeneratorKind::PositionGradient,
                    &[
                        ("axis", 1.0),
                        ("position", 0.35),
                        ("softness", 0.5),
                        ("noise", 0.35),
                        ("invert", 1.0),
                    ],
                ),
                Normal,
            )],
        ),
        mask_of(
            ids,
            "Dust on top",
            vec![
                (
                    generator(
                        GeneratorKind::Light,
                        &[
                            ("azimuth", 0.0),
                            ("elevation", 90.0),
                            ("contrast", 2.5),
                            ("balance", -0.5),
                        ],
                    ),
                    Normal,
                ),
                (patches(18.0, 2.5, 0.0, 6.0), Multiply),
            ],
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::smart::SmartKind;

    #[test]
    fn the_starter_presets_are_complete() {
        let presets = all();
        let materials = presets
            .iter()
            .filter(|p| p.kind == SmartKind::Material)
            .count();
        let masks = presets.iter().filter(|p| p.kind == SmartKind::Mask).count();
        assert_eq!((materials, masks), (12, 6));
        let mut names: Vec<_> = presets.iter().map(|p| p.name.as_str()).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), presets.len(), "every preset has its own name");
        for preset in &presets {
            assert!(!preset.layers.is_empty(), "{}", preset.name);
            if preset.kind == SmartKind::Material {
                fn shapes_height(layers: &[Layer]) -> bool {
                    layers.iter().any(|l| match &l.content {
                        Content::Fill(fill) => fill.channels.contains(Channel::Height),
                        Content::Folder(folder) => shapes_height(&folder.layers),
                        Content::Paint(_) => false,
                    })
                }
                assert!(
                    shapes_height(&preset.layers),
                    "{} has no relief",
                    preset.name
                );
            }
            if preset.kind == SmartKind::Mask {
                assert!(
                    preset.mask_of().is_some_and(|m| !m.effects.is_empty()),
                    "{}",
                    preset.name
                );
            }
        }
    }
}
