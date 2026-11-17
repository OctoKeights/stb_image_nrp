//! The Radiance `.hdr` reader: `stbi__hdr_*`, originally by Nicolas Schulz; and the conversions
//! between floats and 8 bits a channel, `stbi__hdr_to_ldr` and `stbi__ldr_to_hdr`.

use super::{
    Context, ImageType, Info, LoadOptions, STBI_MAX_DIMENSIONS, mad3sizes_valid, mad4sizes_valid,
};

/// `stbi__hdr_test_core`
fn hdr_test_core(s: &mut Context, signature: &[u8]) -> bool {
    for &byte in signature {
        if s.get8() != byte {
            return false;
        }
    }
    s.rewind();
    true
}

/// `stbi__hdr_test`
pub(crate) fn hdr_test(s: &mut Context) -> bool {
    let mut r = hdr_test_core(s, b"#?RADIANCE\n");
    s.rewind();
    if !r {
        r = hdr_test_core(s, b"#?RGBE\n");
        s.rewind();
    }
    r
}

/// `STBI__HDR_BUFLEN`
const HDR_BUFLEN: usize = 1024;

/// `stbi__hdr_gettoken`: a line, as the C string the original makes of it (what comes before
/// any zero byte in it).
fn hdr_gettoken(z: &mut Context) -> Vec<u8> {
    let mut buffer = Vec::new();

    let mut c = z.get8();

    while !z.at_eof() && c != b'\n' {
        buffer.push(c);
        if buffer.len() == HDR_BUFLEN - 1 {
            // flush to end of line
            while !z.at_eof() && z.get8() != b'\n' {}
            break;
        }
        c = z.get8();
    }

    if let Some(end) = buffer.iter().position(|&byte| byte == 0) {
        buffer.truncate(end);
    }
    buffer
}

/// `strtol` of base 10, as glibc's (with a `long` of 64 bits): the number and where it ends,
/// which is where it began when there are no digits.
fn strtol(token: &[u8], start: usize) -> (i64, usize) {
    let mut p = start;
    while p < token.len() && matches!(token[p], b' ' | b'\t' | b'\n' | 0x0b | 0x0c | b'\r') {
        p += 1;
    }
    let mut negative = false;
    if p < token.len() && (token[p] == b'+' || token[p] == b'-') {
        negative = token[p] == b'-';
        p += 1;
    }
    let digits = p;
    let limit = if negative {
        1u64 << 63
    } else {
        (1u64 << 63) - 1
    };
    let mut magnitude = 0u64;
    let mut overflow = false;
    while p < token.len() && token[p].is_ascii_digit() {
        let digit = u64::from(token[p] - b'0');
        match magnitude.checked_mul(10).and_then(|m| m.checked_add(digit)) {
            Some(m) if m <= limit => magnitude = m,
            _ => overflow = true,
        }
        p += 1;
    }
    if p == digits {
        return (0, start);
    }
    let value = if overflow {
        if negative { i64::MIN } else { i64::MAX }
    } else if negative {
        (magnitude as i64).wrapping_neg()
    } else {
        magnitude as i64
    };
    (value, p)
}

/// The height and width in the line `-Y <height> +X <width>`, as the original reads them, or
/// None for a line not of that layout.
fn hdr_size(token: &[u8]) -> Option<(i32, i32)> {
    if !token.starts_with(b"-Y ") {
        return None;
    }
    let (height, mut at) = strtol(token, 3);
    while at < token.len() && token[at] == b' ' {
        at += 1;
    }
    if !token[at..].starts_with(b"+X ") {
        return None;
    }
    let (width, _) = strtol(token, at + 3);
    Some((height as i32, width as i32))
}

/// `stbi__hdr_makelut`
fn hdr_makelut(req_comp: i32) -> [f32; 256] {
    let mut explut = [0.0f32; 256];
    explut[0] = 0.0; // exponent 0 always gives 0.0f
    for (k, value) in explut.iter_mut().enumerate().skip(1) {
        // ldexp(1.0, k - (128 + 8)), which is exact
        *value = f64::from_bits(((k as u64 + 1023) - (128 + 8)) << 52) as f32;
        // when req_comp <= 2, we convert to grayscale instead by averaging the three channels;
        // fold the /3 into the scale factor
        if req_comp <= 2 {
            *value /= 3.0;
        }
    }
    explut
}

/// `stbi__hdr_convert`
fn hdr_convert(output: &mut [f32], input: &[u8], req_comp: i32, explut: &[f32; 256]) {
    let scale = explut[input[3] as usize];
    if req_comp <= 2 {
        output[0] = ((i32::from(input[0]) + i32::from(input[1]) + i32::from(input[2])) as f32
            + 1.5)
            * scale;
        if req_comp == 2 {
            output[1] = 1.0;
        }
    } else {
        output[0] = (f32::from(input[0]) + 0.5) * scale;
        output[1] = (f32::from(input[1]) + 0.5) * scale;
        output[2] = (f32::from(input[2]) + 0.5) * scale;
        if req_comp == 4 {
            output[3] = 1.0;
        }
    }
}

