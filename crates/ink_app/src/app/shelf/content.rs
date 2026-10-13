//! What the shelf shows without any files: the alphas made in code, and the pictures that
//! stand for brushes.

use eframe::egui;
use glam::{Vec2, Vec3};
use ink_core::{Alpha, AlphaImage, Brush, Dab, DabSpacer, Rng, StrokeSpace, shape_dabs};
use std::sync::Arc;

const ALPHA_SIZE: u32 = 256;

/// The alphas that ship with the app, made in code so they are ours.
pub fn builtin_alphas() -> Vec<Arc<AlphaImage>> {
    vec![
        make("Cloud", cloud),
        make("Grain", grain),
        make("Splatter", splatter),
        make("Scratches", scratches),
        make("Hair strokes", hair),
        make("Cracks", cracks),
        make("Blotches", blotches),
        make("Dust", dust),
    ]
}

fn make(name: &str, draw: fn(&mut [f32])) -> Arc<AlphaImage> {
    let mut values = vec![0.0; (ALPHA_SIZE * ALPHA_SIZE) as usize];
    draw(&mut values);
    // Soft round edge so no alpha ends in a hard square.
    let size = ALPHA_SIZE as f32;
    let pixels = values
        .iter()
        .enumerate()
        .map(|(i, v)| {
            let x = ((i as u32 % ALPHA_SIZE) as f32 + 0.5) / size * 2.0 - 1.0;
            let y = ((i as u32 / ALPHA_SIZE) as f32 + 0.5) / size * 2.0 - 1.0;
            let edge = ((1.0 - (x * x + y * y).sqrt()) * 6.0).clamp(0.0, 1.0);
            (v.clamp(0.0, 1.0) * edge * 255.0) as u8
        })
        .collect();
    Arc::new(AlphaImage::new(
        name.to_owned(),
        ALPHA_SIZE,
        ALPHA_SIZE,
        pixels,
    ))
}

/// Smooth value noise, `cells` lattice points across.
fn value_noise(rng: &mut Rng, cells: u32) -> impl Fn(f32, f32) -> f32 + use<> {
    let n = cells as usize + 1;
    let lattice: Vec<f32> = (0..n * n).map(|_| rng.next_f32()).collect();
    move |x: f32, y: f32| {
        let (fx, fy) = (x * cells as f32, y * cells as f32);
        let (ix, iy) = ((fx as usize).min(n - 2), (fy as usize).min(n - 2));
        let (tx, ty) = (fx - ix as f32, fy - iy as f32);
        let smooth = |t: f32| t * t * (3.0 - 2.0 * t);
        let (tx, ty) = (smooth(tx), smooth(ty));
        let at = |dx: usize, dy: usize| lattice[(iy + dy) * n + ix + dx];
        let top = at(0, 0) + (at(1, 0) - at(0, 0)) * tx;
        let bottom = at(0, 1) + (at(1, 1) - at(0, 1)) * tx;
        top + (bottom - top) * ty
    }
}

fn each_pixel(values: &mut [f32], mut f: impl FnMut(f32, f32) -> f32) {
    for (i, v) in values.iter_mut().enumerate() {
        let x = ((i as u32 % ALPHA_SIZE) as f32 + 0.5) / ALPHA_SIZE as f32;
        let y = ((i as u32 / ALPHA_SIZE) as f32 + 0.5) / ALPHA_SIZE as f32;
        *v = f(x, y);
    }
}

fn cloud(values: &mut [f32]) {
    let mut rng = Rng::new(11);
    let layers: Vec<_> = [3, 6, 12, 24]
        .into_iter()
        .map(|c| value_noise(&mut rng, c))
        .collect();
    each_pixel(values, |x, y| {
        let mut sum = 0.0;
        let mut weight = 0.5;
        let mut total = 0.0;
        for layer in &layers {
            sum += layer(x, y) * weight;
            total += weight;
            weight *= 0.5;
        }
        ((sum / total - 0.25) * 2.0).clamp(0.0, 1.0)
    });
}

