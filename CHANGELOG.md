# Changelog

## 0.3.0

- **Softimage `.pic`, EXIF orientation, streaming readers and `fast-zlib`.** Ported
  `stb_image.h`'s Softimage `.pic` reader (`ImageFormat::Pic`), added EXIF orientation tag
  parsing and lossless pixel transforms (`exif_orientation`,
  `LoadOptions::apply_exif_orientation`), added `std::io::Read` and file-path loading helpers
  under the default `std` feature, and added an optional `fast-zlib` feature using
  `libdeflate_nrp` 0.3 for faster PNG decompression.

- **`#![no_std]` and defensive `LoadOptions` limits.** Disabling default features builds with
  `#![no_std]` and `alloc`. `LoadOptions` adds `max_width`, `max_height`, `max_alloc_bytes`
  and `preserve_16bit_in_f32`, `LoadError` exposes a structured `ErrorKind`, and `Image<T>`
  adds `rows`, `pixel` and `as_bytes`.

- **Memory, 32-bit overflow and DoS hardening.** Widened GIF stream offsets to `usize`,
  checked element byte widths in `convert_format`, capped `zlib` expansion with fallible
  `try_reserve`, checked binary PNM and TGA stream lengths before allocation, rejected 0-pixel
  BMP/PSD loads, converted GIF frames in place (~3x lower peak RAM), reused PNG Adam7 pass
  buffers, and replaced `ldr_to_hdr`'s `powf` with a bit-identical 256-entry lookup table.

- Output is unchanged under default options, checked against `stb_image.h` on 360,000 files.

## 0.2.1

- **Stricter lints, enforced in CI.** `Cargo.toml` turns on clippy's pedantic group for every
  target, and forbids `unsafe` code in the tests, examples and benchmarks as well as the
  library. Functions that return a `Result` document their errors.

- **Idiomatic internals.** The decoders' steps return a `Result` in place of a success flag,
  so a failure passes up with `?`; the reason for it is recorded where stb_image records it,
  so every reason given is the same. The zlib decoder's Huffman tables report their own
  failure in place of filling in an out-parameter.

- Output is unchanged, checked against `stb_image.h` on 360,000 files, and so is speed (the
  same instruction count to within 0.001%).

## 0.2.0

- **Breaking: an idiomatic API.** The readers drop the original's `_from_memory` suffix:
  `load`, `load_16`, `load_f32`, `load_gif`, `info`, `is_16_bit` and `is_hdr`. The channels
  to read are an `Option<Channels>` in place of a count where 0 meant the file's own. An
  image is an `Image<T>` with `width`, `height`, `channels` and `channels_in_file` in place
  of `Loaded`'s `x`, `y` and integer counts; an animated GIF is an `Animation` of `Frame`s,
  each with its delay. `Info` has `format` (an `ImageFormat`, whose `code()` gives the
  original's number), `width`, `height` and `channels`. `LoadError` keeps its reason
  private, behind `reason()`. The zlib decoder's four `zlib_decode_*` functions are
  `zlib::decompress(stream, format)` and `zlib::decompress_with_limit(stream, format,
  max_len)`, with a `zlib::Format` in place of a header flag.

- **One behaviour change.** `info` gives a positive height for a BMP stored top row first,
  where the original gives a negative one (see the Guide's
  [differences](docs/guide.md#differences-from-stb_imageh)).

- **Faster.** PNG reads 9% faster, JPEG 11% to 17% and BMP 14% (see
  [Performance](docs/performance.md)). The pixels are unchanged, checked against
  `stb_image.h` on 360,000 files.

- **Rewritten as idiomatic Rust.** The code no longer mirrors the original's C line by line.

- **Documentation.** Every public item is documented, the README is a short overview, and
  the detail is in [Guide](docs/guide.md), [Verification](docs/verification.md) and
  [Performance](docs/performance.md).

## 0.1.0

- First release: the port of stb_image.h's readers, moved out of [rustbcx](https://github.com/OctoKeights/rustbcx), where
  it was `rustbcx::stb_image`.
