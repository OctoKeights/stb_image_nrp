//! EXIF orientation parsing (JPEG `APP1` and PNG `eXIf`) and in-place / buffer-rearranging
//! orientation transforms (`1..=8`).

use alloc::vec::Vec;

use super::{Image, try_alloc_elem, vertical_flip};

const PNG_SIG: [u8; 8] = [137, 80, 78, 71, 13, 10, 26, 10];

/// Reads the EXIF `Orientation` tag (`1..=8`) from a JPEG (`APP1`) or PNG (`eXIf`) file, if
/// present and valid.
///
/// Returns `None` if the input is neither JPEG nor PNG, contains no EXIF metadata, has no
/// `Orientation` tag in `IFD0`, or stores a malformed or out-of-range orientation value.
#[must_use]
pub fn exif_orientation(bytes: &[u8]) -> Option<u8> {
    if bytes.starts_with(&PNG_SIG) {
        png_exif_orientation(bytes)
    } else if bytes.first() == Some(&0xFF) {
        jpeg_exif_orientation(bytes)
    } else {
        None
    }
}

fn jpeg_exif_orientation(bytes: &[u8]) -> Option<u8> {
    let mut pos = 0usize;
    // Expect SOI (0xFF, optional 0xFF fill, 0xD8).
    if *bytes.get(pos)? != 0xFF {
        return None;
    }
    while bytes.get(pos) == Some(&0xFF) {
        pos += 1;
    }
    if *bytes.get(pos)? != 0xD8 {
        return None;
    }
    pos += 1;

    while pos < bytes.len() {
        // Advance to next marker prefix 0xFF (skipping any padding bytes between segments).
        while pos < bytes.len() && bytes[pos] != 0xFF {
            pos += 1;
        }
        while bytes.get(pos) == Some(&0xFF) {
            pos += 1;
        }
        let &marker = bytes.get(pos)?;
        pos += 1;

        // End of image or start of entropy-coded scan: metadata headers are finished.
        if marker == 0xD9 || marker == 0xDA {
            return None;
        }
        // Standalone markers without a length field (TEM, RST0..RST7, SOI, stuffed 0x00).
        if marker == 0x00 || marker == 0x01 || (0xD0..=0xD8).contains(&marker) {
            continue;
        }

        let len_hi = *bytes.get(pos)?;
        let len_lo = *bytes.get(pos + 1)?;
        let seg_len = usize::from(u16::from_be_bytes([len_hi, len_lo]));
        if seg_len < 2 {
            return None;
        }
        let payload_start = pos + 2;
        let payload_end = pos.checked_add(seg_len)?;
        let payload = bytes.get(payload_start..payload_end)?;
        pos = payload_end;

        // APP1 with "Exif\0\0" header (skip non-Exif APP1 segments such as XMP).
        if marker == 0xE1
            && let Some(tiff) = payload.strip_prefix(b"Exif\0\0")
            && let Some(orientation) = parse_tiff_orientation(tiff)
        {
            return Some(orientation);
        }
    }
    None
}

fn png_exif_orientation(bytes: &[u8]) -> Option<u8> {
    let mut pos = PNG_SIG.len();
    while pos.checked_add(8)? <= bytes.len() {
        let length = u32::from_be_bytes(bytes[pos..pos + 4].try_into().ok()?) as usize;
        let chunk_type = &bytes[pos + 4..pos + 8];
        if chunk_type == b"IEND" {
            return None;
        }
        let data_start = pos + 8;
        let data_end = data_start.checked_add(length)?;
        let payload = bytes.get(data_start..data_end)?;
        if chunk_type == b"eXIf" {
            // Standard PNG eXIf starts directly with the TIFF header ("II" or "MM"); also
            // tolerate encoders that prepend "Exif\0\0".
            let tiff = payload.strip_prefix(b"Exif\0\0").unwrap_or(payload);
            if let Some(orientation) = parse_tiff_orientation(tiff) {
                return Some(orientation);
            }
        }
        // Skip 4-byte CRC.
        pos = data_end.checked_add(4)?;
    }
    None
}

