//! Image files to pixels: a native Rust port of Sean Barrett's
//! [`stb_image.h`](https://github.com/nothings/stb) v2.32, every reader it has, with the zlib
//! decoder its PNG reader uses. Each gives the pixels
//! the original gives, and refuses a file the original refuses, with the same reason.
//!
//! - PNG: 1, 2, 4, 8 and 16 bits a channel, grey, colour and palette, with and without alpha
//!   and interlacing.
//! - BMP: 1, 4, 8, 16, 24 and 32 bits a pixel, without run-length encoding.
//! - JPEG: baseline and progressive, 8 bits a channel.
//! - GIF: every frame, with its delay ([`load_gif_from_memory`]), or the first.
//! - TGA: grey, colour and palette, 8 to 32 bits a pixel, with and without run-length encoding.
//! - PSD: the merged image of an RGB file, 8 or 16 bits a channel, raw or run-length encoded.
//! - PNM: `.pgm` and `.ppm`, binary and text, 8 or 16 bits a channel.
//! - HDR: Radiance `.hdr` files, flat or run-length encoded, as floats
//!   ([`loadf_from_memory`]).
//!
//! [`load_from_memory`] gives 8 bits a channel, [`load_16_from_memory`] 16 and
//! [`loadf_from_memory`] floats; [`info_from_memory`] reads only as far as the size. The
//! settings the original keeps in globals are a [`LoadOptions`] handed to each call.
//!
//! What is ported is the original as it builds with `STBI_NO_SIMD` (it has the one answer for
//! every file; with SSE2 or NEON the original's JPEG reader gives other pixels for some damaged
//! files). Reading is from memory only. Where the original uses memory it has not set, which a
//! damaged file can bring it to, the port reads zero.

#![forbid(unsafe_code)]
// The code follows the original's line for line, and these are its shapes.
#![allow(
    clippy::needless_range_loop,
    clippy::too_many_arguments,
    clippy::manual_range_contains
)]
// (The arithmetic is written as the original has it.)
#![allow(clippy::manual_div_ceil, clippy::int_plus_one)]

mod bmp;
mod gif;
mod hdr;
mod jpeg;
mod png;
mod pnm;
mod psd;
mod tga;
pub mod zlib;

pub use gif::LoadedGif;

/// Why a file was refused: the original's `stbi_failure_reason()`, its short words. A few of
/// its failures set no reason, and the original then shows whatever an earlier call left;
/// here that is the empty string, or the reason an earlier step of the same call left.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LoadError(pub &'static str);

impl std::fmt::Display for LoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.0.is_empty() {
            f.write_str("the image could not be read")
        } else {
            f.write_str(self.0)
        }
    }
}

impl std::error::Error for LoadError {}

/// The settings the original keeps in globals.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LoadOptions {
    /// Give the image with its last row first: `stbi_set_flip_vertically_on_load`.
    pub flip_vertically_on_load: bool,
    /// For an iPhone PNG whose colours were multiplied by its alpha, divide them back:
    /// `stbi_set_unpremultiply_on_load`. Only does anything with the setting below.
    pub unpremultiply_on_load: bool,
    /// Give an iPhone PNG (one with a `CgBI` chunk), which is stored blue first, red first:
    /// `stbi_convert_iphone_png_to_rgb`.
    pub convert_iphone_png_to_rgb: bool,
    /// The gamma an HDR file's floats are brought to 8 bits with: `stbi_hdr_to_ldr_gamma`.
    /// 2.2 unless changed.
    pub hdr_to_ldr_gamma: f32,
    /// What an HDR file's floats are divided by first: `stbi_hdr_to_ldr_scale`. 1 unless
    /// changed.
    pub hdr_to_ldr_scale: f32,
    /// The gamma an 8-bit file's values are made floats with by [`loadf_from_memory`]:
    /// `stbi_ldr_to_hdr_gamma`. 2.2 unless changed.
    pub ldr_to_hdr_gamma: f32,
    /// What those floats are multiplied by: `stbi_ldr_to_hdr_scale`. 1 unless changed.
    pub ldr_to_hdr_scale: f32,
}

