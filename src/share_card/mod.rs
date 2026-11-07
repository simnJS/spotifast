//! The image the Stats page shares: a range's top five artists and top
//! five tracks with their covers, the range's totals and the Spotifast mark,
//! on a dark gradient tinted by the top artist, as a 1080 by 1350 PNG (the
//! 4:5 portrait social feeds show whole).
//!
//! The card is laid out with egui's own fonts and tessellated by epaint as
//! the window is, then filled by the small software renderer in [`raster`].
//! Nothing here needs a GPU, a window or the UI thread: [`render`] is a
//! pure function of a [`Card`], so the page runs it on a worker thread.

mod raster;

use std::sync::Arc;

use egui::epaint::{
    ClippedShape, ColorImage, CornerRadius, RectShape, Shape, TessellationOptions, Tessellator,
    TextureId,
};
use egui::text::{LayoutJob, TextFormat};
use egui::{Align, Color32, FontId, Galley, Pos2, Rect, Vec2, pos2, vec2};

pub use raster::Canvas;

/// The card's size in pixels.
pub const WIDTH: usize = 1080;
pub const HEIGHT: usize = 1350;

/// How many artists and tracks the card shows.
pub const TOP: usize = 5;

/// The left and right margin.
const MARGIN: f32 = 72.0;

/// Spotify's dark background (`--background-base`).
const BASE: Color32 = Color32::from_rgb(0x12, 0x12, 0x12);
/// Spotify's subdued text (`--text-subdued`).
const SUBDUED: Color32 = Color32::from_rgb(0xb3, 0xb3, 0xb3);
/// The fill an image shows while it has none (`--background-tinted-base`).
const PLACEHOLDER: Color32 = Color32::from_rgb(0x28, 0x28, 0x28);
/// Spotify's bright accent.
const GREEN: Color32 = Color32::from_rgb(0x1e, 0xd7, 0x60);

/// An image to paint, as straight RGBA.
#[derive(Clone, Debug, PartialEq)]
pub struct Cover {
    pub size: [usize; 2],
    pub rgba: Vec<u8>,
}

impl Cover {
    /// Decodes an image file (JPEG, PNG or WebP, as covers come), scaled
    /// down to at most `side` pixels on its longer side.
    pub fn decode(bytes: &[u8], side: u32) -> Option<Self> {
        let image = image::load_from_memory(bytes).ok()?;
        let image = if image.width() > side || image.height() > side {
            image.resize(side, side, image::imageops::FilterType::Triangle)
        } else {
            image
        };
        let rgba = image.to_rgba8();
        Some(Self {
            size: [rgba.width() as usize, rgba.height() as usize],
            rgba: rgba.into_raw(),
        })
    }

    fn image(&self) -> ColorImage {
        ColorImage::from_rgba_unmultiplied(self.size, &self.rgba)
    }
}

/// One artist or track on the card.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Entry {
    pub title: String,
    /// The artists of a track; empty for an artist.
    pub subtitle: String,
    /// What the card says it was played: "42 streams".
    pub figure: String,
    pub cover: Option<Cover>,
}

/// Everything the card shows, already in the person's words.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Card {
    /// "Your top" or the account's name.
    pub title: String,
    /// The range: "Last 4 weeks".
    pub range: String,
    /// The totals, as (value, label): ("12,345", "minutes").
    pub figures: Vec<(String, String)>,
    pub artists_title: String,
    pub artists: Vec<Entry>,
    pub tracks_title: String,
    pub tracks: Vec<Entry>,
    /// The top artist's colour, which tints the gradient.
    pub accent: Option<[u8; 3]>,
    /// The line at the bottom: "Made with Spotifast".
    pub footer: String,
}

/// What the card's shapes need from the fonts.
struct Layout<'a> {
    fonts: egui::epaint::text::FontsView<'a>,
    shapes: Vec<Shape>,
}

