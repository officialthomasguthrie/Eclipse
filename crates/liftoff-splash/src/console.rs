//! The console as the boot writes to it. plymouth hands the splash every byte written to
//! /dev/console and every kernel message it shows, and this reads them the way a terminal does:
//! systemd's status lines with their colours and bold unit names, a status line that takes the place
//! of the one before it, carriage returns and erased lines. Sequences a boot has no use for, like the
//! OSC 3008 context marks systemd writes, are read and dropped.

use std::collections::VecDeque;

/// How many lines the console keeps. A boot writes a few hundred.
const KEPT_LINES: usize = 2000;
/// The longest line kept, in characters. What comes after it is dropped.
const LONGEST_LINE: usize = 1024;
/// The most parameters a control sequence keeps.
const MOST_PARAMETERS: usize = 32;

/// A colour text is drawn in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Colour {
    /// The console's own text colour.
    #[default]
    Default,
    /// One of xterm's 256: the sixteen of the Linux console, a cube of 216 and 24 grays.
    Indexed(u8),
    /// A colour of its own.
    Rgb(u8, u8, u8),
}

/// How a character is drawn.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Style {
    /// Its colour.
    pub colour: Colour,
    /// Bold, which systemd writes unit names, prompts and OK in.
    pub bold: bool,
    /// Faint.
    pub dim: bool,
}

/// A character on a line and how it is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cell {
    /// The character.
    pub ch: char,
    /// Its style.
    pub style: Style,
}

impl Cell {
    /// A space in the console's own colour.
    pub const BLANK: Cell = Cell {
        ch: ' ',
        style: Style {
            colour: Colour::Default,
            bold: false,
            dim: false,
        },
    };
}

/// Where the reader is in what it reads.
#[derive(Debug)]
enum State {
    Text,
    Escape,
    /// An escape with intermediate bytes, like the choice of a character set in `ESC ( B`.
    EscapeIntermediate,
    /// A control sequence, `ESC [`, with the parameters so far.
    Control {
        parameters: Vec<u32>,
        current: Option<u32>,
        private: bool,
    },
    /// An operating system command, `ESC ]`, which ends with BEL or `ESC \`.
    Command,
    CommandEscape,
}

/// The console: its lines, oldest first, and where the cursor is.
#[derive(Debug)]
pub struct Console {
    lines: VecDeque<Vec<Cell>>,
    row: usize,
    column: usize,
    style: Style,
    state: State,
    /// The bits of a character whose last bytes are still to come, and how many.
    pending: u32,
    needed: u8,
}

impl Default for Console {
    fn default() -> Self {
        Self::new()
    }
}

impl Console {
    /// An empty console.
    #[must_use]
    pub fn new() -> Self {
        Self {
            lines: VecDeque::from([Vec::new()]),
            row: 0,
            column: 0,
            style: Style::default(),
            state: State::Text,
            pending: 0,
            needed: 0,
        }
    }

