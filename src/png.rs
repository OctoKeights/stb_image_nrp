//! The PNG reader of `stb_image.h`: "public domain 'baseline' PNG decoder v0.10, Sean Barrett
//! 2006-11-18".
//!
//! - no CRC checking
//! - allocates lots of intermediate memory
//!   - avoids problem of streaming data between subsystems
//!   - avoids explicit window management

use super::{
    Context, ImageType, Info, Pixels, STBI_MAX_DIMENSIONS, Sample, Scan, convert_format,
    mad2sizes_valid, mad3sizes_valid, zlib,
};

/// `stbi__pngchunk`
struct PngChunk {
    length: u32,
    kind: u32,
}

/// `stbi__get_chunk_header`
fn get_chunk_header(s: &mut Context) -> PngChunk {
    let length = s.get32be();
    let kind = s.get32be();
    PngChunk { length, kind }
}

/// `stbi__check_png_header`
fn check_png_header(s: &mut Context) -> bool {
    static PNG_SIG: [u8; 8] = [137, 80, 78, 71, 13, 10, 26, 10];
    for byte in PNG_SIG {
        if s.get8() != byte {
            return s.err("bad png sig");
        }
    }
    true
}

/// `stbi__png`
struct Png {
    idata: Vec<u8>,
    /// (Whether there has been an `IDAT` chunk: the original's `idata` is null until one.)
    has_idata: bool,
    out: Pixels,
    depth: i32,
    is_iphone: bool,
}

const STBI_F_NONE: u8 = 0;
const STBI_F_SUB: u8 = 1;
const STBI_F_UP: u8 = 2;
const STBI_F_AVG: u8 = 3;
const STBI_F_PAETH: u8 = 4;
// synthetic filter used for first scanline to avoid needing a dummy row of 0s
const STBI_F_AVG_FIRST: u8 = 5;

static FIRST_ROW_FILTER: [u8; 5] = [
    STBI_F_NONE,
    STBI_F_SUB,
    STBI_F_NONE,
    STBI_F_AVG_FIRST,
    STBI_F_SUB, // Paeth with b=c=0 turns out to be equivalent to sub
];

/// `stbi__paeth`
fn paeth(a: i32, b: i32, c: i32) -> i32 {
    // This formulation looks very different from the reference in the PNG spec, but is
    // actually equivalent and has favorable data dependencies and admits straightforward
    // generation of branch-free code, which helps performance significantly.
    let thresh = c * 3 - (a + b);
    let lo = if a < b { a } else { b };
    let hi = if a < b { b } else { a };
    let t0 = if hi <= thresh { lo } else { c };
    if thresh <= lo { hi } else { t0 }
}

static STBI_DEPTH_SCALE_TABLE: [u8; 9] = [0, 0xff, 0x55, 0, 0x11, 0, 0, 0, 0x01];

/// `stbi__create_png_alpha_expand8`: adds an extra all-255 alpha channel, in place (the
/// values are at the front of `dest`). img_n must be 1 or 3.
fn create_png_alpha_expand8(dest: &mut [u8], x: usize, img_n: i32) {
    // must process data backwards since we allow dest==src
    if img_n == 1 {
        for i in (0..x).rev() {
            dest[i * 2 + 1] = 255;
            dest[i * 2] = dest[i];
        }
    } else {
        debug_assert!(img_n == 3);
        for i in (0..x).rev() {
            dest[i * 4 + 3] = 255;
            dest[i * 4 + 2] = dest[i * 3 + 2];
            dest[i * 4 + 1] = dest[i * 3 + 1];
            dest[i * 4] = dest[i * 3];
        }
    }
}