impl Layout<'_> {
    fn galley(&mut self, text: &str, font: FontId, color: Color32, max_width: f32) -> Arc<Galley> {
        let mut job = LayoutJob::single_section(text.to_owned(), TextFormat::simple(font, color));
        job.wrap.max_width = max_width.max(1.0);
        job.wrap.max_rows = 1;
        job.wrap.break_anywhere = true;
        job.wrap.overflow_character = Some('…');
        self.fonts.layout_job(job)
    }

    /// Lays out one line of text from `at`, aligned on it horizontally as
    /// `align` says, its top at `at.y`. Answers the rect it covers.
    fn text(
        &mut self,
        text: &str,
        font: FontId,
        color: Color32,
        at: Pos2,
        max_width: f32,
        align: Align,
    ) -> Rect {
        let galley = self.galley(text, font, color, max_width);
        let width = galley.size().x;
        let left = match align {
            Align::Min => at.x,
            Align::Center => at.x - width / 2.0,
            Align::Max => at.x - width,
        };
        let rect = Rect::from_min_size(pos2(left, at.y), galley.size());
        self.shapes.push(Shape::galley(rect.min, galley, color));
        rect
    }

    fn rect(&mut self, rect: Rect, radius: f32, fill: Color32) {
        self.shapes.push(Shape::Rect(RectShape::filled(
            rect,
            CornerRadius::same(radius.min(255.0) as u8),
            fill,
        )));
    }

    /// Paints texture `texture` (an index into the card's covers) over
    /// `rect`, cropped to a square at its centre, with rounded corners.
    fn image(&mut self, rect: Rect, radius: f32, texture: Option<(u64, [usize; 2])>) {
        let Some((id, [w, h])) = texture else {
            self.rect(rect, radius, PLACEHOLDER);
            return;
        };
        // Covers are square; an artist's photo may not be. Show its middle.
        let (w, h) = (w.max(1) as f32, h.max(1) as f32);
        let uv = if w > h {
            let inset = (1.0 - h / w) / 2.0;
            Rect::from_min_max(pos2(inset, 0.0), pos2(1.0 - inset, 1.0))
        } else {
            let inset = (1.0 - w / h) / 2.0;
            Rect::from_min_max(pos2(0.0, inset), pos2(1.0, 1.0 - inset))
        };
        self.shapes.push(Shape::Rect(
            RectShape::filled(
                rect,
                CornerRadius::same(radius.min(255.0) as u8),
                Color32::WHITE,
            )
            .with_texture(TextureId::User(id), uv),
        ));
    }

    /// A vertical gradient over `rect`.
    fn gradient(&mut self, rect: Rect, top: Color32, bottom: Color32) {
        let mut mesh = egui::Mesh::default();
        mesh.colored_vertex(rect.left_top(), top);
        mesh.colored_vertex(rect.right_top(), top);
        mesh.colored_vertex(rect.right_bottom(), bottom);
        mesh.colored_vertex(rect.left_bottom(), bottom);
        mesh.add_triangle(0, 1, 2);
        mesh.add_triangle(0, 2, 3);
        self.shapes.push(Shape::mesh(mesh));
    }
}

/// Mixes `amount` of `over` into `base`, both opaque.
fn mix(base: Color32, over: Color32, amount: f32) -> Color32 {
    let lerp = |a: u8, b: u8| (f32::from(a) + (f32::from(b) - f32::from(a)) * amount).round() as u8;
    Color32::from_rgb(
        lerp(base.r(), over.r()),
        lerp(base.g(), over.g()),
        lerp(base.b(), over.b()),
    )
}

/// The top of the gradient: the artist's colour, darkened enough that white
/// text stays readable over even a pale one.
fn wash(accent: Option<[u8; 3]>) -> Color32 {
    let accent = accent.map_or(Color32::from_rgb(0x50, 0x38, 0xc8), |[r, g, b]| {
        Color32::from_rgb(r, g, b)
    });
    // Rec. 709 luma: a brighter colour is mixed in less.
    let luma = (0.2126 * f32::from(accent.r())
        + 0.7152 * f32::from(accent.g())
        + 0.0722 * f32::from(accent.b()))
        / 255.0;
    mix(BASE, accent, 0.75 - 0.35 * luma)
}

