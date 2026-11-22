//! Image files to pixels in pure, safe Rust: a native port of Sean Barrett's
//! [`stb_image.h`](https://github.com/nothings/stb) v2.32, every reader it has, with the zlib
//! decoder its PNG reader uses. Each gives the pixels the original gives, and refuses a file
//! the original refuses, with the same reason.
//!
//! - PNG: 1, 2, 4, 8 and 16 bits a channel, grey, colour and palette, with and without alpha
//!   and interlacing.
//!
//! - BMP: 1, 4, 8, 16, 24 and 32 bits a pixel, without run-length encoding.
//!
//! - JPEG: baseline and progressive, 8 bits a channel.
//!
//! - GIF: every frame, with its delay ([`load_gif`]), or the first.
//!
//! - TGA: grey, colour and palette, 8 to 32 bits a pixel, with and without run-length
//!   encoding.
//!
//! - PSD: the merged image of an RGB file, 8 or 16 bits a channel, raw or run-length encoded.
//!
//! - PNM: `.pgm` and `.ppm`, binary and text, 8 or 16 bits a channel.
//!
//! - HDR: Radiance `.hdr` files, flat or run-length encoded, as floats ([`load_f32`]).
//!
//! - PIC: Softimage `.pic` files, 8 bits a channel, uncompressed or run-length encoded.
//!
//! [`load`] gives 8 bits a channel, [`load_16`] 16 and [`load_f32`] floats; [`info`] reads only
//! as far as the size. The settings the original keeps in globals are a [`LoadOptions`]
//! passed to each call. Disable default features for `#![no_std]` environments with `alloc`, or
//! enable `fast-zlib` to accelerate valid PNG `IDAT` decompression with `libdeflate_nrp`.
//!
//! ```
//! use stb_image_nrp::{Channels, LoadOptions, load};
//!
//! // A 1 by 1 grey PGM file.
//! let image = load(b"P5 1 1 255\n\x80", Some(Channels::Rgb), &LoadOptions::default()).unwrap();
//! assert_eq!((image.width, image.height, image.data), (1, 1, vec![128, 128, 128]));
//! ```

#![cfg_attr(not(feature = "std"), no_std)]
#![forbid(unsafe_code)]
#![warn(missing_docs)]

extern crate alloc;
#[cfg(test)]
extern crate std;

mod bmp;
mod exif;
mod gif;
mod hdr;
mod jpeg;
mod pic;
mod png;
mod pnm;
mod psd;
mod tga;
pub mod zlib;

use alloc::vec::Vec;
use core::fmt;
use core::iter::FusedIterator;

pub use exif::exif_orientation;
pub use gif::{Animation, Frame};

/// The channels of each pixel.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Channels {
    /// One channel: grey.
    Grey = 1,
    /// Two channels: grey, then alpha.
    GreyAlpha = 2,
    /// Three channels: red, green and blue.
    Rgb = 3,
    /// Four channels: red, green, blue and alpha.
    Rgba = 4,
}

impl Channels {
    /// The channels for a count of 1 to 4.
    #[must_use]
    pub const fn from_count(count: u8) -> Option<Channels> {
        match count {
            1 => Some(Channels::Grey),
            2 => Some(Channels::GreyAlpha),
            3 => Some(Channels::Rgb),
            4 => Some(Channels::Rgba),
            _ => None,
        }
    }

    /// The number of channels.
    #[must_use]
    pub const fn count(self) -> usize {
        self as usize
    }

    /// Whether the last channel is alpha (`GreyAlpha` or `Rgba`).
    #[must_use]
    pub const fn has_alpha(self) -> bool {
        matches!(self, Channels::GreyAlpha | Channels::Rgba)
    }

    /// The channels of a count the readers produce, which is always 1 to 4.
    fn of(count: i32) -> Channels {
        Channels::from_count(count as u8).expect("1 to 4 channels")
    }
}

impl From<Channels> for u8 {
    fn from(channels: Channels) -> Self {
        channels as u8
    }
}

impl TryFrom<u8> for Channels {
    type Error = InvalidChannels;

    fn try_from(count: u8) -> Result<Self, Self::Error> {
        Channels::from_count(count).ok_or(InvalidChannels(count))
    }
}

/// The error returned when converting a channel count outside `1..=4` into [`Channels`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct InvalidChannels(pub u8);

impl fmt::Display for InvalidChannels {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "channel count {} is not supported (use 1 to 4)", self.0)
    }
}

impl core::error::Error for InvalidChannels {}

/// The count the readers take for the channels asked for: 0 for those of the file.
fn requested(channels: Option<Channels>) -> i32 {
    channels.map_or(0, |channels| channels as i32)
}

/// Broad category of an image decoding failure ([`LoadError::kind`]).
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ErrorKind {
    /// The input bytes did not match any supported image format.
    UnknownFormat,
    /// The file uses a format variant or feature that `stb_image` does not decode.
    Unsupported,
    /// Dimensions, stream length, or caller-configured limits exceed the maximum allowed.
    TooLarge,
    /// Memory allocation failed or the output buffer size overflowed `i32::MAX`.
    OutOfMemory,
    /// The input ended before the header or pixel stream was complete.
    UnexpectedEof,
    /// An invalid parameter was supplied by the caller.
    InvalidInput,
    /// The file header or compressed data is malformed or damaged.
    CorruptData,
}

/// Why a file was refused: the original's `stbi_failure_reason()`.
///
/// A few of the original's failures set no reason, and it then reports whatever an earlier
/// call left; here the reason is then empty, or the one an earlier step of the same call
/// left.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct LoadError {
    reason: &'static str,
}

impl LoadError {
    pub(crate) const fn new(reason: &'static str) -> LoadError {
        LoadError { reason }
    }