/// `stbi__create_png_image_raw`: create the png data from post-deflated data
fn create_png_image_raw(
    s: &mut Context,
    a: &mut Png,
    raw: &[u8],
    out_n: i32,
    x: u32,
    y: u32,
    depth: i32,
    color: i32,
) -> bool {
    let bytes = if depth == 16 { 2 } else { 1 };
    let img_n = s.img_n; // copy it into a local for later
    let output_bytes = out_n * bytes;
    let mut filter_bytes = (img_n * bytes) as usize;
    let mut width = x as usize;

    debug_assert!(out_n == s.img_n || out_n == s.img_n + 1);
    if !mad3sizes_valid(x as i32, y as i32, output_bytes, 0) {
        return s.err("outofmem");
    }

    if !mad3sizes_valid(img_n, x as i32, depth, 7) {
        return s.err("too large");
    }
    let img_width_bytes = ((img_n as u32 * x * depth as u32) + 7) >> 3;
    if !mad2sizes_valid(img_width_bytes as i32, y as i32, img_width_bytes as i32) {
        return s.err("too large");
    }
    let img_len = (img_width_bytes + 1) * y;

    // we used to check for exact match between raw_len and img_len on non-interlaced PNGs,
    // but issue #276 reported a PNG in the wild that had extra data at the end (all zeros),
    // so just check for raw_len < img_len always.
    if (raw.len() as u64) < u64::from(img_len) {
        return s.err("not enough pixels");
    }

    // (The original makes room for the pixels before these checks; nothing is seen of it
    // when one of them fails.)
    let (x, y, out_n_, img_n_) = (x as usize, y as usize, out_n as usize, img_n as usize);
    let img_width_bytes = img_width_bytes as usize;
    let mut out8 = if depth == 16 {
        Vec::new()
    } else {
        vec![0u8; x * y * out_n_]
    };
    let mut out16 = if depth == 16 {
        vec![0u16; x * y * out_n_]
    } else {
        Vec::new()
    };
    let stride = x * out_n_;

    // Allocate two scan lines worth of filter workspace buffer.
    let mut filter_buf = vec![0u8; img_width_bytes * 2];

    // Filtering for low-bit-depth images
    if depth < 8 {
        filter_bytes = 1;
        width = img_width_bytes;
    }

    let mut all_ok = true;
    let mut raw_at = 0;
    for j in 0..y {
        // cur/prior filter buffers alternate
        let (first, second) = filter_buf.split_at_mut(img_width_bytes);
        let (cur, prior) = if j & 1 == 0 {
            (first, &*second)
        } else {
            (second, &*first)
        };
        let nk = width * filter_bytes;
        let mut filter = raw[raw_at];
        raw_at += 1;
        let raw = &raw[raw_at..raw_at + nk];

        // check filter type
        if filter > 4 {
            all_ok = s.err("invalid filter");
            break;
        }

        // if first row, use special filter that doesn't sample previous row
        if j == 0 {
            filter = FIRST_ROW_FILTER[usize::from(filter)];
        }

        // perform actual filtering
        match filter {
            STBI_F_NONE => cur[..nk].copy_from_slice(raw),
            STBI_F_SUB => {
                cur[..filter_bytes].copy_from_slice(&raw[..filter_bytes]);
                for k in filter_bytes..nk {
                    cur[k] = raw[k].wrapping_add(cur[k - filter_bytes]);
                }
            }
            STBI_F_UP => {
                for k in 0..nk {
                    cur[k] = raw[k].wrapping_add(prior[k]);
                }
            }
            STBI_F_AVG => {
                for k in 0..filter_bytes {
                    cur[k] = raw[k].wrapping_add(prior[k] >> 1);
                }
                for k in filter_bytes..nk {
                    cur[k] = raw[k].wrapping_add(
                        ((i32::from(prior[k]) + i32::from(cur[k - filter_bytes])) >> 1) as u8,
                    );
                }
            }
            STBI_F_PAETH => {
                for k in 0..filter_bytes {
                    cur[k] = raw[k].wrapping_add(prior[k]); // prior[k] == stbi__paeth(0,prior[k],0)
                }
                for k in filter_bytes..nk {
                    cur[k] = raw[k].wrapping_add(paeth(
                        i32::from(cur[k - filter_bytes]),
                        i32::from(prior[k]),
                        i32::from(prior[k - filter_bytes]),
                    ) as u8);
                }
            }
            _ => {
                // STBI__F_avg_first
                cur[..filter_bytes].copy_from_slice(&raw[..filter_bytes]);
                for k in filter_bytes..nk {
                    cur[k] = raw[k].wrapping_add(cur[k - filter_bytes] >> 1);
                }
            }
        }

        raw_at += nk;

        // expand decoded bits in cur to dest, also adding an extra alpha channel if desired
        if depth < 8 {
            let dest = &mut out8[stride * j..stride * (j + 1)];
            let scale = if color == 0 {
                STBI_DEPTH_SCALE_TABLE[depth as usize]
            } else {
                1
            }; // scale grayscale values to 0..255 range
            let mut input = 0;
            let mut inb = 0u8;
            let nsmp = x * img_n_;

            // expand bits to bytes first
            if depth == 4 {
                for i in 0..nsmp {
                    if i & 1 == 0 {
                        inb = cur[input];
                        input += 1;
                    }
                    dest[i] = scale.wrapping_mul(inb >> 4);
                    inb <<= 4;
                }
            } else if depth == 2 {
                for i in 0..nsmp {
                    if i & 3 == 0 {
                        inb = cur[input];
                        input += 1;
                    }
                    dest[i] = scale.wrapping_mul(inb >> 6);
                    inb <<= 2;
                }
            } else {
                debug_assert!(depth == 1);
                for i in 0..nsmp {
                    if i & 7 == 0 {
                        inb = cur[input];
                        input += 1;
                    }
                    dest[i] = scale.wrapping_mul(inb >> 7);
                    inb <<= 1;
                }
            }

            // insert alpha=255 values if desired
            if img_n != out_n {
                create_png_alpha_expand8(dest, x, img_n);
            }
        } else if depth == 8 {
            let dest = &mut out8[stride * j..stride * (j + 1)];
            dest[..x * img_n_].copy_from_slice(&cur[..x * img_n_]);
            if img_n != out_n {
                create_png_alpha_expand8(dest, x, img_n);
            }
        } else if depth == 16 {
            // convert the image data from big-endian to platform-native
            let dest16 = &mut out16[stride * j..stride * (j + 1)];
            let value = |i: usize| (u16::from(cur[i * 2]) << 8) | u16::from(cur[i * 2 + 1]);
            if img_n == out_n {
                for (i, dest) in dest16.iter_mut().enumerate() {
                    *dest = value(i);
                }
            } else {
                debug_assert!(img_n + 1 == out_n);
                for (i, dest) in dest16.chunks_exact_mut(out_n_).enumerate() {
                    for c in 0..img_n_ {
                        dest[c] = value(i * img_n_ + c);
                    }
                    dest[img_n_] = 0xffff;
                }
            }
        }
    }

    if !all_ok {
        return false;
    }
    a.out = if depth == 16 {
        Pixels::U16(out16)
    } else {
        Pixels::U8(out8)
    };
    true
}

/// The copying of one pass of an interlaced image to its places in the whole.
fn spread<T: Copy>(
    whole: &mut [T],
    pass: &[T],
    img_x: usize,
    out_n: usize,
    x: usize,
    y: usize,
    xorig: usize,
    yorig: usize,
    xspc: usize,
    yspc: usize,
) {
    for j in 0..y {
        for i in 0..x {
            let out_y = j * yspc + yorig;
            let out_x = i * xspc + xorig;
            whole[out_y * img_x * out_n + out_x * out_n..][..out_n]
                .copy_from_slice(&pass[(j * x + i) * out_n..][..out_n]);
        }
    }
}

