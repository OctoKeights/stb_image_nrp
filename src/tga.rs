//! The TGA reader: `stbi__tga_*`, Targa Truevision, by Jonathan Dummer.

use super::{
    Context, ImageType, Info, STBI_MAX_DIMENSIONS, convert_format, mad3sizes_valid, vertical_flip,
};

/// `stbi__tga_get_comp`: returns STBI_rgb or whatever, 0 on error; with it, whether the pixels
/// are 15 or 16 bits of RGB.
fn tga_get_comp(bits_per_pixel: i32, is_grey: bool) -> (i32, bool) {
    // only RGB or RGBA (incl. 16bit) or grey allowed
    if is_grey && bits_per_pixel == 16 {
        return (2, false);
    }
    if bits_per_pixel == 15 || bits_per_pixel == 16 {
        return (3, true);
    }
    let comp = match bits_per_pixel {
        8 => 1,
        24 => 3,
        32 => 4,
        _ => 0,
    };
    (comp, false)
}

/// `stbi__tga_info_impl`: the width, height and channels, or None.
fn tga_info_impl(s: &mut Context) -> Option<(i32, i32, i32)> {
    s.get8(); // discard Offset
    let tga_colormap_type = s.get8(); // colormap type
    if tga_colormap_type > 1 {
        return None; // only RGB/indexed allowed
    }
    let tga_image_type = s.get8(); // image type
    let tga_colormap_bpp = if tga_colormap_type == 1 {
        // colormapped (paletted) image
        if tga_image_type != 1 && tga_image_type != 9 {
            return None;
        }
        s.skip(4); // skip index of first colormap entry and number of entries
        let sz = s.get8(); //   check bits per palette color entry
        if sz != 8 && sz != 15 && sz != 16 && sz != 24 && sz != 32 {
            return None;
        }
        s.skip(4); // skip image x and y origin
        i32::from(sz)
    } else {
        // "normal" image w/o colormap - only RGB or grey allowed, +/- RLE
        if tga_image_type != 2
            && tga_image_type != 3
            && tga_image_type != 10
            && tga_image_type != 11
        {
            return None; // only RGB or grey allowed, +/- RLE
        }
        s.skip(9); // skip colormap specification and image x/y origin
        0
    };
    let tga_w = s.get16le();
    let tga_h = s.get16le();
    if tga_w < 1 || tga_h < 1 {
        return None;
    }
    let tga_bits_per_pixel = i32::from(s.get8()); // bits per pixel
    s.get8(); // ignore alpha bits
    let tga_comp = if tga_colormap_bpp != 0 {
        // only support 8- and 16-bit indexes for colormaps
        if tga_bits_per_pixel != 8 && tga_bits_per_pixel != 16 {
            return None;
        }
        tga_get_comp(tga_colormap_bpp, false).0
    } else {
        tga_get_comp(
            tga_bits_per_pixel,
            tga_image_type == 3 || tga_image_type == 11,
        )
        .0
    };
    if tga_comp == 0 {
        return None;
    }
    Some((tga_w, tga_h, tga_comp)) // seems to have passed everything
}

/// `stbi__tga_info`
pub(crate) fn tga_info(s: &mut Context) -> Option<Info> {
    let Some((x, y, comp)) = tga_info_impl(s) else {
        s.rewind();
        return None;
    };
    Some(Info {
        kind: ImageType::Tga,
        x,
        y,
        comp,
        is_16_bit: false,
        is_hdr: false,
        is_iphone_png: false,
    })
}

/// `stbi__tga_test`
pub(crate) fn tga_test(s: &mut Context) -> bool {
    let res = tga_info_impl(s).is_some();
    s.rewind();
    res
}