fn read_u16(buf: &[u8], offset: usize, little_endian: bool) -> Option<u16> {
    let slice: [u8; 2] = buf.get(offset..offset.checked_add(2)?)?.try_into().ok()?;
    Some(if little_endian {
        u16::from_le_bytes(slice)
    } else {
        u16::from_be_bytes(slice)
    })
}

fn read_u32(buf: &[u8], offset: usize, little_endian: bool) -> Option<u32> {
    let slice: [u8; 4] = buf.get(offset..offset.checked_add(4)?)?.try_into().ok()?;
    Some(if little_endian {
        u32::from_le_bytes(slice)
    } else {
        u32::from_be_bytes(slice)
    })
}

fn parse_tiff_orientation(tiff: &[u8]) -> Option<u8> {
    let little_endian = match tiff.get(..2)? {
        b"II" => true,
        b"MM" => false,
        _ => return None,
    };
    if read_u16(tiff, 2, little_endian)? != 0x002A {
        return None;
    }
    let ifd0_offset = read_u32(tiff, 4, little_endian)? as usize;
    let num_entries = usize::from(read_u16(tiff, ifd0_offset, little_endian)?);
    let entries_start = ifd0_offset.checked_add(2)?;

    for i in 0..num_entries {
        let entry = entries_start.checked_add(i.checked_mul(12)?)?;
        // Ensure the full 12-byte IFD entry lies within the TIFF payload.
        tiff.get(entry..entry.checked_add(12)?)?;
        let tag = read_u16(tiff, entry, little_endian)?;
        if tag == 0x0112 {
            let field_type = read_u16(tiff, entry + 2, little_endian)?;
            let count = read_u32(tiff, entry + 4, little_endian)?;
            if count != 1 {
                return None;
            }
            let raw = match field_type {
                // BYTE (1)
                1 => u32::from(*tiff.get(entry + 8)?),
                // SHORT (3): standard EXIF Orientation type
                3 => u32::from(read_u16(tiff, entry + 8, little_endian)?),
                // LONG (4)
                4 => read_u32(tiff, entry + 8, little_endian)?,
                _ => return None,
            };
            return if (1..=8).contains(&raw) {
                Some(raw as u8)
            } else {
                None
            };
        }
    }
    None
}

