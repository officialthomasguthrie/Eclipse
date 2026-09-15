//! What the text boot shows. At the top the logo in characters with the system's name, then the
//! console with its newest line at the bottom, and a question plymouth asks as the line after the
//! console's last. Where the logo does not fit whole, and in the details view Esc switches to, there
//! is no logo and the console starts at the top.

use crate::console::{Cell, Colour, Console, Style};
use crate::font::Fonts;
use crate::logo::Logo;

/// The background: the near black the logo was drawn on, which the terminal has too.
pub const BACKGROUND: [u8; 3] = [4, 4, 6];
/// Text in the console's own colour, as plymouth's console view draws it.
pub const FOREGROUND: [u8; 3] = [255, 255, 255];
/// The Linux console's sixteen colours, which plymouth's console view has too. systemd's OK is the
/// green, 0, 170, 0.
pub const PALETTE: [[u8; 3]; 16] = [
    [0, 0, 0],
    [170, 0, 0],
    [0, 170, 0],
    [170, 85, 0],
    [0, 0, 170],
    [170, 0, 170],
    [0, 170, 170],
    [170, 170, 170],
    [85, 85, 85],
    [255, 85, 85],
    [85, 255, 85],
    [255, 255, 85],
    [85, 85, 255],
    [255, 85, 255],
    [85, 255, 255],
    [255, 255, 255],
];
/// Empty cells left and right of the text and above its first line.
pub const MARGIN: usize = 1;
/// The fewest console lines under the logo. With less room there is no logo.
pub const FEWEST_LINES: usize = 8;
/// The most asterisks a passphrase shows.
const MOST_TYPED: usize = 256;

/// A question plymouth is asking.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Prompt {
    /// A passphrase and how many characters of it are typed. Each shows as an asterisk.
    Secret {
        /// The question, as the program that asks words it.
        question: String,
        /// Characters typed so far.
        typed: usize,
    },
    /// A question whose answer shows as it is typed.
    Visible {
        /// The question.
        question: String,
        /// What is typed so far.
        answer: String,
    },
}

impl Prompt {
    /// The line it shows: the question in bold, a colon when it has none, what is typed and the
    /// cursor.
    #[must_use]
    pub fn cells(&self) -> Vec<Cell> {
        let (question, answer) = match self {
            Self::Secret { question, typed } => (question, "*".repeat((*typed).min(MOST_TYPED))),
            Self::Visible { question, answer } => (question, answer.clone()),
        };
        let bold = Style {
            bold: true,
            ..Style::default()
        };
        let question = question.trim_end();
        let mut cells: Vec<Cell> = question
            .chars()
            .filter(|ch| !ch.is_control())
            .map(|ch| Cell { ch, style: bold })
            .collect();
        if !question.ends_with(':') {
            cells.push(Cell {
                ch: ':',
                style: bold,
            });
        }
        cells.push(Cell::BLANK);
        cells.extend(answer.chars().filter(|ch| !ch.is_control()).map(|ch| Cell {
            ch,
            style: Style::default(),
        }));
        cells.push(Cell {
            ch: '_',
            style: Style::default(),
        });
        cells
    }
}

/// Where the system's name goes when the logo shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Banner {
    /// On a line of its own under the logo.
    Under,
    /// Beside the logo, halfway down it.
    Beside,
}

/// What a frame shows.
#[derive(Clone, Copy, Debug)]
pub struct View<'a> {
    /// The console.
    pub console: &'a Console,
    /// The question being asked, if one is.
    pub prompt: Option<&'a Prompt>,
    /// The system's name and version.
    pub title: &'a str,
    /// The details view: the console alone.
    pub details: bool,
    /// Where the name goes.
    pub banner: Banner,
}

/// Where things go on a screen, in cells.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Layout {
    /// The logo's top left cell, when it shows.
    pub logo: Option<(usize, usize)>,
    /// The name's first cell, when it shows.
    pub title: Option<(usize, usize)>,
    /// The console's first row.
    pub console_top: usize,
}

