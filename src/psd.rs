//! The PSD reader: `stbi__psd_*`, by Thatcher Ulrich, integration by Nicolas Schulz, tweaked by
//! stb. The merged image of an RGB file only.

use super::{
    Context, ImageType, Info, Pixels, STBI_MAX_DIMENSIONS, convert_format, mad3sizes_valid,
};

/// `stbi__psd_test`
pub(crate) fn psd_test(s: &mut Context) -> bool {
    let r = s.get32be() == 0x38425053;
    s.rewind();
    r
}

/// `stbi__psd_decode_rle`: `pixel_count` values into every fourth byte of `out` from `start`.
fn psd_decode_rle(s: &mut Context, out: &mut [u8], start: usize, pixel_count: i32) -> bool {
    let mut p = start;
    let mut count = 0i32;
    loop {
        let nleft = pixel_count - count;
        if nleft <= 0 {
            break;
        }
        let mut len = i32::from(s.get8());
        if len == 128 {
            // No-op.
        } else if len < 128 {
            // Copy next len+1 bytes literally.
            len += 1;
            if len > nleft {
                return false; // corrupt data
            }
            count += len;
            while len != 0 {
                out[p] = s.get8();
                p += 4;
                len -= 1;
            }
        } else {
            // Next -len+1 bytes in the dest are replicated from next source byte.
            // (Interpret len as a negative 8-bit int.)
            len = 257 - len;
            if len > nleft {
                return false; // corrupt data
            }
            let val = s.get8();
            count += len;
            while len != 0 {
                out[p] = val;
                p += 4;
                len -= 1;
            }
        }
    }
    true
}

/// A float made an integer as x86 does it, as the C does with `(unsigned char)` and
/// `(stbi__uint16)` of a float: to a 32-bit int (0x80000000 for what does not fit), then its low
/// bits.
fn x86_int(value: f32) -> i32 {
    // (Rust's `as` gives the same, but for what is too large above zero, which this never has.)
    value as i32
}