impl Default for LoadOptions {
    fn default() -> LoadOptions {
        LoadOptions {
            flip_vertically_on_load: false,
            unpremultiply_on_load: false,
            convert_iphone_png_to_rgb: false,
            hdr_to_ldr_gamma: 2.2,
            hdr_to_ldr_scale: 1.0,
            ldr_to_hdr_gamma: 2.2,
            ldr_to_hdr_scale: 1.0,
        }
    }
}

/// An image out of a file: `x` by `y` pixels from the top row, each of as many values as were
/// asked for, or of `channels_in_file` when none were (1 grey, 2 grey and alpha, 3 RGB, 4
/// RGBA).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Loaded<T> {
    pub x: i32,
    pub y: i32,
    /// How many channels the file itself has, whatever was asked for.
    pub channels_in_file: i32,
    pub data: Vec<T>,
}

/// The kind of a file: `STBI_type_*`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageType {
    Jpeg,
    Png,
    Gif,
    Bmp,
    Psd,
    Pnm,
    Hdr,
    Tga,
}

impl ImageType {
    /// The original's number for the kind: `STBI_type_jpeg` and the rest, from 1.
    pub fn number(self) -> i32 {
        self as i32 + 1
    }
}

/// What [`info_from_memory`] finds: `stbi_info_res`. (For a BMP stored top row first, `y` is
/// below zero, as in the original.)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Info {
    pub kind: ImageType,
    pub x: i32,
    pub y: i32,
    pub comp: i32,
    pub is_16_bit: bool,
    /// Whether the file is a Radiance `.hdr` file, whose values are floats.
    pub is_hdr: bool,
    pub is_iphone_png: bool,
}

/// `STBI_MAX_DIMENSIONS`
pub(crate) const STBI_MAX_DIMENSIONS: u32 = 1 << 24;

/// `STBI__SCAN_*`
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Scan {
    Load,
    Type,
    Header,
}

/// `stbi__context`: the file's bytes and how far into them the reader is, with what the
/// readers share. (Also the failure reason and the settings, which are globals in the
/// original.)
pub(crate) struct Context<'a> {
    pub img_x: u32,
    pub img_y: u32,
    pub img_n: i32,
    pub img_out_n: i32,
    buffer: &'a [u8],
    /// (Can be past the end, as the original's pointer can.)
    pos: usize,
    pub reason: &'static str,
    pub options: LoadOptions,
}