/// `stbi__create_png_image`
fn create_png_image(
    s: &mut Context,
    a: &mut Png,
    image_data: &[u8],
    out_n: i32,
    depth: i32,
    color: i32,
    interlaced: bool,
) -> bool {
    let bytes = if depth == 16 { 2 } else { 1 };
    let out_bytes = out_n * bytes;
    if !interlaced {
        return create_png_image_raw(s, a, image_data, out_n, s.img_x, s.img_y, depth, color);
    }

    // de-interlacing
    if !mad3sizes_valid(s.img_x as i32, s.img_y as i32, out_bytes, 0) {
        return s.err("outofmem");
    }
    let (img_x, img_y, out_n_) = (s.img_x as usize, s.img_y as usize, out_n as usize);
    let mut whole = if depth == 16 {
        Pixels::U16(vec![0; img_x * img_y * out_n_])
    } else {
        Pixels::U8(vec![0; img_x * img_y * out_n_])
    };
    let mut image_data = image_data;
    for p in 0..7 {
        let xorig = [0u32, 4, 0, 2, 0, 1, 0];
        let yorig = [0u32, 0, 4, 0, 2, 0, 1];
        let xspc = [8u32, 8, 4, 4, 2, 2, 1];
        let yspc = [8u32, 8, 8, 4, 4, 2, 2];
        // pass1_x[4] = 0, pass1_x[5] = 1, pass1_x[12] = 1
        let x = (s.img_x.wrapping_sub(xorig[p]).wrapping_add(xspc[p] - 1)) / xspc[p];
        let y = (s.img_y.wrapping_sub(yorig[p]).wrapping_add(yspc[p] - 1)) / yspc[p];
        if x != 0 && y != 0 {
            let img_len = ((((s.img_n as u32 * x * depth as u32) + 7) >> 3) + 1) * y;
            if !create_png_image_raw(s, a, image_data, out_n, x, y, depth, color) {
                return false;
            }
            let (x, y) = (x as usize, y as usize);
            let place = (
                xorig[p] as usize,
                yorig[p] as usize,
                xspc[p] as usize,
                yspc[p] as usize,
            );
            match (&mut whole, &a.out) {
                (Pixels::U8(whole), Pixels::U8(pass)) => spread(
                    whole, pass, img_x, out_n_, x, y, place.0, place.1, place.2, place.3,
                ),
                (Pixels::U16(whole), Pixels::U16(pass)) => spread(
                    whole, pass, img_x, out_n_, x, y, place.0, place.1, place.2, place.3,
                ),
                _ => unreachable!(),
            }
            image_data = &image_data[img_len as usize..];
        }
    }
    a.out = whole;

    true
}

/// `stbi__compute_transparency`, `stbi__compute_transparency16`: compute color-based
/// transparency, assuming we've already got 255 (or 65535) as the alpha value in the output
fn compute_transparency<T: Sample>(p: &mut [T], tc: [T; 3], out_n: i32) {
    debug_assert!(out_n == 2 || out_n == 4);

    if out_n == 2 {
        for p in p.as_chunks_mut::<2>().0.iter_mut() {
            p[1] = if p[0] == tc[0] { T::default() } else { T::FULL };
        }
    } else {
        for p in p.as_chunks_mut::<4>().0.iter_mut() {
            if p[0] == tc[0] && p[1] == tc[1] && p[2] == tc[2] {
                p[3] = T::default();
            }
        }
    }
}

/// `stbi__expand_png_palette`
fn expand_png_palette(s: &mut Context, a: &mut Png, palette: &[u8; 1024], pal_img_n: i32) -> bool {
    let pixel_count = s.img_x.wrapping_mul(s.img_y);
    let Pixels::U8(orig) = &a.out else {
        unreachable!()
    };

    if !mad2sizes_valid(pixel_count as i32, pal_img_n, 0) {
        return s.err("outofmem");
    }
    let pixel_count = pixel_count as usize;
    let mut p = Vec::with_capacity(pixel_count * pal_img_n as usize);

    if pal_img_n == 3 {
        for &index in &orig[..pixel_count] {
            let n = usize::from(index) * 4;
            p.extend_from_slice(&palette[n..n + 3]);
        }
    } else {
        for &index in &orig[..pixel_count] {
            let n = usize::from(index) * 4;
            p.extend_from_slice(&palette[n..n + 4]);
        }
    }
    a.out = Pixels::U8(p);

    true
}

/// `stbi__de_iphone`
fn de_iphone(s: &Context, a: &mut Png) {
    let pixel_count = s.img_x.wrapping_mul(s.img_y) as usize;
    match &mut a.out {
        Pixels::U8(p) => de_iphone_bytes(s, p, pixel_count),
        Pixels::U16(wide) => {
            // (The original goes over a 16-bit image as bytes all the same, which is the
            // first half of it. As on a machine that keeps the low byte first.)
            let mut p: Vec<u8> = wide.iter().flat_map(|value| value.to_le_bytes()).collect();
            de_iphone_bytes(s, &mut p, pixel_count);
            for (value, bytes) in wide.iter_mut().zip(p.as_chunks::<2>().0.iter()) {
                *value = u16::from_le_bytes([bytes[0], bytes[1]]);
            }
        }
    }
}

