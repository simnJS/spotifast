//! A small software renderer for epaint's meshes: the share card is laid
//! out and tessellated exactly as egui draws the window, then filled into
//! an RGBA buffer here, off the UI thread, without a GPU.
//!
//! Colours are premultiplied, as epaint's are, and blended source-over.

use egui::epaint::{ClippedPrimitive, ColorImage, Mesh, Primitive, TextureId, Vertex};
use egui::{Color32, Rect};

/// A premultiplied RGBA canvas.
pub struct Canvas {
    pub width: usize,
    pub height: usize,
    /// Premultiplied, row by row, four floats per pixel in 0..=1.
    pixels: Vec<[f32; 4]>,
}

/// A texture the meshes sample: premultiplied colours.
pub struct Texture<'a> {
    pub image: &'a ColorImage,
}

impl Texture<'_> {
    /// Bilinear sample at `uv` (0..=1 on both axes), premultiplied.
    fn sample(&self, u: f32, v: f32) -> [f32; 4] {
        let [w, h] = self.image.size;
        if w == 0 || h == 0 {
            return [1.0; 4];
        }
        let x = (u * w as f32 - 0.5).clamp(0.0, (w - 1) as f32);
        let y = (v * h as f32 - 0.5).clamp(0.0, (h - 1) as f32);
        let (x0, y0) = (x.floor() as usize, y.floor() as usize);
        let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
        let (fx, fy) = (x - x0 as f32, y - y0 as f32);
        let at = |x: usize, y: usize| to_floats(self.image.pixels[y * w + x]);
        let (a, b, c, d) = (at(x0, y0), at(x1, y0), at(x0, y1), at(x1, y1));
        let mut out = [0.0; 4];
        for i in 0..4 {
            let top = a[i] + (b[i] - a[i]) * fx;
            let bottom = c[i] + (d[i] - c[i]) * fx;
            out[i] = top + (bottom - top) * fy;
        }
        out
    }
}

fn to_floats(color: Color32) -> [f32; 4] {
    let [r, g, b, a] = color.to_array();
    [
        f32::from(r) / 255.0,
        f32::from(g) / 255.0,
        f32::from(b) / 255.0,
        f32::from(a) / 255.0,
    ]
}

impl Canvas {
    pub fn new(width: usize, height: usize, fill: Color32) -> Self {
        Self {
            width,
            height,
            pixels: vec![to_floats(fill); width * height],
        }
    }

