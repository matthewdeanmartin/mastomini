//! Small JPEG previews of images from other servers, made at night on this
//! board so the household board never decodes one (spec/remote_follow_rss.md
//! "Images").
//!
//! A JPEG is decoded already shrunk (1/2, 1/4 or 1/8 of its size, in the
//! decoder's DCT step), box-averaged down to the preview size, turned the
//! right way up, and encoded again. Encoding again drops all metadata
//! (EXIF: camera, GPS position). A PNG (Mastodon keeps screenshots and
//! many avatars as PNG) is read a row at a time and averaged down as it is
//! read, so a large one never sits whole in memory; transparency is laid on
//! white. Anything else (WebP, GIF), too large, or progressive and large
//! (the decoder would hold every coefficient) is refused: the post keeps a
//! link to the original.

use jpeg_decoder::{CodingProcess, Decoder, PixelFormat};

/// The most pixels a source may have.
pub const MAX_SOURCE_PIXELS: u64 = 24_000_000;
/// Progressive JPEGs keep all coefficients while decoding: about 2 bytes
/// per pixel per component, so keep them small. Interlaced PNGs are
/// decoded whole: 4 bytes per pixel.
pub const MAX_PROGRESSIVE_PIXELS: u64 = 2_000_000;

const PNG_SIGNATURE: &[u8] = &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];

/// A JPEG preview of a JPEG or PNG, at most `max_px` on its longest side
/// and `max_bytes` long.
pub fn preview(image: &[u8], max_px: u16, max_bytes: usize) -> Result<Vec<u8>, String> {
    let (rgb, w, h) = if image.starts_with(PNG_SIGNATURE) {
        png_rgb(image, max_px)?
    } else {
        jpeg_rgb(image, max_px)?
    };
    for quality in [80, 65, 50] {
        let mut out = Vec::new();
        jpeg_encoder::Encoder::new(&mut out, quality)
            .encode(&rgb, w, h, jpeg_encoder::ColorType::Rgb)
            .map_err(|e| format!("encoding: {e}"))?;
        if out.len() <= max_bytes {
            return Ok(out);
        }
    }
    Err("the preview stays too large".into())
}

fn jpeg_rgb(jpeg: &[u8], max_px: u16) -> Result<(Vec<u8>, u16, u16), String> {
    let mut decoder = Decoder::new(jpeg);
    decoder
        .read_info()
        .map_err(|e| format!("not a JPEG: {e}"))?;
    let info = decoder.info().ok_or("not a JPEG")?;
    let pixels = u64::from(info.width) * u64::from(info.height);
    if pixels == 0 || pixels > MAX_SOURCE_PIXELS {
        return Err(format!("{}x{} is too large", info.width, info.height));
    }
    if info.coding_process != CodingProcess::DctSequential && pixels > MAX_PROGRESSIVE_PIXELS {
        return Err("a large progressive JPEG".into());
    }
    let orientation = decoder.exif_data().map_or(1, exif_orientation);
    let (w, h) = decoder
        .scale(max_px, max_px)
        .map_err(|e| format!("scaling: {e}"))?;
    let raw = decoder.decode().map_err(|e| format!("decoding: {e}"))?;
    let rgb = match info.pixel_format {
        PixelFormat::RGB24 => raw,
        PixelFormat::L8 => raw.iter().flat_map(|&v| [v, v, v]).collect(),
        PixelFormat::L16 => raw
            .chunks_exact(2)
            .flat_map(|v| [v[0], v[0], v[0]])
            .collect(),
        PixelFormat::CMYK32 => raw
            .chunks_exact(4)
            .flat_map(|p| {
                let k = u16::from(255 - p[3]);
                [p[0], p[1], p[2]].map(|c| ((255 - u16::from(c)) * k / 255) as u8)
            })
            .collect(),
    };
    if rgb.len() != usize::from(w) * usize::from(h) * 3 {
        return Err("unexpected decoded size".into());
    }
    let (rgb, w, h) = fit(&rgb, w, h, max_px);
    Ok(orient(rgb, w, h, orientation))
}

