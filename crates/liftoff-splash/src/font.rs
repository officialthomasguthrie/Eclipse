//! The console font, regular and bold, drawn a glyph at a time into coverage maps the screen blends
//! into its pixels. The size makes a character exactly one cell wide, so columns line up on whole
//! pixels, and a cell is twice as tall as it is wide: 8 by 16 pixels at scale 1.

use std::collections::HashMap;

use ab_glyph::{Font as _, FontVec, InvalidFont, PxScale, point};

/// A glyph drawn for a cell: how much of each pixel it covers, from 0 to 255, and where that map
/// sits in the cell.
#[derive(Debug, Default)]
pub struct Glyph {
    /// Pixels from the cell's left edge to the map's.
    pub left: i32,
    /// Pixels from the cell's top edge to the map's.
    pub top: i32,
    /// The map's width.
    pub width: usize,
    /// The map's height.
    pub height: usize,
    /// The map, row by row.
    pub coverage: Vec<u8>,
}

/// The two faces at one cell size, with the glyphs drawn so far.
pub struct Fonts {
    regular: FontVec,
    bold: FontVec,
    scale: PxScale,
    baseline: f32,
    cell_width: usize,
    glyphs: HashMap<(char, bool), Glyph>,
}

impl Fonts {
    /// Both faces from their files' bytes, sized for cells `cell_width` pixels wide.
    ///
    /// # Errors
    ///
    /// When either is not a font.
    pub fn new(regular: Vec<u8>, bold: Vec<u8>, cell_width: usize) -> Result<Self, InvalidFont> {
        let regular = FontVec::try_from_vec(regular)?;
        let bold = FontVec::try_from_vec(bold)?;
        let advance = regular.h_advance_unscaled(regular.glyph_id('0'));
        let height = regular.height_unscaled();
        // ab_glyph's scale is the height from descent to ascent in pixels. at this one a digit's
        // advance is the cell's width
        let cell = pixels(cell_width);
        let size = if advance > 0.0 {
            cell * height / advance
        } else {
            cell * 2.0
        };
        let scale = PxScale::from(size);
        let ascent = regular.ascent_unscaled() * size / height;
        // the line's height is a little under two cells, the rest is shared above and below it
        let baseline = (cell * 2.0 - size) / 2.0 + ascent;
        Ok(Self {
            regular,
            bold,
            scale,
            baseline,
            cell_width,
            glyphs: HashMap::new(),
        })
    }

    /// A cell's width in pixels.
    #[must_use]
    pub fn cell_width(&self) -> usize {
        self.cell_width
    }

    /// A cell's height in pixels.
    #[must_use]
    pub fn cell_height(&self) -> usize {
        self.cell_width * 2
    }

    /// The glyph for a character in the regular or the bold face.
    pub fn glyph(&mut self, ch: char, bold: bool) -> &Glyph {
        let Self {
            regular,
            bold: bold_face,
            scale,
            baseline,
            glyphs,
            ..
        } = self;
        glyphs.entry((ch, bold)).or_insert_with(|| {
            let face = if bold { &*bold_face } else { &*regular };
            draw_glyph(face, ch, *scale, *baseline)
        })
    }
}

fn pixels(count: usize) -> f32 {
    u16::try_from(count).map_or(f32::from(u16::MAX), f32::from)
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the bounds of one glyph at a boot console's size, a few dozen pixels each way"
)]
fn draw_glyph(face: &FontVec, ch: char, scale: PxScale, baseline: f32) -> Glyph {
    let glyph = face
        .glyph_id(ch)
        .with_scale_and_position(scale, point(0.0, baseline));
    let Some(outline) = face.outline_glyph(glyph) else {
        return Glyph::default();
    };
    let bounds = outline.px_bounds();
    let width = bounds.width().max(0.0) as usize;
    let height = bounds.height().max(0.0) as usize;
    let mut coverage = vec![0; width * height];
    outline.draw(|x, y, amount| {
        if let Some(pixel) = coverage.get_mut(y as usize * width + x as usize) {
            *pixel = (amount.clamp(0.0, 1.0) * 255.0).round() as u8;
        }
    });
    Glyph {
        left: bounds.min.x.floor() as i32,
        top: bounds.min.y.floor() as i32,
        width,
        height,
        coverage,
    }
}