fn de_iphone_bytes(s: &Context, p: &mut [u8], pixel_count: usize) {
    if s.img_out_n == 3 {
        // convert bgr to rgb
        for p in p[..pixel_count * 3].as_chunks_mut::<3>().0.iter_mut() {
            p.swap(0, 2);
        }
    } else {
        debug_assert!(s.img_out_n == 4);
        if s.options.unpremultiply_on_load {
            // convert bgr to rgb and unpremultiply
            for p in p[..pixel_count * 4].as_chunks_mut::<4>().0.iter_mut() {
                let a = u32::from(p[3]);
                let t = u32::from(p[0]);
                if a != 0 {
                    let half = a / 2;
                    p[0] = ((u32::from(p[2]) * 255 + half) / a) as u8;
                    p[1] = ((u32::from(p[1]) * 255 + half) / a) as u8;
                    p[2] = ((t * 255 + half) / a) as u8;
                } else {
                    p[0] = p[2];
                    p[2] = t as u8;
                }
            }
        } else {
            // convert bgr to rgb
            for p in p[..pixel_count * 4].as_chunks_mut::<4>().0.iter_mut() {
                p.swap(0, 2);
            }
        }
    }
}

/// `STBI__PNG_TYPE`
const fn png_type(name: &[u8; 4]) -> u32 {
    ((name[0] as u32) << 24) + ((name[1] as u32) << 16) + ((name[2] as u32) << 8) + name[3] as u32
}

const CGBI: u32 = png_type(b"CgBI");
const IHDR: u32 = png_type(b"IHDR");
const PLTE: u32 = png_type(b"PLTE");
const TRNS: u32 = png_type(b"tRNS");
const IDAT: u32 = png_type(b"IDAT");
const IEND: u32 = png_type(b"IEND");

