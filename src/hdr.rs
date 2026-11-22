//! The Radiance `.hdr` reader: `stbi__hdr_*`, originally by Nicolas Schulz; and the conversions
//! between floats and 8/16 bits a channel, `stbi__hdr_to_ldr` and `stbi__ldr_to_hdr`.

use alloc::vec::Vec;

use super::{
    Context, Failed, ImageFormat, LoadOptions, RawInfo, STBI_MAX_DIMENSIONS, mad3sizes_valid,
    mad4sizes_valid, try_alloc_zeroed,
};

#[cfg(feature = "std")]
#[inline]
fn powf64(base: f64, exp: f64) -> f64 {
    base.powf(exp)
}

#[cfg(not(feature = "std"))]
#[inline]
fn powf64(base: f64, exp: f64) -> f64 {
    powf64_nostd(base, exp)
}

/// Integer test for negative-base / signed-zero IEEE-754 rules: returns 0 if not an integer,
/// 1 if an even integer, 2 if an odd integer.
#[cfg_attr(feature = "std", allow(dead_code))]
fn int_parity(val: f64) -> u8 {
    if !val.is_finite() {
        return 0;
    }
    let bits = val.to_bits() & 0x7fff_ffff_ffff_ffff;
    if bits == 0 {
        return 1;
    }
    let exp = ((bits >> 52) & 0x7ff) as i32 - 1023;
    if exp < 0 {
        return 0;
    }
    if exp >= 53 {
        return 1; // 2^53 and higher are always even integers
    }
    let frac_bits = 52 - exp;
    let mant = (bits & 0x000f_ffff_ffff_ffff) | (1u64 << 52);
    if frac_bits > 0 && (mant & ((1u64 << frac_bits) - 1)) != 0 {
        return 0;
    }
    if ((mant >> frac_bits) & 1) != 0 { 2 } else { 1 }
}

#[cfg_attr(feature = "std", allow(dead_code))]
#[allow(clippy::float_cmp)]
fn ldexp64(mut val: f64, mut exp: i32) -> f64 {
    if val == 0.0 || !val.is_finite() {
        return val;
    }
    while exp > 1023 {
        val *= f64::from_bits(2046u64 << 52); // 2^1023
        exp -= 1023;
        if !val.is_finite() {
            return val;
        }
    }
    while exp < -1022 {
        val *= f64::from_bits(1u64 << 52); // 2^-1022
        exp += 1022;
        if val == 0.0 {
            return val;
        }
    }
    val * f64::from_bits(((exp + 1023) as u64) << 52)
}