/// `stbi__psd_load`: the pixels, at 16 bits a channel when the file has them uncompressed and
/// `bpc` asks for 16; with them, the channels in the file (always 4). The size goes into the
/// context.
pub(crate) fn psd_load(s: &mut Context, req_comp: i32, bpc: i32) -> Option<(Pixels, i32)> {
    // Check identifier
    if s.get32be() != 0x38425053 {
        // "8BPS"
        return s.errp("not PSD");
    }

    // Check file type version.
    if s.get16be() != 1 {
        return s.errp("wrong version");
    }

    // Skip 6 reserved bytes.
    s.skip(6);

    // Read the number of channels (R, G, B, A, etc).
    let channel_count = s.get16be();
    if !(0..=16).contains(&channel_count) {
        return s.errp("wrong channel count");
    }

    // Read the rows and columns of the image.
    let h = s.get32be() as i32;
    let w = s.get32be() as i32;

    if h > STBI_MAX_DIMENSIONS as i32 {
        return s.errp("too large");
    }
    if w > STBI_MAX_DIMENSIONS as i32 {
        return s.errp("too large");
    }

    // Make sure the depth is 8 bits.
    let bitdepth = s.get16be();
    if bitdepth != 8 && bitdepth != 16 {
        return s.errp("unsupported bit depth");
    }

    // Make sure the color mode is RGB.
    // Valid options are:
    //   0: Bitmap
    //   1: Grayscale
    //   2: Indexed color
    //   3: RGB color
    //   4: CMYK color
    //   7: Multichannel
    //   8: Duotone
    //   9: Lab color
    if s.get16be() != 3 {
        return s.errp("wrong color format");
    }

    // Skip the Mode Data.  (It's the palette for indexed color; other info for other modes.)
    let n = s.get32be() as i32;
    s.skip(n);

    // Skip the image resources.  (resolution, pen tool paths, etc)
    let n = s.get32be() as i32;
    s.skip(n);

    // Skip the reserved data.
    let n = s.get32be() as i32;
    s.skip(n);

    // Find out if the data is compressed.
    // Known values:
    //   0: no compression
    //   1: RLE compressed
    let compression = s.get16be();
    if compression > 1 {
        return s.errp("bad compression");
    }

    // Check size
    if !mad3sizes_valid(4, w, h, 0) {
        return s.errp("too large");
    }

    // Create the destination image.
    let sixteen = compression == 0 && bitdepth == 16 && bpc == 16;
    if sixteen && !mad3sizes_valid(8, w, h, 0) {
        return s.errp("outofmem");
    }
    let pixel_count = w * h;
    let pixels = pixel_count as usize;
    let mut out = vec![0u8; if sixteen { 0 } else { 4 * pixels }];
    let mut out16 = vec![0u16; if sixteen { 4 * pixels } else { 0 }];

    // Finally, the image data.
    if compression != 0 {
        // RLE as used by .PSD and .TIFF
        // Loop until you get the number of unpacked bytes you are expecting:
        //     Read the next source byte into n.
        //     If n is between 0 and 127 inclusive, copy the next n+1 bytes literally.
        //     Else if n is between -127 and -1 inclusive, copy the next byte -n+1 times.
        //     Else if n is 128, noop.
        // Endloop

        // The RLE-compressed data is preceded by a 2-byte data count for each row in the data,
        // which we're going to just skip.
        s.skip(h.wrapping_mul(channel_count).wrapping_mul(2));

        // Read the RLE data by channel.
        for channel in 0..4 {
            if channel >= channel_count {
                // Fill this channel with default data.
                let value = if channel == 3 { 255 } else { 0 };
                for p in out.iter_mut().skip(channel as usize).step_by(4) {
                    *p = value;
                }
            } else {
                // Read the RLE data.
                if !psd_decode_rle(s, &mut out, channel as usize, pixel_count) {
                    return s.errp("corrupt");
                }
            }
        }
    } else {
        // We're at the raw image data.  It's each channel in order (Red, Green, Blue, Alpha, ...)
        // where each channel consists of an 8-bit (or 16-bit) value for each pixel in the image.

        // Read the data by channel.
        for channel in 0..4 {
            if channel >= channel_count {
                // Fill this channel with default data.
                if bitdepth == 16 && bpc == 16 {
                    let value = if channel == 3 { 65535 } else { 0 };
                    for q in out16.iter_mut().skip(channel as usize).step_by(4) {
                        *q = value;
                    }
                } else {
                    let value = if channel == 3 { 255 } else { 0 };
                    for p in out.iter_mut().skip(channel as usize).step_by(4) {
                        *p = value;
                    }
                }
            } else if sixteen {
                // output bpc
                for i in 0..pixels {
                    out16[channel as usize + 4 * i] = s.get16be() as u16;
                }
            } else if bitdepth == 16 {
                // input bpc
                for i in 0..pixels {
                    out[channel as usize + 4 * i] = (s.get16be() >> 8) as u8;
                }
            } else {
                for i in 0..pixels {
                    out[channel as usize + 4 * i] = s.get8();
                }
            }
        }
    }

    // remove weird white matte from PSD
    if channel_count >= 4 {
        if sixteen {
            for pixel in out16.as_chunks_mut::<4>().0.iter_mut() {
                if pixel[3] != 0 && pixel[3] != 65535 {
                    let a = f32::from(pixel[3]) / 65535.0;
                    let ra = 1.0f32 / a;
                    let inv_a = 65535.0f32 * (1.0 - ra);
                    for value in &mut pixel[..3] {
                        *value = x86_int(f32::from(*value) * ra + inv_a) as u16;
                    }
                }
            }
        } else {
            for pixel in out.as_chunks_mut::<4>().0.iter_mut() {
                if pixel[3] != 0 && pixel[3] != 255 {
                    let a = f32::from(pixel[3]) / 255.0;
                    let ra = 1.0f32 / a;
                    let inv_a = 255.0f32 * (1.0 - ra);
                    for value in &mut pixel[..3] {
                        *value = x86_int(f32::from(*value) * ra + inv_a) as u8;
                    }
                }
            }
        }
    }

    // convert to desired output format
    let result = if sixteen {
        if req_comp != 0 && req_comp != 4 {
            out16 = convert_format(s, out16, 4, req_comp, w as u32, h as u32)?;
        }
        Pixels::U16(out16)
    } else {
        if req_comp != 0 && req_comp != 4 {
            out = convert_format(s, out, 4, req_comp, w as u32, h as u32)?;
        }
        Pixels::U8(out)
    };

    s.img_x = w as u32;
    s.img_y = h as u32;
    Some((result, 4))
}

