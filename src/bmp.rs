//! The BMP reader of `stb_image.h`: Microsoft/Windows BMP image.

use super::{
    Context, ImageType, Info, STBI_MAX_DIMENSIONS, convert_format, mad3sizes_valid, vertical_flip,
};

/// `stbi__bmp_test_raw`
fn bmp_test_raw(s: &mut Context) -> bool {
    if s.get8() != b'B' {
        return false;
    }
    if s.get8() != b'M' {
        return false;
    }
    s.get32le(); // discard filesize
    s.get16le(); // discard reserved
    s.get16le(); // discard reserved
    s.get32le(); // discard data offset
    let sz = s.get32le();
    sz == 12 || sz == 40 || sz == 56 || sz == 108 || sz == 124
}

/// `stbi__bmp_test`
pub(crate) fn bmp_test(s: &mut Context) -> bool {
    let r = bmp_test_raw(s);
    s.rewind();
    r
}

/// `stbi__high_bit`: returns 0..31 for the highest set bit
fn high_bit(mut z: u32) -> i32 {
    let mut n = 0;
    if z == 0 {
        return -1;
    }
    if z >= 0x10000 {
        n += 16;
        z >>= 16;
    }
    if z >= 0x00100 {
        n += 8;
        z >>= 8;
    }
    if z >= 0x00010 {
        n += 4;
        z >>= 4;
    }
    if z >= 0x00004 {
        n += 2;
        z >>= 2;
    }
    if z >= 0x00002 {
        n += 1; /* >>=  1;*/
    }
    n
}

/// `stbi__bitcount`
fn bitcount(mut a: u32) -> i32 {
    a = (a & 0x55555555) + ((a >> 1) & 0x55555555); // max 2
    a = (a & 0x33333333) + ((a >> 2) & 0x33333333); // max 4
    a = (a + (a >> 4)) & 0x0f0f0f0f; // max 8 per 4, now 8 bits
    a = a.wrapping_add(a >> 8); // max 16 per 8 bits
    a = a.wrapping_add(a >> 16); // max 32 per 8 bits
    (a & 0xff) as i32
}

/// `stbi__shiftsigned`: extract an arbitrarily-aligned N-bit value (N=bits) from v, and then
/// make it 8-bits long and fractionally extend it to full full range.
fn shiftsigned(mut v: u32, shift: i32, bits: i32) -> i32 {
    static MUL_TABLE: [u32; 9] = [
        0, 0xff, /*0b11111111*/
        0x55, /*0b01010101*/
        0x49, /*0b01001001*/
        0x11, /*0b00010001*/
        0x21, /*0b00100001*/
        0x41, /*0b01000001*/
        0x81, /*0b10000001*/
        0x01, /*0b00000001*/
    ];
    static SHIFT_TABLE: [u32; 9] = [0, 0, 0, 1, 0, 2, 4, 6, 0];
    if shift < 0 {
        v = v.wrapping_shl(shift.unsigned_abs());
    } else {
        v = v.wrapping_shr(shift as u32);
    }
    debug_assert!(v < 256);
    v >>= 8 - bits;
    debug_assert!((0..=8).contains(&bits));
    (v.wrapping_mul(MUL_TABLE[bits as usize]) as i32) >> SHIFT_TABLE[bits as usize]
}

/// `stbi__bmp_data`
#[derive(Default)]
struct BmpData {
    bpp: i32,
    offset: i32,
    hsz: i32,
    mr: u32,
    mg: u32,
    mb: u32,
    ma: u32,
    all_a: u32,
    extra_read: i32,
}

/// `stbi__bmp_set_mask_defaults`
fn bmp_set_mask_defaults(info: &mut BmpData, compress: i32) -> bool {
    // BI_BITFIELDS specifies masks explicitly, don't override
    if compress == 3 {
        return true;
    }

    if compress == 0 {
        if info.bpp == 16 {
            info.mr = 31 << 10;
            info.mg = 31 << 5;
            info.mb = 31;
            info.ma = 0;
        } else if info.bpp == 32 {
            info.mr = 0xff << 16;
            info.mg = 0xff << 8;
            info.mb = 0xff;
            info.ma = 0; // BI_BITFIELDS BMPs can have alpha, but BI_RGB ones never do
            info.all_a = 0; // if all_a is 0 at end, then we loaded alpha channel but it was all 0
        } else {
            // otherwise, use defaults, which is all-0
            info.mr = 0;
            info.mg = 0;
            info.mb = 0;
            info.ma = 0;
        }
        return true;
    }
    false // error
}