    /// Reads bytes written to the console. A character or a sequence may be cut between two writes.
    pub fn write(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.read(byte);
        }
    }

    /// Adds a line of plymouth's own: a message a program asked it to show.
    pub fn message(&mut self, text: &str) {
        if self
            .lines
            .get(self.row)
            .is_some_and(|line| !line.is_empty())
        {
            self.line_feed();
        }
        self.column = 0;
        let style = std::mem::take(&mut self.style);
        for ch in text.chars().filter(|ch| !ch.is_control()) {
            self.print(ch);
        }
        self.style = style;
        self.line_feed();
    }

    /// The lines, oldest first, up to the last one with something on it.
    pub fn lines(&self) -> impl DoubleEndedIterator<Item = &[Cell]> {
        let used = self
            .lines
            .iter()
            .rposition(|line| !line.is_empty())
            .map_or(0, |last| last + 1);
        self.lines.iter().take(used).map(Vec::as_slice)
    }

    fn read(&mut self, byte: u8) {
        if self.needed > 0 {
            if byte & 0xc0 == 0x80 {
                self.pending = self.pending << 6 | u32::from(byte & 0x3f);
                self.needed -= 1;
                if self.needed == 0 {
                    self.print(char::from_u32(self.pending).unwrap_or(char::REPLACEMENT_CHARACTER));
                }
                return;
            }
            self.needed = 0;
            self.print(char::REPLACEMENT_CHARACTER);
        }
        match std::mem::replace(&mut self.state, State::Text) {
            State::Text => self.text(byte),
            State::Escape => self.escape(byte),
            State::EscapeIntermediate => {
                if (0x20..=0x2f).contains(&byte) {
                    self.state = State::EscapeIntermediate;
                }
            }
            State::Control {
                parameters,
                current,
                private,
            } => self.control(byte, parameters, current, private),
            State::Command => match byte {
                0x07 => {}
                0x1b => self.state = State::CommandEscape,
                _ => self.state = State::Command,
            },
            State::CommandEscape => {
                if byte != b'\\' {
                    self.escape(byte);
                }
            }
        }
    }

    fn text(&mut self, byte: u8) {
        match byte {
            0x1b => self.state = State::Escape,
            b'\n' => self.line_feed(),
            b'\r' => self.column = 0,
            b'\t' => self.column = ((self.column / 8 + 1) * 8).min(LONGEST_LINE),
            0x08 => self.column = self.column.saturating_sub(1),
            0x20..=0x7e => self.print(char::from(byte)),
            0xc2..=0xdf => self.start(byte & 0x1f, 1),
            0xe0..=0xef => self.start(byte & 0x0f, 2),
            0xf0..=0xf4 => self.start(byte & 0x07, 3),
            0x80..=0xff => self.print(char::REPLACEMENT_CHARACTER),
            _ => {}
        }
    }

    fn start(&mut self, bits: u8, needed: u8) {
        self.pending = u32::from(bits);
        self.needed = needed;
    }

    fn escape(&mut self, byte: u8) {
        match byte {
            b'[' => {
                self.state = State::Control {
                    parameters: Vec::new(),
                    current: None,
                    private: false,
                };
            }
            b']' => self.state = State::Command,
            // reverse index: systemd goes back up to write over a status line that only said a job
            // was still running
            b'M' => self.row = self.row.saturating_sub(1),
            b'D' => self.index(),
            b'E' => self.line_feed(),
            0x20..=0x2f => self.state = State::EscapeIntermediate,
            _ => {}
        }
    }

    fn control(
        &mut self,
        byte: u8,
        mut parameters: Vec<u32>,
        mut current: Option<u32>,
        mut private: bool,
    ) {
        match byte {
            b'0'..=b'9' => {
                let digit = u32::from(byte - b'0');
                current = Some(
                    current
                        .unwrap_or(0)
                        .saturating_mul(10)
                        .saturating_add(digit),
                );
            }
            b';' | b':' => {
                if parameters.len() < MOST_PARAMETERS {
                    parameters.push(current.take().unwrap_or(0));
                }
                current = None;
            }
            b'<'..=b'?' => private = true,
            0x40..=0x7e => {
                if let Some(value) = current {
                    parameters.push(value);
                } else if !parameters.is_empty() {
                    parameters.push(0);
                }
                if !private {
                    self.dispatch(byte, &parameters);
                }
                return;
            }
            0x1b => {
                self.state = State::Escape;
                return;
            }
            0x18 | 0x1a => return,
            _ => {}
        }
        self.state = State::Control {
            parameters,
            current,
            private,
        };
    }

    fn dispatch(&mut self, command: u8, parameters: &[u32]) {
        let first = parameters.first().copied().unwrap_or(0);
        let count = usize::try_from(first.max(1)).unwrap_or(1);
        match command {
            b'm' => self.select_graphic_rendition(parameters),
            b'K' => self.erase_in_line(first),
            b'J' => self.erase_in_display(first),
            b'G' | b'`' => self.column = (count - 1).min(LONGEST_LINE),
            b'C' => self.column = self.column.saturating_add(count).min(LONGEST_LINE),
            b'D' => self.column = self.column.saturating_sub(count),
            b'A' => self.row = self.row.saturating_sub(count),
            b'B' => self.row = self.row.saturating_add(count).min(self.lines.len() - 1),
            _ => {}
        }
    }

    fn select_graphic_rendition(&mut self, parameters: &[u32]) {
        if parameters.is_empty() {
            self.style = Style::default();
            return;
        }
        let mut rest = parameters.iter().copied();
        while let Some(parameter) = rest.next() {
            match parameter {
                0 => self.style = Style::default(),
                1 => self.style.bold = true,
                2 => self.style.dim = true,
                22 => {
                    self.style.bold = false;
                    self.style.dim = false;
                }
                30..=37 => self.style.colour = Colour::Indexed(byte(parameter - 30)),
                90..=97 => self.style.colour = Colour::Indexed(byte(parameter - 90 + 8)),
                39 => self.style.colour = Colour::Default,
                38 => {
                    if let Some(colour) = extended_colour(&mut rest) {
                        self.style.colour = colour;
                    }
                }
                // backgrounds are not drawn, but their colour's parameters are read past
                48 => {
                    extended_colour(&mut rest);
                }
                _ => {}
            }
        }
    }

    fn erase_in_line(&mut self, mode: u32) {
        let column = self.column;
        if let Some(line) = self.lines.get_mut(self.row) {
            match mode {
                0 => line.truncate(column),
                1 => line
                    .iter_mut()
                    .take(column + 1)
                    .for_each(|cell| *cell = Cell::BLANK),
                _ => line.clear(),
            }
        }
    }

    fn erase_in_display(&mut self, mode: u32) {
        match mode {
            0 => {
                self.erase_in_line(0);
                self.lines.truncate(self.row + 1);
            }
            1 => {}
            _ => {
                self.lines.clear();
                self.lines.push_back(Vec::new());
                self.row = 0;
                self.column = 0;
            }
        }
    }

    fn print(&mut self, ch: char) {
        if self.column >= LONGEST_LINE {
            return;
        }
        let cell = Cell {
            ch,
            style: self.style,
        };
        let column = self.column;
        if let Some(line) = self.lines.get_mut(self.row) {
            if let Some(old) = line.get_mut(column) {
                *old = cell;
            } else {
                line.resize(column, Cell::BLANK);
                line.push(cell);
            }
        }
        self.column += 1;
    }

    /// A line feed. The pty plymouth reads from turns a newline into a carriage return and a line
    /// feed, and kernel messages come as lines, so both start the next line at its beginning.
    fn line_feed(&mut self) {
        self.index();
        self.column = 0;
    }

    fn index(&mut self) {
        if self.row + 1 < self.lines.len() {
            self.row += 1;
            return;
        }
        self.lines.push_back(Vec::new());
        if self.lines.len() > KEPT_LINES {
            self.lines.pop_front();
        } else {
            self.row += 1;
        }
    }
}

