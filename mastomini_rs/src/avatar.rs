//! Generated avatars: a member's initial on a colour picked from their
//! account id (open question 17). Drawn on request from a 5×7 bitmap font into
//! a 1-bit palette PNG of about 2 KiB, so avatars cost no flash and there is
//! nothing to upload.

/// Side of the square image, in pixels.
pub const SIZE: usize = 120;
const SCALE: usize = 12;

/// Background colours, all readable with white text.
const COLOURS: [[u8; 3]; 12] = [
    [0x56, 0x3a, 0xcc],
    [0xc2, 0x41, 0x0c],
    [0x0f, 0x76, 0x6e],
    [0x1d, 0x4e, 0xd8],
    [0xb9, 0x1c, 0x1c],
    [0x7e, 0x22, 0xce],
    [0x04, 0x78, 0x57],
    [0xa1, 0x62, 0x07],
    [0xbe, 0x18, 0x5d],
    [0x03, 0x69, 0xa1],
    [0x4d, 0x7c, 0x0f],
    [0x47, 0x55, 0x69],
];

/// 5×7 glyphs, one byte per row, the low five bits left to right.
#[rustfmt::skip]
fn glyph(c: char) -> [u8; 7] {
    match c.to_ascii_uppercase() {
        'A' => [0b01110, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001],
        'B' => [0b11110, 0b10001, 0b10001, 0b11110, 0b10001, 0b10001, 0b11110],
        'C' => [0b01110, 0b10001, 0b10000, 0b10000, 0b10000, 0b10001, 0b01110],
        'D' => [0b11110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b11110],
        'E' => [0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b11111],
        'F' => [0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b10000],
        'G' => [0b01110, 0b10001, 0b10000, 0b10111, 0b10001, 0b10001, 0b01111],
        'H' => [0b10001, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001],
        'I' => [0b01110, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110],
        'J' => [0b00111, 0b00010, 0b00010, 0b00010, 0b00010, 0b10010, 0b01100],
        'K' => [0b10001, 0b10010, 0b10100, 0b11000, 0b10100, 0b10010, 0b10001],
        'L' => [0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b11111],
        'M' => [0b10001, 0b11011, 0b10101, 0b10101, 0b10001, 0b10001, 0b10001],
        'N' => [0b10001, 0b10001, 0b11001, 0b10101, 0b10011, 0b10001, 0b10001],
        'O' => [0b01110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110],
        'P' => [0b11110, 0b10001, 0b10001, 0b11110, 0b10000, 0b10000, 0b10000],
        'Q' => [0b01110, 0b10001, 0b10001, 0b10001, 0b10101, 0b10010, 0b01101],
        'R' => [0b11110, 0b10001, 0b10001, 0b11110, 0b10100, 0b10010, 0b10001],
        'S' => [0b01111, 0b10000, 0b10000, 0b01110, 0b00001, 0b00001, 0b11110],
        'T' => [0b11111, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100],
        'U' => [0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110],
        'V' => [0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01010, 0b00100],
        'W' => [0b10001, 0b10001, 0b10001, 0b10101, 0b10101, 0b10101, 0b01010],
        'X' => [0b10001, 0b10001, 0b01010, 0b00100, 0b01010, 0b10001, 0b10001],
        'Y' => [0b10001, 0b10001, 0b10001, 0b01010, 0b00100, 0b00100, 0b00100],
        'Z' => [0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b10000, 0b11111],
        '0' => [0b01110, 0b10001, 0b10011, 0b10101, 0b11001, 0b10001, 0b01110],
        '1' => [0b00100, 0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110],
        '2' => [0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0b01000, 0b11111],
        '3' => [0b11111, 0b00010, 0b00100, 0b00010, 0b00001, 0b10001, 0b01110],
        '4' => [0b00010, 0b00110, 0b01010, 0b10010, 0b11111, 0b00010, 0b00010],
        '5' => [0b11111, 0b10000, 0b11110, 0b00001, 0b00001, 0b10001, 0b01110],
        '6' => [0b00110, 0b01000, 0b10000, 0b11110, 0b10001, 0b10001, 0b01110],
        '7' => [0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b01000, 0b01000],
        '8' => [0b01110, 0b10001, 0b10001, 0b01110, 0b10001, 0b10001, 0b01110],
        '9' => [0b01110, 0b10001, 0b10001, 0b01111, 0b00001, 0b00010, 0b01100],
        _ => [0, 0, 0, 0, 0, 0, 0b11111],
    }
}

/// The colour for an account id: stable, and spread over the palette.
fn colour(account_id: u64) -> [u8; 3] {
    // FNV-1a over the id's bytes.
    let hash = account_id
        .to_le_bytes()
        .iter()
        .fold(0xcbf2_9ce4_8422_2325u64, |h, b| {
            (h ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3)
        });
    COLOURS[(hash % COLOURS.len() as u64) as usize]
}