impl Layout {
    /// Where things go on a screen of `columns` by `rows` cells, for a logo and a name `title`
    /// characters long.
    #[must_use]
    pub fn new(
        columns: usize,
        rows: usize,
        logo: &Logo,
        title: usize,
        details: bool,
        banner: Banner,
    ) -> Self {
        if details {
            return Self {
                logo: None,
                title: None,
                console_top: MARGIN,
            };
        }
        let room = columns.saturating_sub(2 * MARGIN);
        let (logo_columns, logo_rows) = (logo.columns(), logo.rows());
        let beside = logo_columns + 3 + title;
        let (fits, title_cell, console_top) = match banner {
            Banner::Under => (
                logo_columns <= room,
                (MARGIN, MARGIN + logo_rows + 1),
                MARGIN + logo_rows + 3,
            ),
            Banner::Beside => (
                beside <= room,
                (MARGIN + logo_columns + 3, MARGIN + logo_rows / 2),
                MARGIN + logo_rows + 1,
            ),
        };
        if fits && logo_rows > 0 && console_top + FEWEST_LINES <= rows {
            Self {
                logo: Some((MARGIN, MARGIN)),
                title: Some(title_cell),
                console_top,
            }
        } else {
            Self {
                logo: None,
                title: Some((MARGIN, MARGIN)),
                console_top: MARGIN + 2,
            }
        }
    }
}

/// The rows the console shows under its top, newest last: its lines broken at `width` cells and the
/// question after them, as many as `count` from the bottom.
#[must_use]
pub fn tail(
    console: &Console,
    prompt: Option<&Prompt>,
    width: usize,
    count: usize,
) -> Vec<Vec<Cell>> {
    let width = width.max(1);
    let mut rows: Vec<Vec<Cell>> = Vec::new();
    let prompt = prompt.map(Prompt::cells);
    for line in prompt.as_deref().into_iter().chain(console.lines().rev()) {
        if rows.len() >= count {
            break;
        }
        if line.is_empty() {
            rows.push(Vec::new());
            continue;
        }
        for chunk in line.chunks(width).rev() {
            if rows.len() >= count {
                break;
            }
            rows.push(chunk.to_vec());
        }
    }
    rows.reverse();
    rows
}

/// Pixels, row by row, each 0xffRRGGBB.
#[derive(Debug, Default)]
pub struct Frame {
    width: usize,
    height: usize,
    pixels: Vec<u32>,
}

impl Frame {
    /// A frame of `width` by `height` pixels.
    #[must_use]
    pub fn new(width: usize, height: usize) -> Self {
        let mut frame = Self::default();
        frame.resize(width, height);
        frame
    }

    /// Makes it `width` by `height` pixels.
    pub fn resize(&mut self, width: usize, height: usize) {
        if (width, height) != (self.width, self.height) {
            self.width = width;
            self.height = height;
            self.pixels = vec![pack(BACKGROUND); width * height];
        }
    }

    /// Its width.
    #[must_use]
    pub fn width(&self) -> usize {
        self.width
    }

    /// Its height.
    #[must_use]
    pub fn height(&self) -> usize {
        self.height
    }

    /// Its pixels.
    #[must_use]
    pub fn pixels(&self) -> &[u32] {
        &self.pixels
    }

    /// Its pixels, to hand to plymouth.
    pub fn pixels_mut(&mut self) -> &mut [u32] {
        &mut self.pixels
    }

    /// Its pixels as red, green and blue bytes, for a picture.
    #[must_use]
    pub fn rgb(&self) -> Vec<u8> {
        self.pixels
            .iter()
            .flat_map(|pixel| {
                let [_, red, green, blue] = pixel.to_be_bytes();
                [red, green, blue]
            })
            .collect()
    }