fn fonts() -> egui::epaint::text::Fonts {
    let emoji = egui::FontData::from_static(include_bytes!("../../assets/fonts/NotoEmoji.ttf"));
    let definitions = fastframe_fonts::FontSetup::default()
        .companion("noto_emoji", Arc::new(emoji))
        .definitions();
    let options = egui::epaint::text::TextOptions {
        max_texture_side: 8192,
        ..Default::default()
    };
    egui::epaint::text::Fonts::new(options, definitions)
}

/// The card's shapes, with the covers they paint as `TextureId::User(n)`
/// for the `n`th of `covers`.
fn shapes(card: &Card, fonts: &mut egui::epaint::text::Fonts) -> (Vec<Shape>, Vec<ColorImage>) {
    let mut covers: Vec<ColorImage> = Vec::new();
    let mut texture = |cover: &Option<Cover>| {
        cover.as_ref().map(|cover| {
            covers.push(cover.image());
            ((covers.len() - 1) as u64, cover.size)
        })
    };
    let artists: Vec<_> = card
        .artists
        .iter()
        .take(TOP)
        .map(|a| texture(&a.cover))
        .collect();
    let tracks: Vec<_> = card
        .tracks
        .iter()
        .take(TOP)
        .map(|t| texture(&t.cover))
        .collect();

    let mut layout = Layout {
        fonts: fonts.with_pixels_per_point(1.0),
        shapes: Vec::new(),
    };
    let full = Rect::from_min_size(Pos2::ZERO, vec2(WIDTH as f32, HEIGHT as f32));
    let width = full.width() - 2.0 * MARGIN;
    let right = full.right() - MARGIN;

    // The gradient: the artist's colour fading into Spotify's base by the
    // middle, then a little darker to the bottom.
    let top = wash(card.accent);
    layout.gradient(
        Rect::from_min_max(full.min, pos2(full.right(), 760.0)),
        top,
        BASE,
    );
    layout.gradient(
        Rect::from_min_max(pos2(full.left(), 760.0), full.max),
        BASE,
        Color32::from_rgb(0x0a, 0x0a, 0x0a),
    );

    // The mark and the range.
    let mark = 48.0;
    let logo = Rect::from_min_size(pos2(MARGIN, 64.0), Vec2::splat(mark));
    let logo_image = ColorImage::from_rgba_unmultiplied(
        [mark as usize, mark as usize],
        &crate::util::app_icon_rgba(mark as usize),
    );
    covers.push(logo_image);
    let logo_id = (covers.len() - 1) as u64;
    layout.shapes.push(Shape::image(
        TextureId::User(logo_id),
        logo,
        Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0)),
        Color32::WHITE,
    ));
    layout.text(
        "Spotifast",
        crate::theme::bold(30.0),
        Color32::WHITE,
        pos2(logo.right() + 14.0, logo.center().y - 18.0),
        300.0,
        Align::Min,
    );
    let pill_font = crate::theme::semibold(24.0);
    let pill_text = layout.galley(&card.range, pill_font.clone(), Color32::WHITE, 480.0);
    let pill = Rect::from_min_max(
        pos2(right - pill_text.size().x - 40.0, logo.center().y - 24.0),
        pos2(right, logo.center().y + 24.0),
    );
    layout.rect(pill, 24.0, Color32::from_white_alpha(36));
    layout.shapes.push(Shape::galley(
        pos2(
            pill.left() + 20.0,
            pill.center().y - pill_text.size().y / 2.0,
        ),
        pill_text,
        Color32::WHITE,
    ));

    // The title and the totals.
    layout.text(
        &card.title,
        crate::theme::bold(84.0),
        Color32::WHITE,
        pos2(MARGIN, 150.0),
        width,
        Align::Min,
    );
    let figures = card.figures.len().max(1);
    let column = width / figures as f32;
    for (index, (value, label)) in card.figures.iter().enumerate() {
        let left = MARGIN + column * index as f32;
        let value = layout.text(
            value,
            crate::theme::bold(54.0),
            Color32::WHITE,
            pos2(left, 286.0),
            column - 16.0,
            Align::Min,
        );
        layout.text(
            label,
            crate::theme::regular(24.0),
            SUBDUED,
            pos2(left, value.bottom() + 2.0),
            column - 16.0,
            Align::Min,
        );
    }

    // Top artists: round photos across the card, a rank badge on each.
    layout.text(
        &card.artists_title,
        crate::theme::bold(38.0),
        Color32::WHITE,
        pos2(MARGIN, 442.0),
        width,
        Align::Min,
    );
    let photo = 160.0;
    let gap = (width - photo * TOP as f32) / (TOP as f32 - 1.0);
    for (index, (artist, texture)) in card.artists.iter().zip(&artists).enumerate() {
        let left = MARGIN + (photo + gap) * index as f32;
        let rect = Rect::from_min_size(pos2(left, 508.0), Vec2::splat(photo));
        layout.image(rect, photo / 2.0, *texture);
        let badge =
            Rect::from_center_size(rect.left_bottom() + vec2(22.0, -22.0), Vec2::splat(44.0));
        layout.rect(badge, 22.0, if index == 0 { GREEN } else { Color32::WHITE });
        layout.text(
            &(index + 1).to_string(),
            crate::theme::bold(24.0),
            Color32::BLACK,
            pos2(badge.center().x, badge.center().y - 15.0),
            44.0,
            Align::Center,
        );
        let name = layout.text(
            &artist.title,
            crate::theme::semibold(25.0),
            Color32::WHITE,
            pos2(rect.center().x, rect.bottom() + 16.0),
            photo + 16.0,
            Align::Center,
        );
        layout.text(
            &artist.figure,
            crate::theme::regular(21.0),
            SUBDUED,
            pos2(rect.center().x, name.bottom() + 2.0),
            photo + 16.0,
            Align::Center,
        );
    }

    // Top tracks: rows as Spotify's track list draws them.
    layout.text(
        &card.tracks_title,
        crate::theme::bold(38.0),
        Color32::WHITE,
        pos2(MARGIN, 776.0),
        width,
        Align::Min,
    );
    let row = 86.0;
    let art = 68.0;
    for (index, (track, texture)) in card.tracks.iter().zip(&tracks).enumerate() {
        let top = 840.0 + row * index as f32;
        let middle = top + art / 2.0;
        layout.text(
            &(index + 1).to_string(),
            crate::theme::bold(32.0),
            if index == 0 { GREEN } else { Color32::WHITE },
            pos2(MARGIN + 18.0, middle - 21.0),
            48.0,
            Align::Center,
        );
        let cover = Rect::from_min_size(pos2(MARGIN + 52.0, top), Vec2::splat(art));
        layout.image(cover, 6.0, *texture);
        let figure = layout.galley(&track.figure, crate::theme::regular(22.0), SUBDUED, 260.0);
        let figure_width = figure.size().x;
        layout.shapes.push(Shape::galley(
            pos2(right - figure_width, middle - figure.size().y / 2.0),
            figure,
            SUBDUED,
        ));
        let text_left = cover.right() + 22.0;
        let room = right - figure_width - 24.0 - text_left;
        let title = layout.text(
            &track.title,
            crate::theme::semibold(28.0),
            Color32::WHITE,
            pos2(text_left, top + 2.0),
            room,
            Align::Min,
        );
        layout.text(
            &track.subtitle,
            crate::theme::regular(22.0),
            SUBDUED,
            pos2(text_left, title.bottom() + 2.0),
            room,
            Align::Min,
        );
    }

    // The footer.
    layout.rect(
        Rect::from_min_size(pos2(MARGIN, 1268.0), vec2(width, 1.0)),
        0.0,
        Color32::from_white_alpha(28),
    );
    layout.text(
        &card.footer,
        crate::theme::regular(22.0),
        SUBDUED,
        pos2(MARGIN, 1288.0),
        width,
        Align::Min,
    );
    (layout.shapes, covers)
}

