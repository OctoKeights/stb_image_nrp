# Verification

[Overview](../README.md) · [Guide](guide.md) · **Verification** · [Performance](performance.md)

---

stb_image_nrp is checked against `stb_image.h` itself: both are run over the same synthetic
files and their output compared byte for byte.

## The comparison

`compare/compare.sh` builds `compare/dump_stb_image.c` against `stb_image.h` from a checkout
of stb, with every reader, its plain C code path (`STBI_NO_SIMD`), memory that starts as
zeros and wrapping overflow, so that a damaged file has one answer; and `examples/dump.rs`
against stb_image_nrp. It runs both and compares what they print:

```sh
MINIZ=<checkout of miniz> compare/compare.sh <checkout of stb> [count] [seeds] [dumps]
```

Both programs generate the same files from each seed and print a running hash of the pixels
each is read as, or of the reason it is refused:

- **stb_image**: images written by `stb_image_write.h` (stb_image_write_nrp on the Rust
  side), and PNG, BMP and JPEG files assembled with every kind of header the three readers
  accept: PNG of every depth and colour type, with palettes, transparent colours and
  interlacing; BMP of every header size and depth, with palettes and bit masks; and JPEG,
  baseline and progressive, of one to four components sampled every way, with restart
  markers. A third are then damaged. Each is asked its size and read at 8 bits a channel
  with a random number of channels requested, and every third also at 16 bits. Every fourth
  also passes synthetic data, whole and damaged, through the zlib decoder.

- **stb_image_more**: TGA, PSD, GIF, PNM and HDR files of every kind the readers accept,
  a third of them damaged. Each is asked its size and whether it is HDR, read at 8 bits a
  channel, as floats and every other at 16 bits, with a random gamma and scale; GIFs and
  every fourth file are also read as animations, every frame with its delay.

## Results

| Dump | Files | Read | Refused | Result |
|---|---|---|---|---|
| stb_image | 180,000 (483 MB) | 139,256 (314 million pixels) | 40,151 | Every image and every reason identical |
| stb_image_more | 180,000 (273 MB) | 156,996 | 22,652 | Every image and every reason identical |

The remaining files, under 1%, declare dimensions too large to decode in the comparison and
are skipped by both programs.

## Tests

`cargo test` needs no C compiler. `tests/golden.rs` checks stb_image_nrp against hashes
recorded from `stb_image.h` (`tests/data/`, 1,200 files of each dump), reads badly damaged
files to check that each is read or refused without a panic, and the examples in the README
and the Guide are run as tests. `tests/data/README.md` explains how to record the hashes
again.
