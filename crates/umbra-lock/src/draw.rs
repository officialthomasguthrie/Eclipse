//! What the lock screen draws, in software, into a `wl_shm` buffer: the gray ground, the owner's
//! name, the password field with a dot for each character typed, and the sentence under it.
//!
//! Positions and colour channels are worked out as floats and clamped to the buffer before they
//! become integers again, which is what the casts below rely on.
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss
)]

use ab_glyph::{Font, FontVec, GlyphId, PxScale, ScaleFont, VariableFont, point};

use crate::entry::Status;

/// A colour: red, green, blue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgb(pub u8, pub u8, pub u8);

/// The ground, the dark gray of the boot screen.
pub const BACKGROUND: Rgb = Rgb(0x1e, 0x1e, 0x1e);
/// The inside of the field, the gray of corona's field.
pub const FIELD: Rgb = Rgb(0x2e, 0x2e, 0x2e);
/// The owner's name and the dots.
pub const TEXT: Rgb = Rgb(0xe6, 0xe6, 0xe6);
/// The placeholder, and the sentence while PAM checks.
pub const DIM: Rgb = Rgb(0x8c, 0x8c, 0x8c);
/// The focus ring around the field, the one accent on the screen.
pub const ACCENT: Rgb = Rgb(0x78, 0xae, 0xed);
/// The sentence for a password that was refused.
pub const ERROR: Rgb = Rgb(0xe0, 0x6d, 0x6d);

// sizes in logical pixels, like corona's panel
const TEXT_SIZE: f32 = 14.0;
const FIELD_WIDTH: f32 = 280.0;
const FIELD_HEIGHT: f32 = 32.0;
const RADIUS: f32 = 4.0;
const RING: f32 = 2.0;
const PADDING: f32 = 10.0;
const GAP: f32 = 12.0;
const LINE: f32 = 20.0;
// where a line's baseline sits under the top of its row
const BASELINE: f32 = 15.0;
const DOT: f32 = 3.0;
const DOT_STEP: f32 = 12.0;

/// A rectangle in buffer pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    /// Left edge.
    pub x: f32,
    /// Top edge.
    pub y: f32,
    /// Width.
    pub width: f32,
    /// Height.
    pub height: f32,
}

/// Where the parts go on one output.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Layout {
    /// The field, in the middle of the output.
    pub field: Rect,
    /// The baseline of the owner's name, above the field.
    pub name: f32,
    /// The baseline of the sentence, under the field.
    pub sentence: f32,
}

/// The layout for a buffer of `width` by `height` pixels at an integer `scale`.
#[must_use]
pub fn layout(width: u32, height: u32, scale: u32) -> Layout {
    let scale = scale.max(1) as f32;
    let (field_width, field_height) = (FIELD_WIDTH * scale, FIELD_HEIGHT * scale);
    let x = ((width as f32 - field_width) / 2.0).round();
    let y = ((height as f32 - field_height) / 2.0).round();
    Layout {
        field: Rect {
            x,
            y,
            width: field_width,
            height: field_height,
        },
        name: y - (GAP + LINE - BASELINE) * scale,
        sentence: y + field_height + (GAP + BASELINE) * scale,
    }
}

/// Noto Sans, regular and bold. Either can be missing: the screen still locks and draws the
/// field, only without text.
#[derive(Default)]
pub struct Fonts {
    regular: Option<FontVec>,
    bold: Option<FontVec>,
}

impl Fonts {
    /// Noto Sans from the fonts installed on the system.
    #[must_use]
    pub fn load() -> Self {
        let mut database = fontdb::Database::new();
        database.load_system_fonts();
        Self::from_database(&database)
    }

    /// Noto Sans from the fonts in `database`. The image has it as a variable font, which gives
    /// the bold face through its weight axis.
    #[must_use]
    pub fn from_database(database: &fontdb::Database) -> Self {
        let face = |weight: fontdb::Weight| {
            database.query(&fontdb::Query {
                families: &[fontdb::Family::Name("Noto Sans")],
                weight,
                stretch: fontdb::Stretch::Normal,
                style: fontdb::Style::Normal,
            })
        };
        let open = |id: fontdb::ID| {
            database
                .with_face_data(id, |data, index| {
                    FontVec::try_from_vec_and_index(data.to_vec(), index).ok()
                })
                .flatten()
        };
        let regular_id = face(fontdb::Weight::NORMAL);
        let bold_id = face(fontdb::Weight::BOLD);
        let bold = bold_id.and_then(open).map(|mut bold| {
            if bold_id == regular_id {
                bold.set_variation(b"wght", 700.0);
            }
            bold
        });
        Self {
            regular: regular_id.and_then(open),
            bold,
        }
    }
}

/// A `wl_shm` buffer in the `Argb8888` format: four bytes a pixel, blue, green, red, alpha.
pub struct Canvas<'a> {
    pixels: &'a mut [u8],
    width: usize,
    height: usize,
}

impl<'a> Canvas<'a> {
    /// A canvas over `pixels`, `width` pixels a row. Rows the slice has no room for are left out.
    #[must_use]
    pub fn new(pixels: &'a mut [u8], width: u32, height: u32) -> Self {
        let width = width as usize;
        let height = (height as usize).min(pixels.len() / (width * 4).max(1));
        Self {
            pixels,
            width,
            height,
        }
    }