/// PNG of `username`'s first character on the account's colour.
pub fn png(account_id: u64, username: &str) -> Vec<u8> {
    let rows = glyph(username.chars().next().unwrap_or('_'));
    let (left, top) = ((SIZE - 5 * SCALE) / 2, (SIZE - 7 * SCALE) / 2);
    // Scanlines: a filter byte (0 = none), then 1 bit per pixel, MSB first.
    let stride = 1 + SIZE.div_ceil(8);
    let mut raw = vec![0u8; stride * SIZE];
    for y in top..top + 7 * SCALE {
        let bits = rows[(y - top) / SCALE];
        for x in left..left + 5 * SCALE {
            if bits & (0b10000 >> ((x - left) / SCALE)) != 0 {
                raw[y * stride + 1 + x / 8] |= 0x80 >> (x % 8);
            }
        }
    }
    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&(SIZE as u32).to_be_bytes());
    ihdr.extend_from_slice(&(SIZE as u32).to_be_bytes());
    ihdr.extend_from_slice(&[1, 3, 0, 0, 0]); // 1-bit, palette
    let mut plte = colour(account_id).to_vec();
    plte.extend_from_slice(&[0xff, 0xff, 0xff]);

    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"PLTE", &plte);
    chunk(&mut out, b"IDAT", &zlib_stored(&raw));
    chunk(&mut out, b"IEND", &[]);
    out
}

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    let start = out.len();
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let crc = crc32fast::hash(&out[start..]);
    out.extend_from_slice(&crc.to_be_bytes());
}

/// A zlib stream of uncompressed ("stored") deflate blocks: no compressor
/// needed, and the image is small anyway.
fn zlib_stored(data: &[u8]) -> Vec<u8> {
    let mut out = vec![0x78, 0x01];
    let blocks: Vec<&[u8]> = data.chunks(0xffff).collect();
    for (i, block) in blocks.iter().enumerate() {
        out.push(u8::from(i + 1 == blocks.len()));
        let len = block.len() as u16;
        out.extend_from_slice(&len.to_le_bytes());
        out.extend_from_slice(&(!len).to_le_bytes());
        out.extend_from_slice(block);
    }
    let (mut a, mut b) = (1u32, 0u32);
    for byte in data {
        a = (a + u32::from(*byte)) % 65_521;
        b = (b + a) % 65_521;
    }
    out.extend_from_slice(&((b << 16) | a).to_be_bytes());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Walk the chunks, checking every CRC. Returns (kind, data) pairs.
    fn chunks(png: &[u8]) -> Vec<([u8; 4], Vec<u8>)> {
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
        let mut out = Vec::new();
        let mut at = 8;
        while at < png.len() {
            let len = u32::from_be_bytes(png[at..at + 4].try_into().unwrap()) as usize;
            let body = &png[at + 4..at + 8 + len];
            let crc = u32::from_be_bytes(png[at + 8 + len..at + 12 + len].try_into().unwrap());
            assert_eq!(crc32fast::hash(body), crc);
            out.push((body[..4].try_into().unwrap(), body[4..].to_vec()));
            at += 12 + len;
        }
        out
    }

    #[test]
    fn a_valid_png_with_the_letter_drawn() {
        let png = png(42, "alice");
        let chunks = chunks(&png);
        let kinds: Vec<&[u8; 4]> = chunks.iter().map(|(k, _)| k).collect();
        assert_eq!(kinds, [b"IHDR", b"PLTE", b"IDAT", b"IEND"]);
        assert_eq!(&chunks[0].1[..8], &[0, 0, 0, 120, 0, 0, 0, 120]);
        // Undo the stored deflate: 2-byte header, 5-byte block header.
        let idat = &chunks[2].1;
        let raw = &idat[7..idat.len() - 4];
        let stride = 16;
        assert_eq!(raw.len(), stride * SIZE);
        let lit = |x: usize, y: usize| raw[y * stride + 1 + x / 8] & (0x80 >> (x % 8)) != 0;
        // 'A' has its apex in the middle column of the top row, not the corners.
        let (left, top) = (30, 18);
        assert!(lit(left + 2 * SCALE, top));
        assert!(!lit(left, top));
        assert!(!lit(0, 0));
        assert!(png.len() < 2200);
    }

    #[test]
    fn stable_colours_per_account() {
        assert_eq!(png(7, "bob"), png(7, "bob"));
        let colours: std::collections::BTreeSet<[u8; 3]> = (0..64).map(colour).collect();
        assert!(colours.len() > 6);
    }

    #[test]
    fn every_username_character_has_a_glyph() {
        for c in ('a'..='z').chain('0'..='9') {
            assert_ne!(glyph(c), glyph('_'), "{c}");
        }
    }
}