/// The card's pixels, as straight RGBA, `WIDTH` by `HEIGHT`.
pub fn render(card: &Card) -> ColorImage {
    let mut fonts = fonts();
    let (shapes, covers) = shapes(card, &mut fonts);
    let atlas = fonts.image();
    let mut tessellator = Tessellator::new(
        1.0,
        TessellationOptions::default(),
        fonts.font_image_size(),
        fonts.texture_atlas().prepared_discs(),
    );
    let full = Rect::from_min_size(Pos2::ZERO, vec2(WIDTH as f32, HEIGHT as f32));
    let primitives = tessellator.tessellate_shapes(
        shapes
            .into_iter()
            .map(|shape| ClippedShape {
                clip_rect: full,
                shape,
            })
            .collect(),
    );
    let mut canvas = Canvas::new(WIDTH, HEIGHT, BASE);
    canvas.draw(&primitives, |id| match id {
        TextureId::Managed(0) => Some(raster::Texture { image: &atlas }),
        TextureId::User(index) => covers
            .get(index as usize)
            .map(|image| raster::Texture { image }),
        TextureId::Managed(_) => None,
    });
    ColorImage::from_rgba_unmultiplied([WIDTH, HEIGHT], &canvas.to_rgba())
}

/// The card as a PNG file's bytes.
pub fn png(image: &ColorImage) -> Result<Vec<u8>, String> {
    let [width, height] = image.size;
    let rgba: Vec<u8> = image
        .pixels
        .iter()
        .flat_map(|pixel| pixel.to_srgba_unmultiplied())
        .collect();
    let mut out = std::io::Cursor::new(Vec::new());
    image::write_buffer_with_format(
        &mut out,
        &rgba,
        width as u32,
        height as u32,
        image::ExtendedColorType::Rgba8,
        image::ImageFormat::Png,
    )
    .map_err(|error| error.to_string())?;
    Ok(out.into_inner())
}

