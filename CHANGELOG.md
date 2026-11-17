# Changelog

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
