# Guide

[Overview](../README.md) · **Guide** · [Verification](verification.md) · [Performance](performance.md)

---

- [Reading images](#reading-images)
- [Channels](#channels)
- [Options and limits](#options-and-limits)
- [EXIF orientation](#exif-orientation)
- [Animated GIFs](#animated-gifs)
- [Information without pixels](#information-without-pixels)
- [Errors](#errors)
- [Cargo features](#cargo-features)
- [zlib](#zlib)
- [Correspondence with stb_image.h](#correspondence-with-stb_imageh)
- [Differences from stb_image.h](#differences-from-stb_imageh)

## Reading images

Three functions read an image from a byte slice, each recognising the format from the file's
contents:

| Function | Values | Conversion |
|---|---|---|
| `load` | `u8` | 16-bit files are reduced to 8 bits, HDR files tone-mapped with `hdr_to_ldr_gamma` and `hdr_to_ldr_scale` |
| `load_16` | `u16` | 8-bit files are widened, each value repeated as its low byte |
| `load_f32` | `f32` | HDR files as they are; others through 8 bits (or 16 bits when `preserve_16bit_in_f32` is set), linearised with `ldr_to_hdr_gamma` and `ldr_to_hdr_scale` |

With the default `std` feature enabled, `load_from_read`, `load_16_from_read`, and
`load_f32_from_read` read from any `std::io::Read`, while `load_file` and `info_file` read
from a filesystem path.

Each returns an `Image`: its `width` and `height`, the `channels` of its `data`, the
`channels_in_file`, and the values row by row from the top (`as_bytes` on `Image<u8>`, plus
`row`, `rows`, and `pixel`).

```rust
use stb_image_nrp::{Channels, LoadOptions, load_f32};

// A 1 by 1 grey PGM file, read as floats.
let image = load_f32(b"P5 1 1 255\n\xff", None, &LoadOptions::default()).unwrap();
assert_eq!(image.channels, Channels::Grey);
assert_eq!(image.data, [1.0]);
```

## Channels

The second argument chooses the channels of the result: `None` for the file's, or
`Some(channels)` to convert. `Channels` converts to and from `1..=4` via `u8::from` and
`Channels::try_from`. Grey is expanded to RGB by repetition; RGB is reduced to grey by the
original's weighted sum, `(77 r + 150 g + 29 b) / 256`; and alpha, where added, is opaque.

## Options and limits

`LoadOptions` holds the settings the original keeps in globals, together with opt-in bounds
and post-processing flags (all configurable via `LoadOptions::new().with_*` builders):

| Field | Default | Effect |
|---|---|---|
| `flip_vertically` | `false` | Return the rows from the bottom up |
| `convert_iphone_png_to_rgb` | `false` | Return an iPhone PNG (with a `CgBI` chunk), stored as BGR, as RGB |
| `unpremultiply` | `false` | With the above, divide an iPhone PNG's colours by its alpha |
| `hdr_to_ldr_gamma`, `hdr_to_ldr_scale` | 2.2, 1.0 | How `load` and `load_16` tone-map HDR files |
| `ldr_to_hdr_gamma`, `ldr_to_hdr_scale` | 2.2, 1.0 | How `load_f32` linearises other files |
| `max_width`, `max_height` | `None` | Refuse images wider or taller than the limit before allocating |
| `max_alloc_bytes` | `None` | Refuse decoded pixel buffers larger than the byte limit |
| `preserve_16bit_in_f32` | `false` | Linearise 16-bit PNG, PSD, and PNM files directly from `0..=65535` in `load_f32` |
| `apply_exif_orientation` | `false` | Apply JPEG `APP1` / PNG `eXIf` orientation (`1..=8`) before returning |

## EXIF orientation

`exif_orientation(bytes)` extracts the EXIF `Orientation` tag (`1..=8`) from JPEG `APP1` or
PNG `eXIf` chunks without decoding the pixels, and `Image::apply_orientation` transforms an
already-decoded `Image` in place.

## Animated GIFs

`load` reads a GIF's first frame. `load_gif` reads every frame as an `Animation`, each
`Frame` being the whole image as it stands once the frame is drawn, with its delay:

```rust
use stb_image_nrp::{LoadOptions, load_gif};

let gif = b"GIF89a\x01\x00\x01\x00\x80\x00\x00\x00\x00\x00\xff\xff\xff\
\x2c\x00\x00\x00\x00\x01\x00\x01\x00\x00\x02\x02\x44\x01\x00\x3b";
let animation = load_gif(gif, None, &LoadOptions::default()).unwrap();
assert_eq!(animation.frames.len(), 1);
assert_eq!(animation.frames[0].data.len(), 4); // one RGBA pixel
```

## Information without pixels

`info` reads only as far as the header: the `ImageFormat`, size and channels, and whether
the file has 16 bits a channel, is HDR, or is an iPhone PNG. `is_16_bit` and `is_hdr` answer
those questions alone.

## Errors

A file that cannot be read gives a `LoadError`, whose `reason` is the original's short
description (such as `"bad zlib header"` or `"too large"`) and whose `kind` classifies the
failure into `ErrorKind` (`UnknownFormat`, `Unsupported`, `TooLarge`, `OutOfMemory`,
`UnexpectedEof`, `InvalidInput`, or `CorruptData`). Where the original fails without setting
a reason, `reason` is empty, or the one an earlier step of the same call set. Matching
`stb_image.h`, `info` reports `"unknown image type"` whenever no format header succeeds, and
`load_f32` reports `"unknown image type"` when a non-HDR file's 8-bit decode fails.

## Cargo features

- `std` *(enabled by default)*: enables `load_from_read`, `load_16_from_read`,
  `load_f32_from_read`, `load_file`, `info_file`, and `std`'s `f64::powf`. Disable default
  features for `#![no_std]` environments with `alloc`.
- `fast-zlib`: uses `libdeflate_nrp` to decompress valid RFC 1950 PNG `IDAT` streams at high
  speed, falling back automatically to the built-in `stb_image` zlib decoder on iPhone
  (`CgBI`) or damaged streams.

## zlib

The `zlib` module exposes the PNG reader's zlib decoder: `zlib::decompress` reads a zlib or
raw DEFLATE stream (`zlib::Format`), and `zlib::decompress_with_limit` refuses a stream that
decompresses to more than a given length. Like the original, it does not verify the zlib
checksum.

## Correspondence with stb_image.h

| stb_image.h | stb_image_nrp |
|---|---|
| `stbi_load_from_memory(buffer, len, &x, &y, &comp, req_comp)` | `load(buffer, channels, &options)` |
| `stbi_load`, `stbi_load_from_callbacks` | `load_file`, `load_from_read` |
| `stbi_load_16_from_memory`, `stbi_load_16_from_callbacks` | `load_16`, `load_16_from_read` |
| `stbi_loadf_from_memory`, `stbi_loadf_from_callbacks` | `load_f32`, `load_f32_from_read` |
| `stbi_load_gif_from_memory` | `load_gif` |
| `stbi_info_from_memory`, `stbi_info`, `stbi_is_16_bit_from_memory`, `stbi_is_hdr_from_memory` | `info`, `info_file`, `is_16_bit`, `is_hdr` |
| `stbi_failure_reason()` | `LoadError::reason` |
| `stbi_set_flip_vertically_on_load` and the other settings | `LoadOptions` |
| `stbi_zlib_decode_malloc`, `stbi_zlib_decode_noheader_malloc` | `zlib::decompress` |
| `stbi_zlib_decode_buffer`, `stbi_zlib_decode_noheader_buffer` | `zlib::decompress_with_limit` |

## Differences from stb_image.h

- **Settings per call.** The original's global settings are a `LoadOptions` passed to each
  call, so threads can read with different settings.

- **The plain C code path.** The port is of the original built with `STBI_NO_SIMD`, whose
  JPEG reader gives the same pixels on every processor; its SSE2 and NEON paths give
  slightly different pixels for some damaged files.

- **No uninitialised memory.** Where a damaged file leads the original to read memory it
  has not written, the port reads zeros.

- **Positive heights from `info` for top-down BMPs.** For a BMP stored top row first, the
  original's `stbi_info` reports a negative height while its loader reports a positive one;
  `info` reports the positive height.