/// `stbi__bmp_parse_header`
fn bmp_parse_header(s: &mut Context, info: &mut BmpData) -> bool {
    if s.get8() != b'B' || s.get8() != b'M' {
        return s.err("not BMP");
    }
    s.get32le(); // discard filesize
    s.get16le(); // discard reserved
    s.get16le(); // discard reserved
    info.offset = s.get32le() as i32;
    let hsz = s.get32le() as i32;
    info.hsz = hsz;
    info.mr = 0;
    info.mg = 0;
    info.mb = 0;
    info.ma = 0;
    info.extra_read = 14;

    if info.offset < 0 {
        return s.err("bad BMP");
    }

    if hsz != 12 && hsz != 40 && hsz != 56 && hsz != 108 && hsz != 124 {
        return s.err("unknown BMP");
    }
    if hsz == 12 {
        s.img_x = s.get16le() as u32;
        s.img_y = s.get16le() as u32;
    } else {
        s.img_x = s.get32le();
        s.img_y = s.get32le();
    }
    if s.get16le() != 1 {
        return s.err("bad BMP");
    }
    info.bpp = s.get16le();
    if hsz != 12 {
        let compress = s.get32le() as i32;
        if compress == 1 || compress == 2 {
            return s.err("BMP RLE");
        }
        if compress >= 4 {
            return s.err("BMP JPEG/PNG"); // this includes PNG/JPEG modes
        }
        if compress == 3 && info.bpp != 16 && info.bpp != 32 {
            return s.err("bad BMP"); // bitfields requires 16 or 32 bits/pixel
        }
        s.get32le(); // discard sizeof
        s.get32le(); // discard hres
        s.get32le(); // discard vres
        s.get32le(); // discard colorsused
        s.get32le(); // discard max important
        if hsz == 40 {
            // BITMAPINFOHEADER
            if info.bpp == 16 || info.bpp == 32 {
                if compress == 0 {
                    bmp_set_mask_defaults(info, compress);
                } else if compress == 3 {
                    info.mr = s.get32le();
                    info.mg = s.get32le();
                    info.mb = s.get32le();
                    info.extra_read += 12;
                    // not documented, but generated by photoshop and handled by mspaint
                    if info.mr == info.mg && info.mg == info.mb {
                        // ?!?!?
                        return s.err("bad BMP");
                    }
                } else {
                    return s.err("bad BMP");
                }
            }
        } else {
            // V3/V4/V5 header
            if hsz != 56 && hsz != 108 && hsz != 124 {
                return s.err("bad BMP");
            }
            info.mr = s.get32le();
            info.mg = s.get32le();
            info.mb = s.get32le();
            info.ma = s.get32le();
            if compress != 3 {
                // override mr/mg/mb unless in BI_BITFIELDS mode, as per docs
                bmp_set_mask_defaults(info, compress);
            }
            if hsz >= 108 {
                s.get32le(); // discard color space
                for _ in 0..12 {
                    s.get32le(); // discard color space parameters
                }
            }
            if hsz == 124 {
                s.get32le(); // discard rendering intent
                s.get32le(); // discard offset of profile data
                s.get32le(); // discard size of profile data
                s.get32le(); // discard reserved
            }
        }
    }
    true
}