/// `stbi__parse_png_file`
fn parse_png_file(s: &mut Context, z: &mut Png, scan: Scan, req_comp: i32) -> bool {
    let mut palette = [0u8; 1024];
    let mut pal_img_n = 0u8;
    let mut has_trans = false;
    let mut tc = [0u8; 3];
    let mut tc16 = [0u16; 3];
    let mut ioff = 0u32;
    let mut idata_limit = 0u32;
    let mut pal_len = 0u32;
    let mut first = true;
    let mut interlace = 0;
    let mut color = 0;

    z.idata = Vec::new();
    z.has_idata = false;
    z.out = Pixels::U8(Vec::new());
    z.is_iphone = false;

    if !check_png_header(s) {
        return false;
    }

    if scan == Scan::Type {
        return true;
    }

    loop {
        let c = get_chunk_header(s);
        match c.kind {
            CGBI => {
                z.is_iphone = true;
                s.skip(c.length as i32);
            }
            IHDR => {
                if !first {
                    return s.err("multiple IHDR");
                }
                first = false;
                if c.length != 13 {
                    return s.err("bad IHDR len");
                }
                s.img_x = s.get32be();
                s.img_y = s.get32be();
                if s.img_y > STBI_MAX_DIMENSIONS {
                    return s.err("too large");
                }
                if s.img_x > STBI_MAX_DIMENSIONS {
                    return s.err("too large");
                }
                z.depth = i32::from(s.get8());
                if z.depth != 1 && z.depth != 2 && z.depth != 4 && z.depth != 8 && z.depth != 16 {
                    return s.err("1/2/4/8/16-bit only");
                }
                color = i32::from(s.get8());
                if color > 6 {
                    return s.err("bad ctype");
                }
                if color == 3 && z.depth == 16 {
                    return s.err("bad ctype");
                }
                if color == 3 {
                    pal_img_n = 3;
                } else if color & 1 != 0 {
                    return s.err("bad ctype");
                }
                if z.depth < 8 && color != 0 && color != 3 {
                    return s.err("bad ctype");
                }
                let comp = s.get8();
                if comp != 0 {
                    return s.err("bad comp method");
                }
                let filter = s.get8();
                if filter != 0 {
                    return s.err("bad filter method");
                }
                interlace = s.get8();
                if interlace > 1 {
                    return s.err("bad interlace method");
                }
                if s.img_x == 0 || s.img_y == 0 {
                    return s.err("0-pixel image");
                }
                if pal_img_n == 0 {
                    s.img_n =
                        (if color & 2 != 0 { 3 } else { 1 }) + (if color & 4 != 0 { 1 } else { 0 });
                    if (1 << 30) / s.img_x / (s.img_n as u32) < s.img_y {
                        return s.err("too large");
                    }
                } else {
                    // if paletted, then pal_n is our final components, and
                    // img_n is # components to decompress/filter.
                    s.img_n = 1;
                    if (1 << 30) / s.img_x / 4 < s.img_y {
                        return s.err("too large");
                    }
                    palette = [0; 1024];
                }
                // even with SCAN_header, have to scan to see if we have a tRNS
            }
            PLTE => {
                if first {
                    return s.err("first not IHDR");
                }
                if c.length > 256 * 3 {
                    return s.err("invalid PLTE");
                }
                pal_len = c.length / 3;
                if pal_len * 3 != c.length {
                    return s.err("invalid PLTE");
                }
                for i in 0..pal_len as usize {
                    palette[i * 4] = s.get8();
                    palette[i * 4 + 1] = s.get8();
                    palette[i * 4 + 2] = s.get8();
                    palette[i * 4 + 3] = 255;
                }
            }
            TRNS => {
                if first {
                    return s.err("first not IHDR");
                }
                if z.has_idata {
                    return s.err("tRNS after IDAT");
                }
                if pal_img_n != 0 {
                    if scan == Scan::Header {
                        s.img_n = 4;
                        return true;
                    }
                    if pal_len == 0 {
                        return s.err("tRNS before PLTE");
                    }
                    if c.length > pal_len {
                        return s.err("bad tRNS len");
                    }
                    pal_img_n = 4;
                    for i in 0..c.length as usize {
                        palette[i * 4 + 3] = s.get8();
                    }
                } else {
                    if s.img_n & 1 == 0 {
                        return s.err("tRNS with alpha");
                    }
                    if c.length != s.img_n as u32 * 2 {
                        return s.err("bad tRNS len");
                    }
                    has_trans = true;
                    // non-paletted with tRNS = constant alpha. if header-scanning, we can stop now.
                    if scan == Scan::Header {
                        s.img_n += 1;
                        return true;
                    }
                    if z.depth == 16 {
                        for k in 0..(s.img_n as usize).min(3) {
                            tc16[k] = s.get16be() as u16; // copy the values as-is
                        }
                    } else {
                        for k in 0..(s.img_n as usize).min(3) {
                            tc[k] = ((s.get16be() & 255) as u8)
                                .wrapping_mul(STBI_DEPTH_SCALE_TABLE[z.depth as usize]);
                            // non 8-bit images will be larger
                        }
                    }
                }
            }
            IDAT => {
                if first {
                    return s.err("first not IHDR");
                }
                if pal_img_n != 0 && pal_len == 0 {
                    return s.err("no PLTE");
                }
                if scan == Scan::Header {
                    // header scan definitely stops at first IDAT
                    if pal_img_n != 0 {
                        s.img_n = i32::from(pal_img_n);
                    }
                    return true;
                }
                if c.length > (1 << 30) {
                    return s.err("IDAT size limit");
                }
                if !super::addsizes_valid(ioff as i32, c.length as i32) {
                    return false;
                }
                if ioff + c.length > idata_limit {
                    if idata_limit == 0 {
                        idata_limit = if c.length > 4096 { c.length } else { 4096 };
                    }
                    while ioff + c.length > idata_limit {
                        if idata_limit > i32::MAX as u32 / 2 {
                            return s.err("IDAT size limit");
                        }
                        idata_limit *= 2;
                    }
                    // (Where the original gets the memory `idata` points to: a first chunk
                    // of no bytes leaves it null.)
                    z.has_idata = true;
                }
                if !s.getn(&mut z.idata, c.length as i32) {
                    return s.err("outofdata");
                }
                ioff += c.length;
            }
            IEND => {
                if first {
                    return s.err("first not IHDR");
                }
                if scan != Scan::Load {
                    return true;
                }
                if !z.has_idata {
                    return s.err("no IDAT");
                }
                // initial guess for decoded data size to avoid unnecessary reallocs
                let bpl = (s.img_x * z.depth as u32 + 7) / 8; // bytes per line, per component
                let raw_len = bpl.wrapping_mul(s.img_y).wrapping_mul(s.img_n as u32) /* pixels */ .wrapping_add(s.img_y) /* filter mode per row */;
                let expanded = match zlib::decode_for_png(&z.idata, raw_len, !z.is_iphone) {
                    Ok(expanded) => expanded,
                    Err(reason) => {
                        // zlib should set error
                        if let Some(reason) = reason {
                            s.reason = reason;
                        }
                        return false;
                    }
                };
                z.idata = Vec::new();
                if (req_comp == s.img_n + 1 && req_comp != 3 && pal_img_n == 0) || has_trans {
                    s.img_out_n = s.img_n + 1;
                } else {
                    s.img_out_n = s.img_n;
                }
                if !create_png_image(s, z, &expanded, s.img_out_n, z.depth, color, interlace != 0) {
                    return false;
                }
                if has_trans {
                    match &mut z.out {
                        Pixels::U16(out) => compute_transparency(out, tc16, s.img_out_n),
                        Pixels::U8(out) => compute_transparency(out, tc, s.img_out_n),
                    }
                }
                if z.is_iphone && s.options.convert_iphone_png_to_rgb && s.img_out_n > 2 {
                    de_iphone(s, z);
                }
                if pal_img_n != 0 {
                    // pal_img_n == 3 or 4
                    s.img_n = i32::from(pal_img_n); // record the actual colors we had
                    s.img_out_n = i32::from(pal_img_n);
                    if req_comp >= 3 {
                        s.img_out_n = req_comp;
                    }
                    if !expand_png_palette(s, z, &palette, s.img_out_n) {
                        return false;
                    }
                } else if has_trans {
                    // non-paletted image with tRNS -> source image has (constant) alpha
                    s.img_n += 1;
                }
                // end of PNG chunk, read and skip CRC
                s.get32be();
                return true;
            }
            _ => {
                // if critical, fail
                if first {
                    return s.err("first not IHDR");
                }
                if c.kind & (1 << 29) == 0 {
                    return s.err("PNG chunk not known");
                }
                s.skip(c.length as i32);
            }
        }
        // end of PNG chunk, read and skip CRC
        s.get32be();
    }
}

/// `stbi__png_load`, `stbi__do_png`: the pixels and how many channels the file has.
pub(crate) fn png_load(s: &mut Context, req_comp: i32) -> Option<(Pixels, i32)> {
    let mut p = Png {
        idata: Vec::new(),
        has_idata: false,
        out: Pixels::U8(Vec::new()),
        depth: 0,
        is_iphone: false,
    };
    if !(0..=4).contains(&req_comp) {
        return s.errp("bad req_comp");
    }
    if !parse_png_file(s, &mut p, Scan::Load, req_comp) {
        return None;
    }
    let mut result = p.out;
    if req_comp != 0 && req_comp != s.img_out_n {
        result = match result {
            Pixels::U8(result) => Pixels::U8(convert_format(
                s,
                result,
                s.img_out_n,
                req_comp,
                s.img_x,
                s.img_y,
            )?),
            Pixels::U16(result) => Pixels::U16(convert_format(
                s,
                result,
                s.img_out_n,
                req_comp,
                s.img_x,
                s.img_y,
            )?),
        };
        s.img_out_n = req_comp;
    }
    Some((result, s.img_n))
}