/// A file name for the card: `spotifast-top-last-4-weeks.png`.
pub fn file_name(range: &str) -> String {
    let mut slug = String::new();
    for character in range.chars() {
        if character.is_alphanumeric() {
            slug.extend(character.to_lowercase());
        } else if !slug.ends_with('-') && !slug.is_empty() {
            slug.push('-');
        }
    }
    let slug = slug.trim_end_matches('-');
    if slug.is_empty() {
        "spotifast-top.png".to_owned()
    } else {
        format!("spotifast-top-{slug}.png")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(color: [u8; 3]) -> Cover {
        Cover {
            size: [8, 8],
            rgba: [color[0], color[1], color[2], 255].repeat(64),
        }
    }

    fn card() -> Card {
        let entry = |index: usize, kind: &str| Entry {
            title: format!("{kind} {index}"),
            subtitle: format!("Artist {index}"),
            figure: format!("{} streams", 50 - index),
            cover: Some(solid([200, 30 * index as u8, 40])),
        };
        Card {
            title: "Your top".into(),
            range: "Last 4 weeks".into(),
            figures: vec![
                ("12,345".into(), "minutes".into()),
                ("3,210".into(), "streams".into()),
                ("120".into(), "artists".into()),
                ("640".into(), "tracks".into()),
            ],
            artists_title: "Top artists".into(),
            artists: (0..5).map(|index| entry(index, "Artist")).collect(),
            tracks_title: "Top tracks".into(),
            tracks: (0..5).map(|index| entry(index, "Track")).collect(),
            accent: Some([220, 40, 60]),
            footer: "Made with Spotifast".into(),
        }
    }

    fn pixel(image: &ColorImage, x: usize, y: usize) -> [u8; 4] {
        image.pixels[y * image.size[0] + x].to_srgba_unmultiplied()
    }

    #[test]
    fn the_card_is_portrait_opaque_and_tinted_at_the_top() {
        let image = render(&card());
        assert_eq!(image.size, [WIDTH, HEIGHT]);
        assert!(image.pixels.iter().all(|pixel| pixel.a() == 255));
        // The top corner carries the artist's red; the bottom is near black.
        let [r, g, b, _] = pixel(&image, 4, 4);
        assert!(r > g + 40 && r > b + 30, "{r} {g} {b}");
        let [r, g, b, _] = pixel(&image, 4, HEIGHT - 4);
        assert!(r < 20 && g < 20 && b < 20, "{r} {g} {b}");
    }

    #[test]
    fn covers_and_text_are_painted_where_the_layout_puts_them() {
        let image = render(&card());
        // The middle of the first artist's photo is its cover's colour.
        let [r, g, b, _] = pixel(&image, (MARGIN + 80.0) as usize, 588);
        assert_eq!((r, g, b), (200, 0, 40));
        // The middle of the second track's cover.
        let [r, g, b, _] = pixel(
            &image,
            (MARGIN + 86.0) as usize,
            (840.0 + 86.0 + 34.0) as usize,
        );
        assert_eq!((r, g, b), (200, 30, 40));
        // The title row holds white glyph pixels.
        let bright = (150..250)
            .flat_map(|y| (72..500).map(move |x| (x, y)))
            .filter(|&(x, y)| pixel(&image, x, y)[1] > 230)
            .count();
        assert!(bright > 500, "{bright}");
    }

    #[test]
    fn a_card_without_covers_or_entries_still_renders() {
        let mut card = card();
        card.artists.truncate(2);
        for entry in card.artists.iter_mut().chain(&mut card.tracks) {
            entry.cover = None;
        }
        card.tracks.clear();
        card.accent = None;
        let image = render(&card);
        // The placeholder fills the missing photo.
        let [r, g, b, _] = pixel(&image, (MARGIN + 80.0) as usize, 588);
        assert_eq!((r, g, b), (0x28, 0x28, 0x28));
        let png = png(&image).unwrap();
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
        let decoded = image::load_from_memory(&png).unwrap();
        assert_eq!(
            (decoded.width(), decoded.height()),
            (WIDTH as u32, HEIGHT as u32)
        );
    }

    #[test]
    fn covers_decode_and_shrink() {
        let mut png = std::io::Cursor::new(Vec::new());
        image::RgbaImage::from_pixel(600, 300, image::Rgba([1, 2, 3, 255]))
            .write_to(&mut png, image::ImageFormat::Png)
            .unwrap();
        let cover = Cover::decode(png.get_ref(), 256).unwrap();
        assert_eq!(cover.size, [256, 128]);
        assert_eq!(&cover.rgba[..4], &[1, 2, 3, 255]);
        assert!(Cover::decode(b"not an image", 256).is_none());
    }

    #[test]
    fn file_names_follow_the_range() {
        assert_eq!(file_name("Last 4 weeks"), "spotifast-top-last-4-weeks.png");
        assert_eq!(file_name("All time"), "spotifast-top-all-time.png");
        assert_eq!(file_name(" "), "spotifast-top.png");
    }

    #[test]
    fn a_pale_artist_colour_is_darkened_more_than_a_deep_one() {
        let pale = wash(Some([250, 250, 200]));
        let deep = wash(Some([40, 20, 120]));
        let luma = |c: Color32| u32::from(c.r()) + u32::from(c.g()) + u32::from(c.b());
        assert!(luma(pale) < 3 * 190, "{pale:?}");
        assert!(luma(deep) > luma(BASE));
    }
}