    /// Draws a view with the cells of `fonts`.
    pub fn draw(&mut self, fonts: &mut Fonts, logo: &Logo, view: &View<'_>) {
        self.pixels.fill(pack(BACKGROUND));
        let (cell_width, cell_height) = (fonts.cell_width(), fonts.cell_height());
        if cell_width == 0 {
            return;
        }
        let (columns, rows) = (self.width / cell_width, self.height / cell_height);
        let title = view.title.chars().count();
        let layout = Layout::new(columns, rows, logo, title, view.details, view.banner);
        if let Some((left, top)) = layout.logo {
            for (row, line) in logo.lines().iter().enumerate() {
                for (column, &(ch, colour)) in line.iter().enumerate() {
                    if let Some(colour) = colour {
                        self.cell(fonts, left + column, top + row, ch, false, colour);
                    }
                }
            }
        }
        if let Some((left, top)) = layout.title {
            for (column, ch) in view
                .title
                .chars()
                .enumerate()
                .take(columns.saturating_sub(left))
            {
                self.cell(fonts, left + column, top, ch, true, FOREGROUND);
            }
        }
        let width = columns.saturating_sub(2 * MARGIN);
        let count = rows.saturating_sub(layout.console_top);
        for (row, line) in tail(view.console, view.prompt, width, count)
            .iter()
            .enumerate()
        {
            for (column, cell) in line.iter().enumerate() {
                if cell.ch != ' ' {
                    let colour = colour_of(cell.style);
                    self.cell(
                        fonts,
                        MARGIN + column,
                        layout.console_top + row,
                        cell.ch,
                        cell.style.bold,
                        colour,
                    );
                }
            }
        }
    }

    /// Blends a character's glyph into the cell at `column` and `row`.
    fn cell(
        &mut self,
        fonts: &mut Fonts,
        column: usize,
        row: usize,
        ch: char,
        bold: bool,
        colour: [u8; 3],
    ) {
        let (cell_width, cell_height) = (fonts.cell_width(), fonts.cell_height());
        let glyph = fonts.glyph(ch, bold);
        let left = signed(column * cell_width) + i64::from(glyph.left);
        let top = signed(row * cell_height) + i64::from(glyph.top);
        for (glyph_row, coverage) in glyph.coverage.chunks(glyph.width.max(1)).enumerate() {
            let Ok(y) = usize::try_from(top + signed(glyph_row)) else {
                continue;
            };
            if y >= self.height {
                break;
            }
            for (glyph_column, &amount) in coverage.iter().enumerate() {
                let Ok(x) = usize::try_from(left + signed(glyph_column)) else {
                    continue;
                };
                if amount == 0 || x >= self.width {
                    continue;
                }
                if let Some(pixel) = self.pixels.get_mut(y * self.width + x) {
                    *pixel = blend(*pixel, colour, amount);
                }
            }
        }
    }
}

fn signed(value: usize) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

fn pack([red, green, blue]: [u8; 3]) -> u32 {
    u32::from_be_bytes([0xff, red, green, blue])
}

/// A colour over a pixel, `amount` of 255 of it.
fn blend(under: u32, over: [u8; 3], amount: u8) -> u32 {
    let [_, red, green, blue] = under.to_be_bytes();
    let amount = u16::from(amount);
    let mix = |under: u8, over: u8| {
        let value = (u16::from(under) * (255 - amount) + u16::from(over) * amount + 127) / 255;
        u8::try_from(value).unwrap_or(u8::MAX)
    };
    u32::from_be_bytes([
        0xff,
        mix(red, over[0]),
        mix(green, over[1]),
        mix(blue, over[2]),
    ])
}

/// The colour a style draws in.
#[must_use]
pub fn colour_of(style: Style) -> [u8; 3] {
    let colour = match style.colour {
        Colour::Default => FOREGROUND,
        Colour::Indexed(index) => indexed(index),
        Colour::Rgb(red, green, blue) => [red, green, blue],
    };
    if style.dim {
        let [red, green, blue] = colour;
        let half = |value: u8, ground: u8| value / 2 + ground / 2;
        [
            half(red, BACKGROUND[0]),
            half(green, BACKGROUND[1]),
            half(blue, BACKGROUND[2]),
        ]
    } else {
        colour
    }
}

