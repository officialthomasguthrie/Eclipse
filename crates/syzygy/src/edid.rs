//! Just enough EDID to answer two questions: how big the panel is in centimetres, and what mode
//! it would rather run at. Everything else in the block is ignored.
//!
//! Layout of the 128 byte base block, from the VESA spec: eight header bytes, then the vendor
//! and product block, then the physical size at 21 and 22, then four 18 byte descriptors from
//! offset 54. The first descriptor is the preferred timing when it is a timing at all.

/// The eight bytes every EDID starts with.
const HEADER: [u8; 8] = [0x00, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x00];

/// The base block is 128 bytes and its bytes sum to a multiple of 256.
const BLOCK: usize = 128;

/// Where the first descriptor starts.
const DESCRIPTOR: usize = 54;

/// What we take out of an EDID.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Edid {
    /// Preferred mode in pixels, `None` when the first descriptor is not a timing.
    pub mode: Option<(u32, u32)>,
    /// Physical size in centimetres, `None` when the display does not report one.
    pub size_cm: Option<(u32, u32)>,
}

/// Reads the base block of an EDID.
///
/// Returns `None` when the bytes are not an EDID: too short, wrong header, or a bad checksum.
/// A monitor that gives no size and no timing still parses, it just answers `None` to both.
#[must_use]
pub fn parse(bytes: &[u8]) -> Option<Edid> {
    let block = bytes.get(..BLOCK)?;
    if block[..8] != HEADER {
        return None;
    }
    let sum = block.iter().fold(0u8, |acc, b| acc.wrapping_add(*b));
    if sum != 0 {
        return None;
    }
    let size_cm = match (u32::from(block[21]), u32::from(block[22])) {
        (0, _) | (_, 0) => None,
        size => Some(size),
    };
    Some(Edid {
        mode: preferred_mode(&block[DESCRIPTOR..DESCRIPTOR + 18]),
        size_cm,
    })
}

/// The active pixels of an 18 byte detailed timing descriptor. A descriptor whose first two
/// bytes are zero is a text descriptor (a name, a serial number), not a timing.
fn preferred_mode(d: &[u8]) -> Option<(u32, u32)> {
    if d[0] == 0 && d[1] == 0 {
        return None;
    }
    let horizontal = u32::from(d[2]) | (u32::from(d[4] & 0xf0) << 4);
    let vertical = u32::from(d[5]) | (u32::from(d[7] & 0xf0) << 4);
    if horizontal == 0 || vertical == 0 {
        return None;
    }
    Some((horizontal, vertical))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A base block with the size in centimetres and one detailed timing, checksum fixed up.
    fn block(width_cm: u8, height_cm: u8, mode: Option<(u32, u32)>) -> Vec<u8> {
        let mut b = vec![0u8; BLOCK];
        b[..8].copy_from_slice(&HEADER);
        b[21] = width_cm;
        b[22] = height_cm;
        if let Some((h, v)) = mode {
            let d = DESCRIPTOR;
            b[d] = 0x01; // a pixel clock, so this is a timing and not a text descriptor
            b[d + 2] = u8::try_from(h & 0xff).unwrap();
            b[d + 4] = u8::try_from((h >> 8) << 4).unwrap();
            b[d + 5] = u8::try_from(v & 0xff).unwrap();
            b[d + 7] = u8::try_from((v >> 8) << 4).unwrap();
        }
        let sum = b.iter().fold(0u8, |acc, x| acc.wrapping_add(*x));
        b[127] = 0u8.wrapping_sub(sum);
        b
    }

    #[test]
    fn a_laptop_panel() {
        let edid = parse(&block(30, 19, Some((2880, 1800)))).unwrap();
        assert_eq!(edid.mode, Some((2880, 1800)));
        assert_eq!(edid.size_cm, Some((30, 19)));
    }

    #[test]
    fn a_wide_desktop_monitor() {
        let edid = parse(&block(80, 34, Some((3440, 1440)))).unwrap();
        assert_eq!(edid.mode, Some((3440, 1440)));
        assert_eq!(edid.size_cm, Some((80, 34)));
    }

    #[test]
    fn missing_parts_are_none_not_an_error() {
        let edid = parse(&block(0, 0, None)).unwrap();
        assert_eq!(edid.mode, None);
        assert_eq!(edid.size_cm, None);
        // a projector reports a mode but no size
        let edid = parse(&block(0, 0, Some((1920, 1080)))).unwrap();
        assert_eq!(edid.mode, Some((1920, 1080)));
        assert_eq!(edid.size_cm, None);
    }

    #[test]
    fn junk_is_rejected() {
        assert_eq!(parse(&[]), None);
        assert_eq!(parse(&[0u8; 128]), None);
        assert_eq!(parse(&block(30, 19, None)[..64]), None);
        let mut bad = block(30, 19, Some((1920, 1080)));
        bad[127] = bad[127].wrapping_add(1);
        assert_eq!(parse(&bad), None);
    }

    #[test]
    fn extension_blocks_are_ignored() {
        let mut long = block(30, 19, Some((1920, 1080)));
        long.extend_from_slice(&[0xaa; 128]);
        assert_eq!(parse(&long).unwrap().mode, Some((1920, 1080)));
    }
}