/// Pure-Rust IEEE-754 `f64::powf` fallback for `#![no_std]` builds.
#[cfg_attr(feature = "std", allow(dead_code))]
#[allow(clippy::float_cmp)]
fn powf64_nostd(mut base: f64, exp: f64) -> f64 {
    if exp == 0.0 || base == 1.0 {
        return 1.0;
    }
    if base.is_nan() || exp.is_nan() {
        return f64::NAN;
    }
    if base == 0.0 {
        let sign = if base.is_sign_negative() && int_parity(exp) == 2 {
            -1.0
        } else {
            1.0
        };
        return if exp < 0.0 {
            sign * f64::INFINITY
        } else {
            sign * 0.0
        };
    }

    let mut sign = 1.0;
    if base < 0.0 {
        match int_parity(exp) {
            0 => return f64::NAN,
            2 => sign = -1.0,
            _ => {}
        }
        base = -base;
    }
    if base.is_infinite() {
        return sign * if exp > 0.0 { f64::INFINITY } else { 0.0 };
    }
    if exp.is_infinite() {
        if base == 1.0 {
            return 1.0;
        }
        return if (base > 1.0) == (exp > 0.0) {
            f64::INFINITY
        } else {
            0.0
        };
    }

    // Decompose base = mant * 2^exp2_base with mant in [sqrt(2)/2, sqrt(2)].
    let mut exp2_base = 0i32;
    if (base.to_bits() & 0x7ff0_0000_0000_0000) == 0 {
        base *= 4_503_599_627_370_496.0; // 2^52 to normalize subnormals
        exp2_base -= 52;
    }
    let bits = base.to_bits();
    exp2_base += ((bits >> 52) & 0x7ff) as i32 - 1023;
    let mut mant = f64::from_bits((bits & 0x000f_ffff_ffff_ffff) | (1023u64 << 52));
    if mant > core::f64::consts::SQRT_2 {
        mant *= 0.5;
        exp2_base += 1;
    }

    // ln(mant) = 2 * ratio * sum_{j=0..10} ratio^(2j) / (2j + 1), |ratio| < 0.1716.
    let ratio = (mant - 1.0) / (mant + 1.0);
    let ratio2 = ratio * ratio;
    let mut poly = 1.0 / 21.0;
    for &denom in &[19.0, 17.0, 15.0, 13.0, 11.0, 9.0, 7.0, 5.0, 3.0, 1.0] {
        poly = poly * ratio2 + 1.0 / denom;
    }
    let ln_m = 2.0 * ratio * poly;

    let exp2_total = exp * (f64::from(exp2_base) + ln_m * core::f64::consts::LOG2_E);
    if exp2_total > 1024.0 {
        return sign * f64::INFINITY;
    }
    if exp2_total < -1075.0 {
        return sign * 0.0;
    }

    // Range reduce exp2_total = exp2_out + rem2 with |rem2| <= 0.5 (exact by Sterbenz's lemma).
    let exp2_out = (if exp2_total >= 0.0 {
        exp2_total + 0.5
    } else {
        exp2_total - 0.5
    }) as i32;
    let rem = (exp2_total - f64::from(exp2_out)) * core::f64::consts::LN_2;

    let mut exp_r = 1.0 / 6_227_020_800.0; // 1 / 13!
    for idx in (1..=12).rev() {
        exp_r = exp_r * rem + 1.0 / factorial(idx);
    }
    exp_r = exp_r * rem + 1.0;

    sign * ldexp64(exp_r, exp2_out)
}