/// Box sums for one output row of a PNG being shrunk.
struct Rows {
    channels: usize,
    width: usize,
    height: usize,
    dw: usize,
    dh: usize,
    sums: Vec<u32>,
    counts: Vec<u32>,
    current: usize,
    out: Vec<u8>,
}

impl Rows {
    fn add(&mut self, y: usize, row: &[u8]) {
        let target = y * self.dh / self.height;
        while target > self.current {
            self.flush();
            self.current += 1;
        }
        for (x, px) in row.chunks_exact(self.channels).take(self.width).enumerate() {
            let (r, g, b, a) = match *px {
                [v] => (v, v, v, 255),
                [v, a] => (v, v, v, a),
                [r, g, b] => (r, g, b, 255),
                [r, g, b, a] => (r, g, b, a),
                _ => continue,
            };
            // Transparency laid on white.
            let on_white = |c: u8| (u32::from(c) * u32::from(a) + 255 * (255 - u32::from(a))) / 255;
            let col = x * self.dw / self.width;
            self.sums[col * 3] += on_white(r);
            self.sums[col * 3 + 1] += on_white(g);
            self.sums[col * 3 + 2] += on_white(b);
            self.counts[col] += 1;
        }
    }

    /// Append one averaged output row and start the next.
    fn flush(&mut self) {
        if self.out.len() >= self.dw * self.dh * 3 {
            return;
        }
        for (x, n) in self.counts.iter_mut().enumerate() {
            for c in 0..3 {
                let s = &mut self.sums[x * 3 + c];
                self.out.push(s.checked_div(*n).map_or(255, |v| v as u8));
                *s = 0;
            }
            *n = 0;
        }
    }
}

/// A PNG as RGB, at most `max_px` on its longest side, box-averaged a row at
/// a time while it is read.
fn png_rgb(bytes: &[u8], max_px: u16) -> Result<(Vec<u8>, u16, u16), String> {
    let mut decoder = png::Decoder::new(bytes);
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().map_err(|e| format!("not a PNG: {e}"))?;
    let (w, h, interlaced) = {
        let info = reader.info();
        (info.width, info.height, info.interlaced)
    };
    let pixels = u64::from(w) * u64::from(h);
    if pixels == 0 || pixels > MAX_SOURCE_PIXELS || w > 65_535 || h > 65_535 {
        return Err(format!("{w}x{h} is too large"));
    }
    if interlaced && pixels > MAX_PROGRESSIVE_PIXELS {
        return Err("a large interlaced PNG".into());
    }
    let channels = match reader.output_color_type().0 {
        png::ColorType::Grayscale => 1,
        png::ColorType::GrayscaleAlpha => 2,
        png::ColorType::Rgb => 3,
        png::ColorType::Rgba => 4,
        png::ColorType::Indexed => return Err("an indexed PNG was not expanded".into()),
    };
    let (w, h) = (w as usize, h as usize);
    let longest = w.max(h);
    let max = usize::from(max_px).max(1);
    let (dw, dh) = if longest <= max {
        (w, h)
    } else {
        ((w * max / longest).max(1), (h * max / longest).max(1))
    };
    let mut rows = Rows {
        channels,
        width: w,
        height: h,
        dw,
        dh,
        sums: vec![0; dw * 3],
        counts: vec![0; dw],
        current: 0,
        out: Vec::with_capacity(dw * dh * 3),
    };
    if interlaced {
        let mut frame = vec![0; reader.output_buffer_size()];
        let info = reader
            .next_frame(&mut frame)
            .map_err(|e| format!("decoding: {e}"))?;
        for (y, row) in frame.chunks_exact(info.line_size).take(h).enumerate() {
            rows.add(y, row);
        }
    } else {
        let mut y = 0;
        while let Some(row) = reader.next_row().map_err(|e| format!("decoding: {e}"))? {
            rows.add(y, row.data());
            y += 1;
        }
    }
    rows.flush();
    let mut out = rows.out;
    out.resize(dw * dh * 3, 255);
    Ok((out, dw as u16, dh as u16))
}