/// `stbi__tga_read_rgb16`: read 16bit value and convert to 24bit RGB
fn tga_read_rgb16(s: &mut Context, out: &mut [u8]) {
    let px = s.get16le() as u16;
    let five_bit_mask = 31u16;
    // we have 3 channels with 5bits each
    let r = i32::from((px >> 10) & five_bit_mask);
    let g = i32::from((px >> 5) & five_bit_mask);
    let b = i32::from(px & five_bit_mask);
    // Note that this saves the data in RGB(A) order, so it doesn't need to be swapped later
    out[0] = ((r * 255) / 31) as u8;
    out[1] = ((g * 255) / 31) as u8;
    out[2] = ((b * 255) / 31) as u8;

    // some people claim that the most significant bit might be used for alpha
    // (possibly if an alpha-bit is set in the "image descriptor byte")
    // but that only made 16bit test images completely translucent..
    // so let's treat all 15 and 16bit TGAs as RGB with no alpha.
}

/// `stbi__tga_load`: the pixels and the channels in the file; the size goes into the context.
pub(crate) fn tga_load(s: &mut Context, req_comp: i32) -> Option<(Vec<u8>, i32)> {
    // read in the TGA header
    let tga_offset = i32::from(s.get8());
    let tga_indexed = s.get8() != 0;
    let mut tga_image_type = i32::from(s.get8());
    let mut tga_is_rle = false;
    let tga_palette_start = s.get16le();
    let tga_palette_len = s.get16le();
    let tga_palette_bits = i32::from(s.get8());
    let _tga_x_origin = s.get16le();
    let _tga_y_origin = s.get16le();
    let tga_width = s.get16le();
    let tga_height = s.get16le();
    let tga_bits_per_pixel = i32::from(s.get8());
    let tga_desc = s.get8();

    if tga_width > STBI_MAX_DIMENSIONS as i32 || tga_height > STBI_MAX_DIMENSIONS as i32 {
        return s.errp("too large");
    }
    if tga_width == 0 || tga_height == 0 {
        return s.errp("zero size");
    }

    //   do a tiny bit of precessing
    if tga_image_type >= 8 {
        tga_image_type -= 8;
        tga_is_rle = true;
    }

    //   If I'm paletted, then I'll use the number of bits from the palette
    let (tga_comp, tga_rgb16) = if tga_indexed {
        tga_get_comp(tga_palette_bits, false)
    } else {
        tga_get_comp(tga_bits_per_pixel, tga_image_type == 3)
    };

    if tga_comp == 0 {
        // shouldn't really happen, stbi__tga_test() should have ensured basic consistency
        return s.errp("bad format");
    }

    //   tga info
    s.img_x = tga_width as u32;
    s.img_y = tga_height as u32;

    if !mad3sizes_valid(tga_width, tga_height, tga_comp, 0) {
        return s.errp("too large");
    }

    let (width, height, comp) = (tga_width as usize, tga_height as usize, tga_comp as usize);
    let mut tga_data = vec![0u8; width * height * comp];

    // skip to the data's starting position (offset usually = 0)
    s.skip(tga_offset);

    if !tga_indexed && !tga_is_rle && !tga_rgb16 {
        for row in tga_data.chunks_exact_mut(width * comp) {
            if !s.getn_into(row) {
                return s.errp("premature EOF");
            }
        }
    } else {
        //   do I need to load a palette?
        let mut tga_palette = Vec::new();
        if tga_indexed {
            if tga_palette_len == 0 {
                /* you have to have at least one entry! */
                return s.errp("bad palette");
            }

            s.skip(tga_palette_start);
            tga_palette = vec![0u8; tga_palette_len as usize * comp];
            if tga_rgb16 {
                debug_assert_eq!(comp, 3);
                for entry in tga_palette.chunks_exact_mut(comp) {
                    tga_read_rgb16(s, entry);
                }
            } else if !s.getn_into(&mut tga_palette) {
                return s.errp("bad palette");
            }
        }
        //   load the data
        let mut raw_data = [0u8; 4];
        let mut rle_count = 0i32;
        let mut rle_repeating = 0i32;
        let mut read_next_pixel = true;
        for i in 0..width * height {
            //   if I'm in RLE mode, do I need to get a RLE stbi__pngchunk?
            if tga_is_rle {
                if rle_count == 0 {
                    //   yep, get the next byte as a RLE command
                    let rle_cmd = i32::from(s.get8());
                    rle_count = 1 + (rle_cmd & 127);
                    rle_repeating = rle_cmd >> 7;
                    read_next_pixel = true;
                } else if rle_repeating == 0 {
                    read_next_pixel = true;
                }
            } else {
                read_next_pixel = true;
            }
            //   OK, if I need to read a pixel, do it now
            if read_next_pixel {
                //   load however much data we did have
                if tga_indexed {
                    // read in index, then perform the lookup
                    let mut pal_idx = if tga_bits_per_pixel == 8 {
                        i32::from(s.get8())
                    } else {
                        s.get16le()
                    };
                    if pal_idx >= tga_palette_len {
                        // invalid index
                        pal_idx = 0;
                    }
                    let at = pal_idx as usize * comp;
                    raw_data[..comp].copy_from_slice(&tga_palette[at..at + comp]);
                } else if tga_rgb16 {
                    debug_assert_eq!(comp, 3);
                    tga_read_rgb16(s, &mut raw_data);
                } else {
                    //   read in the data raw
                    for value in &mut raw_data[..comp] {
                        *value = s.get8();
                    }
                }
                //   clear the reading flag for the next pixel
                read_next_pixel = false;
            } // end of reading a pixel

            // copy data
            tga_data[i * comp..(i + 1) * comp].copy_from_slice(&raw_data[..comp]);

            //   in case we're in RLE mode, keep counting down
            rle_count -= 1;
        }
    }

    // horizontal flip if required
    if tga_desc & 0x10 != 0 {
        for row in tga_data.chunks_exact_mut(width * comp) {
            for j in 0..width / 2 {
                for k in 0..comp {
                    row.swap(j * comp + k, (width - 1 - j) * comp + k);
                }
            }
        }
    }

    // vertical flip if required
    if tga_desc & 0x20 == 0 {
        vertical_flip(&mut tga_data, width, height, comp);
    }

    // swap RGB - if the source data was RGB16, it already is in the right order
    if comp >= 3 && !tga_rgb16 {
        for pixel in tga_data.chunks_exact_mut(comp) {
            pixel.swap(0, 2);
        }
    }

    // convert to target component count
    if req_comp != 0 && req_comp != tga_comp {
        tga_data = convert_format(
            s,
            tga_data,
            tga_comp,
            req_comp,
            tga_width as u32,
            tga_height as u32,
        )?;
    }

    //   OK, done
    Some((tga_data, tga_comp))
}