/// `stbi__hdr_load_pixels`
fn hdr_load_pixels(
    s: &mut Context,
    hdr_data: &mut [f32],
    width: i32,
    height: i32,
    req_comp: i32,
    mut isrle: bool,
    scanline: &mut [u8],
) -> bool {
    let explut = hdr_makelut(req_comp);
    let (w, n) = (width as usize, req_comp as usize);

    // image data is stored as some number of scanlines
    for y in 0..height as usize {
        let mut rgbe = [s.get8(), s.get8(), s.get8(), s.get8()];
        if !isrle || rgbe[0] != 2 || rgbe[1] != 2 || (rgbe[2] & 0x80) != 0 {
            // data is not RLE, this was the first pixel
            isrle = false;
            let mut x = 0;
            loop {
                let at = (y * w + x) * n;
                hdr_convert(&mut hdr_data[at..at + n], &rgbe, req_comp, &explut);
                x += 1;
                if x >= w {
                    break;
                }
                rgbe = [s.get8(), s.get8(), s.get8(), s.get8()];
            }
            continue;
        }
        if ((i32::from(rgbe[2]) << 8) | i32::from(rgbe[3])) != width {
            return s.err("invalid decoded scanline length");
        }

        for k in 0..4 {
            let mut x = 0i32;
            while x < width {
                let mut count = s.get8();
                if count > 128 {
                    // Run
                    let value = s.get8();
                    count -= 128;
                    if count == 0 || width - x < i32::from(count) {
                        return s.err("corrupt");
                    }
                    for _ in 0..count {
                        scanline[x as usize * 4 + k] = value;
                        x += 1;
                    }
                } else {
                    // Dump
                    if count == 0 || width - x < i32::from(count) {
                        return s.err("corrupt");
                    }
                    for _ in 0..count {
                        scanline[x as usize * 4 + k] = s.get8();
                        x += 1;
                    }
                }
            }
        }
        for x in 0..w {
            let at = (y * w + x) * n;
            hdr_convert(
                &mut hdr_data[at..at + n],
                &scanline[x * 4..x * 4 + 4],
                req_comp,
                &explut,
            );
        }
    }

    true
}

/// `stbi__hdr_load`: the floats, and the channels in the file (always 3); the size goes into
/// the context.
pub(crate) fn hdr_load(s: &mut Context, mut req_comp: i32) -> Option<(Vec<f32>, i32)> {
    if !(0..=4).contains(&req_comp) {
        return s.errp("bad req_comp");
    }

    // Check identifier
    let header_token = hdr_gettoken(s);
    if header_token != b"#?RADIANCE" && header_token != b"#?RGBE" {
        return s.errp("not HDR");
    }

    // Parse header
    let mut valid = false;
    loop {
        let token = hdr_gettoken(s);
        if token.is_empty() {
            break;
        }
        if token == b"FORMAT=32-bit_rle_rgbe" {
            valid = true;
        }
    }

    if !valid {
        return s.errp("unsupported format");
    }

    // Parse width and height
    // can't use sscanf() if we're not using stdio!
    let Some((height, width)) = hdr_size(&hdr_gettoken(s)) else {
        return s.errp("unsupported data layout");
    };

    if width <= 0
        || width > STBI_MAX_DIMENSIONS as i32
        || height <= 0
        || height > STBI_MAX_DIMENSIONS as i32
    {
        return s.errp("invalid size");
    }

    s.img_x = width as u32;
    s.img_y = height as u32;

    if req_comp == 0 {
        req_comp = 3;
    }

    if !mad4sizes_valid(width, height, req_comp, 4, 0) {
        return s.errp("too large");
    }

    // Read data
    let mut hdr_data = vec![0.0f32; (width * height * req_comp) as usize];

    // for sizes where it's legal, assume RLE unless proven otherwise
    let isrle = (8..32768).contains(&width);
    let mut scanline = vec![0u8; if isrle { width as usize * 4 } else { 0 }];

    if !hdr_load_pixels(
        s,
        &mut hdr_data,
        width,
        height,
        req_comp,
        isrle,
        &mut scanline,
    ) {
        return None;
    }

    Some((hdr_data, 3))
}

/// `stbi__hdr_info`
pub(crate) fn hdr_info(s: &mut Context) -> Option<Info> {
    if !hdr_test(s) {
        s.rewind();
        return None;
    }

    let mut valid = false;
    loop {
        let token = hdr_gettoken(s);
        if token.is_empty() {
            break;
        }
        if token == b"FORMAT=32-bit_rle_rgbe" {
            valid = true;
        }
    }

    if !valid {
        s.rewind();
        return None;
    }
    let Some((y, x)) = hdr_size(&hdr_gettoken(s)) else {
        s.rewind();
        return None;
    };

    Some(Info {
        kind: ImageType::Hdr,
        x,
        y,
        comp: 3,
        is_16_bit: false,
        is_hdr: true,
        is_iphone_png: false,
    })
}

