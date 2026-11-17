# Performance

[Overview](../README.md) · [Guide](guide.md) · [Verification](verification.md) · **Performance**

---

`examples/bench.rs` writes a synthetic 1024 by 768 RGB image, photograph-like (smooth
gradients with a little noise, and some flat areas), as PNG, as JPEG at qualities 90 and 95
and as BMP with stb_image_write_nrp, then reads each back, reporting the best of nine runs:

```sh
cargo run --release --example bench
```

## Throughput

Measured on one core of an Intel Xeon at 2.1 GHz, built for generic x86-64, in megapixels
per second:

| Format | Read |
|---|---|
| PNG | 74.3 |
| JPEG, quality 90 | 109 |
| JPEG, quality 95 | 68.2 |
| BMP | 363 |

Above quality 90, the JPEG writer keeps the chroma at full resolution, which doubles the
blocks to decode.

## Compared with 0.1

Version 0.2 rewrote the crate as idiomatic Rust. Measured alternately with 0.1 on the same
machine:

| Format | Read |
|---|---|
| PNG | 9% faster |
| JPEG, quality 90 | 17% faster |
| JPEG, quality 95 | 11% faster |
| BMP | 14% faster |

PNG gains from copying zlib matches that do not overlap their source in one operation, and
JPEG from converting Y'CbCr to RGB a pixel's worth at a time without bounds checks.