/// `stbi__psd_info`
pub(crate) fn psd_info(s: &mut Context) -> Option<Info> {
    if s.get32be() != 0x38425053 {
        s.rewind();
        return None;
    }
    if s.get16be() != 1 {
        s.rewind();
        return None;
    }
    s.skip(6);
    let channel_count = s.get16be();
    if !(0..=16).contains(&channel_count) {
        s.rewind();
        return None;
    }
    let y = s.get32be() as i32;
    let x = s.get32be() as i32;
    let depth = s.get16be();
    if depth != 8 && depth != 16 {
        s.rewind();
        return None;
    }
    if s.get16be() != 3 {
        s.rewind();
        return None;
    }
    Some(Info {
        kind: ImageType::Psd,
        x,
        y,
        comp: 4,
        is_16_bit: depth == 16,
        is_hdr: false,
        is_iphone_png: false,
    })
}

#[cfg(test)]
mod tests {
    use super::super::{LoadOptions, load_16_from_memory, load_from_memory};

    /// A PSD file of RGB, without the mode data, resources or reserved data.
    fn psd(channels: u16, w: u32, h: u32, depth: u16, compression: u16, data: &[u8]) -> Vec<u8> {
        let mut file = b"8BPS".to_vec();
        file.extend_from_slice(&1u16.to_be_bytes());
        file.extend_from_slice(&[0; 6]);
        file.extend_from_slice(&channels.to_be_bytes());
        file.extend_from_slice(&h.to_be_bytes());
        file.extend_from_slice(&w.to_be_bytes());
        file.extend_from_slice(&depth.to_be_bytes());
        file.extend_from_slice(&3u16.to_be_bytes());
        file.extend_from_slice(&[0; 12]);
        file.extend_from_slice(&compression.to_be_bytes());
        file.extend_from_slice(data);
        file
    }

    #[test]
    fn channels_are_planes() {
        // Two pixels, three channels, one plane after another; alpha made opaque.
        let file = psd(3, 2, 1, 8, 0, &[10, 20, 30, 40, 50, 60]);
        let image = load_from_memory(&file, 0, &LoadOptions::default()).unwrap();
        assert_eq!((image.x, image.y, image.channels_in_file), (2, 1, 4));
        assert_eq!(image.data, [10, 30, 50, 255, 20, 40, 60, 255]);
    }

    #[test]
    fn sixteen_bits_stay_sixteen() {
        let file = psd(1, 1, 1, 16, 0, &[0x12, 0x34]);
        let image = load_16_from_memory(&file, 1, &LoadOptions::default()).unwrap();
        assert_eq!(image.data, [((0x1234u32 * 77) >> 8) as u16]);
        let image = load_16_from_memory(&file, 4, &LoadOptions::default()).unwrap();
        assert_eq!(image.data, [0x1234, 0, 0, 0xffff]);
    }

    #[test]
    fn run_length_encoding_is_read_by_channel() {
        // Four pixels of one channel: a run of three, then one literal; the row counts first.
        let file = psd(1, 4, 1, 8, 1, &[0, 0, 254, 7, 0, 9]);
        let image = load_from_memory(&file, 1, &LoadOptions::default()).unwrap();
        let (seven, nine) = (((7u32 * 77) >> 8) as u8, ((9u32 * 77) >> 8) as u8);
        assert_eq!(image.data, [seven, seven, seven, nine]);
    }
}