    /// The original's short description of the failure, such as `"bad zlib header"`.
    #[must_use]
    pub const fn reason(&self) -> &'static str {
        self.reason
    }

    /// Broad category of this failure.
    #[must_use]
    pub const fn kind(&self) -> ErrorKind {
        match self.reason.as_bytes() {
            b"unknown image type"
            | b"not GIF"
            | b"not BMP"
            | b"unknown BMP"
            | b"not PSD"
            | b"not HDR"
            | b"not PIC"
            | b"bad png sig"
            | b"no SOI" => ErrorKind::UnknownFormat,
            b"BMP RLE"
            | b"BMP JPEG/PNG"
            | b"1/2/4/8/16-bit only"
            | b"only 8-bit"
            | b"no header height"
            | b"wrong version"
            | b"unsupported bit depth"
            | b"wrong color format"
            | b"unsupported format"
            | b"unsupported data layout"
            | b"no preset dict"
            | b"unsupported" => ErrorKind::Unsupported,
            b"too large"
            | b"toobig"
            | b"IDAT size limit"
            | b"output buffer limit"
            | b"max value > 65535"
            | b"integer parse overflow" => ErrorKind::TooLarge,
            b"outofmem" => ErrorKind::OutOfMemory,
            b"outofdata" | b"read past buffer" | b"unexpected end" | b"premature EOF"
            | b"not enough pixels" => ErrorKind::UnexpectedEof,
            b"bad req_comp" => ErrorKind::InvalidInput,
            _ => ErrorKind::CorruptData,
        }
    }
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.reason.is_empty() {
            f.write_str("the image could not be read")
        } else {
            f.write_str(self.reason)
        }
    }
}

impl core::error::Error for LoadError {}

/// The settings the original keeps in globals, plus optional safety limits and post-processing
/// flags.
#[derive(Clone, Copy, Debug, PartialEq)]
#[allow(clippy::struct_excessive_bools)]
pub struct LoadOptions {
    /// Give the image with its last row first (`stbi_set_flip_vertically_on_load`).
    pub flip_vertically: bool,
    /// For an iPhone PNG whose colours were multiplied by its alpha, divide them back
    /// (`stbi_set_unpremultiply_on_load`). Only takes effect with `convert_iphone_png_to_rgb`.
    pub unpremultiply: bool,
    /// Give an iPhone PNG (one with a `CgBI` chunk), which is stored blue first, red first
    /// (`stbi_convert_iphone_png_to_rgb`).
    pub convert_iphone_png_to_rgb: bool,
    /// The gamma with which an HDR file's floats are brought to 8 or 16 bits
    /// (`stbi_hdr_to_ldr_gamma`). 2.2 by default.
    pub hdr_to_ldr_gamma: f32,
    /// What an HDR file's floats are multiplied by first (`stbi_hdr_to_ldr_scale`). 1 by
    /// default.
    pub hdr_to_ldr_scale: f32,
    /// The gamma with which [`load_f32`] makes another file's values floats
    /// (`stbi_ldr_to_hdr_gamma`). 2.2 by default.
    pub ldr_to_hdr_gamma: f32,
    /// What those floats are multiplied by (`stbi_ldr_to_hdr_scale`). 1 by default.
    pub ldr_to_hdr_scale: f32,
    /// Optional upper bound on image width in pixels; wider images fail with `"too large"`.
    /// `None` by default (`1 << 24` always applies).
    pub max_width: Option<u32>,
    /// Optional upper bound on image height in pixels; taller images fail with `"too large"`.
    /// `None` by default (`1 << 24` always applies).
    pub max_height: Option<u32>,
    /// Optional upper bound on decoded pixel buffer allocation in bytes; larger allocations
    /// fail with `"too large"`. `None` by default.
    pub max_alloc_bytes: Option<usize>,
    /// In [`load_f32`], linearise 16-bit files (PNG, PSD, PNM) directly from `0..=65535`
    /// instead of narrowing to 8 bits first as `stb_image.h` does. `false` by default.
    pub preserve_16bit_in_f32: bool,
    /// Apply any EXIF `Orientation` tag (`1..=8`, from JPEG `APP1` or PNG `eXIf`) before
    /// returning (and before `flip_vertically`). `false` by default.
    pub apply_exif_orientation: bool,
}

impl LoadOptions {
    /// Default options matching `stb_image.h`'s initial state.
    #[must_use]
    pub const fn new() -> LoadOptions {
        LoadOptions {
            flip_vertically: false,
            unpremultiply: false,
            convert_iphone_png_to_rgb: false,
            hdr_to_ldr_gamma: 2.2,
            hdr_to_ldr_scale: 1.0,
            ldr_to_hdr_gamma: 2.2,
            ldr_to_hdr_scale: 1.0,
            max_width: None,
            max_height: None,
            max_alloc_bytes: None,
            preserve_16bit_in_f32: false,
            apply_exif_orientation: false,
        }
    }

    /// Sets [`LoadOptions::flip_vertically`].
    #[must_use]
    pub const fn with_flip_vertically(mut self, flip: bool) -> LoadOptions {
        self.flip_vertically = flip;
        self
    }

    /// Sets [`LoadOptions::unpremultiply`].
    #[must_use]
    pub const fn with_unpremultiply(mut self, unpremultiply: bool) -> LoadOptions {
        self.unpremultiply = unpremultiply;
        self
    }

    /// Sets [`LoadOptions::convert_iphone_png_to_rgb`].
    #[must_use]
    pub const fn with_convert_iphone_png_to_rgb(mut self, convert: bool) -> LoadOptions {
        self.convert_iphone_png_to_rgb = convert;
        self
    }

    /// Sets [`LoadOptions::hdr_to_ldr_gamma`].
    #[must_use]
    pub const fn with_hdr_to_ldr_gamma(mut self, gamma: f32) -> LoadOptions {
        self.hdr_to_ldr_gamma = gamma;
        self
    }

    /// Sets [`LoadOptions::hdr_to_ldr_scale`].
    #[must_use]
    pub const fn with_hdr_to_ldr_scale(mut self, scale: f32) -> LoadOptions {
        self.hdr_to_ldr_scale = scale;
        self
    }

    /// Sets [`LoadOptions::ldr_to_hdr_gamma`].
    #[must_use]
    pub const fn with_ldr_to_hdr_gamma(mut self, gamma: f32) -> LoadOptions {
        self.ldr_to_hdr_gamma = gamma;
        self
    }

    /// Sets [`LoadOptions::ldr_to_hdr_scale`].
    #[must_use]
    pub const fn with_ldr_to_hdr_scale(mut self, scale: f32) -> LoadOptions {
        self.ldr_to_hdr_scale = scale;
        self
    }

    /// Sets [`LoadOptions::max_width`].
    #[must_use]
    pub const fn with_max_width(mut self, max_width: Option<u32>) -> LoadOptions {
        self.max_width = max_width;
        self
    }