    /// The colour at `x`, `y`, or nothing outside the canvas.
    #[must_use]
    pub fn pixel(&self, x: usize, y: usize) -> Option<Rgb> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let i = (y * self.width + x) * 4;
        Some(Rgb(self.pixels[i + 2], self.pixels[i + 1], self.pixels[i]))
    }

    fn fill(&mut self, color: Rgb) {
        for pixel in self
            .pixels
            .chunks_exact_mut(4)
            .take(self.width * self.height)
        {
            pixel.copy_from_slice(&[color.2, color.1, color.0, 0xff]);
        }
    }

    fn blend(&mut self, x: i64, y: i64, color: Rgb, coverage: f32) {
        if coverage <= 0.0 || x < 0 || y < 0 || x >= self.width as i64 || y >= self.height as i64 {
            return;
        }
        let i = (y as usize * self.width + x as usize) * 4;
        let coverage = coverage.min(1.0);
        for (offset, channel) in [(0, color.2), (1, color.1), (2, color.0)] {
            let old = f32::from(self.pixels[i + offset]);
            self.pixels[i + offset] = (old + (f32::from(channel) - old) * coverage).round() as u8;
        }
        self.pixels[i + 3] = 0xff;
    }

    /// A rounded rectangle filled with `fill`, with a ring of `edge` `ring` pixels wide inside
    /// its outline.
    fn field(&mut self, rect: Rect, radius: f32, fill: Rgb, edge: Rgb, ring: f32) {
        let (half_width, half_height) = (rect.width / 2.0, rect.height / 2.0);
        let (cx, cy) = (rect.x + half_width, rect.y + half_height);
        for y in rect.y.floor() as i64..(rect.y + rect.height).ceil() as i64 {
            for x in rect.x.floor() as i64..(rect.x + rect.width).ceil() as i64 {
                // how far the pixel's centre is outside the rounded outline, negative inside
                let qx = (x as f32 + 0.5 - cx).abs() - half_width + radius;
                let qy = (y as f32 + 0.5 - cy).abs() - half_height + radius;
                let outside = qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - radius;
                self.blend(x, y, edge, (0.5 - outside).clamp(0.0, 1.0));
                self.blend(x, y, fill, (0.5 - outside - ring).clamp(0.0, 1.0));
            }
        }
    }

    fn dot(&mut self, cx: f32, cy: f32, radius: f32, color: Rgb) {
        for y in (cy - radius - 1.0).floor() as i64..=(cy + radius + 1.0).ceil() as i64 {
            for x in (cx - radius - 1.0).floor() as i64..=(cx + radius + 1.0).ceil() as i64 {
                let outside = (x as f32 + 0.5 - cx).hypot(y as f32 + 0.5 - cy) - radius;
                self.blend(x, y, color, (0.5 - outside).clamp(0.0, 1.0));
            }
        }
    }

    fn text(&mut self, font: &FontVec, size: f32, x: f32, baseline: f32, color: Rgb, text: &str) {
        let scale = px_scale(font, size);
        let scaled = font.as_scaled(scale);
        let mut caret = x;
        let mut previous: Option<GlyphId> = None;
        for c in text.chars() {
            let id = font.glyph_id(c);
            if let Some(previous) = previous {
                caret += scaled.kern(previous, id);
            }
            let glyph = id.with_scale_and_position(scale, point(caret, baseline));
            caret += scaled.h_advance(id);
            previous = Some(id);
            if let Some(outline) = font.outline_glyph(glyph) {
                let bounds = outline.px_bounds();
                outline.draw(|gx, gy, coverage| {
                    let (x, y) = (
                        bounds.min.x as i64 + i64::from(gx),
                        bounds.min.y as i64 + i64::from(gy),
                    );
                    self.blend(x, y, color, coverage);
                });
            }
        }
    }
}

/// `ab_glyph` scales by the height from descender to ascender, the size here is the em, as in
/// iced and CSS.
fn px_scale(font: &FontVec, size: f32) -> PxScale {
    let em = font.units_per_em().unwrap_or(1000.0);
    PxScale::from(size * font.height_unscaled() / em)
}

fn text_width(font: &FontVec, size: f32, text: &str) -> f32 {
    let scaled = font.as_scaled(px_scale(font, size));
    let mut width = 0.0;
    let mut previous: Option<GlyphId> = None;
    for c in text.chars() {
        let id = font.glyph_id(c);
        if let Some(previous) = previous {
            width += scaled.kern(previous, id);
        }
        width += scaled.h_advance(id);
        previous = Some(id);
    }
    width
}

/// What there is to draw.
#[derive(Debug, Clone, Copy)]
pub struct View<'a> {
    /// The owner's name, above the field.
    pub name: &'a str,
    /// How many characters are in the field.
    pub typed: usize,
    /// What the line under the field says.
    pub status: Status,
}