    /// Fills every mesh, in order, with `texture` resolving their texture
    /// IDs. A mesh whose texture is unknown is skipped.
    pub fn draw<'a>(
        &mut self,
        primitives: &[ClippedPrimitive],
        texture: impl Fn(TextureId) -> Option<Texture<'a>>,
    ) {
        for primitive in primitives {
            let Primitive::Mesh(mesh) = &primitive.primitive else {
                continue;
            };
            let Some(tex) = texture(mesh.texture_id) else {
                continue;
            };
            self.mesh(mesh, primitive.clip_rect, &tex);
        }
    }

    fn mesh(&mut self, mesh: &Mesh, clip: Rect, texture: &Texture<'_>) {
        let canvas = Rect::from_min_max(
            egui::Pos2::ZERO,
            egui::pos2(self.width as f32, self.height as f32),
        );
        let clip = clip.intersect(canvas);
        if !clip.is_positive() {
            return;
        }
        for triangle in mesh.indices.as_chunks::<3>().0 {
            let [a, b, c] = triangle.map(|index| mesh.vertices[index as usize]);
            self.triangle(a, b, c, clip, texture);
        }
    }

    fn triangle(&mut self, a: Vertex, b: Vertex, c: Vertex, clip: Rect, texture: &Texture<'_>) {
        let area = edge(a.pos, b.pos, c.pos);
        if area.abs() < 1e-6 {
            return;
        }
        let left = a.pos.x.min(b.pos.x).min(c.pos.x).max(clip.left()).floor() as usize;
        let right = a.pos.x.max(b.pos.x).max(c.pos.x).min(clip.right()).ceil() as usize;
        let top = a.pos.y.min(b.pos.y).min(c.pos.y).max(clip.top()).floor() as usize;
        let bottom = a.pos.y.max(b.pos.y).max(c.pos.y).min(clip.bottom()).ceil() as usize;
        let (ca, cb, cc) = (to_floats(a.color), to_floats(b.color), to_floats(c.color));
        // A mesh of one colour on the white texel: no sampling needed.
        let flat_uv = a.uv == b.uv && b.uv == c.uv;
        let flat_texel = flat_uv.then(|| texture.sample(a.uv.x, a.uv.y));
        for y in top..bottom.min(self.height) {
            for x in left..right.min(self.width) {
                let p = egui::pos2(x as f32 + 0.5, y as f32 + 0.5);
                let wa = edge(b.pos, c.pos, p) / area;
                let wb = edge(c.pos, a.pos, p) / area;
                let wc = 1.0 - wa - wb;
                // Pixel centres on a shared edge belong to one triangle.
                if wa < -1e-5 || wb < -1e-5 || wc < -1e-5 {
                    continue;
                }
                let texel = flat_texel.unwrap_or_else(|| {
                    let u = a.uv.x * wa + b.uv.x * wb + c.uv.x * wc;
                    let v = a.uv.y * wa + b.uv.y * wb + c.uv.y * wc;
                    texture.sample(u, v)
                });
                let mut src = [0.0; 4];
                for i in 0..4 {
                    src[i] = (ca[i] * wa + cb[i] * wb + cc[i] * wc) * texel[i];
                }
                let dst = &mut self.pixels[y * self.width + x];
                let keep = 1.0 - src[3];
                for i in 0..4 {
                    dst[i] = src[i] + dst[i] * keep;
                }
            }
        }
    }

    /// The canvas as straight (not premultiplied) 8-bit RGBA, for a PNG.
    pub fn to_rgba(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.pixels.len() * 4);
        for &[r, g, b, a] in &self.pixels {
            let a = a.clamp(0.0, 1.0);
            let straight = |c: f32| {
                let c = if a > 0.0 { c / a } else { 0.0 };
                (c.clamp(0.0, 1.0) * 255.0).round() as u8
            };
            out.extend([
                straight(r),
                straight(g),
                straight(b),
                (a * 255.0).round() as u8,
            ]);
        }
        out
    }
}

fn edge(a: egui::Pos2, b: egui::Pos2, p: egui::Pos2) -> f32 {
    (b.x - a.x) * (p.y - a.y) - (b.y - a.y) * (p.x - a.x)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn white() -> ColorImage {
        ColorImage::new([1, 1], vec![Color32::WHITE])
    }

    #[test]
    fn a_rectangle_fills_its_pixels_and_blends_over_the_canvas() {
        let mut canvas = Canvas::new(4, 4, Color32::BLACK);
        let mut mesh = Mesh::default();
        mesh.add_colored_rect(
            Rect::from_min_max(egui::pos2(1.0, 1.0), egui::pos2(3.0, 3.0)),
            Color32::from_rgba_premultiplied(128, 0, 0, 128),
        );
        let white = white();
        canvas.draw(
            &[ClippedPrimitive {
                clip_rect: Rect::EVERYTHING,
                primitive: Primitive::Mesh(mesh),
            }],
            |_| Some(Texture { image: &white }),
        );
        let rgba = canvas.to_rgba();
        let at = |x: usize, y: usize| &rgba[(y * 4 + x) * 4..(y * 4 + x) * 4 + 4];
        assert_eq!(at(0, 0), &[0, 0, 0, 255]);
        // Half red over black: 128 red, opaque.
        assert_eq!(at(1, 1), &[128, 0, 0, 255]);
        assert_eq!(at(2, 2), &[128, 0, 0, 255]);
        assert_eq!(at(3, 3), &[0, 0, 0, 255]);
    }

    #[test]
    fn a_clip_rect_keeps_a_mesh_inside_it() {
        let mut canvas = Canvas::new(4, 1, Color32::BLACK);
        let mut mesh = Mesh::default();
        mesh.add_colored_rect(
            Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(4.0, 1.0)),
            Color32::WHITE,
        );
        let white = white();
        canvas.draw(
            &[ClippedPrimitive {
                clip_rect: Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(2.0, 1.0)),
                primitive: Primitive::Mesh(mesh),
            }],
            |_| Some(Texture { image: &white }),
        );
        let rgba = canvas.to_rgba();
        assert_eq!(&rgba[0..4], &[255, 255, 255, 255]);
        assert_eq!(&rgba[12..16], &[0, 0, 0, 255]);
    }
}