impl<'a> Context<'a> {
    /// `stbi__start_mem`
    fn new(buffer: &'a [u8], options: &LoadOptions) -> Context<'a> {
        Context {
            img_x: 0,
            img_y: 0,
            img_n: 0,
            img_out_n: 0,
            buffer,
            pos: 0,
            reason: "",
            options: *options,
        }
    }

    /// `stbi__err`
    pub fn err(&mut self, reason: &'static str) -> bool {
        self.reason = reason;
        false
    }

    /// `stbi__errpuc`
    pub fn errp<T>(&mut self, reason: &'static str) -> Option<T> {
        self.reason = reason;
        None
    }

    /// `stbi__rewind`
    pub fn rewind(&mut self) {
        self.pos = 0;
    }

    /// `stbi__get8`
    pub fn get8(&mut self) -> u8 {
        if self.pos < self.buffer.len() {
            self.pos += 1;
            return self.buffer[self.pos - 1];
        }
        0
    }

    /// `stbi__at_eof`
    pub fn at_eof(&self) -> bool {
        self.pos >= self.buffer.len()
    }

    /// `stbi__skip`
    pub fn skip(&mut self, n: i32) {
        if n == 0 {
            return; // already there!
        }
        if n < 0 {
            self.pos = self.buffer.len();
            return;
        }
        self.pos = self.pos.saturating_add(n as usize);
    }

    /// `stbi__getn`, the bytes added to `into`.
    pub fn getn(&mut self, into: &mut Vec<u8>, n: i32) -> bool {
        if n <= 0 {
            return true;
        }
        let n = n as usize;
        if self.pos <= self.buffer.len() && n <= self.buffer.len() - self.pos {
            into.extend_from_slice(&self.buffer[self.pos..self.pos + n]);
            self.pos += n;
            true
        } else {
            false
        }
    }

    /// `stbi__get16be`
    pub fn get16be(&mut self) -> i32 {
        let z = i32::from(self.get8());
        (z << 8) + i32::from(self.get8())
    }

    /// `stbi__get32be`
    pub fn get32be(&mut self) -> u32 {
        let z = self.get16be() as u32;
        (z << 16).wrapping_add(self.get16be() as u32)
    }

    /// `stbi__get16le`
    pub fn get16le(&mut self) -> i32 {
        let z = i32::from(self.get8());
        z + (i32::from(self.get8()) << 8)
    }

    /// `stbi__getn` into the whole of `into`.
    pub fn getn_into(&mut self, into: &mut [u8]) -> bool {
        if into.is_empty() {
            return true;
        }
        let n = into.len();
        if self.pos <= self.buffer.len() && n <= self.buffer.len() - self.pos {
            into.copy_from_slice(&self.buffer[self.pos..self.pos + n]);
            self.pos += n;
            true
        } else {
            false
        }
    }

    /// `stbi__get32le`
    pub fn get32le(&mut self) -> u32 {
        let z = self.get16le() as u32;
        z.wrapping_add((self.get16le() as u32) << 16)
    }
}

// stb_image uses ints pervasively, including for offset calculations.
// therefore the largest decoded image size we can support with the
// current code, even on 64-bit targets, is INT_MAX. this is not a
// significant limitation for the intended use case.
//
// we do, however, need to make sure our size calculations don't
// overflow. hence a few helper functions for size calculations that
// multiply integers together, making sure that they're non-negative
// and no overflow occurs.

/// `stbi__addsizes_valid`: return 1 if the sum is valid, 0 on overflow. negative terms are
/// considered invalid.
pub(crate) fn addsizes_valid(a: i32, b: i32) -> bool {
    if b < 0 {
        return false;
    }
    // now 0 <= b <= INT_MAX, hence also
    // 0 <= INT_MAX - b <= INTMAX.
    // And "a + b <= INT_MAX" (which might overflow) is the
    // same as a <= INT_MAX - b (no overflow)
    a <= i32::MAX - b
}

/// `stbi__mul2sizes_valid`: returns 1 if the product is valid, 0 on overflow. negative factors
/// are considered invalid.
pub(crate) fn mul2sizes_valid(a: i32, b: i32) -> bool {
    if a < 0 || b < 0 {
        return false;
    }
    if b == 0 {
        return true; // mul-by-0 is always safe
    }
    // portable way to check for no overflows in a*b
    a <= i32::MAX / b
}

/// `stbi__mad2sizes_valid`: returns 1 if "a*b + add" has no negative terms/factors and doesn't
/// overflow
pub(crate) fn mad2sizes_valid(a: i32, b: i32, add: i32) -> bool {
    mul2sizes_valid(a, b) && addsizes_valid(a.wrapping_mul(b), add)
}

/// `stbi__mad3sizes_valid`: returns 1 if "a*b*c + add" has no negative terms/factors and
/// doesn't overflow
pub(crate) fn mad3sizes_valid(a: i32, b: i32, c: i32, add: i32) -> bool {
    mul2sizes_valid(a, b)
        && mul2sizes_valid(a.wrapping_mul(b), c)
        && addsizes_valid(a.wrapping_mul(b).wrapping_mul(c), add)
}

/// `stbi__mad4sizes_valid`: returns 1 if "a*b*c*d + add" has no negative terms/factors and
/// doesn't overflow
pub(crate) fn mad4sizes_valid(a: i32, b: i32, c: i32, d: i32, add: i32) -> bool {
    mul2sizes_valid(a, b)
        && mul2sizes_valid(a.wrapping_mul(b), c)
        && mul2sizes_valid(a.wrapping_mul(b).wrapping_mul(c), d)
        && addsizes_valid(a.wrapping_mul(b).wrapping_mul(c).wrapping_mul(d), add)
}

/// `stbi__addints_valid`: returns 1 if the sum of two signed ints is valid (between -2^31 and
/// 2^31-1 inclusive), 0 on overflow.
pub(crate) fn addints_valid(a: i32, b: i32) -> bool {
    a.checked_add(b).is_some()
}

/// `stbi__mul2shorts_valid`: returns 1 if the product of two ints fits in a signed short, 0 on
/// overflow.
pub(crate) fn mul2shorts_valid(a: i32, b: i32) -> bool {
    if b == 0 || b == -1 {
        return true; // multiplication by 0 is always 0; check for -1 so SHRT_MIN/b doesn't overflow
    }
    if (a >= 0) == (b >= 0) {
        return a <= i32::from(i16::MAX) / b; // product is positive, so similar to mul2sizes_valid
    }
    if b < 0 {
        return a <= i32::from(i16::MIN) / b; // same as a * b >= SHRT_MIN
    }
    a >= i32::from(i16::MIN) / b
}

/// What the pixels are held in: 8 bits a channel or 16.
pub(crate) enum Pixels {
    U8(Vec<u8>),
    U16(Vec<u16>),
}

/// A value of a channel, of 8 bits or of 16.
pub(crate) trait Sample: Copy + Default + PartialEq {
    /// All of it: opaque, as an alpha.
    const FULL: Self;
    /// `stbi__compute_y`, `stbi__compute_y_16`
    fn compute_y(r: Self, g: Self, b: Self) -> Self;
}

/// `stbi__compute_y`
pub(crate) fn compute_y(r: i32, g: i32, b: i32) -> u8 {
    (((r * 77) + (g * 150) + (29 * b)) >> 8) as u8
}

impl Sample for u8 {
    const FULL: u8 = 255;
    fn compute_y(r: u8, g: u8, b: u8) -> u8 {
        compute_y(i32::from(r), i32::from(g), i32::from(b))
    }
}

impl Sample for u16 {
    const FULL: u16 = 0xffff;
    fn compute_y(r: u16, g: u16, b: u16) -> u16 {
        (((i32::from(r) * 77) + (i32::from(g) * 150) + (29 * i32::from(b))) >> 8) as u16
    }
}

//  generic converter from built-in img_n to req_comp
//    individual types do this automatically as much as possible (e.g. jpeg
//    does all cases internally since it needs to colorspace convert anyway,
//    and it never has alpha, so very few cases ). png can automatically
//    interleave an alpha=255 channel, but falls back to this for other cases

/// `stbi__convert_format`, `stbi__convert_format16`
pub(crate) fn convert_format<T: Sample>(
    s: &mut Context,
    data: Vec<T>,
    img_n: i32,
    req_comp: i32,
    x: u32,
    y: u32,
) -> Option<Vec<T>> {
    if req_comp == img_n {
        return Some(data);
    }
    debug_assert!((1..=4).contains(&req_comp));

    if !mad3sizes_valid(req_comp, x as i32, y as i32, 0) {
        return s.errp("outofmem");
    }
    let (x, y, img_n, req_comp) = (x as usize, y as usize, img_n as usize, req_comp as usize);
    let mut good = vec![T::default(); req_comp * x * y];

    for j in 0..y {
        let src = &data[j * x * img_n..(j + 1) * x * img_n];
        let dest = &mut good[j * x * req_comp..(j + 1) * x * req_comp];
        let pixels = src.chunks_exact(img_n).zip(dest.chunks_exact_mut(req_comp));
        // convert source image with img_n components to one with req_comp components
        match (img_n, req_comp) {
            (1, 2) => pixels.for_each(|(src, dest)| dest.copy_from_slice(&[src[0], T::FULL])),
            (1, 3) => {
                pixels.for_each(|(src, dest)| dest.copy_from_slice(&[src[0], src[0], src[0]]))
            }
            (1, 4) => pixels
                .for_each(|(src, dest)| dest.copy_from_slice(&[src[0], src[0], src[0], T::FULL])),
            (2, 1) => pixels.for_each(|(src, dest)| dest[0] = src[0]),
            (2, 3) => {
                pixels.for_each(|(src, dest)| dest.copy_from_slice(&[src[0], src[0], src[0]]))
            }
            (2, 4) => pixels
                .for_each(|(src, dest)| dest.copy_from_slice(&[src[0], src[0], src[0], src[1]])),
            (3, 4) => pixels
                .for_each(|(src, dest)| dest.copy_from_slice(&[src[0], src[1], src[2], T::FULL])),
            (3, 1) => pixels.for_each(|(src, dest)| dest[0] = T::compute_y(src[0], src[1], src[2])),
            (3, 2) => pixels.for_each(|(src, dest)| {
                dest.copy_from_slice(&[T::compute_y(src[0], src[1], src[2]), T::FULL])
            }),
            (4, 1) => pixels.for_each(|(src, dest)| dest[0] = T::compute_y(src[0], src[1], src[2])),
            (4, 2) => pixels.for_each(|(src, dest)| {
                dest.copy_from_slice(&[T::compute_y(src[0], src[1], src[2]), src[3]])
            }),
            (4, 3) => {
                pixels.for_each(|(src, dest)| dest.copy_from_slice(&[src[0], src[1], src[2]]))
            }
            _ => return s.errp("unsupported"),
        }
    }

    Some(good)
}

/// `stbi__load_main`: the pixels, and into the context the image's size; with them, how many
/// channels the file has. `bpc` is the bits a channel the caller wants, which only the PSD
/// reader looks at.
fn load_main(s: &mut Context, req_comp: i32, bpc: i32) -> Option<(Pixels, i32)> {
    if !(0..=4).contains(&req_comp) {
        return s.errp("bad req_comp");
    }

    // test the formats with a very explicit header first (at least a FOURCC
    // or distinctive magic number first)
    if png::png_test(s) {
        return png::png_load(s, req_comp);
    }
    if bmp::bmp_test(s) {
        return bmp::bmp_load(s, req_comp).map(|(out, comp)| (Pixels::U8(out), comp));
    }
    if gif::gif_test(s) {
        return gif::gif_load(s, req_comp).map(|(out, comp)| (Pixels::U8(out), comp));
    }
    if psd::psd_test(s) {
        return psd::psd_load(s, req_comp, bpc);
    }

    // then the formats that can end up attempting to load with just 1 or 2
    // bytes matching expectations; these are prone to false positives, so
    // try them later
    if jpeg::jpeg_test(s) {
        return jpeg::jpeg_load(s, req_comp).map(|(out, comp)| (Pixels::U8(out), comp));
    }
    if pnm::pnm_test(s) {
        return pnm::pnm_load(s, req_comp);
    }

    if hdr::hdr_test(s) {
        let (hdr, comp) = hdr::hdr_load(s, req_comp)?;
        let (x, y) = (s.img_x as i32, s.img_y as i32);
        let options = s.options;
        return hdr::hdr_to_ldr(
            s,
            hdr,
            x,
            y,
            if req_comp != 0 { req_comp } else { comp },
            &options,
        )
        .map(|out| (Pixels::U8(out), comp));
    }

    // test tga last because it's a crappy test!
    if tga::tga_test(s) {
        return tga::tga_load(s, req_comp).map(|(out, comp)| (Pixels::U8(out), comp));
    }

    s.errp("unknown image type")
}

/// `stbi__convert_16_to_8`
fn convert_16_to_8(
    s: &mut Context,
    orig: Vec<u16>,
    w: i32,
    h: i32,
    channels: i32,
) -> Option<Vec<u8>> {
    if !mad3sizes_valid(w, h, channels, 0) {
        return s.errp("too large");
    }
    let img_len = (w * h * channels) as usize;

    // top half of each byte is sufficient approx of 16->8 bit scaling
    Some(
        orig[..img_len]
            .iter()
            .map(|&value| (value >> 8) as u8)
            .collect(),
    )
}

/// `stbi__convert_8_to_16`
fn convert_8_to_16(
    s: &mut Context,
    orig: Vec<u8>,
    w: i32,
    h: i32,
    channels: i32,
) -> Option<Vec<u16>> {
    if !mad4sizes_valid(w, h, channels, 2, 0) {
        return s.errp("too large");
    }
    let img_len = (w * h * channels) as usize;

    // replicate to high and low byte, maps 0->0, 255->0xffff
    Some(
        orig[..img_len]
            .iter()
            .map(|&value| (u16::from(value) << 8) + u16::from(value))
            .collect(),
    )
}

/// `stbi__vertical_flip`
pub(crate) fn vertical_flip<T>(image: &mut [T], w: usize, h: usize, per_pixel: usize) {
    let per_row = w * per_pixel;
    if per_row == 0 {
        return;
    }
    for row in 0..h >> 1 {
        // swap row0 with row1
        let (top, bottom) = image.split_at_mut((h - row - 1) * per_row);
        top[row * per_row..(row + 1) * per_row].swap_with_slice(&mut bottom[..per_row]);
    }
}

/// The image out of an image file's bytes at 8 bits a channel: `stbi_load_from_memory`.
/// `desired_channels` is how many values a pixel is to have, 1 to 4, or 0 for as many as the
/// file has. A file of 16 bits a channel is brought down to 8, and an HDR file's floats with
/// [`LoadOptions::hdr_to_ldr_gamma`]. Of a GIF, the first frame.
pub fn load_from_memory(
    buffer: &[u8],
    desired_channels: i32,
    options: &LoadOptions,
) -> Result<Loaded<u8>, LoadError> {
    let mut s = Context::new(buffer, options);
    load_and_postprocess_8bit(&mut s, desired_channels).ok_or(LoadError(s.reason))
}

/// `stbi__load_and_postprocess_8bit`
fn load_and_postprocess_8bit(s: &mut Context, desired_channels: i32) -> Option<Loaded<u8>> {
    let options = s.options;
    let (result, comp) = load_main(s, desired_channels, 8)?;
    let (x, y) = (s.img_x as i32, s.img_y as i32);
    let channels = if desired_channels == 0 {
        comp
    } else {
        desired_channels
    };

    let mut result = match result {
        Pixels::U8(result) => result,
        Pixels::U16(result) => convert_16_to_8(s, result, x, y, channels)?,
    };

    if options.flip_vertically_on_load {
        vertical_flip(&mut result, x as usize, y as usize, channels as usize);
    }

    Some(Loaded {
        x,
        y,
        channels_in_file: comp,
        data: result,
    })
}

/// The image out of an image file's bytes at 16 bits a channel: `stbi_load_16_from_memory`.
/// A file of 8 bits a channel is brought up to 16, each value with itself as its lower half.
pub fn load_16_from_memory(
    buffer: &[u8],
    desired_channels: i32,
    options: &LoadOptions,
) -> Result<Loaded<u16>, LoadError> {
    let mut s = Context::new(buffer, options);
    // stbi__load_and_postprocess_16bit
    let Some((result, comp)) = load_main(&mut s, desired_channels, 16) else {
        return Err(LoadError(s.reason));
    };
    let (x, y) = (s.img_x as i32, s.img_y as i32);
    let channels = if desired_channels == 0 {
        comp
    } else {
        desired_channels
    };

    let mut result = match result {
        Pixels::U16(result) => result,
        Pixels::U8(result) => match convert_8_to_16(&mut s, result, x, y, channels) {
            Some(result) => result,
            None => return Err(LoadError(s.reason)),
        },
    };

    if options.flip_vertically_on_load {
        vertical_flip(&mut result, x as usize, y as usize, channels as usize);
    }

    Ok(Loaded {
        x,
        y,
        channels_in_file: comp,
        data: result,
    })
}

/// The image out of an image file's bytes as floats: `stbi_loadf_from_memory`. An HDR file's
/// values come as they are; any other file's come by way of 8 bits a channel, made floats
/// with [`LoadOptions::ldr_to_hdr_gamma`] (alpha in a straight line, from 0 to 1).
pub fn loadf_from_memory(
    buffer: &[u8],
    desired_channels: i32,
    options: &LoadOptions,
) -> Result<Loaded<f32>, LoadError> {
    let mut s = Context::new(buffer, options);
    // stbi__loadf_main
    if hdr::hdr_test(&mut s) {
        let Some((mut data, comp)) = hdr::hdr_load(&mut s, desired_channels) else {
            return Err(LoadError(s.reason));
        };
        let (x, y) = (s.img_x as i32, s.img_y as i32);
        // stbi__float_postprocess
        if options.flip_vertically_on_load {
            let channels = if desired_channels != 0 {
                desired_channels
            } else {
                comp
            };
            vertical_flip(&mut data, x as usize, y as usize, channels as usize);
        }
        return Ok(Loaded {
            x,
            y,
            channels_in_file: comp,
            data,
        });
    }
    if let Some(loaded) = load_and_postprocess_8bit(&mut s, desired_channels) {
        let channels = if desired_channels != 0 {
            desired_channels
        } else {
            loaded.channels_in_file
        };
        return match hdr::ldr_to_hdr(&mut s, loaded.data, loaded.x, loaded.y, channels, options) {
            Some(data) => Ok(Loaded {
                x: loaded.x,
                y: loaded.y,
                channels_in_file: loaded.channels_in_file,
                data,
            }),
            None => Err(LoadError(s.reason)),
        };
    }
    Err(LoadError("unknown image type"))
}

/// Every frame of a GIF file, one after another, each with its delay:
/// `stbi_load_gif_from_memory`.
pub fn load_gif_from_memory(
    buffer: &[u8],
    desired_channels: i32,
    options: &LoadOptions,
) -> Result<LoadedGif, LoadError> {
    let mut s = Context::new(buffer, options);
    if !(0..=4).contains(&desired_channels) {
        return Err(LoadError("bad req_comp"));
    }
    gif::load_gif_main(&mut s, desired_channels).ok_or(LoadError(s.reason))
}

/// Whether a file's bytes are a Radiance `.hdr` file: `stbi_is_hdr_from_memory`.
pub fn is_hdr_from_memory(buffer: &[u8]) -> bool {
    hdr::hdr_test(&mut Context::new(buffer, &LoadOptions::default()))
}

/// The size and kind of the image in a file's bytes, without its pixels:
/// `stbi_info2_from_memory`.
pub fn info_from_memory(buffer: &[u8]) -> Result<Info, LoadError> {
    let mut s = Context::new(buffer, &LoadOptions::default());
    // stbi__info2_main
    if let Some(info) = jpeg::jpeg_info(&mut s) {
        return Ok(info);
    }
    if let Some(info) = png::png_info(&mut s) {
        return Ok(info);
    }
    if let Some(info) = gif::gif_info(&mut s) {
        return Ok(info);
    }
    if let Some(info) = bmp::bmp_info(&mut s) {
        return Ok(info);
    }
    if let Some(info) = psd::psd_info(&mut s) {
        return Ok(info);
    }
    if let Some(info) = pnm::pnm_info(&mut s) {
        return Ok(info);
    }
    if let Some(info) = hdr::hdr_info(&mut s) {
        return Ok(info);
    }
    // test tga last because it's a crappy test!
    if let Some(info) = tga::tga_info(&mut s) {
        return Ok(info);
    }
    Err(LoadError("unknown image type"))
}

/// Whether the image in a file's bytes has 16 bits a channel: `stbi_is_16_bit_from_memory`.
pub fn is_16_bit_from_memory(buffer: &[u8]) -> bool {
    info_from_memory(buffer).is_ok_and(|info| info.is_16_bit)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_size_checks_are_the_originals() {
        assert!(
            addsizes_valid(i32::MAX - 5, 5)
                && !addsizes_valid(i32::MAX - 5, 6)
                && !addsizes_valid(1, -1)
        );
        assert!(
            mul2sizes_valid(46340, 46340)
                && !mul2sizes_valid(46341, 46341)
                && mul2sizes_valid(i32::MAX, 0)
                && !mul2sizes_valid(-1, 1)
        );
        assert!(mad3sizes_valid(1024, 1024, 2047, 1023) && !mad3sizes_valid(1024, 1024, 2048, 0));
        assert!(mad4sizes_valid(256, 256, 256, 127, 0) && !mad4sizes_valid(256, 256, 256, 128, 0));
        assert!(
            addints_valid(i32::MIN, 0)
                && !addints_valid(i32::MIN, -1)
                && !addints_valid(i32::MAX, 1)
                && addints_valid(i32::MAX, i32::MIN)
        );
        assert!(
            mul2shorts_valid(181, 181)
                && !mul2shorts_valid(182, 182)
                && mul2shorts_valid(-32768, 1)
                && !mul2shorts_valid(-32769, 1)
        );
        assert!(
            mul2shorts_valid(16384, -2)
                && !mul2shorts_valid(16385, -2)
                && mul2shorts_valid(-2, 16384)
                && mul2shorts_valid(123456, 0)
                && mul2shorts_valid(5, -1)
        );
    }

    #[test]
    fn channels_are_made_and_dropped() {
        let mut s = Context::new(&[], &LoadOptions::default());
        let rgba: Vec<u8> = vec![10, 20, 30, 40, 200, 100, 50, 25];
        assert_eq!(
            convert_format(&mut s, rgba.clone(), 4, 3, 2, 1).unwrap(),
            [10, 20, 30, 200, 100, 50]
        );
        assert_eq!(
            convert_format(&mut s, rgba.clone(), 4, 2, 2, 1).unwrap(),
            [compute_y(10, 20, 30), 40, compute_y(200, 100, 50), 25]
        );
        assert_eq!(
            convert_format(&mut s, rgba.clone(), 4, 1, 2, 1).unwrap(),
            [18, 124]
        );
        assert_eq!(
            convert_format(&mut s, vec![7u8, 9], 1, 4, 2, 1).unwrap(),
            [7, 7, 7, 255, 9, 9, 9, 255]
        );
        assert_eq!(
            convert_format(&mut s, vec![7u16, 9], 2, 4, 1, 1).unwrap(),
            [7, 7, 7, 9]
        );
        assert_eq!(
            convert_format(&mut s, vec![7u16, 9, 11], 3, 2, 1, 1).unwrap(),
            [(7 * 77 + 9 * 150 + 29 * 11) >> 8, 0xffff]
        );
        assert_eq!(
            convert_format(&mut s, rgba.clone(), 4, 4, 2, 1).unwrap(),
            rgba
        );
    }

    #[test]
    fn rows_are_turned_over() {
        let mut image = vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12];
        vertical_flip(&mut image, 2, 3, 2);
        assert_eq!(image, [9, 10, 11, 12, 5, 6, 7, 8, 1, 2, 3, 4]);
    }

    #[test]
    fn what_is_no_image_is_refused() {
        let options = LoadOptions::default();
        assert_eq!(
            load_from_memory(&[], 0, &options),
            Err(LoadError("unknown image type"))
        );
        assert_eq!(
            load_from_memory(b"not an image at all", 0, &options),
            Err(LoadError("unknown image type"))
        );
        assert_eq!(
            load_from_memory(&[], 5, &options),
            Err(LoadError("bad req_comp"))
        );
        assert_eq!(
            info_from_memory(b"GIF8"),
            Err(LoadError("unknown image type"))
        );
        assert!(!is_16_bit_from_memory(&[]));
    }
}

/// (The README's examples, compiled and run with the tests.)
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
struct ReadmeDoctests;
