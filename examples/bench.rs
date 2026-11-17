//! Decoding throughput on synthetic 1024 by 768 images, written with stb_image_write_nrp.
//!
//! ```sh
//! cargo run --release --example bench
//! ```
//!
//! Each figure is the best of several runs, in megapixels per second.

use stb_image_nrp::{Channels, LoadOptions, load};
use stb_image_write_nrp as write;
use std::time::Instant;

const RUNS: usize = 9;

/// A photograph-like RGB image: smooth gradients with a little noise, and some flat areas.
fn photo_like(w: usize, h: usize) -> Vec<u8> {
    let mut state = 1u64;
    let mut out = Vec::with_capacity(w * h * 3);
    for y in 0..h {
        for x in 0..w {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let noise = (state >> 61) as usize;
            let flat = (x / 128 + y / 128) % 5 == 0;
            for c in 0..3 {
                out.push(if flat {
                    (40 * c + 60) as u8
                } else {
                    ((x * (c + 1) + y * (3 - c)) / 4 + noise) as u8
                });
            }
        }
    }
    out
}

/// The files the benchmark reads: PNG, JPEG at qualities 90 and 95, and BMP.
pub fn files() -> Vec<(&'static str, Vec<u8>)> {
    let (w, h) = (1024, 768);
    let pixels = photo_like(w, h);
    let image = write::Image::new(&pixels, w as u32, h as u32, write::Channels::Rgb).unwrap();
    let options = write::WriteOptions::default();
    vec![
        ("png", write::write_png(&image, &options)),
        ("jpg q90", write::write_jpg(&image, 90, &options)),
        ("jpg q95", write::write_jpg(&image, 95, &options)),
        ("bmp", write::write_bmp(&image, &options)),
    ]
}

fn main() {
    let options = LoadOptions::default();
    for (name, file) in files() {
        let mut best = f64::MAX;
        let mut pixels = 0;
        for _ in 0..RUNS {
            let start = Instant::now();
            let image = load(&file, Some(Channels::Rgb), &options).unwrap();
            best = best.min(start.elapsed().as_secs_f64());
            pixels = image.width as usize * image.height as usize;
        }
        println!("{name:8} {:8.2} Mpixel/s", pixels as f64 / 1e6 / best);
    }
}