fn grain(values: &mut [f32]) {
    let mut rng = Rng::new(23);
    let fine = value_noise(&mut rng, 96);
    let coarse = value_noise(&mut rng, 20);
    each_pixel(values, |x, y| {
        let v = fine(x, y) * 0.7 + coarse(x, y) * 0.3;
        ((v - 0.5) * 5.0 + 0.5).clamp(0.0, 1.0)
    });
}

/// Stamps a soft disc into the alpha.
fn blob(values: &mut [f32], cx: f32, cy: f32, radius: f32, strength: f32) {
    let size = ALPHA_SIZE as f32;
    let (x0, x1) = (
        ((cx - radius) * size).floor().max(0.0),
        ((cx + radius) * size).ceil().min(size - 1.0),
    );
    let (y0, y1) = (
        ((cy - radius) * size).floor().max(0.0),
        ((cy + radius) * size).ceil().min(size - 1.0),
    );
    for py in y0 as u32..=y1 as u32 {
        for px in x0 as u32..=x1 as u32 {
            let dx = (px as f32 + 0.5) / size - cx;
            let dy = (py as f32 + 0.5) / size - cy;
            let d = (dx * dx + dy * dy).sqrt() / radius;
            if d < 1.0 {
                let v = &mut values[(py * ALPHA_SIZE + px) as usize];
                *v = v.max(strength * ((1.0 - d) / 0.35).min(1.0));
            }
        }
    }
}

fn splatter(values: &mut [f32]) {
    let mut rng = Rng::new(31);
    for _ in 0..90 {
        let angle = rng.next_f32() * std::f32::consts::TAU;
        let distance = rng.next_f32().powf(0.7) * 0.36;
        let radius = 0.004 + rng.next_f32().powi(3) * 0.04;
        blob(
            values,
            0.5 + angle.cos() * distance,
            0.5 + angle.sin() * distance,
            radius,
            1.0,
        );
    }
    blob(values, 0.5, 0.5, 0.11, 1.0);
}

/// Draws a line of dots with a width.
fn stroke(values: &mut [f32], from: Vec2, to: Vec2, radius: f32, strength: f32) {
    let steps = ((to - from).length() / (radius * 0.5)).ceil().max(1.0) as u32;
    for i in 0..=steps {
        let p = from.lerp(to, i as f32 / steps as f32);
        blob(values, p.x, p.y, radius, strength);
    }
}

fn scratches(values: &mut [f32]) {
    let mut rng = Rng::new(47);
    for _ in 0..22 {
        let start = Vec2::new(0.1 + rng.next_f32() * 0.8, 0.1 + rng.next_f32() * 0.8);
        let angle = -0.5 + rng.next_f32() * 0.35;
        let length = 0.15 + rng.next_f32() * 0.35;
        let end = start + Vec2::from_angle(angle) * length;
        stroke(
            values,
            start,
            end,
            0.004 + rng.next_f32() * 0.004,
            0.5 + rng.next_f32() * 0.5,
        );
    }
}

fn hair(values: &mut [f32]) {
    let mut rng = Rng::new(59);
    for i in 0..26 {
        let x = 0.2 + 0.6 * (i as f32 + rng.next_f32() * 0.6) / 26.0;
        let wobble = (rng.next_f32() - 0.5) * 0.06;
        let top = 0.12 + rng.next_f32() * 0.12;
        let bottom = 0.78 + rng.next_f32() * 0.12;
        stroke(
            values,
            Vec2::new(x, top),
            Vec2::new(x + wobble, bottom),
            0.006,
            0.45 + rng.next_f32() * 0.55,
        );
    }
}