/// Draws the lock screen for one output onto `canvas` at an integer `scale`.
pub fn paint(canvas: &mut Canvas<'_>, fonts: &Fonts, view: &View<'_>, scale: u32) {
    canvas.fill(BACKGROUND);
    let layout = layout(canvas.width as u32, canvas.height as u32, scale);
    let scale = scale.max(1) as f32;
    let size = TEXT_SIZE * scale;
    let center = canvas.width as f32 / 2.0;

    if let Some(bold) = &fonts.bold {
        let x = (center - text_width(bold, size, view.name) / 2.0).round();
        canvas.text(bold, size, x, layout.name, TEXT, view.name);
    }

    let field = layout.field;
    canvas.field(field, RADIUS * scale, FIELD, ACCENT, RING * scale);
    let middle = field.y + field.height / 2.0;
    if view.typed == 0 {
        if let Some(regular) = &fonts.regular {
            let baseline = middle + 5.0 * scale;
            canvas.text(
                regular,
                size,
                field.x + PADDING * scale,
                baseline,
                DIM,
                "Password",
            );
        }
    } else {
        let room = ((field.width - 2.0 * PADDING * scale) / (DOT_STEP * scale)) as usize;
        for i in 0..view.typed.min(room) {
            let cx = field.x + (PADDING + DOT + i as f32 * DOT_STEP) * scale;
            canvas.dot(cx, middle, DOT * scale, TEXT);
        }
    }

    if let (Some(sentence), Some(regular)) = (view.status.sentence(), &fonts.regular) {
        let color = if view.status.is_error() { ERROR } else { DIM };
        let x = (center - text_width(regular, size, sentence) / 2.0).round();
        canvas.text(regular, size, x, layout.sentence, color, sentence);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn painted(width: u32, height: u32, typed: usize, scale: u32) -> Vec<u8> {
        let mut pixels = vec![0; width as usize * height as usize * 4];
        let view = View {
            name: "Eclipse owner",
            typed,
            status: Status::Typing,
        };
        paint(
            &mut Canvas::new(&mut pixels, width, height),
            &Fonts::default(),
            &view,
            scale,
        );
        pixels
    }

    fn count(pixels: &mut [u8], width: u32, height: u32, color: Rgb) -> usize {
        let canvas = Canvas::new(pixels, width, height);
        (0..height as usize)
            .flat_map(|y| (0..width as usize).map(move |x| (x, y)))
            .filter(|&(x, y)| canvas.pixel(x, y) == Some(color))
            .count()
    }

    #[test]
    fn the_field_sits_in_the_middle() {
        let layout = layout(1280, 800, 1);
        assert_eq!(
            layout.field,
            Rect {
                x: 500.0,
                y: 384.0,
                width: 280.0,
                height: 32.0
            }
        );
        assert!(layout.name < layout.field.y);
        assert!(layout.sentence > layout.field.y + layout.field.height);
    }

    #[test]
    fn scale_doubles_the_sizes() {
        let layout = layout(2560, 1600, 2);
        assert_eq!(
            layout.field,
            Rect {
                x: 1000.0,
                y: 768.0,
                width: 560.0,
                height: 64.0
            }
        );
    }

    #[test]
    fn the_ground_the_field_and_its_ring() {
        let mut pixels = painted(1280, 800, 0, 1);
        let canvas = Canvas::new(&mut pixels, 1280, 800);
        assert_eq!(canvas.pixel(0, 0), Some(BACKGROUND));
        assert_eq!(canvas.pixel(1279, 799), Some(BACKGROUND));
        assert_eq!(canvas.pixel(640, 400), Some(FIELD));
        assert_eq!(canvas.pixel(640, 384), Some(ACCENT));
        assert_eq!(canvas.pixel(640, 415), Some(ACCENT));
        assert_eq!(canvas.pixel(640, 383), Some(BACKGROUND));
        // the inside of the field minus the ring, and the ring along its straight edges
        let field = count(&mut pixels, 1280, 800, FIELD);
        assert!(
            (276 * 28 - 100..=276 * 28).contains(&field),
            "{field} field pixels"
        );
        let ring = count(&mut pixels, 1280, 800, ACCENT);
        assert!((1000..=1250).contains(&ring), "{ring} ring pixels");
    }

    #[test]
    fn a_dot_for_each_character_until_the_field_is_full() {
        let dots = |typed| {
            let mut pixels = painted(1280, 800, typed, 1);
            count(&mut pixels, 1280, 800, TEXT)
        };
        assert_eq!(dots(0), 0);
        let one = dots(1);
        assert!(one > 10, "{one} pixels for a dot");
        assert_eq!(dots(3), 3 * one);
        assert_eq!(dots(100), dots(21));
    }

    #[test]
    fn a_short_buffer_loses_rows_not_the_process() {
        let mut pixels = vec![0; 1280 * 4 * 10];
        let view = View {
            name: "",
            typed: 5,
            status: Status::Refused,
        };
        paint(
            &mut Canvas::new(&mut pixels, 1280, 800),
            &Fonts::default(),
            &view,
            1,
        );
        assert_eq!(
            Canvas::new(&mut pixels, 1280, 800).pixel(0, 9),
            Some(BACKGROUND)
        );
    }
}