/// One of xterm's 256 colours.
fn indexed(index: u8) -> [u8; 3] {
    match index {
        0..=15 => PALETTE[usize::from(index)],
        16..=231 => {
            let cube = index - 16;
            let level = |step: u8| if step == 0 { 0 } else { 55 + step * 40 };
            [level(cube / 36), level(cube / 6 % 6), level(cube % 6)]
        }
        _ => [8 + (index - 232) * 10; 3],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(cells: &[Cell]) -> String {
        cells.iter().map(|cell| cell.ch).collect()
    }

    #[test]
    fn the_logo_and_the_name_on_a_1280_by_800_screen() {
        let logo = Logo::new();
        assert_eq!(
            Layout::new(160, 50, &logo, 10, false, Banner::Under),
            Layout {
                logo: Some((1, 1)),
                title: Some((1, 33)),
                console_top: 35
            }
        );
        assert_eq!(
            Layout::new(160, 50, &logo, 10, false, Banner::Beside),
            Layout {
                logo: Some((1, 1)),
                title: Some((114, 16)),
                console_top: 33
            }
        );
    }

    #[test]
    fn no_logo_where_it_does_not_fit_whole_or_in_the_details_view() {
        let logo = Logo::new();
        let alone = Layout {
            logo: None,
            title: Some((1, 1)),
            console_top: 3,
        };
        assert_eq!(Layout::new(100, 50, &logo, 10, false, Banner::Under), alone);
        assert_eq!(Layout::new(160, 40, &logo, 10, false, Banner::Under), alone);
        assert_eq!(
            Layout::new(120, 50, &logo, 10, false, Banner::Beside),
            alone
        );
        assert_eq!(
            Layout::new(160, 50, &logo, 10, true, Banner::Under),
            Layout {
                logo: None,
                title: None,
                console_top: 1
            }
        );
    }

    #[test]
    fn the_newest_rows_with_the_question_last() {
        let mut console = Console::new();
        console.write(b"one\r\n\r\ntwo two two\r\nthree\r\n");
        let prompt = Prompt::Secret {
            question: "Please enter passphrase for disk persist:".to_string(),
            typed: 3,
        };
        let rows: Vec<String> = tail(&console, Some(&prompt), 5, 4)
            .iter()
            .map(|row| text(row))
            .collect();
        assert_eq!(rows, ["sk pe", "rsist", ": ***", "_"]);
        let rows: Vec<String> = tail(&console, Some(&prompt), 80, 4)
            .iter()
            .map(|row| text(row))
            .collect();
        assert_eq!(
            rows,
            [
                "",
                "two two two",
                "three",
                "Please enter passphrase for disk persist: ***_"
            ]
        );
        let rows: Vec<String> = tail(&console, None, 5, 3)
            .iter()
            .map(|row| text(row))
            .collect();
        assert_eq!(rows, ["wo tw", "o", "three"]);
    }

    #[test]
    fn a_question_in_bold_with_its_answer() {
        let cells = Prompt::Visible {
            question: "Choose a passphrase".to_string(),
            answer: "abc".to_string(),
        }
        .cells();
        assert_eq!(text(&cells), "Choose a passphrase: abc_");
        assert!(cells[0].style.bold && cells[19].style.bold);
        assert!(!cells[21].style.bold);
    }

    #[test]
    fn systemds_colours() {
        let bold_green = Style {
            colour: Colour::Indexed(2),
            bold: true,
            dim: false,
        };
        assert_eq!(colour_of(bold_green), [0, 170, 0]);
        assert_eq!(indexed(185), [215, 215, 95]);
        assert_eq!(indexed(245), [138, 138, 138]);
        assert_eq!(colour_of(Style::default()), FOREGROUND);
    }

    #[test]
    fn blending_a_glyph_into_the_background() {
        assert_eq!(blend(pack(BACKGROUND), [0, 170, 0], 255), pack([0, 170, 0]));
        assert_eq!(blend(pack(BACKGROUND), [0, 170, 0], 0), pack(BACKGROUND));
        assert_eq!(
            blend(pack([0, 0, 0]), [255, 255, 255], 128),
            pack([128, 128, 128])
        );
    }
}