/// Applies an EXIF orientation (`1..=8`) to `image` in place, updating `width`, `height`, and
/// `data` to bring the image upright. Values outside `2..=8` leave the image unchanged.
pub(crate) fn apply_orientation<T: Copy>(image: &mut Image<T>, orientation: u8) {
    if !(2..=8).contains(&orientation) {
        return;
    }
    let w = image.width as usize;
    let h = image.height as usize;
    let c = image.channels.count();
    let Some(total) = w.checked_mul(h).and_then(|px| px.checked_mul(c)) else {
        return;
    };
    if total == 0 || image.data.len() < total {
        return;
    }

    match orientation {
        // 2: Mirror horizontal
        2 => {
            let row_len = w * c;
            for row in image.data[..total].chunks_exact_mut(row_len) {
                for x in 0..w / 2 {
                    let left = x * c;
                    let right = (w - 1 - x) * c;
                    for k in 0..c {
                        row.swap(left + k, right + k);
                    }
                }
            }
        }
        // 3: Rotate 180° (reverse pixel order)
        3 => {
            let pixels = w * h;
            for idx in 0..pixels / 2 {
                let left = idx * c;
                let right = (pixels - 1 - idx) * c;
                for chan in 0..c {
                    image.data.swap(left + chan, right + chan);
                }
            }
        }
        // 4: Mirror vertical
        4 => {
            vertical_flip(&mut image.data[..total], w, h, c);
        }
        // 5..=8: Transpose / 90° CW / Transverse / 270° CW (swaps width and height)
        5..=8 => {
            let Some(mut out): Option<Vec<T>> = try_alloc_elem(image.data[0], total) else {
                return;
            };
            let new_w = h;
            for y in 0..h {
                for x in 0..w {
                    let (ox, oy) = match orientation {
                        5 => (y, x),                 // Transpose
                        6 => (h - 1 - y, x),         // Rotate 90° CW
                        7 => (h - 1 - y, w - 1 - x), // Transverse
                        _ => (y, w - 1 - x),         // 8: Rotate 270° CW (90° CCW)
                    };
                    let src = (y * w + x) * c;
                    let dst = (oy * new_w + ox) * c;
                    out[dst..dst + c].copy_from_slice(&image.data[src..src + c]);
                }
            }
            image.data = out;
            core::mem::swap(&mut image.width, &mut image.height);
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use alloc::vec;
    use alloc::vec::Vec;

    use crate::{Channels, Image, LoadOptions, exif_orientation, load};
    use miniz_nrp::crc32;
    use stb_image_write_nrp::{
        Channels as WChannels, Image as WImage, WriteOptions, write_jpg, write_png,
    };

    fn make_tiff_orientation(orientation: u16, little_endian: bool) -> Vec<u8> {
        let mut tiff = Vec::new();
        if little_endian {
            tiff.extend_from_slice(b"II");
            tiff.extend_from_slice(&0x002Au16.to_le_bytes());
            tiff.extend_from_slice(&8u32.to_le_bytes()); // IFD0 offset
            tiff.extend_from_slice(&2u16.to_le_bytes()); // 2 entries
            // Dummy tag 0x010F (Make) before 0x0112
            tiff.extend_from_slice(&0x010Fu16.to_le_bytes());
            tiff.extend_from_slice(&2u16.to_le_bytes());
            tiff.extend_from_slice(&4u32.to_le_bytes());
            tiff.extend_from_slice(b"abc\0");
            // Tag 0x0112 (Orientation), SHORT (3), count 1
            tiff.extend_from_slice(&0x0112u16.to_le_bytes());
            tiff.extend_from_slice(&3u16.to_le_bytes());
            tiff.extend_from_slice(&1u32.to_le_bytes());
            tiff.extend_from_slice(&orientation.to_le_bytes());
            tiff.extend_from_slice(&0u16.to_le_bytes());
            tiff.extend_from_slice(&0u32.to_le_bytes()); // next IFD
        } else {
            tiff.extend_from_slice(b"MM");
            tiff.extend_from_slice(&0x002Au16.to_be_bytes());
            tiff.extend_from_slice(&8u32.to_be_bytes());
            tiff.extend_from_slice(&1u16.to_be_bytes());
            tiff.extend_from_slice(&0x0112u16.to_be_bytes());
            tiff.extend_from_slice(&3u16.to_be_bytes());
            tiff.extend_from_slice(&1u32.to_be_bytes());
            tiff.extend_from_slice(&orientation.to_be_bytes());
            tiff.extend_from_slice(&0u16.to_be_bytes());
            tiff.extend_from_slice(&0u32.to_be_bytes());
        }
        tiff
    }

    fn inject_jpeg_exif(jpg: &[u8], orientation: u16, little_endian: bool) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&jpg[..2]); // FFD8
        // Also insert a non-Exif APP1 (XMP) before the Exif APP1 to verify skipping!
        let xmp = b"http://ns.adobe.com/xap/1.0/\0<x/>";
        out.extend_from_slice(&[0xFF, 0xE1]);
        out.extend_from_slice(&((xmp.len() + 2) as u16).to_be_bytes());
        out.extend_from_slice(xmp);

        let tiff = make_tiff_orientation(orientation, little_endian);
        let seg_len = 2 + 6 + tiff.len();
        out.extend_from_slice(&[0xFF, 0xE1]);
        out.extend_from_slice(&(seg_len as u16).to_be_bytes());
        out.extend_from_slice(b"Exif\0\0");
        out.extend_from_slice(&tiff);
        out.extend_from_slice(&jpg[2..]);
        out
    }

    fn inject_png_exif(png: &[u8], orientation: u16, little_endian: bool) -> Vec<u8> {
        let mut out = Vec::new();
        // Insert eXIf right after IHDR (8 sig + 25 IHDR = 33 bytes).
        out.extend_from_slice(&png[..33]);
        let tiff = make_tiff_orientation(orientation, little_endian);
        out.extend_from_slice(&(tiff.len() as u32).to_be_bytes());
        let crc_start = out.len();
        out.extend_from_slice(b"eXIf");
        out.extend_from_slice(&tiff);
        let crc = crc32(&out[crc_start..]);
        out.extend_from_slice(&crc.to_be_bytes());
        out.extend_from_slice(&png[33..]);
        out
    }

    #[test]
    fn parses_jpeg_and_png_exif_orientations_le_and_be() {
        let px = [255u8, 0, 0, 0, 255, 0];
        let wimg = WImage::new(&px, 2, 1, WChannels::Rgb).unwrap();
        let raw_jpg = write_jpg(&wimg, 95, &WriteOptions::default());
        let raw_png = write_png(&wimg, &WriteOptions::default());

        assert_eq!(exif_orientation(&raw_jpg), None);
        assert_eq!(exif_orientation(&raw_png), None);
        assert_eq!(exif_orientation(b"not an image"), None);

        for o in 1..=8u8 {
            let jpg_le = inject_jpeg_exif(&raw_jpg, u16::from(o), true);
            let jpg_be = inject_jpeg_exif(&raw_jpg, u16::from(o), false);
            let png_le = inject_png_exif(&raw_png, u16::from(o), true);
            let png_be = inject_png_exif(&raw_png, u16::from(o), false);
            assert_eq!(exif_orientation(&jpg_le), Some(o));
            assert_eq!(exif_orientation(&jpg_be), Some(o));
            assert_eq!(exif_orientation(&png_le), Some(o));
            assert_eq!(exif_orientation(&png_be), Some(o));
        }

        // Out-of-range orientation values (0 and 9) are rejected as None.
        assert_eq!(exif_orientation(&inject_jpeg_exif(&raw_jpg, 0, true)), None);
        assert_eq!(exif_orientation(&inject_png_exif(&raw_png, 9, false)), None);
    }

    #[test]
    fn applies_all_eight_orientations_and_load_option() {
        // 3x2 Grey image:
        // [1, 2, 3]
        // [4, 5, 6]
        let base = Image {
            width: 3,
            height: 2,
            channels: Channels::Grey,
            channels_in_file: Channels::Grey,
            data: vec![1u8, 2, 3, 4, 5, 6],
        };

        let oriented = |o: u8| {
            let mut img = base.clone();
            img.apply_orientation(o);
            (img.width, img.height, img.data)
        };

        assert_eq!(oriented(1), (3, 2, vec![1, 2, 3, 4, 5, 6]));
        assert_eq!(oriented(2), (3, 2, vec![3, 2, 1, 6, 5, 4]));
        assert_eq!(oriented(3), (3, 2, vec![6, 5, 4, 3, 2, 1]));
        assert_eq!(oriented(4), (3, 2, vec![4, 5, 6, 1, 2, 3]));
        assert_eq!(oriented(5), (2, 3, vec![1, 4, 2, 5, 3, 6]));
        assert_eq!(oriented(6), (2, 3, vec![4, 1, 5, 2, 6, 3]));
        assert_eq!(oriented(7), (2, 3, vec![6, 3, 5, 2, 4, 1]));
        assert_eq!(oriented(8), (2, 3, vec![3, 6, 2, 5, 1, 4]));
        assert_eq!(oriented(99), (3, 2, vec![1, 2, 3, 4, 5, 6]));

        // End-to-end via LoadOptions::with_apply_exif_orientation(true) on a PNG:
        let wimg = WImage::new(&base.data, 3, 2, WChannels::Grey).unwrap();
        let png6 = inject_png_exif(&write_png(&wimg, &WriteOptions::default()), 6, false);

        let unrotated = load(&png6, None, &LoadOptions::default()).unwrap();
        assert_eq!(
            (unrotated.width, unrotated.height, unrotated.data),
            (3, 2, vec![1, 2, 3, 4, 5, 6])
        );

        let rotated = load(
            &png6,
            None,
            &LoadOptions::new().with_apply_exif_orientation(true),
        )
        .unwrap();
        assert_eq!(
            (rotated.width, rotated.height, rotated.data),
            (2, 3, vec![4, 1, 5, 2, 6, 3])
        );
    }
}