/// Box-average an RGB image down so neither side exceeds `max_px`.
fn fit(rgb: &[u8], w: u16, h: u16, max_px: u16) -> (Vec<u8>, u16, u16) {
    let longest = w.max(h);
    if longest <= max_px {
        return (rgb.to_vec(), w, h);
    }
    let (sw, sh) = (usize::from(w), usize::from(h));
    let dw = (sw * usize::from(max_px) / usize::from(longest)).max(1);
    let dh = (sh * usize::from(max_px) / usize::from(longest)).max(1);
    let mut out = Vec::with_capacity(dw * dh * 3);
    for y in 0..dh {
        let (y0, y1) = (y * sh / dh, ((y + 1) * sh / dh).max(y * sh / dh + 1));
        for x in 0..dw {
            let (x0, x1) = (x * sw / dw, ((x + 1) * sw / dw).max(x * sw / dw + 1));
            let mut sum = [0u32; 3];
            for row in y0..y1 {
                for col in x0..x1 {
                    let i = (row * sw + col) * 3;
                    for c in 0..3 {
                        sum[c] += u32::from(rgb[i + c]);
                    }
                }
            }
            let n = ((y1 - y0) * (x1 - x0)) as u32;
            out.extend(sum.map(|s| (s / n) as u8));
        }
    }
    (out, dw as u16, dh as u16)
}

/// Turn the picture the way the camera said: 3 (upside down), 6 (turn
/// right), 8 (turn left). Mirrored orientations are rare and left alone.
fn orient(rgb: Vec<u8>, w: u16, h: u16, orientation: u16) -> (Vec<u8>, u16, u16) {
    let (sw, sh) = (usize::from(w), usize::from(h));
    let px = |x: usize, y: usize| &rgb[(y * sw + x) * 3..(y * sw + x) * 3 + 3];
    match orientation {
        3 => {
            let out = (0..sh)
                .rev()
                .flat_map(|y| (0..sw).rev().map(move |x| (x, y)))
                .flat_map(|(x, y)| px(x, y).to_vec())
                .collect();
            (out, w, h)
        }
        6 => {
            // New row r is old column r, read bottom to top.
            let out = (0..sw)
                .flat_map(|x| (0..sh).rev().map(move |y| (x, y)))
                .flat_map(|(x, y)| px(x, y).to_vec())
                .collect();
            (out, h, w)
        }
        8 => {
            let out = (0..sw)
                .rev()
                .flat_map(|x| (0..sh).map(move |y| (x, y)))
                .flat_map(|(x, y)| px(x, y).to_vec())
                .collect();
            (out, h, w)
        }
        _ => (rgb, w, h),
    }
}

