# Guide

[Overview](../README.md) · **Guide** · [Verification](verification.md) · [Performance](performance.md)

---

- [Reading images](#reading-images)
- [Channels](#channels)
- [Options](#options)
- [Animated GIFs](#animated-gifs)
- [Information without pixels](#information-without-pixels)
- [Errors](#errors)
- [zlib](#zlib)
- [Correspondence with stb_image.h](#correspondence-with-stb_imageh)
- [Differences from stb_image.h](#differences-from-stb_imageh)

## Reading images

Three functions read an image, each recognising the format from the file's contents:

| Function | Values | Conversion |
|---|---|---|
| `load` | `u8` | 16-bit files are reduced to 8 bits, HDR files tone-mapped with `hdr_to_ldr_gamma` and `hdr_to_ldr_scale` |
| `load_16` | `u16` | 8-bit files are widened, each value repeated as its low byte |
| `load_f32` | `f32` | HDR files as they are; others through 8 bits, linearised with `ldr_to_hdr_gamma` and `ldr_to_hdr_scale` |

Each returns an `Image`: its `width` and `height`, the `channels` of its `data`, the
`channels_in_file`, and the values row by row from the top.

```rust
use stb_image_nrp::{Channels, LoadOptions, load_f32};

// A 1 by 1 grey PGM file, read as floats.
let image = load_f32(b"P5 1 1 255\n\xff", None, &LoadOptions::default()).unwrap();
assert_eq!(image.channels, Channels::Grey);
assert_eq!(image.data, [1.0]);
```

## Channels

The second argument chooses the channels of the result: `None` for the file's, or
`Some(channels)` to convert. Grey is expanded to RGB by repetition; RGB is reduced to grey by
the original's weighted sum, `(77 r + 150 g + 29 b) / 256`; and alpha, where added, is
opaque.

## Options

`LoadOptions` holds the settings the original keeps in globals:

| Field | Default | Effect |
|---|---|---|
| `flip_vertically` | `false` | Return the rows from the bottom up |
| `convert_iphone_png_to_rgb` | `false` | Return an iPhone PNG (with a `CgBI` chunk), stored as BGR, as RGB |
| `unpremultiply` | `false` | With the above, divide an iPhone PNG's colours by its alpha |
| `hdr_to_ldr_gamma`, `hdr_to_ldr_scale` | 2.2, 1.0 | How `load` and `load_16` tone-map HDR files |
| `ldr_to_hdr_gamma`, `ldr_to_hdr_scale` | 2.2, 1.0 | How `load_f32` linearises other files |

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
description, such as `"bad zlib header"` or `"too large"`. Where the original fails without
setting a reason, the reason is empty, or the one an earlier step of the same call set.

## zlib

The `zlib` module exposes the PNG reader's zlib decoder: `zlib::decompress` reads a zlib or
raw DEFLATE stream (`zlib::Format`), and `zlib::decompress_with_limit` refuses a stream that
decompresses to more than a given length. Like the original, it does not verify the zlib
checksum.

## Correspondence with stb_image.h

| stb_image.h | stb_image_nrp |
|---|---|
| `stbi_load_from_memory(buffer, len, &x, &y, &comp, req_comp)` | `load(buffer, channels, &options)` |
| `stbi_load_16_from_memory` | `load_16` |
| `stbi_loadf_from_memory` | `load_f32` |
| `stbi_load_gif_from_memory` | `load_gif` |
| `stbi_info_from_memory`, `stbi_is_16_bit_from_memory`, `stbi_is_hdr_from_memory` | `info`, `is_16_bit`, `is_hdr` |
| `stbi_failure_reason()` | `LoadError::reason` |
| `stbi_set_flip_vertically_on_load` and the other settings | `LoadOptions` |
| `stbi_zlib_decode_malloc`, `stbi_zlib_decode_noheader_malloc` | `zlib::decompress` |
| `stbi_zlib_decode_buffer`, `stbi_zlib_decode_noheader_buffer` | `zlib::decompress_with_limit` |

## Differences from stb_image.h

- **Memory only.** Files are read from a slice, not from a path, a `FILE` or callbacks.

- **Settings per call.** The original's global settings are a `LoadOptions` passed to each
  call, so threads can read with different settings.

- **The plain C code path.** The port is of the original built with `STBI_NO_SIMD`, whose
  JPEG reader gives the same pixels on every processor; its SSE2 and NEON paths give
  slightly different pixels for some damaged files.

- **No uninitialised memory.** Where a damaged file leads the original to read memory it
  has not written, the port reads zeros.

- **Positive heights from `info`.** For a BMP stored top row first, the original's
  `stbi_info` reports a negative height while its loader reports a positive one; `info`
  reports the positive height.
