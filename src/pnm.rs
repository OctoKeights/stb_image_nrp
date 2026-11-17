//! The PNM reader: `stbi__pnm_*`, Portable Gray Map and Portable Pixel Map, by Ken Miller.
//! Binary and text, 8 or 16 bits a channel.
//!
//! PGM: <http://netpbm.sourceforge.net/doc/pgm.html>
//! PPM: <http://netpbm.sourceforge.net/doc/ppm.html>

use super::{
    Context, ImageType, Info, Pixels, STBI_MAX_DIMENSIONS, convert_format, mad4sizes_valid,
};

/// `stbi__pnm_test`
pub(crate) fn pnm_test(s: &mut Context) -> bool {
    let p = s.get8();
    let t = s.get8();
    if p != b'P' || (t != b'2' && t != b'3' && t != b'5' && t != b'6') {
        s.rewind();
        return false;
    }
    true
}

/// `stbi__pnm_isspace`
fn pnm_isspace(c: u8) -> bool {
    c == b' ' || c == b'\t' || c == b'\n' || c == 0x0b || c == 0x0c || c == b'\r'
}

/// `stbi__pnm_skip_whitespace`
fn pnm_skip_whitespace(s: &mut Context, c: &mut u8) {
    loop {
        while !s.at_eof() && pnm_isspace(*c) {
            *c = s.get8();
        }

        if s.at_eof() || *c != b'#' {
            break;
        }

        while !s.at_eof() && *c != b'\n' && *c != b'\r' {
            *c = s.get8();
        }
    }
}

/// `stbi__pnm_getinteger`: 0, with a reason, when the number is more than an int holds.
fn pnm_getinteger(s: &mut Context, c: &mut u8) -> i32 {
    let mut value = 0i32;

    while !s.at_eof() && c.is_ascii_digit() {
        value = value * 10 + i32::from(*c - b'0');
        *c = s.get8();
        // (The original's `char` is signed: a byte from 0x80 up is below '7'.)
        if value > 214748364 || (value == 214748364 && (*c as i8) > b'7' as i8) {
            s.err("integer parse overflow");
            return 0;
        }
    }

    value
}

/// What `stbi__pnm_info_internal` finds.
struct PnmHeader {
    x: i32,
    y: i32,
    comp: i32,
    maxv: i32,
    isascii: bool,
}

/// `stbi__pnm_info_internal`: the bits a channel (0 when the header is refused) and the
/// header, as far as it got.
fn pnm_info_internal(s: &mut Context) -> (i32, PnmHeader) {
    let mut header = PnmHeader {
        x: 0,
        y: 0,
        comp: 0,
        maxv: 0,
        isascii: false,
    };

    s.rewind();

    // Get identifier
    let p = s.get8();
    let t = s.get8();
    if p != b'P' || (t != b'2' && t != b'3' && t != b'5' && t != b'6') {
        s.rewind();
        return (0, header);
    }

    // '2'/'5' is 1-component .pgm; '3'/'6' is 3-component .ppm
    header.comp = if t == b'3' || t == b'6' { 3 } else { 1 };
    header.isascii = t == b'2' || t == b'3';

    let mut c = s.get8();
    pnm_skip_whitespace(s, &mut c);

    header.x = pnm_getinteger(s, &mut c); // read width
    if header.x == 0 {
        s.err("invalid width");
        return (0, header);
    }
    pnm_skip_whitespace(s, &mut c);

    header.y = pnm_getinteger(s, &mut c); // read height
    if header.y == 0 {
        s.err("invalid width");
        return (0, header);
    }
    pnm_skip_whitespace(s, &mut c);

    header.maxv = pnm_getinteger(s, &mut c); // read max value
    let bits = if header.maxv == 0 {
        s.err("invalid maxv");
        0
    } else if header.maxv > 65535 {
        s.err("max value > 65535");
        0
    } else if header.maxv > 255 {
        16
    } else {
        8
    };
    (bits, header)
}