#[cfg(test)]
mod tests {
    use super::super::{LoadOptions, load_from_memory};

    /// A TGA file of uncompressed true colour: the header, then the pixels.
    fn tga(image_type: u8, w: u16, h: u16, bits: u8, desc: u8, pixels: &[u8]) -> Vec<u8> {
        let mut file = vec![0, 0, image_type, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        file.extend_from_slice(&w.to_le_bytes());
        file.extend_from_slice(&h.to_le_bytes());
        file.extend_from_slice(&[bits, desc]);
        file.extend_from_slice(pixels);
        file
    }

    #[test]
    fn rows_come_bottom_first_and_blue_first() {
        // Two by two, BGR, stored bottom row first (the descriptor's bit 5 clear).
        let file = tga(2, 2, 2, 24, 0, &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12]);
        let image = load_from_memory(&file, 0, &LoadOptions::default()).unwrap();
        assert_eq!((image.x, image.y, image.channels_in_file), (2, 2, 3));
        assert_eq!(image.data, [9, 8, 7, 12, 11, 10, 3, 2, 1, 6, 5, 4]);
    }

    #[test]
    fn run_length_encoding_repeats_a_pixel() {
        // Grey, run-length encoded: a run of three, then one raw pixel.
        let file = tga(11, 4, 1, 8, 0x20, &[0x82, 50, 0x00, 60]);
        let image = load_from_memory(&file, 0, &LoadOptions::default()).unwrap();
        assert_eq!(image.data, [50, 50, 50, 60]);
    }
}
