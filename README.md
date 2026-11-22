# stb_image_nrp

**Image file readers in pure, safe Rust, bit-identical to stb_image.h.**

**Overview** · [Guide](docs/guide.md) · [Verification](docs/verification.md) · [Performance](docs/performance.md)

---

stb_image_nrp is a native Rust port of every reader of Sean Barrett's
[`stb_image.h`](https://github.com/nothings/stb) v2.32: PNG, BMP, JPEG, GIF, TGA, PSD, PNM,
HDR and PIC. It produces the pixels the original produces, and refuses the files the original
refuses, with the same reason, damaged files included.

## Highlights

- **The same pixels.** At 8 and 16 bits a channel and as floats, with any number of
  channels requested, and every frame of an animated GIF with its delay.

- **The same refusals.** A damaged file is read as the original reads it, or refused with
  the original's reason (categorised via `LoadError::kind` / `ErrorKind`).

- **`#![no_std]` ready.** Zero dependencies by default (`std` enabled by default for
  `std::io::Read` and file-path helpers; disable default features for `no_std` + `alloc`).
  Enable the optional `fast-zlib` feature to accelerate valid PNG `IDAT` decompression with
  `libdeflate_nrp`.

- **No `unsafe` code.** Enforced by `#![forbid(unsafe_code)]`.

## Installation

```toml
[dependencies]
stb_image_nrp = "0.2"
```

## Quick start

```rust
use stb_image_nrp::{Channels, ImageFormat, LoadOptions, info, load};

// A PPM file of 2 by 1 pixels: one red, one blue.
let file = b"P6\n2 1\n255\n\xff\x00\x00\x00\x00\xff";

let info = info(file).unwrap();
assert_eq!((info.format, info.width, info.height), (ImageFormat::Pnm, 2, 1));

// Read as RGBA (None keeps the file's channels).
let image = load(file, Some(Channels::Rgba), &LoadOptions::default()).unwrap();
assert_eq!(image.channels_in_file, Channels::Rgb);
assert_eq!(image.as_bytes(), [255, 0, 0, 255, 0, 0, 255, 255]);
assert_eq!(image.pixel(1, 0), Some(&[0, 0, 255, 255][..]));

// A damaged file is refused with the original's reason.
assert!(load(&file[..12], None, &LoadOptions::default()).is_err());
```

## Scope

| Format | What is read |
|---|---|
| PNG | 1 to 16 bits a channel, grey, colour and palette, with and without alpha and interlacing |
| BMP | 1, 4, 8, 16, 24 and 32 bits a pixel, without run-length encoding |
| JPEG | Baseline and progressive, 8 bits a channel |
| GIF | Every frame with its delay, or the first |
| TGA | Grey, colour and palette, 8 to 32 bits a pixel, with and without run-length encoding |
| PSD | The merged image of an RGB file, 8 or 16 bits a channel, raw or run-length encoded |
| PNM | PGM and PPM, binary and text, 8 or 16 bits a channel |
| HDR | Radiance RGBE files, flat or run-length encoded |
| PIC | Softimage PIC files, 8 bits a channel, uncompressed or run-length encoded |

Files are read from slices (and, with the default `std` feature, from `std::io::Read` streams
or file paths). EXIF orientation (`1..=8`) can be inspected with `exif_orientation` or
applied automatically via `LoadOptions::with_apply_exif_orientation`. The PNG reader's zlib
decoder is available on its own, as the `zlib` module.

## Licence

As stb is: in the public domain (the Unlicense) or MIT, whichever you prefer; see
[`LICENSE`](LICENSE), which keeps stb's licence.

Bug reports and contributions are welcome, above all any file that stb_image_nrp reads
differently from `stb_image.h`.