/// `stbi__pnm_load`: the pixels and the channels in the file; the size goes into the context.
pub(crate) fn pnm_load(s: &mut Context, req_comp: i32) -> Option<(Pixels, i32)> {
    let (bits_per_channel, header) = pnm_info_internal(s);
    // (The original reads the header straight into the context.)
    s.img_x = header.x as u32;
    s.img_y = header.y as u32;
    s.img_n = header.comp;
    if bits_per_channel == 0 {
        return None;
    }

    if s.img_y > STBI_MAX_DIMENSIONS {
        return s.errp("too large");
    }
    if s.img_x > STBI_MAX_DIMENSIONS {
        return s.errp("too large");
    }

    let bpv = bits_per_channel / 8; // bytes per value

    if !mad4sizes_valid(s.img_n, s.img_x as i32, s.img_y as i32, bpv, 0) {
        return s.errp("too large");
    }

    let maxv = header.maxv;
    let nvals = (s.img_n * s.img_x as i32 * s.img_y as i32) as usize;
    let mut out = vec![0u8; nvals * bpv as usize];
    if !header.isascii {
        if !s.getn_into(&mut out) {
            return s.errp("bad PNM");
        }
    } else {
        let mut c = s.get8();
        for i in 0..nvals {
            pnm_skip_whitespace(s, &mut c);
            let mut val = pnm_getinteger(s, &mut c);
            if val < 0 {
                return None; // error already set
            }
            if val > maxv {
                val = maxv - 1;
            }
            if bpv == 1 {
                out[i] = val as u8;
            } else {
                out[i * 2] = (val >> 8) as u8; // BE to match binary reader, fixed up below
                out[i * 2 + 1] = val as u8;
            }
        }
    }

    // expected_maxv conversion
    let (n, x, y) = (s.img_n, s.img_x, s.img_y);
    if bpv == 2 {
        let expected_maxv = 65535u32;
        let mut out16 = vec![0u16; nvals];
        for (i, value) in out16.iter_mut().enumerate() {
            let mut val = (u32::from(out[i * 2]) << 8) + u32::from(out[i * 2 + 1]); // BE
            if maxv as u32 != expected_maxv {
                if val >= maxv as u32 {
                    val = (maxv - 1) as u32;
                }
                val = (val * expected_maxv) / maxv as u32;
            }
            *value = val as u16;
        }
        if req_comp != 0 && req_comp != n {
            out16 = convert_format(s, out16, n, req_comp, x, y)?;
        }
        Some((Pixels::U16(out16), n))
    } else {
        let expected_maxv = 255;
        if maxv != expected_maxv {
            for value in &mut out {
                let mut val = i32::from(*value);
                if val >= maxv {
                    val = maxv - 1;
                }
                *value = ((val * expected_maxv) / maxv) as u8;
            }
        }
        if req_comp != 0 && req_comp != n {
            out = convert_format(s, out, n, req_comp, x, y)?;
        }
        Some((Pixels::U8(out), n))
    }
}

/// `stbi__pnm_info`
pub(crate) fn pnm_info(s: &mut Context) -> Option<Info> {
    let (bits, header) = pnm_info_internal(s);
    if bits == 0 {
        return None;
    }
    Some(Info {
        kind: ImageType::Pnm,
        x: header.x,
        y: header.y,
        comp: header.comp,
        is_16_bit: bits == 16,
        is_hdr: false,
        is_iphone_png: false,
    })
}

#[cfg(test)]
mod tests {
    use super::super::{LoadOptions, load_16_from_memory, load_from_memory};

    #[test]
    fn binary_and_text_files() {
        let image = load_from_memory(
            b"P5 2 1\n# a comment\n255\n\x07\x09",
            0,
            &LoadOptions::default(),
        )
        .unwrap();
        assert_eq!(
            (image.x, image.y, image.channels_in_file, image.data.clone()),
            (2, 1, 1, vec![7, 9])
        );
        let image = load_from_memory(b"P3\n1 1 15 15 0 5\n", 0, &LoadOptions::default()).unwrap();
        assert_eq!(image.data, [238, 0, 85]);
        let image =
            load_16_from_memory(b"P5 1 1 65535 \x12\x34", 0, &LoadOptions::default()).unwrap();
        assert_eq!(image.data, [0x1234]);
    }

    #[test]
    fn a_header_that_is_too_large_is_refused() {
        let error =
            load_from_memory(b"P5 2147483648 1 255 ", 0, &LoadOptions::default()).unwrap_err();
        assert_eq!(error.0, "invalid width");
    }
}