/// `stbi__bmp_load`: the pixels and how many channels the file has.
pub(crate) fn bmp_load(s: &mut Context, req_comp: i32) -> Option<(Vec<u8>, i32)> {
    let mut pal = [[0u8; 4]; 256];
    let mut psize = 0i32;
    let mut info = BmpData {
        all_a: 255,
        ..BmpData::default()
    };

    if !bmp_parse_header(s, &mut info) {
        return None; // error code already set
    }

    let flip_vertically = (s.img_y as i32) > 0;
    s.img_y = (s.img_y as i32).wrapping_abs() as u32;

    if s.img_y > STBI_MAX_DIMENSIONS {
        return s.errp("too large");
    }
    if s.img_x > STBI_MAX_DIMENSIONS {
        return s.errp("too large");
    }

    let (mr, mg, mb) = (info.mr, info.mg, info.mb);
    let mut ma = info.ma;
    let mut all_a = info.all_a;
    let bytes_read_so_far = info.hsz + info.extra_read;

    if info.offset < bytes_read_so_far {
        return s.errp("bad header");
    }

    if info.hsz == 12 {
        if info.bpp < 24 {
            psize = (info.offset - bytes_read_so_far) / 3;
        }
    } else if info.bpp < 16 {
        psize = (info.offset - bytes_read_so_far) >> 2;
    }
    if psize == 0 {
        // accept some number of extra bytes after the header, but if the offset points either
        // to before the header ends or implies a large amount of extra data, reject the file as
        // malformed

        // max we actually read is below 256 bytes currently.
        let header_limit = 1024;
        // what ordinarily goes here is a palette; 256 entries*4 bytes is its max size.
        let extra_data_limit = 256 * 4;
        if bytes_read_so_far <= 0 || bytes_read_so_far > header_limit {
            return s.errp("bad header");
        }
        // we established that bytes_read_so_far is positive and sensible.
        // the first half of this test rejects offsets that are either too small positives, or
        // negative, and guarantees that info.offset >= bytes_read_so_far > 0. this in turn
        // ensures the number computed in the second half of the test can't overflow.
        if info.offset < bytes_read_so_far || info.offset - bytes_read_so_far > extra_data_limit {
            return s.errp("bad offset");
        } else {
            s.skip(info.offset - bytes_read_so_far);
        }
    }

    if info.bpp == 24 && ma == 0xff000000 {
        s.img_n = 3;
    } else {
        s.img_n = if ma != 0 { 4 } else { 3 };
    }
    let target = if req_comp != 0 && req_comp >= 3 {
        // we can directly decode 3 or 4
        req_comp
    } else {
        s.img_n // if they want monochrome, we'll post-convert
    };

    // sanity-check size
    if !mad3sizes_valid(target, s.img_x as i32, s.img_y as i32, 0) {
        return s.errp("too large");
    }

    // (The original makes room for the pixels here, and so do we below, once it is known
    // that nothing in the header refuses the file. What is not written stays zero.)
    let (img_x, img_y) = (s.img_x as i32, s.img_y as i32);
    let size = target as usize * img_x as usize * img_y as usize;
    let mut out;
    let mut z = 0usize;
    if info.bpp < 16 {
        if psize == 0 || psize > 256 {
            return s.errp("invalid");
        }
        for entry in pal.iter_mut().take(psize as usize) {
            entry[2] = s.get8();
            entry[1] = s.get8();
            entry[0] = s.get8();
            if info.hsz != 12 {
                s.get8();
            }
            entry[3] = 255;
        }
        s.skip(
            info.offset
                .wrapping_sub(info.extra_read)
                .wrapping_sub(info.hsz)
                .wrapping_sub(psize * (if info.hsz == 12 { 3 } else { 4 })),
        );
        let width: i32 = if info.bpp == 1 {
            ((s.img_x + 7) >> 3) as i32
        } else if info.bpp == 4 {
            ((s.img_x + 1) >> 1) as i32
        } else if info.bpp == 8 {
            s.img_x as i32
        } else {
            return s.errp("bad bpp");
        };
        let pad = (-width) & 3;
        out = vec![0u8; size];
        if info.bpp == 1 {
            for _ in 0..img_y {
                let mut bit_offset = 7;
                let mut v = i32::from(s.get8());
                for i in 0..img_x {
                    let color = ((v >> bit_offset) & 0x1) as usize;
                    out[z..z + 3].copy_from_slice(&pal[color][..3]);
                    z += 3;
                    if target == 4 {
                        out[z] = 255;
                        z += 1;
                    }
                    if i + 1 == img_x {
                        break;
                    }
                    bit_offset -= 1;
                    if bit_offset < 0 {
                        bit_offset = 7;
                        v = i32::from(s.get8());
                    }
                }
                s.skip(pad);
            }
        } else {
            for _ in 0..img_y {
                let mut i = 0;
                while i < img_x {
                    let mut v = usize::from(s.get8());
                    let mut v2 = 0;
                    if info.bpp == 4 {
                        v2 = v & 15;
                        v >>= 4;
                    }
                    out[z..z + 3].copy_from_slice(&pal[v][..3]);
                    z += 3;
                    if target == 4 {
                        out[z] = 255;
                        z += 1;
                    }
                    if i + 1 == img_x {
                        break;
                    }
                    v = if info.bpp == 8 {
                        usize::from(s.get8())
                    } else {
                        v2
                    };
                    out[z..z + 3].copy_from_slice(&pal[v][..3]);
                    z += 3;
                    if target == 4 {
                        out[z] = 255;
                        z += 1;
                    }
                    i += 2;
                }
                s.skip(pad);
            }
        }
    } else {
        let (
            mut rshift,
            mut gshift,
            mut bshift,
            mut ashift,
            mut rcount,
            mut gcount,
            mut bcount,
            mut acount,
        ) = (0, 0, 0, 0, 0, 0, 0, 0);
        let mut easy = 0;
        let mut alpha_override = 0u8;
        // psize is 0, already skipped to line data above
        let width: i32 = if info.bpp == 24 {
            3i32.wrapping_mul(img_x)
        } else if info.bpp == 16 {
            2i32.wrapping_mul(img_x)
        } else {
            /* bpp = 32 and pad = 0 */
            0
        };
        let pad = width.wrapping_neg() & 3;
        if info.bpp == 24 {
            easy = 1;
        } else if info.bpp == 32 {
            if info.hsz < 56 {
                // before v3 header, no real alpha support, so treat alpha as not present
                ma = 0;
            }
            if mb == 0xff && mg == 0xff00 && mr == 0x00ff0000 && (ma == 0xff000000 || ma == 0) {
                easy = 2;
                alpha_override = if ma != 0 { 0 } else { 255 };
            }
        }
        if easy == 0 {
            if mr == 0 || mg == 0 || mb == 0 {
                return s.errp("bad masks");
            }
            // right shift amt to put high bit in position #7
            rshift = high_bit(mr) - 7;
            rcount = bitcount(mr);
            gshift = high_bit(mg) - 7;
            gcount = bitcount(mg);
            bshift = high_bit(mb) - 7;
            bcount = bitcount(mb);
            ashift = high_bit(ma) - 7;
            acount = bitcount(ma);
            if rcount > 8 || gcount > 8 || bcount > 8 || acount > 8 {
                return s.errp("bad masks");
            }
        }
        out = vec![0u8; size];
        for _ in 0..img_y {
            if easy != 0 {
                for _ in 0..img_x {
                    out[z + 2] = s.get8();
                    out[z + 1] = s.get8();
                    out[z] = s.get8();
                    z += 3;
                    let a = if easy == 2 {
                        s.get8() | alpha_override
                    } else {
                        255
                    };
                    all_a |= u32::from(a);
                    if target == 4 {
                        out[z] = a;
                        z += 1;
                    }
                }
            } else {
                let bpp = info.bpp;
                for _ in 0..img_x {
                    let v = if bpp == 16 {
                        s.get16le() as u32
                    } else {
                        s.get32le()
                    };
                    out[z] = shiftsigned(v & mr, rshift, rcount) as u8;
                    out[z + 1] = shiftsigned(v & mg, gshift, gcount) as u8;
                    out[z + 2] = shiftsigned(v & mb, bshift, bcount) as u8;
                    z += 3;
                    let a = if ma != 0 {
                        shiftsigned(v & ma, ashift, acount) as u32
                    } else {
                        255
                    };
                    all_a |= a;
                    if target == 4 {
                        out[z] = a as u8;
                        z += 1;
                    }
                }
            }
            s.skip(pad);
        }
    }

    // if alpha channel is all 0s, replace with all 255s
    if target == 4 && all_a == 0 {
        for pixel in out.as_chunks_mut::<4>().0.iter_mut() {
            pixel[3] = 255;
        }
    }

    if flip_vertically {
        vertical_flip(
            &mut out,
            s.img_x as usize,
            s.img_y as usize,
            target as usize,
        );
    }

    if req_comp != 0 && req_comp != target {
        out = convert_format(s, out, target, req_comp, s.img_x, s.img_y)?;
    }

    Some((out, s.img_n))
}