/// The EXIF Orientation tag (0x0112), 1 when absent or unreadable.
fn exif_orientation(exif: &[u8]) -> u16 {
    let tiff = exif.strip_prefix(b"Exif\0\0").unwrap_or(exif);
    let big = match tiff.get(..2) {
        Some(b"MM") => true,
        Some(b"II") => false,
        _ => return 1,
    };
    let u16_at = |i: usize| -> Option<u16> {
        let b: [u8; 2] = tiff.get(i..i + 2)?.try_into().ok()?;
        Some(if big {
            u16::from_be_bytes(b)
        } else {
            u16::from_le_bytes(b)
        })
    };
    let u32_at = |i: usize| -> Option<u32> {
        let b: [u8; 4] = tiff.get(i..i + 4)?.try_into().ok()?;
        Some(if big {
            u32::from_be_bytes(b)
        } else {
            u32::from_le_bytes(b)
        })
    };
    let find = || -> Option<u16> {
        let ifd = u32_at(4)? as usize;
        let count = usize::from(u16_at(ifd)?);
        (0..count.min(64))
            .map(|n| ifd + 2 + n * 12)
            .find(|&entry| u16_at(entry) == Some(0x0112))
            .and_then(|entry| u16_at(entry + 8))
    };
    find().filter(|o| (1..=8).contains(o)).unwrap_or(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn jpeg(w: u16, h: u16) -> Vec<u8> {
        let rgb: Vec<u8> = (0..usize::from(w) * usize::from(h))
            .flat_map(|i| [(i % 251) as u8, 128, 64])
            .collect();
        let mut out = Vec::new();
        jpeg_encoder::Encoder::new(&mut out, 90)
            .encode(&rgb, w, h, jpeg_encoder::ColorType::Rgb)
            .unwrap();
        out
    }

    fn size(jpeg: &[u8]) -> (u16, u16) {
        let mut d = Decoder::new(jpeg);
        d.read_info().unwrap();
        let i = d.info().unwrap();
        (i.width, i.height)
    }

    #[test]
    fn large_images_shrink_to_the_preview_size() {
        let out = preview(&jpeg(2000, 1000), 640, 128 * 1024).unwrap();
        assert_eq!(size(&out), (640, 320));
        let small = preview(&jpeg(300, 200), 640, 128 * 1024).unwrap();
        assert_eq!(size(&small), (300, 200));
    }

    fn png(w: u32, h: u32) -> Vec<u8> {
        let mut out = Vec::new();
        let mut encoder = png::Encoder::new(&mut out, w, h);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().unwrap();
        // Opaque red on the left half, fully transparent on the right.
        let data: Vec<u8> = (0..w * h)
            .flat_map(|i| {
                if i % w < w / 2 {
                    [255, 0, 0, 255]
                } else {
                    [0, 0, 0, 0]
                }
            })
            .collect();
        writer.write_image_data(&data).unwrap();
        drop(writer);
        out
    }

    #[test]
    fn pngs_shrink_row_by_row_onto_white() {
        let (rgb, w, h) = png_rgb(&png(1280, 640), 640).unwrap();
        assert_eq!((w, h), (640, 320));
        assert_eq!(&rgb[..3], &[255, 0, 0], "left: red");
        let right = (319 * 640 + 639) * 3;
        assert_eq!(
            &rgb[right..right + 3],
            &[255, 255, 255],
            "right: transparent on white"
        );
        let (_, w, h) = png_rgb(&png(100, 50), 640).unwrap();
        assert_eq!((w, h), (100, 50), "small ones keep their size");
        let jpeg = preview(&png(1280, 640), 640, 128 * 1024).unwrap();
        assert_eq!(size(&jpeg), (640, 320));
    }

    #[test]
    fn what_is_not_a_jpeg_is_refused() {
        assert!(preview(b"GIF89a....", 640, 1 << 17).is_err());
        assert!(preview(b"\x89PNG\r\n\x1a\n", 640, 1 << 17).is_err());
        assert!(preview(&[], 640, 1 << 17).is_err());
        assert!(preview(&jpeg(64, 64)[..40], 640, 1 << 17).is_err());
    }

    #[test]
    fn turns_pictures_the_right_way_up() {
        // 2x1: red, blue. Turned right it is 1x2: red on top.
        let rgb = vec![255, 0, 0, 0, 0, 255];
        let (out, w, h) = orient(rgb.clone(), 2, 1, 6);
        assert_eq!((w, h), (1, 2));
        assert_eq!(out, vec![255, 0, 0, 0, 0, 255]);
        let (out, w, h) = orient(rgb.clone(), 2, 1, 8);
        assert_eq!((w, h), (1, 2));
        assert_eq!(out, vec![0, 0, 255, 255, 0, 0]);
        let (out, _, _) = orient(rgb, 2, 1, 3);
        assert_eq!(out, vec![0, 0, 255, 255, 0, 0]);
        // Little-endian EXIF with one entry: Orientation = 6.
        let mut exif = b"Exif\0\0II*\0\x08\0\0\0\x01\0".to_vec();
        exif.extend_from_slice(&[0x12, 0x01, 3, 0, 1, 0, 0, 0, 6, 0, 0, 0]);
        assert_eq!(exif_orientation(&exif), 6);
        assert_eq!(exif_orientation(b"junk"), 1);
    }
}
