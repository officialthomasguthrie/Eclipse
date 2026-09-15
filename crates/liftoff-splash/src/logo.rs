//! The logo in characters: the black hole in ice from nix/liftoff/logo, every character in its own
//! colour, exactly as the files have it.

const TEXT: &str = include_str!("../../../nix/liftoff/logo/rift-logo.txt");
const COLOURS: &str = include_str!("../../../nix/liftoff/logo/rift-logo.colours");

/// A character of the logo and its colour. A space has none.
pub type Character = (char, Option<[u8; 3]>);

/// The logo's lines, each a list of characters with their colours.
#[derive(Debug)]
pub struct Logo {
    lines: Vec<Vec<Character>>,
}

impl Logo {
    /// The logo from nix/liftoff/logo.
    #[must_use]
    pub fn new() -> Self {
        Self::parse(TEXT, COLOURS)
    }

    /// A logo from its characters and their colours: a line of `r;g;b` for each line, one for each
    /// character, `-` for a space.
    #[must_use]
    pub fn parse(text: &str, colours: &str) -> Self {
        let lines = text
            .lines()
            .zip(colours.lines().chain(std::iter::repeat("")))
            .map(|(chars, colours)| {
                chars
                    .chars()
                    .zip(colours.split(' ').chain(std::iter::repeat("-")))
                    .map(|(ch, colour)| (ch, if ch == ' ' { None } else { rgb(colour) }))
                    .collect()
            })
            .collect();
        Self { lines }
    }

    /// Its lines, top first.
    #[must_use]
    pub fn lines(&self) -> &[Vec<Character>] {
        &self.lines
    }

    /// How many lines it has.
    #[must_use]
    pub fn rows(&self) -> usize {
        self.lines.len()
    }

    /// How many characters its longest line has.
    #[must_use]
    pub fn columns(&self) -> usize {
        self.lines.iter().map(Vec::len).max().unwrap_or(0)
    }
}

impl Default for Logo {
    fn default() -> Self {
        Self::new()
    }
}

fn rgb(colour: &str) -> Option<[u8; 3]> {
    let mut channels = colour.split(';').map(|channel| channel.parse::<u8>().ok());
    Some([channels.next()??, channels.next()??, channels.next()??])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_logo_is_110_by_31_with_a_colour_for_every_character() {
        let logo = Logo::new();
        assert_eq!((logo.columns(), logo.rows()), (110, 31));
        let coloured = logo
            .lines()
            .iter()
            .flatten()
            .filter(|(ch, colour)| *ch != ' ' && colour.is_some())
            .count();
        let characters = logo
            .lines()
            .iter()
            .flatten()
            .filter(|(ch, _)| *ch != ' ')
            .count();
        assert_eq!(coloured, characters);
        assert_eq!(characters, 2468);
    }

    #[test]
    fn a_line_and_its_colours() {
        let logo = Logo::parse("a b\n\nc\n", "1;2;3 - 4;5;6\n\n7;8;9\n");
        assert_eq!(
            logo.lines(),
            [
                vec![('a', Some([1, 2, 3])), (' ', None), ('b', Some([4, 5, 6]))],
                vec![],
                vec![('c', Some([7, 8, 9]))],
            ]
        );
    }
}