/// `stbi__bmp_info`
pub(crate) fn bmp_info(s: &mut Context) -> Option<Info> {
    let mut info = BmpData {
        all_a: 255,
        ..BmpData::default()
    };
    if !bmp_parse_header(s, &mut info) {
        s.rewind();
        return None;
    }
    let comp = if info.bpp == 24 && info.ma == 0xff000000 {
        3
    } else if info.ma != 0 {
        4
    } else {
        3
    };
    Some(Info {
        kind: ImageType::Bmp,
        x: s.img_x as i32,
        y: s.img_y as i32,
        comp,
        is_16_bit: false,
        is_hdr: false,
        is_iphone_png: false,
    })
}

#[cfg(test)]
mod tests {
    use super::super::{LoadError, LoadOptions, info_from_memory, load_from_memory};
    use super::*;
    use stb_image_write_nrp::{Channels, Image, WriteOptions, write_bmp};

    /// A BMP file with the 40-byte header.
    fn bmp(x: i32, y: i32, bpp: u16, compress: u32, after_header: &[u8], rows: &[u8]) -> Vec<u8> {
        let mut file = b"BM".to_vec();
        let offset = 14 + 40 + after_header.len();
        file.extend_from_slice(&((offset + rows.len()) as u32).to_le_bytes());
        file.extend_from_slice(&[0; 4]);
        file.extend_from_slice(&(offset as u32).to_le_bytes());
        file.extend_from_slice(&40u32.to_le_bytes());
        file.extend_from_slice(&x.to_le_bytes());
        file.extend_from_slice(&y.to_le_bytes());
        file.extend_from_slice(&1u16.to_le_bytes());
        file.extend_from_slice(&bpp.to_le_bytes());
        file.extend_from_slice(&compress.to_le_bytes());
        file.extend_from_slice(&[0; 20]);
        file.extend_from_slice(after_header);
        file.extend_from_slice(rows);
        file
    }