/// `stbi__png_test`
pub(crate) fn png_test(s: &mut Context) -> bool {
    let r = check_png_header(s);
    s.rewind();
    r
}

/// `stbi__png_info`
pub(crate) fn png_info(s: &mut Context) -> Option<Info> {
    let mut p = Png {
        idata: Vec::new(),
        has_idata: false,
        out: Pixels::U8(Vec::new()),
        depth: 0,
        is_iphone: false,
    };
    if !parse_png_file(s, &mut p, Scan::Header, 0) {
        s.rewind();
        return None;
    }
    Some(Info {
        kind: ImageType::Png,
        x: s.img_x as i32,
        y: s.img_y as i32,
        comp: s.img_n,
        is_16_bit: p.depth == 16,
        is_hdr: false,
        is_iphone_png: p.is_iphone,
    })
}

#[cfg(test)]
mod tests {
    use super::super::{
        LoadError, LoadOptions, info_from_memory, is_16_bit_from_memory, load_16_from_memory,
        load_from_memory,
    };
    use super::*;
    use miniz_nrp::{self as miniz, Level, crc32};
    use stb_image_write_nrp::{Channels, Image, PngFilter, WriteOptions, write_png};

    fn chunk(file: &mut Vec<u8>, name: &[u8; 4], bytes: &[u8]) {
        file.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
        let from = file.len();
        file.extend_from_slice(name);
        file.extend_from_slice(bytes);
        let crc = crc32(&file[from..]);
        file.extend_from_slice(&crc.to_be_bytes());
    }

    /// A PNG file of rows already filtered (each with its filter byte in front).
    fn png(
        x: u32,
        y: u32,
        depth: u8,
        color: u8,
        interlace: u8,
        more: &[(&[u8; 4], &[u8])],
        rows: &[u8],
    ) -> Vec<u8> {
        let mut file = vec![137, 80, 78, 71, 13, 10, 26, 10];
        let mut header = Vec::new();
        header.extend_from_slice(&x.to_be_bytes());
        header.extend_from_slice(&y.to_be_bytes());
        header.extend_from_slice(&[depth, color, 0, 0, interlace]);
        chunk(&mut file, b"IHDR", &header);
        for (name, bytes) in more {
            chunk(&mut file, name, bytes);
        }
        chunk(
            &mut file,
            b"IDAT",
            &miniz::compress(rows, Level::new(6).unwrap()),
        );
        chunk(&mut file, b"IEND", &[]);
        file
    }

    fn picture(x: usize, y: usize, comp: usize) -> Vec<u8> {
        (0..x * y * comp)
            .map(|i| {
                ((i / comp % x) * 23 + (i / comp / x) * 41 + i % comp * 70 + (i * i) % 7) as u8
            })
            .collect()
    }

    #[test]
    fn what_the_writer_writes_comes_back() {
        let options = LoadOptions::default();
        for comp in 1..=4 {
            let pixels = picture(19, 11, comp);
            let image =
                Image::new(&pixels, 19, 11, Channels::from_count(comp as u8).unwrap()).unwrap();
            for filter in [
                None,
                Some(PngFilter::None),
                Some(PngFilter::Sub),
                Some(PngFilter::Up),
                Some(PngFilter::Average),
                Some(PngFilter::Paeth),
            ] {
                let file = write_png(
                    &image,
                    &WriteOptions {
                        png_filter: filter,
                        ..WriteOptions::default()
                    },
                );
                let loaded = load_from_memory(&file, 0, &options).unwrap();
                assert_eq!(
                    (loaded.x, loaded.y, loaded.channels_in_file),
                    (19, 11, comp as i32)
                );
                assert_eq!(loaded.data, pixels, "{comp} channels, filter {filter:?}");
                let info = info_from_memory(&file).unwrap();
                assert_eq!(
                    (info.kind, info.x, info.y, info.comp, info.is_16_bit),
                    (ImageType::Png, 19, 11, comp as i32, false)
                );
            }
            // Other numbers of channels than the file has.
            let file = write_png(&image, &WriteOptions::default());
            for wanted in 1..=4 {
                let loaded = load_from_memory(&file, wanted, &options).unwrap();
                assert_eq!(loaded.channels_in_file, comp as i32);
                let mut s = Context::new(&[], &options);
                assert_eq!(
                    loaded.data,
                    convert_format(&mut s, pixels.clone(), comp as i32, wanted, 19, 11).unwrap()
                );
            }
            // Upside down, and as 16 bits.
            let flipped = load_from_memory(
                &file,
                0,
                &LoadOptions {
                    flip_vertically_on_load: true,
                    ..options
                },
            )
            .unwrap();
            let rows: Vec<u8> = pixels.chunks(19 * comp).rev().flatten().copied().collect();
            assert_eq!(flipped.data, rows);
            let wide = load_16_from_memory(&file, 0, &options).unwrap();
            assert!(
                wide.data
                    .iter()
                    .zip(&pixels)
                    .all(|(&wide, &narrow)| wide == u16::from(narrow) * 257)
            );
        }
    }