fn cracks(values: &mut [f32]) {
    let mut rng = Rng::new(71);
    for _ in 0..9 {
        let mut p = Vec2::new(0.15 + rng.next_f32() * 0.7, 0.15 + rng.next_f32() * 0.7);
        let mut angle = rng.next_f32() * std::f32::consts::TAU;
        let mut width = 0.006 + rng.next_f32() * 0.004;
        for step in 0..26 {
            angle += (rng.next_f32() - 0.5) * 1.1;
            let next = p + Vec2::from_angle(angle) * 0.022;
            stroke(values, p, next, width, 0.8 + rng.next_f32() * 0.2);
            // Now and then a thinner branch splits off.
            if step % 6 == 3 {
                let side = angle + (rng.next_f32() - 0.5) * 2.4;
                stroke(
                    values,
                    next,
                    next + Vec2::from_angle(side) * 0.07,
                    width * 0.5,
                    0.7,
                );
            }
            p = next;
            width *= 0.97;
        }
    }
}

fn blotches(values: &mut [f32]) {
    let mut rng = Rng::new(83);
    let layers: Vec<_> = [2, 4, 9]
        .into_iter()
        .map(|c| value_noise(&mut rng, c))
        .collect();
    each_pixel(values, |x, y| {
        let n = layers[0](x, y) * 0.55 + layers[1](x, y) * 0.3 + layers[2](x, y) * 0.15;
        ((n - 0.5) * 7.0 + 0.5).clamp(0.0, 1.0)
    });
}

fn dust(values: &mut [f32]) {
    let mut rng = Rng::new(97);
    for _ in 0..1200 {
        let angle = rng.next_f32() * std::f32::consts::TAU;
        let distance = rng.next_f32().sqrt() * 0.4;
        blob(
            values,
            0.5 + angle.cos() * distance,
            0.5 + angle.sin() * distance,
            0.004 + rng.next_f32().powi(2) * 0.01,
            0.5 + rng.next_f32() * 0.5,
        );
    }
}

// ---- Brush pictures ----

/// Width and height of a brush's picture in pixels.
pub const BRUSH_PICTURE: [usize; 2] = [128, 56];

/// A picture of what the brush paints: a wavy stroke, stamped with the brush's tip, spacing,
/// jitter and follow path, white on dark.
pub fn brush_picture(brush: &Brush) -> egui::ColorImage {
    let [w, h] = BRUSH_PICTURE;
    // The size of the tip on the picture grows with the brush's size, within the picture.
    let radius = (3.0 + (brush.size.max(1.0).ln() / 100f32.ln()) * 9.0).min(h as f32 * 0.4);
    let curve = |t: f32| {
        Vec2::new(
            12.0 + t * (w as f32 - 24.0),
            h as f32 * 0.5 + (t * std::f32::consts::TAU * 1.2).sin() * h as f32 * 0.2,
        )
    };
    let mut spacer = DabSpacer::new(brush.spacing);
    let mut dabs = Vec::new();
    for i in 0..=200 {
        let p = curve(i as f32 / 200.0);
        spacer.add(
            Dab {
                position: p.extend(0.0),
                normal: Vec3::Z,
                up: Vec3::NEG_Y,
                view: Vec3::NEG_Z,
                radius,
                flow: brush.flow,
            },
            &mut dabs,
        );
    }
    // The picture must stay light: thin out dabs of very small spacing.
    if dabs.len() > 400 {
        let step = dabs.len() / 400 + 1;
        dabs = dabs.into_iter().step_by(step).collect();
    }
    shape_dabs(brush, StrokeSpace::Uv, 1, 0, None, &mut dabs);

    let mut coverage = vec![1.0f32; w * h];
    for dab in &dabs {
        let (cx, cy, r) = (dab.position.x, dab.position.y, dab.radius.max(0.5));
        let up = Vec2::new(dab.up.x, dab.up.y);
        let right = Vec2::new(-up.y, up.x);
        for py in (cy - r).floor().max(0.0) as usize..((cy + r).ceil() as usize).min(h - 1) + 1 {
            for px in (cx - r).floor().max(0.0) as usize..((cx + r).ceil() as usize).min(w - 1) + 1
            {
                let offset = Vec2::new(px as f32 + 0.5 - cx, py as f32 + 0.5 - cy);
                let local = Vec2::new(offset.dot(right), offset.dot(up)) / r;
                let value = tip(brush, local);
                coverage[py * w + px] *= 1.0 - (value * dab.flow).clamp(0.0, 1.0);
            }
        }
    }
    let background = [0x2B, 0x2B, 0x2B];
    let ink = [0xE6, 0xE6, 0xE6];
    let pixels = coverage
        .iter()
        .map(|keep| {
            let c = 1.0 - keep;
            let mix = |i: usize| {
                (background[i] as f32 + (ink[i] as f32 - background[i] as f32) * c) as u8
            };
            egui::Color32::from_rgb(mix(0), mix(1), mix(2))
        })
        .collect();
    egui::ColorImage {
        size: [w, h],
        pixels,
        source_size: egui::vec2(w as f32, h as f32),
    }
}