    #[test]
    fn what_the_writer_writes_comes_back() {
        let options = LoadOptions::default();
        for comp in [3i32, 4] {
            let pixels: Vec<u8> = (0..7 * 5 * comp)
                .map(|i| (i * 37 + i / comp * 11) as u8 | 1)
                .collect();
            let image =
                Image::new(&pixels, 7, 5, Channels::from_count(comp as u8).unwrap()).unwrap();
            let file = write_bmp(&image, &WriteOptions::default());
            let loaded = load_from_memory(&file, 0, &options).unwrap();
            assert_eq!((loaded.x, loaded.y, loaded.channels_in_file), (7, 5, comp));
            assert_eq!(loaded.data, pixels);
            let info = info_from_memory(&file).unwrap();
            assert_eq!(
                (info.kind, info.x, info.y, info.comp),
                (ImageType::Bmp, 7, 5, comp)
            );
        }
        // Grey comes back as three equal values.
        let file = write_bmp(
            &Image::new(&[7, 9], 2, 1, Channels::Grey).unwrap(),
            &WriteOptions::default(),
        );
        assert_eq!(
            load_from_memory(&file, 0, &options).unwrap().data,
            [7, 7, 7, 9, 9, 9]
        );
        assert_eq!(load_from_memory(&file, 1, &options).unwrap().data, [7, 9]);
    }