    #[test]
    fn sixteen_bits_and_fewer_than_eight() {
        let options = LoadOptions::default();
        // 16 bits, grey with alpha: two pixels on one row.
        let file = png(
            2,
            1,
            16,
            4,
            0,
            &[],
            &[0, 0x12, 0x34, 0xff, 0xff, 0xab, 0xcd, 0x00, 0x01],
        );
        assert!(is_16_bit_from_memory(&file));
        let wide = load_16_from_memory(&file, 0, &options).unwrap();
        assert_eq!(
            (wide.channels_in_file, &wide.data[..]),
            (2, &[0x1234u16, 0xffff, 0xabcd, 0x0001][..])
        );
        assert_eq!(
            load_from_memory(&file, 0, &options).unwrap().data,
            [0x12, 0xff, 0xab, 0x00]
        );
        assert_eq!(
            load_16_from_memory(&file, 4, &options).unwrap().data,
            [
                0x1234, 0x1234, 0x1234, 0xffff, 0xabcd, 0xabcd, 0xabcd, 0x0001
            ]
        );
        // 16 bits, RGB, with one colour made transparent.
        let file = png(
            2,
            1,
            16,
            2,
            0,
            &[(b"tRNS", &[0, 1, 0, 2, 0, 3])],
            &[0, 0, 1, 0, 2, 0, 3, 0, 1, 0, 2, 0, 4],
        );
        let wide = load_16_from_memory(&file, 0, &options).unwrap();
        assert_eq!(
            (wide.channels_in_file, &wide.data[..]),
            (4, &[1u16, 2, 3, 0, 1, 2, 4, 0xffff][..])
        );
        // 1, 2 and 4 bits of grey, scaled to the whole range.
        let file = png(10, 1, 1, 0, 0, &[], &[0, 0b1010_0011, 0b0100_0000]);
        assert_eq!(
            load_from_memory(&file, 0, &options).unwrap().data,
            [255, 0, 255, 0, 0, 0, 255, 255, 0, 255]
        );
        let file = png(3, 1, 2, 0, 0, &[], &[0, 0b0001_1011]);
        assert_eq!(
            load_from_memory(&file, 0, &options).unwrap().data,
            [0, 85, 170]
        );
        let file = png(3, 2, 4, 0, 0, &[], &[0, 0x01, 0xf0, 2, 0x10, 0x00]);
        assert_eq!(
            load_from_memory(&file, 0, &options).unwrap().data,
            [0, 17, 255, 17, 17, 255]
        );
        // 2 bits of grey with a transparent value, asked for with and without alpha.
        let file = png(3, 1, 2, 0, 0, &[(b"tRNS", &[0, 1])], &[0, 0b0001_1011]);
        let loaded = load_from_memory(&file, 0, &options).unwrap();
        assert_eq!(
            (loaded.channels_in_file, &loaded.data[..]),
            (2, &[0u8, 255, 85, 0, 170, 255][..])
        );
        assert_eq!(info_from_memory(&file).unwrap().comp, 2);
    }

    #[test]
    fn palettes() {
        let options = LoadOptions::default();
        let colours = [10, 20, 30, 40, 50, 60, 70, 80, 90];
        // 8 bits of index; the last pixel's index has no colour, and is black.
        let file = png(4, 1, 8, 3, 0, &[(b"PLTE", &colours)], &[0, 2, 0, 1, 7]);
        let loaded = load_from_memory(&file, 0, &options).unwrap();
        assert_eq!(
            (loaded.channels_in_file, &loaded.data[..]),
            (3, &[70u8, 80, 90, 10, 20, 30, 40, 50, 60, 0, 0, 0][..])
        );
        assert_eq!(
            load_from_memory(&file, 4, &options).unwrap().data,
            [
                70, 80, 90, 255, 10, 20, 30, 255, 40, 50, 60, 255, 0, 0, 0, 0
            ]
        );
        // 2 bits of index, with alphas for the first two colours.
        let file = png(
            3,
            1,
            2,
            3,
            0,
            &[(b"PLTE", &colours), (b"tRNS", &[0, 128])],
            &[0, 0b0001_1000],
        );
        let loaded = load_from_memory(&file, 0, &options).unwrap();
        assert_eq!(
            (loaded.channels_in_file, &loaded.data[..]),
            (4, &[10u8, 20, 30, 0, 40, 50, 60, 128, 70, 80, 90, 255][..])
        );
        assert_eq!(info_from_memory(&file).unwrap().comp, 4);
        assert_eq!(load_from_memory(&file, 1, &options).unwrap().data.len(), 3);
        // No palette, alphas before it, alphas for more colours than there are.
        assert_eq!(
            load_from_memory(&png(1, 1, 8, 3, 0, &[], &[0, 0]), 0, &options),
            Err(LoadError("no PLTE"))
        );
        assert_eq!(
            load_from_memory(
                &png(
                    1,
                    1,
                    8,
                    3,
                    0,
                    &[(b"tRNS", &[1]), (b"PLTE", &colours)],
                    &[0, 0]
                ),
                0,
                &options
            ),
            Err(LoadError("tRNS before PLTE"))
        );
        assert_eq!(
            load_from_memory(
                &png(
                    1,
                    1,
                    8,
                    3,
                    0,
                    &[(b"PLTE", &colours), (b"tRNS", &[1, 2, 3, 4])],
                    &[0, 0]
                ),
                0,
                &options
            ),
            Err(LoadError("bad tRNS len"))
        );
    }