/// The tip's value at a point of the dab ([-1, 1], y up).
fn tip(brush: &Brush, local: Vec2) -> f32 {
    match &brush.alpha {
        Alpha::Round | Alpha::Square => {
            let d = if matches!(brush.alpha, Alpha::Square) {
                local.x.abs().max(local.y.abs())
            } else {
                local.length()
            };
            let inner = brush.hardness.clamp(0.0, 0.98);
            1.0 - smoothstep(inner, 1.0, d)
        }
        Alpha::Image(image) => {
            if local.x.abs() > 1.0 || local.y.abs() > 1.0 {
                return 0.0;
            }
            let x = ((local.x * 0.5 + 0.5) * image.width as f32) as u32;
            let y = ((0.5 - local.y * 0.5) * image.height as f32) as u32;
            let i = (y.min(image.height - 1) * image.width + x.min(image.width - 1)) as usize;
            image.pixels[i] as f32 / 255.0
        }
    }
}

fn smoothstep(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// A picture of an alpha: white where it paints, on dark.
pub fn alpha_picture(image: &AlphaImage, side: usize) -> egui::ColorImage {
    let mut pixels = Vec::with_capacity(side * side);
    for y in 0..side {
        for x in 0..side {
            let sx = (x * image.width as usize / side).min(image.width as usize - 1);
            let sy = (y * image.height as usize / side).min(image.height as usize - 1);
            let v = image.pixels[sy * image.width as usize + sx];
            let bg: u16 = 0x2B;
            let c = (bg + ((0xE6 - bg) * v as u16) / 255) as u8;
            pixels.push(egui::Color32::from_rgb(c, c, c));
        }
    }
    egui::ColorImage {
        size: [side, side],
        pixels,
        source_size: egui::vec2(side as f32, side as f32),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_alphas_have_content_and_a_soft_border() {
        for alpha in builtin_alphas() {
            let inside = alpha.pixels.iter().filter(|&&p| p > 128).count();
            let total = alpha.pixels.len();
            assert!(inside > total / 50, "{} is nearly empty", alpha.name);
            assert!(inside < total * 9 / 10, "{} is nearly full", alpha.name);
            assert_eq!(alpha.pixels[0], 0, "{} has a hard corner", alpha.name);
        }
    }

    #[test]
    fn brush_pictures_show_the_stroke() {
        let picture = brush_picture(&Brush::default());
        assert_eq!(picture.size, BRUSH_PICTURE);
        let lit = picture.pixels.iter().filter(|p| p.r() > 0x60).count();
        assert!(
            lit > 50 && lit < picture.pixels.len() / 2,
            "{lit} lit pixels"
        );
        // A scattered brush is sparser than a solid one.
        let scatter = brush_picture(&Brush {
            size: 3.0,
            hardness: 1.0,
            spacing: 2.5,
            ..Brush::default()
        });
        let scatter_lit = scatter.pixels.iter().filter(|p| p.r() > 0x60).count();
        assert!(scatter_lit < lit);
    }
}