    /// Sets [`LoadOptions::max_height`].
    #[must_use]
    pub const fn with_max_height(mut self, max_height: Option<u32>) -> LoadOptions {
        self.max_height = max_height;
        self
    }

    /// Sets both [`LoadOptions::max_width`] and [`LoadOptions::max_height`].
    #[must_use]
    pub const fn with_max_dimensions(mut self, max_width: u32, max_height: u32) -> LoadOptions {
        self.max_width = Some(max_width);
        self.max_height = Some(max_height);
        self
    }

    /// Sets [`LoadOptions::max_alloc_bytes`].
    #[must_use]
    pub const fn with_max_alloc_bytes(mut self, max_alloc_bytes: Option<usize>) -> LoadOptions {
        self.max_alloc_bytes = max_alloc_bytes;
        self
    }

    /// Sets [`LoadOptions::preserve_16bit_in_f32`].
    #[must_use]
    pub const fn with_preserve_16bit_in_f32(mut self, preserve: bool) -> LoadOptions {
        self.preserve_16bit_in_f32 = preserve;
        self
    }

    /// Sets [`LoadOptions::apply_exif_orientation`].
    #[must_use]
    pub const fn with_apply_exif_orientation(mut self, apply: bool) -> LoadOptions {
        self.apply_exif_orientation = apply;
        self
    }
}

impl Default for LoadOptions {
    fn default() -> LoadOptions {
        LoadOptions::new()
    }
}

/// An image read from a file: `height` rows of `width` pixels, from the top, each of
/// `channels` values.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Image<T> {
    /// The width in pixels.
    pub width: u32,
    /// The height in pixels.
    pub height: u32,
    /// The channels of each pixel in `data`: those asked for, or the file's.
    pub channels: Channels,
    /// The channels the file has, whatever was asked for.
    pub channels_in_file: Channels,
    /// The values, row after row.
    pub data: Vec<T>,
}

impl Image<u8> {
    /// The pixel bytes, row after row from the top.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.data
    }
}

impl<T> Image<T> {
    /// The `width * channels.count()` values of row `y` (counted from `0` at the top), or
    /// `None` if `y >= self.height`.
    #[must_use]
    pub fn row(&self, y: u32) -> Option<&[T]> {
        if y >= self.height {
            return None;
        }
        let row_len = (self.width as usize).checked_mul(self.channels.count())?;
        let start = (y as usize).checked_mul(row_len)?;
        self.data.get(start..start.checked_add(row_len)?)
    }

    /// An iterator over the rows of the image, from top to bottom.
    pub fn rows(&self) -> Rows<'_, T> {
        let row_len = (self.width as usize).saturating_mul(self.channels.count());
        let valid_rows = self
            .data
            .len()
            .checked_div(row_len)
            .map_or(self.height as usize, |r| r.min(self.height as usize));
        Rows {
            data: &self.data[..valid_rows * row_len],
            row_len,
            remaining_empty: if row_len == 0 {
                self.height as usize
            } else {
                0
            },
        }
    }

    /// The `channels.count()` values of pixel `(x, y)`, or `None` if `x >= self.width` or
    /// `y >= self.height`.
    #[must_use]
    pub fn pixel(&self, x: u32, y: u32) -> Option<&[T]> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let c = self.channels.count();
        let idx = (y as usize)
            .checked_mul(self.width as usize)?
            .checked_add(x as usize)?
            .checked_mul(c)?;
        self.data.get(idx..idx.checked_add(c)?)
    }
}

impl<T: Copy> Image<T> {
    /// Applies an EXIF `Orientation` value (`1..=8`) to this image in place, updating `width`,
    /// `height`, and `data` so the image is upright. Values outside `2..=8` are no-ops.
    pub fn apply_orientation(&mut self, orientation: u8) {
        exif::apply_orientation(self, orientation);
    }
}

/// An iterator over the rows of an [`Image`], from top to bottom.
#[derive(Clone, Debug)]
#[must_use]
pub struct Rows<'a, T> {
    data: &'a [T],
    row_len: usize,
    remaining_empty: usize,
}

impl<'a, T> Iterator for Rows<'a, T> {
    type Item = &'a [T];

    fn next(&mut self) -> Option<Self::Item> {
        if self.row_len == 0 {
            if self.remaining_empty == 0 {
                return None;
            }
            self.remaining_empty -= 1;
            return Some(&[]);
        }
        if self.data.is_empty() {
            return None;
        }
        let (row, rest) = self.data.split_at(self.row_len);
        self.data = rest;
        Some(row)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let len = self
            .data
            .len()
            .checked_div(self.row_len)
            .unwrap_or(self.remaining_empty);
        (len, Some(len))
    }
}

impl<T> DoubleEndedIterator for Rows<'_, T> {
    fn next_back(&mut self) -> Option<Self::Item> {
        if self.row_len == 0 {
            if self.remaining_empty == 0 {
                return None;
            }
            self.remaining_empty -= 1;
            return Some(&[]);
        }
        if self.data.is_empty() {
            return None;
        }
        let split = self.data.len() - self.row_len;
        let (rest, row) = self.data.split_at(split);
        self.data = rest;
        Some(row)
    }
}

impl<T> ExactSizeIterator for Rows<'_, T> {}
impl<T> FusedIterator for Rows<'_, T> {}

/// The format of an image file.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ImageFormat {
    /// JPEG.
    Jpeg,
    /// PNG.
    Png,
    /// GIF.
    Gif,
    /// BMP.
    Bmp,
    /// Photoshop's PSD.
    Psd,
    /// The portable anymaps: PGM and PPM.
    Pnm,
    /// Radiance's HDR (RGBE).
    Hdr,
    /// Truevision's TGA.
    Tga,
    /// Softimage's PIC.
    Pic,
}

impl ImageFormat {
    /// The original's number for the format: `STBI_type_jpeg` and the rest, from 1.
    #[must_use]
    pub const fn code(self) -> i32 {
        self as i32 + 1
    }
}

/// What [`info`] finds of a file without reading its pixels (`stbi_info_res`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Info {
    /// The file's format.
    pub format: ImageFormat,
    /// The width in pixels.
    pub width: u32,
    /// The height in pixels. (The original reports a BMP stored top row first as of a
    /// negative height; here it is positive, as [`load`] reports it.)
    pub height: u32,
    /// The channels the file has.
    pub channels: Channels,
    /// Whether the file has 16 bits a channel.
    pub is_16_bit: bool,
    /// Whether the file is a Radiance `.hdr` file, whose values are floats.
    pub is_hdr: bool,
    /// Whether the file is an iPhone PNG, with a `CgBI` chunk.
    pub is_iphone_png: bool,
}