/// `stbi__hdr_to_ldr`: floats to 8 bits a channel, by [`LoadOptions::hdr_to_ldr_gamma`] and
/// [`LoadOptions::hdr_to_ldr_scale`]; alpha in a straight line.
pub(crate) fn hdr_to_ldr(
    s: &mut Context,
    data: Vec<f32>,
    x: i32,
    y: i32,
    comp: i32,
    options: &LoadOptions,
) -> Option<Vec<u8>> {
    if !mad3sizes_valid(x, y, comp, 0) {
        return s.errp("outofmem");
    }
    // (The original keeps the reciprocals.)
    let gamma_i = 1.0f32 / options.hdr_to_ldr_gamma;
    let scale_i = 1.0f32 / options.hdr_to_ldr_scale;
    let comp = comp as usize;
    let mut output = vec![0u8; x as usize * y as usize * comp];
    // compute number of non-alpha components
    let n = if comp & 1 != 0 { comp } else { comp - 1 };
    for (pixel, out) in data.chunks_exact(comp).zip(output.chunks_exact_mut(comp)) {
        for k in 0..n {
            let z = (f64::from(pixel[k] * scale_i).powf(f64::from(gamma_i)) as f32) * 255.0 + 0.5;
            out[k] = float_to_u8(z);
        }
        if n < comp {
            let z = pixel[n] * 255.0 + 0.5;
            out[n] = float_to_u8(z);
        }
    }
    Some(output)
}

/// A value made a byte as the original does it: held to 0 to 255, then `(int)` (which gives
/// 0 for not-a-number on x86, and so here).
#[allow(clippy::manual_clamp)] // (as the original has it, which lets not-a-number through)
fn float_to_u8(mut z: f32) -> u8 {
    if z < 0.0 {
        z = 0.0;
    }
    if z > 255.0 {
        z = 255.0;
    }
    z as i32 as u8
}

/// `stbi__ldr_to_hdr`: 8 bits a channel to floats, by [`LoadOptions::ldr_to_hdr_gamma`] and
/// [`LoadOptions::ldr_to_hdr_scale`]; alpha in a straight line.
pub(crate) fn ldr_to_hdr(
    s: &mut Context,
    data: Vec<u8>,
    x: i32,
    y: i32,
    comp: i32,
    options: &LoadOptions,
) -> Option<Vec<f32>> {
    if !mad4sizes_valid(x, y, comp, 4, 0) {
        return s.errp("outofmem");
    }
    let comp = comp as usize;
    let mut output = vec![0.0f32; x as usize * y as usize * comp];
    // compute number of non-alpha components
    let n = if comp & 1 != 0 { comp } else { comp - 1 };
    for (pixel, out) in data.chunks_exact(comp).zip(output.chunks_exact_mut(comp)) {
        for k in 0..n {
            out[k] = (f64::from(f32::from(pixel[k]) / 255.0)
                .powf(f64::from(options.ldr_to_hdr_gamma))
                * f64::from(options.ldr_to_hdr_scale)) as f32;
        }
        if n < comp {
            out[n] = f32::from(pixel[n]) / 255.0;
        }
    }
    Some(output)
}

#[cfg(test)]
mod tests {
    use super::super::{
        ImageType, LoadOptions, info_from_memory, is_hdr_from_memory, load_from_memory,
        loadf_from_memory,
    };
    use super::strtol;

    #[test]
    fn numbers_are_read_as_strtol_reads_them() {
        assert_eq!(strtol(b"  42 +X", 0), (42, 4));
        assert_eq!(strtol(b"-7", 0), (-7, 2));
        assert_eq!(strtol(b"+X 5", 0), (0, 0));
        assert_eq!(strtol(b"99999999999999999999", 0), (i64::MAX, 20));
        assert_eq!(strtol(b"-9223372036854775808", 0), (i64::MIN, 20));
    }

    #[test]
    fn a_flat_file_is_read() {
        let mut file = b"#?RADIANCE\nFORMAT=32-bit_rle_rgbe\n\n-Y 1 +X 2\n".to_vec();
        file.extend_from_slice(&[128, 64, 0, 129, 0, 0, 0, 0]);
        assert!(is_hdr_from_memory(&file));
        let info = info_from_memory(&file).unwrap();
        assert_eq!(
            (info.kind, info.x, info.y, info.comp, info.is_hdr),
            (ImageType::Hdr, 2, 1, 3, true)
        );
        let image = loadf_from_memory(&file, 0, &LoadOptions::default()).unwrap();
        assert_eq!((image.x, image.y, image.channels_in_file), (2, 1, 3));
        assert_eq!(
            image.data,
            [128.5 / 128.0, 64.5 / 128.0, 0.5 / 128.0, 0.0, 0.0, 0.0]
        );
        let image = load_from_memory(&file, 0, &LoadOptions::default()).unwrap();
        assert_eq!(image.data[3..], [0, 0, 0]);
    }
}