    #[test]
    fn palettes_and_bit_fields() {
        let options = LoadOptions::default();
        // Two colours, stored blue first with a byte to spare: 1 bit a pixel, 10 wide, the
        // rows from the bottom, each padded to four bytes.
        let palette = [30, 20, 10, 0, 60, 50, 40, 0];
        let file = bmp(
            10,
            2,
            1,
            0,
            &palette,
            &[
                0b1000_0000,
                0b0100_0000,
                0,
                0,
                0b0000_0001,
                0b1100_0000,
                0,
                0,
            ],
        );
        let loaded = load_from_memory(&file, 0, &options).unwrap();
        let (a, b) = ([10u8, 20, 30], [40u8, 50, 60]);
        let top: Vec<u8> = [a, a, a, a, a, a, a, b, b, b].concat();
        let bottom: Vec<u8> = [b, a, a, a, a, a, a, a, a, b].concat();
        assert_eq!(loaded.data, [top, bottom].concat());
        // The same top row first: a height below zero.
        let file = bmp(
            10,
            -2,
            1,
            0,
            &palette,
            &[
                0b1000_0000,
                0b0100_0000,
                0,
                0,
                0b0000_0001,
                0b1100_0000,
                0,
                0,
            ],
        );
        assert_eq!(info_from_memory(&file).unwrap().y, -2);
        let loaded = load_from_memory(&file, 0, &options).unwrap();
        assert_eq!(loaded.y, 2);
        assert_eq!(&loaded.data[..3], &b);
        // 4 and 8 bits a pixel.
        let file = bmp(3, 1, 4, 0, &palette, &[0x01, 0x10, 0, 0]);
        assert_eq!(
            load_from_memory(&file, 0, &options).unwrap().data,
            [a, b, b].concat()
        );
        let file = bmp(3, 1, 8, 0, &palette, &[1, 0, 1, 0]);
        assert_eq!(
            load_from_memory(&file, 4, &options).unwrap().data,
            [40, 50, 60, 255, 10, 20, 30, 255, 40, 50, 60, 255]
        );
        // 16 bits: five each by default, or as three masks say (5, 6 and 5 here).
        let file = bmp(2, 1, 16, 0, &[], &[0x1f, 0x00, 0x00, 0x7c]);
        assert_eq!(
            load_from_memory(&file, 0, &options).unwrap().data,
            [0, 0, 255, 255, 0, 0]
        );
        let masks = [0x00, 0xf8, 0, 0, 0xe0, 0x07, 0, 0, 0x1f, 0, 0, 0];
        let file = bmp(2, 1, 16, 3, &masks, &[0xe0, 0x07, 0x10, 0x84]);
        assert_eq!(
            load_from_memory(&file, 0, &options).unwrap().data,
            [0, 255, 0, 132, 130, 132]
        );
        // 32 bits with this header: no alpha.
        let file = bmp(1, 1, 32, 0, &[], &[1, 2, 3, 4]);
        let loaded = load_from_memory(&file, 0, &options).unwrap();
        assert_eq!(
            (loaded.channels_in_file, &loaded.data[..]),
            (3, &[3u8, 2, 1][..])
        );
    }

    #[test]
    fn damaged_files_are_refused() {
        let options = LoadOptions::default();
        let refused = |file: &[u8]| load_from_memory(file, 0, &options).unwrap_err();
        assert_eq!(
            refused(&bmp(2, 2, 8, 1, &[0; 8], &[0; 8])),
            LoadError("BMP RLE")
        );
        assert_eq!(
            refused(&bmp(2, 2, 24, 4, &[], &[0; 16])),
            LoadError("BMP JPEG/PNG")
        );
        assert_eq!(
            refused(&bmp(2, 2, 24, 3, &[], &[0; 16])),
            LoadError("bad BMP")
        );
        assert_eq!(
            refused(&bmp(2, 2, 8, 0, &[], &[0; 8])),
            LoadError("invalid")
        );
        assert_eq!(
            refused(&bmp(2, 2, 2, 0, &[0; 16], &[0; 8])),
            LoadError("bad bpp")
        );
        assert_eq!(
            refused(&bmp(
                2,
                2,
                16,
                3,
                &[0x1f, 0, 0, 0, 0x1f, 0, 0, 0, 0x1f, 0, 0, 0],
                &[0; 8]
            )),
            LoadError("bad BMP")
        );
        assert_eq!(
            refused(&bmp(1 << 25, 2, 24, 0, &[], &[])),
            LoadError("too large")
        );
        assert_eq!(
            refused(&bmp(2, 2, 24, 0, &[0; 2000], &[0; 16])),
            LoadError("bad offset")
        );
        // A header of no known size is no BMP at all to the reader, and nothing else either.
        let mut file = bmp(2, 2, 24, 0, &[], &[0; 16]);
        file[14] = 41;
        assert_eq!(refused(&file), LoadError("unknown image type"));
        // A file cut short reads as zeros where its bytes are missing.
        let file = bmp(2, 2, 24, 0, &[], &[9; 16]);
        assert_eq!(
            load_from_memory(&file[..file.len() - 8], 0, &options)
                .unwrap()
                .data,
            [0, 0, 0, 0, 0, 0, 9, 9, 9, 9, 9, 9]
        );
    }
}