/// A parameter that is a colour's number or channel, as a byte.
fn byte(value: u32) -> u8 {
    u8::try_from(value).unwrap_or(u8::MAX)
}

/// The colour after 38 or 48: 5 and an index, or 2 and red, green and blue.
fn extended_colour(rest: &mut impl Iterator<Item = u32>) -> Option<Colour> {
    match rest.next()? {
        5 => rest.next().map(|index| Colour::Indexed(byte(index))),
        2 => {
            let red = rest.next()?;
            let green = rest.next()?;
            let blue = rest.next()?;
            Some(Colour::Rgb(byte(red), byte(green), byte(blue)))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(line: &[Cell]) -> String {
        line.iter().map(|cell| cell.ch).collect()
    }

    fn texts(console: &Console) -> Vec<String> {
        console.lines().map(text).collect()
    }

    #[test]
    fn a_status_line_keeps_its_colours_and_bold_unit_name() {
        let mut console = Console::new();
        console.write(
            b"[\x1b[0;1;32m  OK  \x1b[0m] Started \x1b[0;1;39msystemd-journald.service\x1b[0m - Journal Service.\r\n",
        );
        let lines: Vec<&[Cell]> = console.lines().collect();
        assert_eq!(lines.len(), 1);
        let line = lines[0];
        assert_eq!(
            text(line),
            "[  OK  ] Started systemd-journald.service - Journal Service."
        );
        let ok = line[3];
        assert_eq!(
            (ok.ch, ok.style.colour, ok.style.bold),
            ('O', Colour::Indexed(2), true)
        );
        assert_eq!(line[0].style, Style::default());
        let name = line[17];
        assert_eq!(
            (name.ch, name.style.colour, name.style.bold),
            ('s', Colour::Default, true)
        );
        assert_eq!(line[line.len() - 1].style, Style::default());
    }

    #[test]
    fn a_status_line_that_goes_back_up_replaces_the_one_before() {
        let mut console = Console::new();
        console.write(b"[  OK  ] Reached target Basic System.\r\n");
        console.write(
            b"[\x1b[0;31m*     \x1b[0m] A start job is running for persist (3s / no limit)\r\n",
        );
        console.write(b"\x1bM\r\x1b[K[\x1b[0;1;31m**    \x1b[0m] A start job is running for persist (4s / no limit)\r\n");
        console.write(b"\x1bM\r\x1b[K[\x1b[0;1;32m  OK  \x1b[0m] Finished persist.\r\n");
        assert_eq!(
            texts(&console),
            [
                "[  OK  ] Reached target Basic System.",
                "[  OK  ] Finished persist."
            ]
        );
    }

    #[test]
    fn operating_system_commands_are_dropped() {
        let mut console = Console::new();
        console.write(b"\x1b]3008;start=0f6f;user=root;hostname=rift;type=boot\x1b\\Welcome\r\n");
        console.write(b"\x1b]0;title\x07to Rift\r\n");
        assert_eq!(texts(&console), ["Welcome", "to Rift"]);
    }

    #[test]
    fn colours_by_index_and_by_channel() {
        let mut console = Console::new();
        console.write(
            b"\x1b[0;1;38;5;185mDEPEND\x1b[0m \x1b[38;2;93;172;217mRift\x1b[39m.\x1b[92mx\r\n",
        );
        let line: Vec<Cell> = console.lines().next().unwrap().to_vec();
        assert_eq!(line[0].style.colour, Colour::Indexed(185));
        assert!(line[0].style.bold);
        assert_eq!(line[7].style.colour, Colour::Rgb(93, 172, 217));
        assert!(!line[7].style.bold);
        assert_eq!(line[11].style.colour, Colour::Default);
        assert_eq!(line[12].style.colour, Colour::Indexed(10));
    }

    #[test]
    fn characters_and_sequences_cut_between_writes() {
        let mut console = Console::new();
        let bytes = "\x1b[1mcaf\u{e9}\x1b[0m \u{2026}\r\n".as_bytes();
        for chunk in bytes.chunks(1) {
            console.write(chunk);
        }
        let line: Vec<Cell> = console.lines().next().unwrap().to_vec();
        assert_eq!(text(&line), "caf\u{e9} \u{2026}");
        assert!(line[3].style.bold);
        assert!(!line[5].style.bold);
    }

    #[test]
    fn kernel_messages_and_plymouth_messages_are_lines() {
        let mut console = Console::new();
        console.write(b"partial");
        console.message("Please confirm presence on security token to unlock.");
        console.write(b"kauditd_printk_skb: 43 callbacks suppressed\n");
        console.write(b"next\n");
        assert_eq!(
            texts(&console),
            [
                "partial",
                "Please confirm presence on security token to unlock.",
                "kauditd_printk_skb: 43 callbacks suppressed",
                "next"
            ]
        );
    }

    #[test]
    fn erasing_and_moving_along_a_line() {
        let mut console = Console::new();
        console.write(b"abcdef\x1b[3G\x1b[KX\r\n\tY\x08Z\x1b[2CW\r\n");
        assert_eq!(texts(&console), ["abX", "        Z  W"]);
        console.write(b"\x1b[2Jfresh\r\n");
        assert_eq!(texts(&console), ["fresh"]);
    }

    #[test]
    fn invalid_bytes_show_as_a_replacement() {
        let mut console = Console::new();
        console.write(b"a\xffb\xe2\x80c\r\n");
        assert_eq!(texts(&console), ["a\u{fffd}b\u{fffd}c"]);
    }

    #[test]
    fn old_lines_go_when_there_are_too_many() {
        let mut console = Console::new();
        for n in 0..KEPT_LINES + 10 {
            console.write(format!("line {n}\r\n").as_bytes());
        }
        let lines = texts(&console);
        assert_eq!(lines.len(), KEPT_LINES - 1);
        assert_eq!(lines.last().map(String::as_str), Some("line 2009"));
    }
}