const fn factorial(n: u32) -> f64 {
    let mut acc = 1u64;
    let mut i = 2u32;
    while i <= n {
        acc *= i as u64;
        i += 1;
    }
    acc as f64
}

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
) -> Result<(), Failed> {
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
            return s.fail("invalid decoded scanline length");
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
                        return s.fail("corrupt");
                    }
                    for _ in 0..count {
                        scanline[x as usize * 4 + k] = value;
                        x += 1;
                    }
                } else {
                    // Dump
                    if count == 0 || width - x < i32::from(count) {
                        return s.fail("corrupt");
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

    Ok(())
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

    let len = (width * height * req_comp) as usize;
    if s.check_limits(width as u32, height as u32, len * 4)
        .is_err()
    {
        return None;
    }

    // Read data
    let Some(mut hdr_data) = try_alloc_zeroed::<f32>(len) else {
        return s.errp("outofmem");
    };

    // for sizes where it's legal, assume RLE unless proven otherwise
    let isrle = (8..32768).contains(&width);
    let Some(mut scanline) = try_alloc_zeroed::<u8>(if isrle { width as usize * 4 } else { 0 })
    else {
        return s.errp("outofmem");
    };

    if hdr_load_pixels(
        s,
        &mut hdr_data,
        width,
        height,
        req_comp,
        isrle,
        &mut scanline,
    )
    .is_err()
    {
        return None;
    }

    Some((hdr_data, 3))
}

/// `stbi__hdr_info`
pub(crate) fn hdr_info(s: &mut Context) -> Option<RawInfo> {
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

    Some(RawInfo {
        kind: ImageFormat::Hdr,
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
// (The names are stb_image's, so that the code reads against the original.)
#[allow(clippy::many_single_char_names)]
pub(crate) fn hdr_to_ldr(
    s: &mut Context,
    data: &[f32],
    x: i32,
    y: i32,
    comp: i32,
    options: &LoadOptions,
) -> Option<Vec<u8>> {
    if !mad3sizes_valid(x, y, comp, 0) {
        return s.errp("outofmem");
    }
    let len = x as usize * y as usize * comp as usize;
    if s.check_limits(x as u32, y as u32, len).is_err() {
        return None;
    }
    // (The original keeps the reciprocals.)
    let gamma_i = 1.0f32 / options.hdr_to_ldr_gamma;
    let scale_i = 1.0f32 / options.hdr_to_ldr_scale;
    let comp = comp as usize;
    let Some(mut output) = try_alloc_zeroed::<u8>(len) else {
        return s.errp("outofmem");
    };
    // compute number of non-alpha components
    let n = if comp & 1 != 0 { comp } else { comp - 1 };
    for (pixel, out) in data.chunks_exact(comp).zip(output.chunks_exact_mut(comp)) {
        for k in 0..n {
            let z =
                (powf64(f64::from(pixel[k] * scale_i), f64::from(gamma_i)) as f32) * 255.0 + 0.5;
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
///
/// Uses a 256-entry `f32` lookup table computed with the exact IEEE-754 expression of the
/// original (`(f64::from(f32::from(v) / 255.0).powf(gamma) * scale) as f32`).
pub(crate) fn ldr_to_hdr(
    s: &mut Context,
    data: &[u8],
    x: i32,
    y: i32,
    comp: i32,
    options: &LoadOptions,
) -> Option<Vec<f32>> {
    if !mad4sizes_valid(x, y, comp, 4, 0) {
        return s.errp("outofmem");
    }
    let comp = comp as usize;
    let len = x as usize * y as usize * comp;
    if s.check_limits(x as u32, y as u32, len * 4).is_err() {
        return None;
    }
    let Some(mut output) = try_alloc_zeroed::<f32>(len) else {
        return s.errp("outofmem");
    };

    let gamma = f64::from(options.ldr_to_hdr_gamma);
    let scale = f64::from(options.ldr_to_hdr_scale);
    let mut lut = [0.0f32; 256];
    for (v, entry) in lut.iter_mut().enumerate() {
        *entry = (powf64(f64::from(f32::from(v as u8) / 255.0), gamma) * scale) as f32;
    }

    // compute number of non-alpha components
    let n = if comp & 1 != 0 { comp } else { comp - 1 };
    if n == comp {
        for (&byte, out) in data.iter().zip(&mut output) {
            *out = lut[usize::from(byte)];
        }
    } else {
        let mut alpha_lut = [0.0f32; 256];
        for (v, entry) in alpha_lut.iter_mut().enumerate() {
            *entry = f32::from(v as u8) / 255.0;
        }
        for (pixel, out) in data.chunks_exact(comp).zip(output.chunks_exact_mut(comp)) {
            for k in 0..n {
                out[k] = lut[usize::from(pixel[k])];
            }
            out[n] = alpha_lut[usize::from(pixel[n])];
        }
    }
    Some(output)
}

/// Converts 16-bit samples (`0..=65535`) directly to floats for
/// [`LoadOptions::preserve_16bit_in_f32`].
pub(crate) fn ldr_16_to_hdr(
    s: &mut Context,
    data: &[u16],
    x: i32,
    y: i32,
    comp: i32,
    options: &LoadOptions,
) -> Option<Vec<f32>> {
    if !mad4sizes_valid(x, y, comp, 4, 0) {
        return s.errp("outofmem");
    }
    let comp = comp as usize;
    let len = x as usize * y as usize * comp;
    if s.check_limits(x as u32, y as u32, len * 4).is_err() {
        return None;
    }
    let Some(mut output) = try_alloc_zeroed::<f32>(len) else {
        return s.errp("outofmem");
    };

    let gamma = f64::from(options.ldr_to_hdr_gamma);
    let scale = f64::from(options.ldr_to_hdr_scale);
    let n = if comp & 1 != 0 { comp } else { comp - 1 };
    for (pixel, out) in data.chunks_exact(comp).zip(output.chunks_exact_mut(comp)) {
        for k in 0..n {
            out[k] = (powf64(f64::from(f32::from(pixel[k]) / 65535.0), gamma) * scale) as f32;
        }
        if n < comp {
            out[n] = f32::from(pixel[n]) / 65535.0;
        }
    }
    Some(output)
}

#[cfg(test)]
mod tests {
    use super::super::{ImageFormat, LoadOptions, info, is_hdr, load, load_f32};
    use super::{ldr_to_hdr, powf64_nostd, strtol};
    use alloc::vec::Vec;

    #[test]
    fn numbers_are_read_as_strtol_reads_them() {
        assert_eq!(strtol(b"  42 +X", 0), (42, 4));
        assert_eq!(strtol(b"-7", 0), (-7, 2));
        assert_eq!(strtol(b"+X 5", 0), (0, 0));
        assert_eq!(strtol(b"99999999999999999999", 0), (i64::MAX, 20));
        assert_eq!(strtol(b"-9223372036854775808", 0), (i64::MIN, 20));
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn ldr_to_hdr_lut_and_nostd_powf_are_accurate() {
        let bytes: Vec<u8> = (0..=255).collect();
        for gamma in [1.0f32, 1.8, 2.2, 2.4] {
            for scale in [0.5f32, 1.0, 2.0, 4.0] {
                let opts = LoadOptions {
                    ldr_to_hdr_gamma: gamma,
                    ldr_to_hdr_scale: scale,
                    ..LoadOptions::default()
                };
                let mut ctx = crate::Context::new(&[], &opts);
                let out = ldr_to_hdr(&mut ctx, &bytes, 64, 1, 4, &opts).unwrap();
                for (px_idx, chunk) in out.as_chunks::<4>().0.iter().enumerate() {
                    for k in 0..3 {
                        let v = bytes[px_idx * 4 + k];
                        let std_bits = ((f64::from(f32::from(v) / 255.0).powf(f64::from(gamma))
                            * f64::from(scale)) as f32)
                            .to_bits();
                        let nostd_bits =
                            ((powf64_nostd(f64::from(f32::from(v) / 255.0), f64::from(gamma))
                                * f64::from(scale)) as f32)
                                .to_bits();
                        assert_eq!(chunk[k].to_bits(), std_bits);
                        assert!(
                            chunk[k].to_bits().abs_diff(nostd_bits) <= 1,
                            "nostd powf differed for v={v}, gamma={gamma}, scale={scale}"
                        );
                    }
                    let a = bytes[px_idx * 4 + 3];
                    assert_eq!(chunk[3].to_bits(), (f32::from(a) / 255.0).to_bits());
                }
            }
        }
        assert!(powf64_nostd(-2.0, 1.0 / 2.2).is_nan());
        assert_eq!(powf64_nostd(-2.0, 3.0), -8.0);
        assert_eq!(powf64_nostd(0.0, 0.0), 1.0);
        assert_eq!(powf64_nostd(0.0, 2.2), 0.0);
    }

    #[test]
    fn a_flat_file_is_read() {
        let mut file = b"#?RADIANCE\nFORMAT=32-bit_rle_rgbe\n\n-Y 1 +X 2\n".to_vec();
        file.extend_from_slice(&[128, 64, 0, 129, 0, 0, 0, 0]);
        assert!(is_hdr(&file));
        let info = info(&file).unwrap();
        assert_eq!(
            (
                info.format,
                info.width as i32,
                info.height as i32,
                info.channels as i32,
                info.is_hdr
            ),
            (ImageFormat::Hdr, 2, 1, 3, true)
        );
        let image = load_f32(&file, crate::ch(0), &LoadOptions::default()).unwrap();
        assert_eq!(
            (
                image.width as i32,
                image.height as i32,
                image.channels_in_file as i32
            ),
            (2, 1, 3)
        );
        assert_eq!(
            image.data,
            [128.5 / 128.0, 64.5 / 128.0, 0.5 / 128.0, 0.0, 0.0, 0.0]
        );
        let image = load(&file, crate::ch(0), &LoadOptions::default()).unwrap();
        assert_eq!(image.data[3..], [0, 0, 0]);
    }
}