    #[test]
    fn interlaced() {
        let options = LoadOptions::default();
        // 5 by 5 grey, each pixel its row times 16 plus its column, in the seven passes.
        let passes: [&[(usize, usize)]; 7] = [
            &[(0, 0)],
            &[(4, 0)],
            &[(0, 4), (4, 4)],
            &[(2, 0), (2, 4)],
            &[(0, 2), (2, 2), (4, 2)],
            &[(1, 0), (3, 0), (1, 2), (3, 2), (1, 4), (3, 4)],
            &[
                (0, 1),
                (1, 1),
                (2, 1),
                (3, 1),
                (4, 1),
                (0, 3),
                (1, 3),
                (2, 3),
                (3, 3),
                (4, 3),
            ],
        ];
        let mut rows = Vec::new();
        for pass in passes {
            let mut last = usize::MAX;
            for &(x, y) in pass {
                if y != last {
                    rows.push(0);
                    last = y;
                }
                rows.push((y * 16 + x) as u8);
            }
        }
        let file = png(5, 5, 8, 0, 1, &[], &rows);
        let expected: Vec<u8> = (0..25).map(|i| (i / 5 * 16 + i % 5) as u8).collect();
        assert_eq!(load_from_memory(&file, 0, &options).unwrap().data, expected);
        // With too few rows.
        let file = png(5, 5, 8, 0, 1, &[], &rows[..rows.len() - 1]);
        assert_eq!(
            load_from_memory(&file, 0, &options),
            Err(LoadError("not enough pixels"))
        );
    }

    #[test]
    fn iphone_files() {
        // Blue first, deflate with no zlib header, and a chunk that says so in front.
        let rows = [0, 30, 20, 10, 128];
        let mut file = vec![137, 80, 78, 71, 13, 10, 26, 10];
        chunk(&mut file, b"CgBI", &[0, 0, 0, 0]);
        chunk(&mut file, b"IHDR", &[0, 0, 0, 1, 0, 0, 0, 1, 8, 6, 0, 0, 0]);
        chunk(
            &mut file,
            b"IDAT",
            &miniz::compress(&rows, Level::new(6).unwrap())[2..],
        );
        chunk(&mut file, b"IEND", &[]);
        let options = LoadOptions::default();
        assert!(info_from_memory(&file).unwrap().is_iphone_png);
        assert_eq!(
            load_from_memory(&file, 0, &options).unwrap().data,
            [30, 20, 10, 128]
        );
        let red_first = LoadOptions {
            convert_iphone_png_to_rgb: true,
            ..options
        };
        assert_eq!(
            load_from_memory(&file, 0, &red_first).unwrap().data,
            [10, 20, 30, 128]
        );
        let divided = LoadOptions {
            unpremultiply_on_load: true,
            ..red_first
        };
        assert_eq!(
            load_from_memory(&file, 0, &divided).unwrap().data,
            [20, 40, 60, 128]
        );
    }

    #[test]
    fn damaged_files_are_refused() {
        let options = LoadOptions::default();
        let good = png(
            2,
            2,
            8,
            2,
            0,
            &[],
            &[0, 1, 2, 3, 4, 5, 6, 0, 1, 2, 3, 4, 5, 6],
        );
        assert!(load_from_memory(&good, 0, &options).is_ok());
        let refused = |file: &[u8]| load_from_memory(file, 0, &options).unwrap_err().0;
        assert_eq!(refused(&good[..8]), "first not IHDR");
        assert_eq!(refused(&good[..45]), "outofdata");
        assert_eq!(
            refused(&png(
                2,
                2,
                8,
                2,
                0,
                &[],
                &[0, 1, 2, 3, 4, 5, 6, 0, 1, 2, 3, 4, 5]
            )),
            "not enough pixels"
        );
        assert_eq!(
            refused(&png(
                2,
                2,
                8,
                2,
                0,
                &[],
                &[0, 1, 2, 3, 4, 5, 6, 5, 1, 2, 3, 4, 5, 6]
            )),
            "invalid filter"
        );
        assert_eq!(
            refused(&png(2, 2, 3, 2, 0, &[], &[])),
            "1/2/4/8/16-bit only"
        );
        assert_eq!(refused(&png(2, 2, 8, 5, 0, &[], &[])), "bad ctype");
        assert_eq!(refused(&png(2, 2, 4, 2, 0, &[], &[])), "bad ctype");
        assert_eq!(refused(&png(0, 2, 8, 2, 0, &[], &[])), "0-pixel image");
        assert_eq!(refused(&png(1 << 25, 2, 8, 2, 0, &[], &[])), "too large");
        assert_eq!(
            refused(&png(1 << 16, 1 << 16, 8, 2, 0, &[], &[])),
            "too large"
        );
        assert_eq!(
            refused(&png(2, 2, 8, 2, 2, &[], &[])),
            "bad interlace method"
        );
        assert_eq!(
            refused(&png(2, 2, 8, 2, 0, &[(b"ABCD", &[1, 2])], &[])),
            "PNG chunk not known"
        );
        assert_eq!(
            refused(&png(2, 2, 8, 6, 0, &[(b"tRNS", &[0, 0])], &[])),
            "tRNS with alpha"
        );
        // A chunk that may be skipped is.
        let with_text = png(
            2,
            2,
            8,
            2,
            0,
            &[(b"tEXt", b"key\0value")],
            &[0, 1, 2, 3, 4, 5, 6, 0, 1, 2, 3, 4, 5, 6],
        );
        assert_eq!(
            load_from_memory(&with_text, 0, &options),
            load_from_memory(&good, 0, &options)
        );
        // No pixels at all, and pixels that are no zlib stream.
        let mut file = good[..33].to_vec();
        chunk(&mut file, b"IEND", &[]);
        assert_eq!(refused(&file), "no IDAT");
        let mut file = good[..33].to_vec();
        chunk(&mut file, b"IDAT", &[1, 2, 3, 4]);
        chunk(&mut file, b"IEND", &[]);
        assert_eq!(refused(&file), "bad zlib header");
    }
}