/// What the readers find of a file: the original's `stbi_info_res`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RawInfo {
    pub kind: ImageFormat,
    pub x: i32,
    pub y: i32,
    pub comp: i32,
    pub is_16_bit: bool,
    pub is_hdr: bool,
    pub is_iphone_png: bool,
}

impl From<RawInfo> for Info {
    fn from(raw: RawInfo) -> Info {
        Info {
            format: raw.kind,
            width: raw.x as u32,
            height: if raw.kind == ImageFormat::Bmp {
                raw.y.unsigned_abs()
            } else {
                raw.y as u32
            },
            channels: Channels::of(raw.comp),
            is_16_bit: raw.is_16_bit,
            is_hdr: raw.is_hdr,
            is_iphone_png: raw.is_iphone_png,
        }
    }
}

/// `STBI_MAX_DIMENSIONS`
pub(crate) const STBI_MAX_DIMENSIONS: u32 = 1 << 24;

/// `STBI__SCAN_*`
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Scan {
    Load,
    Header,
}

/// A step of decoding that failed. Its reason is on the [`Context`] (or the zlib decoder's
/// buffer), where `stbi__err` records it, since the last reason recorded is the one given,
/// as in the original, even when a caller tries something else after the failure.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Failed;

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

    /// `stbi__err`, for a step that fails: records `reason` as the failure's.
    pub fn fail<T>(&mut self, reason: &'static str) -> Result<T, Failed> {
        self.set_reason(reason);
        Err(Failed)
    }

    /// `stbi__err` where the caller reports the failure itself: records `reason`.
    pub fn set_reason(&mut self, reason: &'static str) {
        self.reason = reason;
    }

    /// `stbi__errpuc`
    pub fn errp<T>(&mut self, reason: &'static str) -> Option<T> {
        self.reason = reason;
        None
    }

    /// Checks optional [`LoadOptions`] width, height, and byte-allocation limits.
    pub fn check_limits(&mut self, w: u32, h: u32, alloc_bytes: usize) -> Result<(), Failed> {
        if self.options.max_width.is_some_and(|max_w| w > max_w)
            || self.options.max_height.is_some_and(|max_h| h > max_h)
            || self
                .options
                .max_alloc_bytes
                .is_some_and(|max_b| alloc_bytes > max_b)
        {
            return self.fail("too large");
        }
        Ok(())
    }

    /// Bytes remaining in the input slice from the current position.
    pub fn remaining(&self) -> usize {
        self.buffer.len().saturating_sub(self.pos)
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
    pub fn getn(&mut self, into: &mut Vec<u8>, n: i32) -> Result<(), Failed> {
        if n <= 0 {
            return Ok(());
        }
        let n = n as usize;
        if self.pos <= self.buffer.len() && n <= self.buffer.len() - self.pos {
            if into.try_reserve(n).is_err() {
                return self.fail("outofmem");
            }
            into.extend_from_slice(&self.buffer[self.pos..self.pos + n]);
            self.pos += n;
            Ok(())
        } else {
            Err(Failed)
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
    pub fn getn_into(&mut self, into: &mut [u8]) -> Result<(), Failed> {
        if into.is_empty() {
            return Ok(());
        }
        let n = into.len();
        if self.pos <= self.buffer.len() && n <= self.buffer.len() - self.pos {
            into.copy_from_slice(&self.buffer[self.pos..self.pos + n]);
            self.pos += n;
            Ok(())
        } else {
            Err(Failed)
        }
    }

    /// `stbi__get32le`
    pub fn get32le(&mut self) -> u32 {
        let z = self.get16le() as u32;
        z.wrapping_add((self.get16le() as u32) << 16)
    }
}

/// Allocates a `Vec<T>` filled with `len` copies of `elem` without aborting on OOM.
pub(crate) fn try_alloc_elem<T: Clone>(elem: T, len: usize) -> Option<Vec<T>> {
    let mut v = Vec::new();
    v.try_reserve_exact(len).ok()?;
    v.resize(len, elem);
    Some(v)
}

/// Allocates a zero-initialized `Vec<T>` of `len` elements without aborting on OOM.
pub(crate) fn try_alloc_zeroed<T: Default + Clone>(len: usize) -> Option<Vec<T>> {
    try_alloc_elem(T::default(), len)
}

/// Allocates an empty `Vec<T>` with `cap` reserved elements without aborting on OOM.
pub(crate) fn try_with_capacity<T>(cap: usize) -> Option<Vec<T>> {
    let mut v = Vec::new();
    v.try_reserve_exact(cap).ok()?;
    Some(v)
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

/// Converts 4-channel RGBA bytes in `data` in place to `req_comp` (`1`, `2`, or `3`) channels,
/// avoiding allocating a second full-image buffer.
pub(crate) fn convert_rgba_in_place(data: &mut Vec<u8>, req_comp: usize) {
    debug_assert!((1..=3).contains(&req_comp));
    let pixels = data.len() / 4;
    match req_comp {
        1 => {
            for i in 0..pixels {
                let [r, g, b] = [data[4 * i], data[4 * i + 1], data[4 * i + 2]];
                data[i] = compute_y(i32::from(r), i32::from(g), i32::from(b));
            }
        }
        2 => {
            for i in 0..pixels {
                let [r, g, b, a] = [
                    data[4 * i],
                    data[4 * i + 1],
                    data[4 * i + 2],
                    data[4 * i + 3],
                ];
                data[2 * i] = compute_y(i32::from(r), i32::from(g), i32::from(b));
                data[2 * i + 1] = a;
            }
        }
        3 => {
            for i in 0..pixels {
                let [r, g, b] = [data[4 * i], data[4 * i + 1], data[4 * i + 2]];
                data[3 * i] = r;
                data[3 * i + 1] = g;
                data[3 * i + 2] = b;
            }
        }
        _ => return,
    }
    data.truncate(pixels * req_comp);
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

    let elem_size = core::mem::size_of::<T>();
    if !mad4sizes_valid(req_comp, x as i32, y as i32, elem_size as i32, 0) {
        return s.errp("outofmem");
    }
    let (x_usize, y_usize, img_n_usize, req_comp_usize) =
        (x as usize, y as usize, img_n as usize, req_comp as usize);
    let len = req_comp_usize * x_usize * y_usize;
    if s.check_limits(x, y, len * elem_size).is_err() {
        return None;
    }
    let Some(mut good) = try_alloc_zeroed::<T>(len) else {
        return s.errp("outofmem");
    };

    for j in 0..y_usize {
        let src = &data[j * x_usize * img_n_usize..(j + 1) * x_usize * img_n_usize];
        let dest = &mut good[j * x_usize * req_comp_usize..(j + 1) * x_usize * req_comp_usize];
        let pixels = src
            .chunks_exact(img_n_usize)
            .zip(dest.chunks_exact_mut(req_comp_usize));
        // convert source image with img_n components to one with req_comp components
        match (img_n_usize, req_comp_usize) {
            (1, 2) => pixels.for_each(|(src, dest)| dest.copy_from_slice(&[src[0], T::FULL])),
            (1, 3) => {
                pixels.for_each(|(src, dest)| dest.copy_from_slice(&[src[0], src[0], src[0]]));
            }
            (1, 4) => pixels
                .for_each(|(src, dest)| dest.copy_from_slice(&[src[0], src[0], src[0], T::FULL])),
            (2, 1) => pixels.for_each(|(src, dest)| dest[0] = src[0]),
            (2, 3) => {
                pixels.for_each(|(src, dest)| dest.copy_from_slice(&[src[0], src[0], src[0]]));
            }
            (2, 4) => pixels
                .for_each(|(src, dest)| dest.copy_from_slice(&[src[0], src[0], src[0], src[1]])),
            (3, 4) => pixels
                .for_each(|(src, dest)| dest.copy_from_slice(&[src[0], src[1], src[2], T::FULL])),
            (3, 1) => pixels.for_each(|(src, dest)| dest[0] = T::compute_y(src[0], src[1], src[2])),
            (3, 2) => pixels.for_each(|(src, dest)| {
                dest.copy_from_slice(&[T::compute_y(src[0], src[1], src[2]), T::FULL]);
            }),
            (4, 1) => pixels.for_each(|(src, dest)| dest[0] = T::compute_y(src[0], src[1], src[2])),
            (4, 2) => pixels.for_each(|(src, dest)| {
                dest.copy_from_slice(&[T::compute_y(src[0], src[1], src[2]), src[3]]);
            }),
            (4, 3) => {
                pixels.for_each(|(src, dest)| dest.copy_from_slice(&[src[0], src[1], src[2]]));
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
    if pic::pic_test(s) {
        return pic::pic_load(s, req_comp).map(|(out, comp)| (Pixels::U8(out), comp));
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
            &hdr,
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
    orig: &[u16],
    w: i32,
    h: i32,
    channels: i32,
) -> Option<Vec<u8>> {
    if !mad3sizes_valid(w, h, channels, 0) {
        return s.errp("too large");
    }
    let img_len = (w * h * channels) as usize;
    let Some(mut out) = try_with_capacity::<u8>(img_len) else {
        return s.errp("outofmem");
    };
    // top half of each byte is sufficient approx of 16->8 bit scaling
    out.extend(orig[..img_len].iter().map(|&value| (value >> 8) as u8));
    Some(out)
}

/// `stbi__convert_8_to_16`
fn convert_8_to_16(
    s: &mut Context,
    orig: &[u8],
    w: i32,
    h: i32,
    channels: i32,
) -> Option<Vec<u16>> {
    if !mad4sizes_valid(w, h, channels, 2, 0) {
        return s.errp("too large");
    }
    let img_len = (w * h * channels) as usize;
    if s.check_limits(w as u32, h as u32, img_len * 2).is_err() {
        return None;
    }
    let Some(mut out) = try_with_capacity::<u16>(img_len) else {
        return s.errp("outofmem");
    };
    // replicate to high and low byte, maps 0->0, 255->0xffff
    out.extend(
        orig[..img_len]
            .iter()
            .map(|&value| (u16::from(value) << 8) + u16::from(value)),
    );
    Some(out)
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

/// Reads an image file at 8 bits a channel (`stbi_load_from_memory`).
///
/// `channels` is the channels each pixel is to have, or `None` for the file's. A file of 16
/// bits a channel is brought down to 8, and an HDR file's floats with
/// [`LoadOptions::hdr_to_ldr_gamma`]. Of a GIF, the first frame.
///
/// # Errors
///
/// A file in none of the formats read, or one that is corrupt or too large, with `stb_image`'s
/// reason for the failure.
pub fn load(
    file: &[u8],
    channels: Option<Channels>,
    options: &LoadOptions,
) -> Result<Image<u8>, LoadError> {
    let mut s = Context::new(file, options);
    load_and_postprocess_8bit(&mut s, requested(channels)).ok_or(LoadError::new(s.reason))
}

/// An image of the values `data`, of the size in the context and the channels asked for or
/// else the file's.
fn image<T>(s: &Context, desired_channels: i32, comp: i32, data: Vec<T>) -> Image<T> {
    Image {
        width: s.img_x,
        height: s.img_y,
        channels: Channels::of(if desired_channels == 0 {
            comp
        } else {
            desired_channels
        }),
        channels_in_file: Channels::of(comp),
        data,
    }
}

fn postprocess_image<T: Copy>(s: &mut Context, img: &mut Image<T>) -> Option<()> {
    let options = s.options;
    let elem = core::mem::size_of::<T>();
    s.check_limits(img.width, img.height, img.data.len().saturating_mul(elem))
        .ok()?;
    if options.apply_exif_orientation
        && let Some(orientation) = exif::exif_orientation(s.buffer)
    {
        img.apply_orientation(orientation);
        s.check_limits(img.width, img.height, img.data.len().saturating_mul(elem))
            .ok()?;
    }
    if options.flip_vertically {
        vertical_flip(
            &mut img.data,
            img.width as usize,
            img.height as usize,
            img.channels.count(),
        );
    }
    Some(())
}

/// `stbi__load_and_postprocess_8bit`
fn load_and_postprocess_8bit(s: &mut Context, desired_channels: i32) -> Option<Image<u8>> {
    let (result, comp) = load_main(s, desired_channels, 8)?;
    let (x, y) = (s.img_x as i32, s.img_y as i32);
    let channels = if desired_channels == 0 {
        comp
    } else {
        desired_channels
    };

    let result = match result {
        Pixels::U8(result) => result,
        Pixels::U16(result) => convert_16_to_8(s, &result, x, y, channels)?,
    };

    let mut img = image(s, desired_channels, comp, result);
    postprocess_image(s, &mut img)?;
    Some(img)
}

/// Reads an image file at 16 bits a channel (`stbi_load_16_from_memory`).
///
/// A file of 8 bits a channel is brought up to 16, each value repeated as its low byte, and
/// an HDR file's floats as [`load`] brings them to 8 bits.
///
/// # Errors
///
/// As [`load`].
pub fn load_16(
    file: &[u8],
    channels: Option<Channels>,
    options: &LoadOptions,
) -> Result<Image<u16>, LoadError> {
    let desired_channels = requested(channels);
    let mut s = Context::new(file, options);
    // stbi__load_and_postprocess_16bit
    let Some((result, comp)) = load_main(&mut s, desired_channels, 16) else {
        return Err(LoadError::new(s.reason));
    };
    let (x, y) = (s.img_x as i32, s.img_y as i32);
    let count = if desired_channels == 0 {
        comp
    } else {
        desired_channels
    };

    let result = match result {
        Pixels::U16(result) => result,
        Pixels::U8(result) => match convert_8_to_16(&mut s, &result, x, y, count) {
            Some(result) => result,
            None => return Err(LoadError::new(s.reason)),
        },
    };

    let mut img = image(&s, desired_channels, comp, result);
    if postprocess_image(&mut s, &mut img).is_none() {
        return Err(LoadError::new(s.reason));
    }
    Ok(img)
}

/// Reads an image file as floats (`stbi_loadf_from_memory`).
///
/// An HDR file's values come as they are. By default, any other file's come by way of 8 bits a
/// channel, made floats with [`LoadOptions::ldr_to_hdr_gamma`] and
/// [`LoadOptions::ldr_to_hdr_scale`] (alpha linearly, from 0 to 1); set
/// [`LoadOptions::preserve_16bit_in_f32`] to convert 16-bit PNG, PSD, and PNM files directly
/// from `0..=65535` without narrowing to 8 bits first.
///
/// # Errors
///
/// Returns a [`LoadError`] if the file cannot be decoded. Note that, matching `stb_image.h`'s
/// `stbi__loadf_main`, a damaged Radiance `.hdr` file reports its specific failure reason (and
/// an allocation or limit failure during conversion reports `"outofmem"` or `"too large"`),
/// whereas a non-HDR file whose 8-bit decode fails reports `"unknown image type"` unless
/// [`LoadOptions::max_width`], [`LoadOptions::max_height`], or [`LoadOptions::max_alloc_bytes`]
/// triggered `"too large"`.
pub fn load_f32(
    file: &[u8],
    channels: Option<Channels>,
    options: &LoadOptions,
) -> Result<Image<f32>, LoadError> {
    let desired_channels = requested(channels);
    let mut s = Context::new(file, options);
    // stbi__loadf_main
    if hdr::hdr_test(&mut s) {
        let Some((data, comp)) = hdr::hdr_load(&mut s, desired_channels) else {
            return Err(LoadError::new(s.reason));
        };
        let mut img = image(&s, desired_channels, comp, data);
        if postprocess_image(&mut s, &mut img).is_none() {
            return Err(LoadError::new(s.reason));
        }
        return Ok(img);
    }

    if options.preserve_16bit_in_f32 {
        if let Some((pixels, comp)) = load_main(&mut s, desired_channels, 16) {
            return match pixels {
                Pixels::U8(raw) => {
                    let mut img = image(&s, desired_channels, comp, raw);
                    if postprocess_image(&mut s, &mut img).is_none() {
                        return Err(LoadError::new(s.reason));
                    }
                    let count = img.channels as i32;
                    hdr::ldr_to_hdr(
                        &mut s,
                        &img.data,
                        img.width as i32,
                        img.height as i32,
                        count,
                        options,
                    )
                    .map(|data| Image {
                        width: img.width,
                        height: img.height,
                        channels: img.channels,
                        channels_in_file: img.channels_in_file,
                        data,
                    })
                    .ok_or_else(|| LoadError::new(s.reason))
                }
                Pixels::U16(raw) => {
                    let mut img = image(&s, desired_channels, comp, raw);
                    if postprocess_image(&mut s, &mut img).is_none() {
                        return Err(LoadError::new(s.reason));
                    }
                    let count = img.channels as i32;
                    hdr::ldr_16_to_hdr(
                        &mut s,
                        &img.data,
                        img.width as i32,
                        img.height as i32,
                        count,
                        options,
                    )
                    .map(|data| Image {
                        width: img.width,
                        height: img.height,
                        channels: img.channels,
                        channels_in_file: img.channels_in_file,
                        data,
                    })
                    .ok_or_else(|| LoadError::new(s.reason))
                }
            };
        }
    } else if let Some(loaded) = load_and_postprocess_8bit(&mut s, desired_channels) {
        let (x, y) = (loaded.width as i32, loaded.height as i32);
        let count = loaded.channels as i32;
        return match hdr::ldr_to_hdr(&mut s, &loaded.data, x, y, count, options) {
            Some(data) => Ok(Image {
                width: loaded.width,
                height: loaded.height,
                channels: loaded.channels,
                channels_in_file: loaded.channels_in_file,
                data,
            }),
            None => Err(LoadError::new(s.reason)),
        };
    }

    if (options.max_width.is_some()
        || options.max_height.is_some()
        || options.max_alloc_bytes.is_some())
        && s.reason == "too large"
    {
        return Err(LoadError::new(s.reason));
    }
    if !(0..=4).contains(&desired_channels) {
        return Err(LoadError::new("bad req_comp"));
    }
    Err(LoadError::new("unknown image type"))
}

/// Reads every frame of a GIF file, each with its delay (`stbi_load_gif_from_memory`).
///
/// # Errors
///
/// A file that is not a GIF, or one that is corrupt or too large, with `stb_image`'s reason.
pub fn load_gif(
    file: &[u8],
    channels: Option<Channels>,
    options: &LoadOptions,
) -> Result<Animation, LoadError> {
    let mut s = Context::new(file, options);
    gif::load_gif_main(&mut s, requested(channels)).ok_or(LoadError::new(s.reason))
}

/// Whether a file is a Radiance `.hdr` file (`stbi_is_hdr_from_memory`).
#[must_use]
pub fn is_hdr(file: &[u8]) -> bool {
    hdr::hdr_test(&mut Context::new(file, &LoadOptions::default()))
}

/// The format, size and channels of the image in a file, without its pixels
/// (`stbi_info_from_memory`, with `stbi_is_16_bit_from_memory`).
///
/// # Errors
///
/// Returns `LoadError` with reason `"unknown image type"` when no format reader accepts the
/// file's header (matching `stb_image.h`'s `stbi__info_main`, even when a recognized format's
/// header is damaged).
pub fn info(file: &[u8]) -> Result<Info, LoadError> {
    raw_info(file).map(Info::from)
}

/// `stbi__info_main`
fn raw_info(buffer: &[u8]) -> Result<RawInfo, LoadError> {
    let mut s = Context::new(buffer, &LoadOptions::default());
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
    if let Some(info) = pic::pic_info(&mut s) {
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
    Err(LoadError::new("unknown image type"))
}

/// Whether the image in a file has 16 bits a channel (`stbi_is_16_bit_from_memory`).
#[must_use]
pub fn is_16_bit(file: &[u8]) -> bool {
    raw_info(file).is_ok_and(|info| info.is_16_bit)
}

#[cfg(feature = "std")]
fn into_io_error(error: LoadError) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, error)
}

/// Reads an image from a [`std::io::Read`] stream at 8 bits a channel (`stbi_load_from_callbacks`).
///
/// # Errors
///
/// Returns any I/O error from `reader`, or an error of kind [`std::io::ErrorKind::InvalidData`]
/// wrapping [`LoadError`] if the image cannot be decoded.
#[cfg(feature = "std")]
pub fn load_from_read(
    mut reader: impl std::io::Read,
    channels: Option<Channels>,
    options: &LoadOptions,
) -> std::io::Result<Image<u8>> {
    let mut bytes = Vec::new();
    reader.read_to_end(&mut bytes)?;
    load(&bytes, channels, options).map_err(into_io_error)
}

/// Reads an image from a [`std::io::Read`] stream at 16 bits a channel
/// (`stbi_load_16_from_callbacks`).
///
/// # Errors
///
/// Returns any I/O error from `reader`, or an error of kind [`std::io::ErrorKind::InvalidData`]
/// wrapping [`LoadError`] if the image cannot be decoded.
#[cfg(feature = "std")]
pub fn load_16_from_read(
    mut reader: impl std::io::Read,
    channels: Option<Channels>,
    options: &LoadOptions,
) -> std::io::Result<Image<u16>> {
    let mut bytes = Vec::new();
    reader.read_to_end(&mut bytes)?;
    load_16(&bytes, channels, options).map_err(into_io_error)
}

/// Reads an image from a [`std::io::Read`] stream as `f32` values (`stbi_loadf_from_callbacks`).
///
/// # Errors
///
/// Returns any I/O error from `reader`, or an error of kind [`std::io::ErrorKind::InvalidData`]
/// wrapping [`LoadError`] if the image cannot be decoded.
#[cfg(feature = "std")]
pub fn load_f32_from_read(
    mut reader: impl std::io::Read,
    channels: Option<Channels>,
    options: &LoadOptions,
) -> std::io::Result<Image<f32>> {
    let mut bytes = Vec::new();
    reader.read_to_end(&mut bytes)?;
    load_f32(&bytes, channels, options).map_err(into_io_error)
}

/// Reads an image file from `path` at 8 bits a channel (`stbi_load`).
///
/// # Errors
///
/// Returns any filesystem I/O error, or an error of kind [`std::io::ErrorKind::InvalidData`]
/// wrapping [`LoadError`] if the image cannot be decoded.
#[cfg(feature = "std")]
pub fn load_file(
    path: impl AsRef<std::path::Path>,
    channels: Option<Channels>,
    options: &LoadOptions,
) -> std::io::Result<Image<u8>> {
    let bytes = std::fs::read(path)?;
    load(&bytes, channels, options).map_err(into_io_error)
}

/// Reads the format, dimensions, and channel metadata of an image file at `path` (`stbi_info`).
///
/// # Errors
///
/// Returns any filesystem I/O error, or an error of kind [`std::io::ErrorKind::InvalidData`]
/// wrapping [`LoadError`] if no supported image header is recognized.
#[cfg(feature = "std")]
pub fn info_file(path: impl AsRef<std::path::Path>) -> std::io::Result<Info> {
    let bytes = std::fs::read(path)?;
    info(&bytes).map_err(into_io_error)
}

/// The channels for a count as the original takes it, 0 for the file's.
#[cfg(test)]
pub(crate) fn ch(count: i32) -> Option<Channels> {
    Channels::from_count(count as u8)
}

#[cfg(test)]
mod tests {
    use alloc::string::ToString;
    use alloc::vec;
    use alloc::vec::Vec;
    use core::hash::{Hash, Hasher};

    use super::*;

    fn hash_of<T: Hash>(value: &T) -> u64 {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        value.hash(&mut hasher);
        hasher.finish()
    }

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
                && mul2shorts_valid(123_456, 0)
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
    fn convert_format_u16_rejects_byte_size_overflow() {
        // 4 * 20_000 * 20_000 = 1_600_000_000 <= i32::MAX, BUT * size_of::<u16>() (2) =
        // 3_200_000_000 > i32::MAX! Previously `convert_format<u16>` forgot the `* 2` factor and
        // tried to allocate 3.2 GB (panicking on 32-bit targets).
        let mut s = Context::new(&[], &LoadOptions::default());
        assert!(convert_format::<u16>(&mut s, Vec::new(), 1, 4, 20_000, 20_000).is_none());
        assert_eq!(s.reason, "outofmem");
    }

    #[test]
    fn info_from_raw_only_takes_abs_height_for_bmp() {
        let bmp_raw = RawInfo {
            kind: ImageFormat::Bmp,
            x: 10,
            y: -20,
            comp: 3,
            is_16_bit: false,
            is_hdr: false,
            is_iphone_png: false,
        };
        assert_eq!(Info::from(bmp_raw).height, 20);

        let psd_raw = RawInfo {
            kind: ImageFormat::Psd,
            x: 10,
            y: 0x8000_0005_u32 as i32,
            comp: 4,
            is_16_bit: false,
            is_hdr: false,
            is_iphone_png: false,
        };
        assert_eq!(Info::from(psd_raw).height, 0x8000_0005);
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
            load(&[], None, &options),
            Err(LoadError::new("unknown image type"))
        );
        assert_eq!(
            load(b"not an image at all", None, &options),
            Err(LoadError::new("unknown image type"))
        );
        assert_eq!(info(b"GIF8"), Err(LoadError::new("unknown image type")));
        assert!(!is_16_bit(&[]));
        assert_eq!(Channels::from_count(5), None);
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn error_kinds_channels_image_helpers_and_options() {
        assert_eq!(
            LoadError::new("unknown image type").kind(),
            ErrorKind::UnknownFormat
        );
        assert_eq!(LoadError::new("BMP RLE").kind(), ErrorKind::Unsupported);
        assert_eq!(LoadError::new("too large").kind(), ErrorKind::TooLarge);
        assert_eq!(LoadError::new("outofmem").kind(), ErrorKind::OutOfMemory);
        assert_eq!(
            LoadError::new("premature EOF").kind(),
            ErrorKind::UnexpectedEof
        );
        assert_eq!(
            LoadError::new("bad req_comp").kind(),
            ErrorKind::InvalidInput
        );
        assert_eq!(LoadError::new("bad PNM").kind(), ErrorKind::CorruptData);
        assert_eq!(
            LoadError::new("").to_string(),
            "the image could not be read"
        );

        for c in 1..=4u8 {
            let ch = Channels::try_from(c).unwrap();
            assert_eq!(u8::from(ch), c);
            assert_eq!(ch.has_alpha(), c == 2 || c == 4);
        }
        assert_eq!(Channels::try_from(0), Err(InvalidChannels(0)));
        assert!(InvalidChannels(7).to_string().contains('7'));

        let pgm = b"P5 2 2 255\n\x01\x02\x03\x04";
        let img = load(pgm, None, &LoadOptions::default()).unwrap();
        assert_eq!(img.as_bytes(), &[1, 2, 3, 4]);
        assert_eq!(img.row(0), Some(&[1u8, 2][..]));
        assert_eq!(img.row(1), Some(&[3u8, 4][..]));
        assert_eq!(img.row(2), None);
        assert_eq!(img.pixel(1, 0), Some(&[2u8][..]));
        assert_eq!(img.pixel(0, 1), Some(&[3u8][..]));
        assert_eq!(img.pixel(2, 0), None);
        let mut rows = img.rows();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows.next(), Some(&[1u8, 2][..]));
        assert_eq!(rows.next_back(), Some(&[3u8, 4][..]));
        assert_eq!(rows.next(), None);
        assert_eq!(hash_of(&img), hash_of(&img.clone()));

        // Decode limits (max_width, max_height, max_alloc_bytes)
        let err_w = load(pgm, None, &LoadOptions::new().with_max_width(Some(1))).unwrap_err();
        assert_eq!(err_w.kind(), ErrorKind::TooLarge);
        let err_h = load(pgm, None, &LoadOptions::new().with_max_height(Some(1))).unwrap_err();
        assert_eq!(err_h.kind(), ErrorKind::TooLarge);
        let err_b = load(
            pgm,
            Some(Channels::Rgba),
            &LoadOptions::new().with_max_alloc_bytes(Some(15)),
        )
        .unwrap_err();
        assert_eq!(err_b.kind(), ErrorKind::TooLarge);

        // preserve_16bit_in_f32 preserves low-byte precision on 16-bit PNM
        let pgm16 = b"P5 1 1 65535\n\x00\x01";
        let f_trunc =
            load_f32(pgm16, None, &LoadOptions::new().with_ldr_to_hdr_gamma(1.0)).unwrap();
        assert_eq!(f_trunc.data[0], 0.0); // 0x0001 >> 8 == 0 in 8-bit path
        let f_16 = load_f32(
            pgm16,
            None,
            &LoadOptions::new()
                .with_ldr_to_hdr_gamma(1.0)
                .with_preserve_16bit_in_f32(true),
        )
        .unwrap();
        assert!((f_16.data[0] - 1.0 / 65535.0).abs() < 1e-9);
    }

    #[cfg(feature = "std")]
    #[test]
    fn read_and_file_helpers_work_and_wrap_errors() {
        let pgm = b"P5 2 1 255\n\x10\x20";
        let opts = LoadOptions::default();
        let img8 = load_from_read(&pgm[..], None, &opts).unwrap();
        assert_eq!(img8.data, [0x10, 0x20]);
        let img16 = load_16_from_read(&pgm[..], None, &opts).unwrap();
        assert_eq!(img16.data, [0x1010, 0x2020]);
        let imgf = load_f32_from_read(&pgm[..], None, &opts.with_ldr_to_hdr_gamma(1.0)).unwrap();
        assert!((imgf.data[0] - 16.0 / 255.0).abs() < 1e-6);

        let bad_err = load_from_read(&b"not an image"[..], None, &opts).unwrap_err();
        assert_eq!(bad_err.kind(), std::io::ErrorKind::InvalidData);
        let inner = bad_err
            .get_ref()
            .unwrap()
            .downcast_ref::<LoadError>()
            .unwrap();
        assert_eq!(inner.kind(), ErrorKind::UnknownFormat);

        let dir = std::env::temp_dir();
        let path = dir.join("stb_image_nrp_test_helper.pgm");
        std::fs::write(&path, pgm).unwrap();
        let finfo = info_file(&path).unwrap();
        assert_eq!(
            (finfo.format, finfo.width, finfo.height),
            (ImageFormat::Pnm, 2, 1)
        );
        let floaded = load_file(&path, Some(Channels::Rgb), &opts).unwrap();
        assert_eq!(floaded.data, [0x10, 0x10, 0x10, 0x20, 0x20, 0x20]);
        let _ = std::fs::remove_file(&path);
    }
}

/// The README's examples, compiled and run as doctests.
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
struct ReadmeDoctests;

/// The Guide's examples, compiled and run as doctests.
#[cfg(doctest)]
#[doc = include_str!("../docs/guide.md")]
struct GuideDoctests;
