# stb_image_nrp

**Image file readers in pure, safe Rust: a native Rust port of stb_image.h.**

stb_image_nrp is a line-for-line port of every reader of Sean Barrett's
[`stb_image.h`](https://github.com/nothings/stb) v2.32: PNG, BMP, JPEG, GIF, TGA, PSD, PNM and
HDR. Each gives the pixels the original gives, and refuses a file the original refuses, with
the same reason, the damaged ones too.

## Highlights

- **The same pixels.** At 8 and 16 bits a channel and as floats, with every number of
  channels asked for, and every frame of a GIF with its delay.

- **The same refusals.** A damaged file is read as the original reads it, or refused with
  its reason (`stbi_failure_reason()`).

- **Zero dependencies.** Standard library only: no C compiler, no build script.

- **No `unsafe` code.** Enforced by `#![forbid(unsafe_code)]`.

## Installation

```toml
[dependencies]
stb_image_nrp = "0.1"
```

## Quick start

```rust
use stb_image_nrp::{LoadOptions, info_from_memory, load_from_memory};

// A PPM file of 2 by 1 pixels: one red, one blue.
let file = b"P6\n2 1\n255\n\xff\x00\x00\x00\x00\xff";
let info = info_from_memory(file).unwrap();
assert_eq!((info.x, info.y), (2, 1));

// Read with four channels (0 for as many as the file has).
let image = load_from_memory(file, 4, &LoadOptions::default()).unwrap();
assert_eq!(image.channels_in_file, 3);
assert_eq!(image.data, [255, 0, 0, 255, 0, 0, 255, 255]);

// A damaged file is refused with the original's reason.
assert!(load_from_memory(&file[..12], 0, &LoadOptions::default()).is_err());
```

## What is ported

| Format | What is read |
|---|---|
| PNG | 1, 2, 4, 8 and 16 bits a channel, grey, colour and palette, with and without alpha and interlacing; with the original's own zlib decoder (the `zlib` module) |
| BMP | 1, 4, 8, 16, 24 and 32 bits a pixel, without run-length encoding |
| JPEG | Baseline and progressive, 8 bits a channel |
| GIF | Every frame, with its delay (`load_gif_from_memory`), or the first |
| TGA | Grey, colour and palette, 8 to 32 bits a pixel, with and without run-length encoding |
| PSD | The merged image of an RGB file, 8 or 16 bits a channel, raw or run-length encoded |
| PNM | `.pgm` and `.ppm`, binary and text, 8 or 16 bits a channel |
| HDR | Radiance `.hdr` files, flat or run-length encoded, as floats (`loadf_from_memory`) |

`load_from_memory` gives 8 bits a channel, `load_16_from_memory` 16 and `loadf_from_memory`
floats; `info_from_memory` reads only as far as the size. The settings the original keeps in
globals (flipping, gamma and scale, unpremultiplying, and so on) are a `LoadOptions` handed to
each call.

## Differences from the original

The code is translated from C to Rust. Reading is from memory only. What is ported is the
original as it builds with `STBI_NO_SIMD`: its JPEG reader's inverse transform in plain C,
which gives one answer on every processor (the SSE2 and NEON ones give other pixels for some
damaged files, and the same for every file an encoder made). Where the original reads memory
it has not set, which a damaged file can bring it to, the port reads zero. What they compute
is kept: the same pixels from every file, and the same reason for every file refused.

## Verification

`compare/compare.sh` builds `compare/dump_stb_image.c` against `stb_image.h` from a checkout
of stb (and its `stb_image_write.h` and miniz, which it makes files with), and
`examples/dump.rs` against the port; it runs both over the same made-up files and compares
what they print byte for byte:

```sh
MINIZ=<checkout of miniz> compare/compare.sh <checkout of stb> [count] [seeds] [dumps]
```

| Dump | What it covers | Last full run |
|---|---|---|
| `stb_image` | Made-up files asked their size and read at 8 and 16 bits a channel with every number of channels asked for: images written by stb_image_write, and files put together from every kind of header the readers take (PNG of every depth and colour type, with palettes, transparent colours and interlacing; BMP of every header size and depth, with palettes and bit masks; baseline and progressive JPEG of one to four components sampled every way, with restart markers); a third damaged first. Made-up zlib streams, whole and damaged, through its zlib decoder | 60,000 files: 46,500 read (105 million pixels) and 13,300 refused, each for the same reason |
| `stb_image_more` | Made-up `.tga` files of every kind; `.psd` files of none to five channels at 8 and 16 bits, raw and run-length encoded; `.gif` files of one to three frames with global and local colour tables, interlacing, transparency and every way of disposing of a frame; binary and text `.pgm` and `.ppm` files with every kind of maximum value; and Radiance `.hdr` files, flat and run-length encoded; a third damaged first. Each asked its size and whether it is HDR, and read at 8 bits, 16 bits and as floats with made-up gamma and scale; GIFs also as animations | 860,000 files: 750,000 read and 108,000 refused, each for the same reason |

The C was built by gcc and clang at several optimisation levels, and the larger runs repeated
with Rust's overflow checks enabled.

`cargo test` needs no C compiler: the tests check the port against hashes recorded from
`stb_image.h` (`tests/data`), and read badly damaged files to check that each is read or
refused without a panic.

## Licence

As stb is: in the public domain (the Unlicense) or MIT, whichever you prefer
([LICENSE](LICENSE), which keeps stb's licence).
