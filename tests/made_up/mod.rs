//! Synthetic data, generated from a seed exactly as the C side of the comparison
//! (`compare/dump_stb_image.c`) generates it, and the dumps that both the golden tests and
//! `examples/dump.rs` run.

// (The arithmetic is written as the C side has it; not every user of this module uses all of
// it.)
#![allow(
    clippy::manual_is_multiple_of,
    clippy::needless_range_loop,
    clippy::manual_div_ceil,
    clippy::manual_checked_ops,
    dead_code
)]

/// The two programs make the same blocks from the same seed: a 64-bit linear congruential
/// generator, the high half of its state.
pub struct Random {
    pub state: u64,
}

impl Random {
    pub fn next(&mut self) -> u32 {
        self.state = self
            .state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (self.state >> 32) as u32
    }
}

/// FNV-1a, 64 bits.
pub struct Hash {
    pub value: u64,
}

impl Hash {
    pub fn new() -> Hash {
        Hash {
            value: 14695981039346656037,
        }
    }

    pub fn add(&mut self, data: &[u8]) {
        for &byte in data {
            self.value ^= u64::from(byte);
            self.value = self.value.wrapping_mul(1099511628211);
        }
    }
}

const WORDS: [&str; 16] = [
    "the", "texture", "block", "of", "and", "compress", "a", "level", "mip", "to", "pixels", "in",
    "format", "is", "cube", "data",
];

/// Synthetic data of one of eight kinds, the same in both programs.
pub fn make_data(r: &mut Random) -> Vec<u8> {
    let len = match r.next() % 8 {
        0 => r.next() % 16,
        1 => r.next() % 300,
        2 | 3 => r.next() % 5000,
        4 | 5 => r.next() % 40000,
        6 => 60000 + r.next() % 20000,
        _ => r.next() % 200000,
    } as usize;
    let kind = r.next() % 8;
    let mut buf = Vec::with_capacity(len);
    match kind {
        // noise
        0 => {
            for _ in 0..len {
                buf.push((r.next() >> 13) as u8);
            }
        }
        // four letters
        1 => {
            for _ in 0..len {
                buf.push(b'a' + (r.next() % 4) as u8);
            }
        }
        // words
        2 => {
            while buf.len() < len {
                let word = WORDS[(r.next() % 16) as usize];
                for &byte in word.as_bytes() {
                    if buf.len() < len {
                        buf.push(byte);
                    }
                }
                if buf.len() < len {
                    buf.push(b' ');
                }
            }
        }
        // runs
        3 => {
            while buf.len() < len {
                let byte = r.next() as u8;
                let run = 1 + r.next() % 300;
                for _ in 0..run {
                    if buf.len() < len {
                        buf.push(byte);
                    }
                }
            }
        }
        // copies of what came before
        4 => {
            while buf.len() < len {
                if buf.len() < 64 || r.next() % 4 == 0 {
                    buf.push((r.next() >> 9) as u8);
                } else {
                    let reach = buf.len().min(40000);
                    let dist = 1 + r.next() as usize % reach;
                    let n = 3 + r.next() % 260;
                    for _ in 0..n {
                        if buf.len() < len {
                            buf.push(buf[buf.len() - dist]);
                        }
                    }
                }
            }
        }
        // nothing
        5 => buf.resize(len, 0),
        // a slope
        6 => {
            for i in 0..len {
                buf.push((i / 7 + i % 13) as u8);
            }
        }
        // stretches of noise, of one value and of counting
        _ => {
            while buf.len() < len {
                let (stretch, what, value) = (1 + r.next() % 2000, r.next() % 3, r.next());
                for i in 0..stretch {
                    if buf.len() < len {
                        buf.push(match what {
                            0 => (r.next() >> 11) as u8,
                            1 => value as u8,
                            _ => value.wrapping_add(i) as u8,
                        });
                    }
                }
            }
        }
    }
    buf
}

/// A synthetic image of 8 bits a channel: its channels, its size, how far its rows are apart,
/// and its bytes, of one of seven kinds. What lies between the rows is 0xCD.
pub struct Picture {
    pub w: i32,
    pub h: i32,
    pub comp: i32,
    pub stride: usize,
    pub bytes: Vec<u8>,
}

fn wave(t: u32) -> u32 {
    let v = (t & 511) as i32 - 256;
    v.unsigned_abs().min(255)
}

pub fn make_picture(r: &mut Random) -> Picture {
    let comp = 1 + (r.next() % 4) as usize;
    let (w, h) = match r.next() % 6 {
        0 => (1 + r.next() % 4, 1 + r.next() % 4),
        1 | 2 => (1 + r.next() % 40, 1 + r.next() % 40),
        3 | 4 => (1 + r.next() % 100, 1 + r.next() % 100),
        _ => (1 + r.next() % 300, 1 + r.next() % 200),
    };
    let (w, h) = (w as usize, h as usize);
    let stride = w * comp
        + if r.next() % 3 == 0 {
            (r.next() % 7) as usize
        } else {
            0
        };
    let mut bytes = vec![0xCDu8; stride * h];
    let kind = r.next() % 7;
    let mut coefficient = [[0u32; 3]; 4];
    for c in &mut coefficient {
        *c = [r.next() % 64, r.next() % 64, r.next() % 4096];
    }
    let mut palette = [[0u32; 4]; 16];
    for colour in &mut palette {
        for c in colour {
            *c = r.next() & 255;
        }
    }
    let cell = 1 + r.next() % 12;
    let colours = 2 + r.next() % 4;
    let noise = 1 + r.next() % 32;
    for y in 0..h {
        for x in 0..w {
            let (ux, uy) = (x as u32, y as u32);
            let pick = if kind == 5 {
                r.next() % colours
            } else {
                ((ux / cell) * 7 + (uy / cell) * 13 + (ux / cell) * (uy / cell)) % 16
            } as usize;
            for c in 0..comp {
                let along = ux * coefficient[c][0] + uy * coefficient[c][1] + coefficient[c][2];
                let slope = along >> 4;
                let value = match kind {
                    0 => r.next() >> 13,           // noise
                    1 => palette[0][c],            // one colour
                    2 => slope,                    // a slope
                    3 => palette[pick][c],         // squares
                    4 => slope + r.next() % noise, // a slope with noise on it
                    5 => palette[pick][c],         // a few colours, anywhere
                    _ => wave(along),              // waves
                };
                bytes[y * stride + x * comp + c] = value as u8;
            }
        }
    }
    Picture {
        w: w as i32,
        h: h as i32,
        comp: comp as i32,
        stride,
        bytes,
    }
}

fn p16be(f: &mut Vec<u8>, value: u32) {
    f.extend_from_slice(&(value as u16).to_be_bytes());
}

fn p32be(f: &mut Vec<u8>, value: u32) {
    f.extend_from_slice(&value.to_be_bytes());
}

fn p16le(f: &mut Vec<u8>, value: u32) {
    f.extend_from_slice(&(value as u16).to_le_bytes());
}

fn p32le(f: &mut Vec<u8>, value: u32) {
    f.extend_from_slice(&value.to_le_bytes());
}

/// A chunk of a PNG file. (No checksum: the reader does not look at it.)
fn chunk(f: &mut Vec<u8>, name: &[u8; 4], bytes: &[u8]) {
    p32be(f, bytes.len() as u32);
    f.extend_from_slice(name);
    f.extend_from_slice(bytes);
    p32be(f, 0);
}

/// A synthetic PNG file: a header of any of the fifteen kinds, and synthetic bytes as its rows.
pub fn make_png(r: &mut Random, f: &mut Vec<u8>) {
    const SIG: [u8; 8] = [137, 80, 78, 71, 13, 10, 26, 10];
    const DEPTH: [u8; 16] = [1, 2, 4, 8, 16, 8, 16, 1, 2, 4, 8, 8, 16, 8, 16, 8];
    const COLOR: [u8; 16] = [0, 0, 0, 0, 0, 2, 2, 3, 3, 3, 3, 4, 4, 6, 6, 2];
    const CHANNELS: [u8; 7] = [1, 0, 3, 1, 2, 0, 4];
    const X_ORIG: [u32; 7] = [0, 4, 0, 2, 0, 1, 0];
    const Y_ORIG: [u32; 7] = [0, 0, 4, 0, 2, 0, 1];
    const X_SPC: [u32; 7] = [8, 8, 4, 4, 2, 2, 1];
    const Y_SPC: [u32; 7] = [8, 8, 8, 4, 4, 2, 2];
    let kind = (r.next() % 16) as usize;
    let (depth, color) = (u32::from(DEPTH[kind]), u32::from(COLOR[kind]));
    let channels = u32::from(CHANNELS[color as usize]);
    let (w, h) = if r.next() % 8 == 0 {
        let w = 1 + r.next() % 200;
        (w, 1 + r.next() % 120)
    } else {
        let w = 1 + r.next() % 40;
        (w, 1 + r.next() % 40)
    };
    let interlace = r.next() % 3 == 0;
    let style = r.next() % 3;
    let iphone = r.next() % 16 == 0;

    // The rows, each behind its filter: of the whole image, or of each of the seven passes.
    let mut raw = Vec::new();
    for pass in 0..if interlace { 7 } else { 1 } {
        let (mut pw, mut ph) = (w, h);
        if interlace {
            pw = (w.wrapping_sub(X_ORIG[pass]).wrapping_add(X_SPC[pass] - 1)) / X_SPC[pass];
            ph = (h.wrapping_sub(Y_ORIG[pass]).wrapping_add(Y_SPC[pass] - 1)) / Y_SPC[pass];
        }
        if pw == 0 || ph == 0 {
            continue;
        }
        let row_bytes = (pw * channels * depth + 7) / 8;
        for y in 0..ph {
            let mut filter = r.next() % 5;
            if r.next() % 4000 == 0 {
                filter = 5 + r.next() % 3;
            }
            raw.push(filter as u8);
            for x in 0..row_bytes {
                raw.push(match style {
                    0 => r.next() >> 13,
                    1 => r.next() % 4,
                    _ => x * 3 + y,
                } as u8);
            }
        }
    }
    // Now and then too few of them, or more than are needed.
    let how = r.next() % 40;
    if how == 0 {
        let fewer = 1 + r.next() as usize % raw.len();
        raw.truncate(raw.len() - fewer);
    } else if how == 1 {
        raw.extend_from_slice(&[0; 5]);
    }

    let z = if r.next() % 4 == 0 {
        stb_image_write_nrp::zlib_compress(&raw, 5 + r.next() % 8)
    } else {
        miniz_nrp::compress(&raw, miniz_nrp::Level::new((r.next() % 10) as u8).unwrap())
    };

    f.extend_from_slice(&SIG);
    if iphone {
        chunk(f, b"CgBI", &[0; 4]);
    }
    let mut header = Vec::new();
    p32be(&mut header, w);
    p32be(&mut header, h);
    header.extend_from_slice(&[depth as u8, color as u8, 0, 0, u8::from(interlace)]);
    chunk(f, b"IHDR", &header);
    if r.next() % 4 == 0 {
        chunk(f, b"tEXt", b"made up\0text");
    }
    // A palette: nearly always where one is needed, now and then where none is.
    let mut pal_len = 0u32;
    if if color == 3 {
        r.next() % 20 != 0
    } else {
        r.next() % 8 == 0
    } {
        pal_len = 1 + r.next() % 256;
        let bytes: Vec<u8> = (0..pal_len * 3).map(|_| (r.next() >> 12) as u8).collect();
        chunk(f, b"PLTE", &bytes);
    }
    // Transparency: alphas for a palette, or the one colour that is to be clear.
    if r.next() % 3 == 0 {
        let mut n = 0;
        if color == 3 {
            n = 1 + r.next() % if pal_len != 0 { pal_len } else { 1 };
            if r.next() % 30 == 0 {
                n = pal_len + 1;
            }
        } else if color == 0 {
            n = 2;
        } else if color == 2 {
            n = 6;
        } else if r.next() % 10 == 0 {
            n = 2;
        }
        if n > 256 {
            n = 256;
        }
        let bytes: Vec<u8> = (0..n)
            .map(|_| {
                if color == 3 {
                    (r.next() >> 9) as u8
                } else {
                    (r.next() % 4) as u8
                }
            })
            .collect();
        if n != 0 {
            chunk(f, b"tRNS", &bytes);
        }
    }
    let parts = 1 + r.next() % 3;
    if r.next() % 16 == 0 {
        chunk(f, b"IDAT", &[]);
    }
    // (An iPhone file's stream has no zlib header.)
    let mut at = if iphone && z.len() > 2 { 2 } else { 0 };
    for k in 0..parts {
        let n = if k + 1 == parts {
            z.len() - at
        } else {
            r.next() as usize % (z.len() - at + 1)
        };
        chunk(f, b"IDAT", &z[at..at + n]);
        at += n;
    }
    chunk(f, b"IEND", &[]);
}

/// A synthetic BMP file: a header of any of the five sizes, and synthetic bytes as its rows.
pub fn make_bmp(r: &mut Random, f: &mut Vec<u8>) {
    const HSZ: [u32; 7] = [12, 40, 40, 40, 56, 108, 124];
    const BPP: [u32; 6] = [1, 4, 8, 16, 24, 32];
    let hsz = HSZ[(r.next() % 7) as usize];
    let mut bpp = BPP[(r.next() % 6) as usize];
    let mut compress = 0;
    let mut masks = [0u32; 4];
    let (mut psize, mut extra) = (0u32, 0u32);
    if r.next() % 40 == 0 {
        bpp = if r.next() % 2 != 0 { 2 } else { 20 };
    }
    if (bpp == 16 || bpp == 32) && r.next() % 2 != 0 {
        compress = 3;
    }
    if r.next() % 50 == 0 {
        compress = 1 + r.next() % 4;
    }
    let w = 1 + r.next() % 40;
    let h = 1 + r.next() % 40;
    let top_down = hsz != 12 && r.next() % 4 == 0;
    let mask_style = r.next() % 3;
    if mask_style == 0 {
        masks = if bpp == 16 {
            [0xf800, 0x07e0, 0x001f, 0]
        } else {
            [0xff0000, 0xff00, 0xff, 0xff000000]
        };
    } else if mask_style == 1 {
        masks = if bpp == 16 {
            [0x7c00, 0x03e0, 0x001f, 0x8000]
        } else {
            [0xff, 0xff00, 0xff0000, 0xff000000]
        };
    } else {
        for mask in &mut masks {
            let count = 1 + r.next() % 8;
            let top = if bpp == 16 { 16 } else { 32 } - count;
            let shift = r.next() % (top + 1);
            *mask = ((1u32 << count) - 1) << shift;
        }
    }
    if bpp < 16 {
        psize = 1 << bpp;
        if r.next() % 4 == 0 {
            psize = 1 + r.next() % psize;
        }
    }
    if r.next() % 10 == 0 {
        extra = 1 + r.next() % 20;
    }
    let offset = 14
        + hsz
        + if hsz == 40 && compress == 3 { 12 } else { 0 }
        + psize * if hsz == 12 { 3 } else { 4 }
        + extra;

    f.extend_from_slice(b"BM");
    p32le(f, 0);
    p32le(f, 0);
    p32le(f, offset);
    p32le(f, hsz);
    if hsz == 12 {
        p16le(f, w);
        p16le(f, h);
        p16le(f, 1);
        p16le(f, bpp);
    } else {
        p32le(f, w);
        p32le(f, if top_down { 0u32.wrapping_sub(h) } else { h });
        p16le(f, 1);
        p16le(f, bpp);
        p32le(f, compress);
        for _ in 0..5 {
            p32le(f, 0);
        }
        if hsz == 40 && compress == 3 {
            for mask in &masks[..3] {
                p32le(f, *mask);
            }
        }
        if hsz >= 56 {
            for mask in masks {
                p32le(f, mask);
            }
        }
        if hsz >= 108 {
            for _ in 0..13 {
                p32le(f, 0);
            }
        }
        if hsz == 124 {
            for _ in 0..4 {
                p32le(f, 0);
            }
        }
    }
    for _ in 0..psize * if hsz == 12 { 3 } else { 4 } + extra {
        f.push((r.next() >> 10) as u8);
    }
    let row_bytes = ((w * bpp + 31) / 32) * 4;
    let mut pixels_size = row_bytes * h;
    // Now and then cut short.
    if r.next() % 10 == 0 {
        pixels_size -= r.next() % (pixels_size / 2 + 1);
    }
    for _ in 0..pixels_size {
        f.push((r.next() >> 13) as u8);
    }
}

/// Synthetic bytes for a JPEG scan: none of them the 0xFF that begins a marker, unless followed
/// by the zero that says it is none (and, once in a while, by whatever comes next).
fn entropy_bytes(r: &mut Random, f: &mut Vec<u8>, n: u32, style: u32) {
    for _ in 0..n {
        let mut b = (r.next() >> 11) & 255;
        if style == 1 && r.next() % 4 != 0 {
            b = 0;
        }
        if style == 2 {
            b &= 0x0f;
        }
        f.push(b as u8);
        if b == 0xff && r.next() % 64 != 0 {
            f.push(0);
        }
    }
}

/// The bytes of a scan. With a restart interval, in stretches with a restart marker after
/// each: no more of them than the scan has intervals, and each short enough that the reader
/// has come to its marker by the time the interval is over.
fn entropy(r: &mut Random, f: &mut Vec<u8>, blocks: u32, mcus: u32, restart: u32) {
    let style = r.next() % 3;
    if restart != 0 {
        let segments = r.next() % ((mcus + restart - 1) / restart);
        for k in 0..segments {
            let n = r.next() % (restart / 2 + 2);
            entropy_bytes(r, f, n, style);
            f.push(0xff);
            f.push((0xd0 + (k & 7)) as u8);
        }
        let n = r.next() % (restart / 2 + 2);
        entropy_bytes(r, f, n, style);
    } else {
        let n = blocks * (1 + r.next() % 6);
        entropy_bytes(r, f, n, style);
    }
}

/// A synthetic JPEG file: headers of every kind the reader takes, Huffman tables in which most
/// bits mean something, and synthetic bytes as the picture.
pub fn make_jpeg(r: &mut Random, f: &mut Vec<u8>) {
    const COMPS: [u32; 5] = [1, 3, 3, 3, 4];
    const SAMPLING: [u8; 8] = [0x11, 0x22, 0x21, 0x12, 0x41, 0x22, 0x11, 0x42];
    const DC: [&[u8]; 3] = [
        &[0, 1, 2, 3],
        &[0, 2, 1, 5],
        &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15],
    ];
    const DC_LENGTH: [u32; 3] = [2, 2, 4];
    const DC_PICK: [usize; 5] = [0, 0, 1, 1, 2];
    const AC: [&[u8]; 2] = [
        &[0x00, 0x01, 0x11, 0x21, 0x10, 0xF0, 0x31, 0x20],
        &[
            0x00, 0x01, 0x02, 0x03, 0x11, 0x12, 0x21, 0x31, 0x41, 0x10, 0x20, 0xF0, 0x04, 0x13,
            0x51, 0x30,
        ],
    ];
    let progressive = r.next() % 2 != 0;
    let ncomp = COMPS[(r.next() % 5) as usize];
    let w = 1 + r.next() % 64;
    let h = 1 + r.next() % 64;
    let y_sampling = u32::from(SAMPLING[(r.next() % 8) as usize]);
    let rgb_ids = r.next() % 8 == 0;
    let (mut dc_kind, mut ac_kind, mut ac_offset) = ([0usize; 2], [0u32; 2], [0u32; 2]);
    let mut restart = 0;
    for t in 0..2 {
        dc_kind[t] = DC_PICK[(r.next() % 5) as usize];
        ac_kind[t] = r.next() % 3;
        ac_offset[t] = if r.next() % 2 != 0 { 0 } else { r.next() & 255 };
    }
    if r.next() % 3 == 0 {
        restart = 1 + r.next() % 8;
    }

    p16be(f, 0xffd8);
    if r.next() % 2 != 0 {
        p16be(f, 0xffe0);
        p16be(f, 16);
        f.extend_from_slice(b"JFIF\0\x01\x01\0\0\x01\0\x01\0\0");
    }
    if if ncomp == 4 {
        r.next() % 4 != 0
    } else {
        r.next() % 3 == 0
    } {
        p16be(f, 0xffee);
        p16be(f, 14);
        f.extend_from_slice(b"Adobe\0\x64\0\0\0\0");
        f.push((r.next() % 3) as u8);
    }
    if r.next() % 4 == 0 {
        let n = r.next() % 20;
        p16be(f, 0xfffe);
        p16be(f, 2 + n);
        for _ in 0..n {
            f.push((r.next() >> 10) as u8);
        }
    }

    // Two tables of divisors, of one byte a value or of two.
    let sixteen = r.next() % 6 == 0;
    let range = if r.next() % 3 == 0 { 64 } else { 4 };
    p16be(f, 0xffdb);
    p16be(f, 2 + 2 * if sixteen { 129 } else { 65 });
    for t in 0..2u8 {
        f.push((u8::from(sixteen) << 4) | t);
        for _ in 0..64 {
            let v = 1 + r.next() % range;
            if sixteen {
                p16be(f, v);
            } else {
                f.push(v as u8);
            }
        }
    }
    if restart != 0 {
        p16be(f, 0xffdd);
        p16be(f, 4);
        p16be(f, restart);
    }

    // The frame.
    f.push(0xff);
    f.push(if progressive {
        0xc2
    } else if r.next() % 4 == 0 {
        0xc1
    } else {
        0xc0
    });
    p16be(f, 8 + 3 * ncomp);
    f.push(8);
    p16be(f, h);
    p16be(f, w);
    f.push(ncomp as u8);
    let (mut id, mut hv) = ([0u32; 4], [0u32; 4]);
    for c in 0..ncomp as usize {
        id[c] = if rgb_ids && ncomp == 3 {
            u32::from(b"RGB"[c])
        } else {
            c as u32 + 1
        };
        hv[c] = if c == 0 || (c == 3 && r.next() % 2 != 0) {
            y_sampling
        } else {
            0x11
        };
        f.extend_from_slice(&[id[c] as u8, hv[c] as u8, if c == 0 { 0 } else { 1 }]);
    }
    let (hmax, vmax) = (y_sampling >> 4, y_sampling & 15);
    let mcus = ((w + 8 * hmax - 1) / (8 * hmax)) * ((h + 8 * vmax - 1) / (8 * vmax));
    let blocks = mcus * (hmax * vmax + ncomp - 1);

    // The Huffman tables: two for the first values of blocks, two for the rest.
    let mut size = 2;
    for t in 0..2 {
        size += 17 + DC[dc_kind[t]].len() as u32 + 17 + [192, 8, 16][ac_kind[t] as usize];
    }
    p16be(f, 0xffc4);
    p16be(f, size);
    for t in 0..2 {
        f.push(t as u8);
        for k in 1..=16 {
            f.push(if k == DC_LENGTH[dc_kind[t]] {
                DC[dc_kind[t]].len() as u8
            } else {
                0
            });
        }
        f.extend_from_slice(DC[dc_kind[t]]);
    }
    for t in 0..2 {
        f.push(0x10 | t as u8);
        if ac_kind[t] == 0 {
            // 64 codes of seven bits and 128 of eight, which between them use every run of
            // bits, for 192 synthetic symbols.
            for k in 1..=16 {
                f.push(match k {
                    7 => 64,
                    8 => 128,
                    _ => 0,
                });
            }
            for k in 0..192u32 {
                f.push((k * 167 + ac_offset[t]) as u8);
            }
        } else {
            let table = AC[ac_kind[t] as usize - 1];
            for k in 1..=16 {
                f.push(if k == if ac_kind[t] == 1 { 3 } else { 4 } {
                    table.len() as u8
                } else {
                    0
                });
            }
            f.extend_from_slice(table);
        }
    }

    // The scans.
    let selectors = |c: usize| if c == 0 { 0x00 } else { 0x11 };
    if !progressive {
        if ncomp > 1 && r.next() % 3 == 0 {
            for c in 0..ncomp as usize {
                p16be(f, 0xffda);
                p16be(f, 8);
                f.extend_from_slice(&[1, id[c] as u8, selectors(c), 0, 63, 0]);
                entropy(r, f, blocks, mcus, restart);
                if c == 0 && r.next() % 16 == 0 {
                    let lines = if r.next() % 8 == 0 { h + 1 } else { h };
                    p16be(f, 0xffdc);
                    p16be(f, 4);
                    p16be(f, lines);
                }
            }
        } else {
            p16be(f, 0xffda);
            p16be(f, 6 + 2 * ncomp);
            f.push(ncomp as u8);
            for c in 0..ncomp as usize {
                f.extend_from_slice(&[id[c] as u8, selectors(c)]);
            }
            f.extend_from_slice(&[0, 63, 0]);
            entropy(r, f, blocks, mcus, restart);
        }
    } else {
        let scans = 1 + r.next() % 6;
        for scan in 0..scans {
            let what = if scan == 0 { 0 } else { r.next() % 4 };
            let (mut ah, mut ss, mut se, mut first, mut n) = (0, 0, 0, 0, ncomp);
            if r.next() % 3 == 0 {
                ah = 1 + r.next() % 2;
            }
            let al = if ah != 0 { ah - 1 } else { r.next() % 2 };
            if what == 0 {
                // The first values of blocks: of all the components together, or of one.
                if r.next() % 4 == 0 {
                    first = r.next() % ncomp;
                    n = 1;
                }
            } else {
                // A stretch of the rest, of one component.
                first = r.next() % ncomp;
                n = 1;
                ss = 1 + r.next() % 10;
                se = ss + r.next() % (64 - ss);
            }
            p16be(f, 0xffda);
            p16be(f, 6 + 2 * n);
            f.push(n as u8);
            for c in first as usize..(first + n) as usize {
                f.extend_from_slice(&[id[c] as u8, selectors(c)]);
            }
            f.extend_from_slice(&[ss as u8, se as u8, ((ah << 4) | al) as u8]);
            entropy(r, f, blocks, mcus, restart);
        }
    }

    // The end: now and then bytes that mean nothing before it, or no end.
    if r.next() % 10 == 0 {
        let n = r.next() % 6;
        for _ in 0..n {
            f.push(((r.next() >> 10) & 0x7f) as u8);
        }
    }
    if r.next() % 30 != 0 {
        p16be(f, 0xffd9);
    }
}

/// The channels to ask for, as the C side numbers them: 0 for the file's.
fn channels(count: i32) -> Option<stb_image_nrp::Channels> {
    stb_image_nrp::Channels::from_count(count as u8)
}

/// Adds what a read came to: the size, the channels and the values, or the reason there are
/// none. True when there were values.
fn add_result<T: Copy, const N: usize>(
    hash: &mut Hash,
    result: &Result<stb_image_nrp::Image<T>, stb_image_nrp::LoadError>,
    bytes: fn(T) -> [u8; N],
) -> bool {
    match result {
        Ok(loaded) => {
            hash.add(&loaded.width.to_le_bytes());
            hash.add(&loaded.height.to_le_bytes());
            hash.add(&(loaded.channels_in_file as u32).to_le_bytes());
            for &value in &loaded.data {
                hash.add(&bytes(value));
            }
            true
        }
        Err(error) => {
            hash.add(&0xffffffffu32.to_le_bytes());
            hash.add(error.reason().as_bytes());
            hash.add(&[0]);
            false
        }
    }
}

/// The `stb_image` dump: synthetic `.png`, `.bmp` and `.jpg` files, written by the port of
/// stb_image_write and put together here with every kind of header and synthetic bytes for the
/// picture, a third of them then damaged; each asked its size and read at 8 bits a channel,
/// every third at 16; and for every fourth, synthetic data through the zlib decoder.
pub fn stb_image(seed: u64, count: u32) -> String {
    use stb_image_nrp::zlib::{Format, decompress, decompress_with_limit};
    use stb_image_nrp::{LoadOptions, info, is_16_bit, load, load_16};
    use stb_image_write_nrp::{
        Channels, Image, PngFilter, WriteOptions, write_bmp, write_jpg, write_png, zlib_compress,
    };
    let mut r = Random { state: seed };
    let mut hash = Hash::new();
    let (mut loaded, mut refused, mut too_large) = (0u32, 0u32, 0u32);
    let mut kinds = [0u32; 4];
    let (mut pixel_total, mut file_total) = (0u64, 0u64);
    let mut out = String::new();
    for i in 0..count {
        let mut file = Vec::new();
        let kind = r.next() % 8;
        if kind < 3 {
            let p = make_picture(&mut r);
            let options = WriteOptions {
                png_compression_level: 5 + r.next() % 8,
                // (The C side's -1 to 4: the best filter of each row, or a filter.)
                png_filter: match r.next() % 6 {
                    0 => None,
                    k => Some(
                        [
                            PngFilter::None,
                            PngFilter::Sub,
                            PngFilter::Up,
                            PngFilter::Average,
                            PngFilter::Paeth,
                        ][k as usize - 1],
                    ),
                },
                flip_vertically: false,
            };
            let image = Image::with_stride(
                &p.bytes,
                p.w as u32,
                p.h as u32,
                Channels::from_count(p.comp as u8).unwrap(),
                p.stride,
            )
            .unwrap();
            file = match kind {
                0 => write_png(&image, &options),
                1 => write_bmp(&image, &options),
                _ => write_jpg(&image, 1 + (r.next() % 100) as u8, &options),
            };
        } else if kind < 5 {
            make_png(&mut r, &mut file);
        } else if kind == 5 {
            make_bmp(&mut r, &mut file);
        } else {
            make_jpeg(&mut r, &mut file);
        }

        // A third of them damaged: bits turned over, the end cut off, or a byte made up.
        if r.next() % 3 == 0 {
            match r.next() % 4 {
                0 => {
                    for _ in 0..1 + r.next() % 3 {
                        let bit = r.next() % (file.len() as u32 * 8);
                        file[(bit >> 3) as usize] ^= 1 << (bit & 7);
                    }
                }
                1 => file.truncate(r.next() as usize % file.len()),
                2 => {
                    let at = r.next() % file.len() as u32;
                    file[at as usize] = r.next() as u8;
                }
                _ => {
                    let bit = r.next() % (file.len().min(64) as u32 * 8);
                    file[(bit >> 3) as usize] ^= 1 << (bit & 7);
                }
            }
        }
        hash.add(&(file.len() as u32).to_le_bytes());
        hash.add(&file);
        file_total += file.len() as u64;

        let info = info(&file);
        let mut large = false;
        match &info {
            Ok(info) => {
                let number = info.format.code() as u32;
                for value in [
                    number,
                    info.width,
                    info.height,
                    info.channels as u32,
                    u32::from(info.is_16_bit),
                    u32::from(info.is_iphone_png),
                ] {
                    hash.add(&value.to_le_bytes());
                }
                kinds[match number {
                    1 => 1,
                    2 => 2,
                    4 => 3,
                    _ => 0,
                }] += 1;
                large = u64::from(info.width) * u64::from(info.height) > 1 << 20;
            }
            Err(_) => {
                hash.add(&0u32.to_le_bytes());
                kinds[0] += 1;
            }
        }
        hash.add(&u32::from(is_16_bit(&file)).to_le_bytes());

        let req = (r.next() % 5) as i32;
        let options = LoadOptions {
            flip_vertically: r.next() % 4 == 0,
            unpremultiply: r.next() % 2 != 0,
            convert_iphone_png_to_rgb: r.next() % 2 != 0,
            ..LoadOptions::default()
        };
        let req16 = (r.next() % 5) as i32;
        // (A damaged size can ask for more memory and time than there is; those are left.)
        if large {
            too_large += 1;
        } else {
            let result = load(&file, channels(req), &options);
            if add_result(&mut hash, &result, |value: u8| [value]) {
                loaded += 1;
                let image = result.unwrap();
                pixel_total += u64::from(image.width) * u64::from(image.height);
            } else {
                refused += 1;
            }
            if i % 3 == 0 {
                add_result(
                    &mut hash,
                    &load_16(&file, channels(req16), &options),
                    u16::to_le_bytes,
                );
            }
        }

        // The zlib decoder by itself.
        if i % 4 == 0 {
            let data = make_data(&mut r);
            let mut z = if r.next() % 2 != 0 {
                zlib_compress(&data, 5 + r.next() % 8)
            } else {
                miniz_nrp::compress(&data, miniz_nrp::Level::new((r.next() % 10) as u8).unwrap())
            };
            if r.next() % 3 == 0 {
                let bit = r.next() % (z.len() as u32 * 8);
                z[(bit >> 3) as usize] ^= 1 << (bit & 7);
            }
            match decompress(&z, Format::Zlib) {
                Ok(bytes) => {
                    hash.add(&(bytes.len() as u32).to_le_bytes());
                    hash.add(&bytes);
                }
                Err(error) => {
                    hash.add(error.reason().as_bytes());
                    hash.add(&[0]);
                }
            }
            // Into just enough room, or one byte too little.
            let room = data.len() - usize::from(!data.is_empty() && r.next() % 2 != 0);
            match decompress_with_limit(&z, Format::Zlib, room) {
                Ok(bytes) => {
                    hash.add(&(bytes.len() as u32).to_le_bytes());
                    hash.add(&bytes);
                }
                Err(error) => {
                    hash.add(&0xffffffffu32.to_le_bytes());
                    hash.add(error.reason().as_bytes());
                    hash.add(&[0]);
                }
            }
            // Without its header.
            match decompress(&z[2..], Format::Raw) {
                Ok(bytes) => {
                    hash.add(&(bytes.len() as u32).to_le_bytes());
                    hash.add(&bytes);
                }
                Err(error) => {
                    hash.add(error.reason().as_bytes());
                    hash.add(&[0]);
                }
            }
        }

        if (i + 1) % 200 == 0 || i + 1 == count {
            out += &format!("stb_image {} {:016x}\n", i + 1, hash.value);
        }
    }
    out + &format!(
        "stb_image bytes of files: {file_total}; read: {loaded} ({pixel_total} pixels), refused: {refused}, left as too large: {too_large}; found to be .jpg: {}, .png: {}, .bmp: {}, nothing: {}\n",
        kinds[1], kinds[2], kinds[3], kinds[0]
    )
}

/// Damages a third of the files: bits turned over, the end cut off, or a byte made up.
fn damage_stb_file(r: &mut Random, file: &mut Vec<u8>) {
    if r.next() % 3 == 0 {
        match r.next() % 4 {
            0 => {
                for _ in 0..1 + r.next() % 3 {
                    let bit = r.next() % (file.len() as u32 * 8);
                    file[(bit >> 3) as usize] ^= 1 << (bit & 7);
                }
            }
            1 => file.truncate(r.next() as usize % file.len()),
            2 => {
                let at = r.next() % file.len() as u32;
                file[at as usize] = r.next() as u8;
            }
            _ => {
                let bit = r.next() % (file.len().min(64) as u32 * 8);
                file[(bit >> 3) as usize] ^= 1 << (bit & 7);
            }
        }
    }
}

/// A pixel of a TGA file: an index into its palette (now and then one past its end), or bytes.
fn tga_pixel(r: &mut Random, f: &mut Vec<u8>, indexed: bool, pal_len: u32, bytes: u32) {
    if indexed {
        let index = if r.next() % 16 == 0 {
            r.next() % 400
        } else {
            r.next() % pal_len
        };
        f.push(index as u8);
        if bytes == 2 {
            f.push((index >> 8) as u8);
        }
    } else {
        for _ in 0..bytes {
            f.push((r.next() >> 9) as u8);
        }
    }
}

/// A synthetic TGA file: palette or not, grey or colour, of every depth, raw or run-length
/// encoded, either way up and round.
pub fn make_tga(r: &mut Random, f: &mut Vec<u8>) {
    const PALETTE_BITS: [u32; 5] = [8, 15, 16, 24, 32];
    const BITS: [u32; 4] = [15, 16, 24, 32];
    let indexed = r.next() % 3 == 0;
    let rle = r.next() % 2 != 0;
    let w = 1 + r.next() % 40;
    let h = 1 + r.next() % 40;
    let mut desc = if r.next() % 2 != 0 { 0x20u8 } else { 0 };
    if r.next() % 4 == 0 {
        desc |= 0x10;
    }
    let id_length = r.next() % 3;
    f.push(id_length as u8);
    let mut pal_len = 0;
    let bytes_per_pixel = if indexed {
        let pal_bits = PALETTE_BITS[(r.next() % 5) as usize];
        let index_bits = if r.next() % 4 == 0 { 16 } else { 8 };
        let pal_start = if r.next() % 4 == 0 { r.next() % 3 } else { 0 };
        let entry = if pal_bits == 15 { 2 } else { pal_bits / 8 };
        pal_len = 1 + r.next() % (if index_bits == 8 { 256 } else { 300 });
        f.push(1);
        f.push(if rle { 9 } else { 1 });
        p16le(f, pal_start);
        p16le(f, pal_len);
        f.push(pal_bits as u8);
        p16le(f, 0);
        p16le(f, 0);
        p16le(f, w);
        p16le(f, h);
        f.push(index_bits as u8);
        f.push(desc);
        for _ in 0..id_length + pal_start + pal_len * entry {
            f.push((r.next() >> 7) as u8);
        }
        index_bits / 8
    } else {
        let grey = r.next() % 3 == 0;
        let bits = if grey {
            if r.next() % 2 != 0 { 16 } else { 8 }
        } else {
            BITS[(r.next() % 4) as usize]
        };
        f.push(0);
        f.push((if grey { 3 } else { 2 }) + (if rle { 8 } else { 0 }));
        p16le(f, 0);
        p16le(f, 0);
        f.push(0);
        p16le(f, 0);
        p16le(f, 0);
        p16le(f, w);
        p16le(f, h);
        f.push(bits as u8);
        f.push(desc);
        for _ in 0..id_length {
            f.push((r.next() >> 7) as u8);
        }
        (bits + 7) / 8
    };
    let mut done = 0;
    while done < w * h {
        if !rle {
            tga_pixel(r, f, indexed, pal_len, bytes_per_pixel);
            done += 1;
        } else {
            let count = 1 + r.next() % 128;
            let repeat = r.next() % 2;
            f.push(((repeat << 7) | (count - 1)) as u8);
            if repeat != 0 {
                tga_pixel(r, f, indexed, pal_len, bytes_per_pixel);
            } else {
                for _ in 0..count {
                    tga_pixel(r, f, indexed, pal_len, bytes_per_pixel);
                }
            }
            done += count;
        }
    }
}

/// A synthetic PSD file: none to five channels, 8 or 16 bits, raw or run-length encoded, with now
/// and then mode data, resources and reserved bytes to skip.
pub fn make_psd(r: &mut Random, f: &mut Vec<u8>) {
    let channels = r.next() % 6;
    let h = 1 + r.next() % 30;
    let w = 1 + r.next() % 30;
    let depth = if r.next() % 2 != 0 { 16 } else { 8 };
    let compression = r.next() % 2;
    let pixels = w * h;
    f.extend_from_slice(b"8BPS");
    p16be(f, if r.next() % 20 == 0 { 2 } else { 1 });
    f.extend_from_slice(&[0; 6]);
    p16be(f, channels);
    p32be(f, h);
    p32be(f, w);
    p16be(f, depth);
    p16be(f, if r.next() % 20 == 0 { 1 } else { 3 });
    for _ in 0..3 {
        let n = if r.next() % 3 == 0 { r.next() % 20 } else { 0 };
        p32be(f, n);
        for _ in 0..n {
            f.push((r.next() >> 5) as u8);
        }
    }
    p16be(f, if r.next() % 20 == 0 { 2 } else { compression });
    if compression == 0 {
        for c in 0..channels {
            for _ in 0..pixels {
                let v = if c == 3 && r.next() % 2 != 0 {
                    if r.next() % 2 != 0 { 0xffffffff } else { 0 }
                } else {
                    r.next()
                };
                f.push((v >> 8) as u8);
                if depth == 16 {
                    f.push(v as u8);
                }
            }
        }
    } else {
        for _ in 0..h * channels {
            p16be(f, r.next() % 200);
        }
        for _ in 0..channels {
            let mut done = 0;
            while done < pixels {
                let left = pixels - done;
                let how = r.next() % 8;
                if how == 0 {
                    f.push(128);
                } else if how < 4 && left >= 2 {
                    let n = 2 + r.next() % (left.min(128) - 1);
                    f.push((257 - n) as u8);
                    f.push((r.next() >> 3) as u8);
                    done += n;
                } else {
                    let n = 1 + r.next() % left.min(128);
                    f.push((n - 1) as u8);
                    for _ in 0..n {
                        f.push((r.next() >> 3) as u8);
                    }
                    done += n;
                }
            }
        }
    }
}

/// LZW as a GIF has it, written here: the codes in blocks, and the code size as the reader
/// works it out.
struct Lzw<'a> {
    f: &'a mut Vec<u8>,
    bits: u32,
    nbits: u32,
    block_max: u32,
    block: Vec<u8>,
    mincs: u32,
    cs: u32,
    avail: u32,
    hasold: bool,
}

impl Lzw<'_> {
    fn byte(&mut self, b: u8) {
        self.block.push(b);
        if self.block.len() as u32 == self.block_max {
            self.f.push(self.block.len() as u8);
            self.f.extend_from_slice(&self.block);
            self.block.clear();
        }
    }

    fn code(&mut self, code: u32) {
        let clear = 1u32 << self.mincs;
        self.bits |= code << self.nbits;
        self.nbits += self.cs;
        while self.nbits >= 8 {
            self.byte(self.bits as u8);
            self.bits >>= 8;
            self.nbits -= 8;
        }
        if code == clear {
            self.cs = self.mincs + 1;
            self.avail = clear + 2;
            self.hasold = false;
        } else {
            if self.hasold && self.avail < 4096 {
                self.avail += 1;
                if self.avail > (1 << self.cs) - 1 && self.avail < 4096 {
                    self.cs += 1;
                }
            }
            self.hasold = true;
        }
    }
}

fn lzw_encode(f: &mut Vec<u8>, px: &[u8], mincs: u32, block_max: u32) {
    let clear = 1u32 << mincs;
    f.push(mincs as u8);
    let mut z = Lzw {
        f,
        bits: 0,
        nbits: 0,
        block_max,
        block: Vec::new(),
        mincs,
        cs: mincs + 1,
        avail: clear + 2,
        hasold: false,
    };
    let mut dict = std::collections::HashMap::new();
    z.code(clear);
    let mut next = clear + 2;
    if let Some((&first, rest)) = px.split_first() {
        let mut prefix = u32::from(first);
        for &k in rest {
            let k = u32::from(k);
            if let Some(&e) = dict.get(&(prefix, k)) {
                prefix = e;
                continue;
            }
            z.code(prefix);
            if next < 4096 {
                dict.insert((prefix, k), next);
                next += 1;
            } else {
                z.code(clear);
                dict.clear();
                next = clear + 2;
            }
            prefix = k;
        }
        z.code(prefix);
    }
    z.code(clear + 1);
    if z.nbits > 0 {
        let b = z.bits as u8;
        z.byte(b);
    }
    if !z.block.is_empty() {
        z.f.push(z.block.len() as u8);
        let block = std::mem::take(&mut z.block);
        z.f.extend_from_slice(&block);
    }
    z.f.push(0);
}

/// A synthetic GIF file: one to three frames, each with or without a graphic control extension
/// (a delay, a transparent colour, a way of disposing of it) and a comment, at a synthetic place,
/// with a local colour table or the global one, interlaced or not.
pub fn make_gif(r: &mut Random, f: &mut Vec<u8>) {
    let big_w = 1 + r.next() % 40;
    let big_h = 1 + r.next() % 40;
    let gct = r.next() % 5 != 0;
    let gsize = r.next() % 8;
    let frames = 1 + r.next() % 3;
    f.extend_from_slice(if r.next() % 2 != 0 {
        b"GIF89a"
    } else {
        b"GIF87a"
    });
    p16le(f, big_w);
    p16le(f, big_h);
    let mut flags = (r.next() & 0x70) | gsize;
    if gct {
        flags |= 0x80;
    }
    f.push(flags as u8);
    f.push((r.next() % 256) as u8);
    f.push(0);
    if gct {
        for _ in 0..(2u32 << gsize) * 3 {
            f.push((r.next() >> 9) as u8);
        }
    }
    for _ in 0..frames {
        if r.next() % 2 != 0 {
            let mut eflags = (r.next() % 4) << 2;
            eflags |= r.next() % 2;
            f.extend_from_slice(&[0x21, 0xF9, 4, eflags as u8]);
            p16le(f, r.next() % 100);
            f.push((r.next() % 256) as u8);
            f.push(0);
        }
        if r.next() % 5 == 0 {
            let blocks = r.next() % 3;
            f.extend_from_slice(&[0x21, 0xFE]);
            for _ in 0..blocks {
                let len = 1 + r.next() % 20;
                f.push(len as u8);
                for _ in 0..len {
                    f.push((u32::from(b'a') + r.next() % 26) as u8);
                }
            }
            f.push(0);
        }
        let x = r.next() % big_w;
        let y = r.next() % big_h;
        let w = if r.next() % 20 == 0 {
            0
        } else {
            1 + r.next() % (big_w - x)
        };
        let h = 1 + r.next() % (big_h - y);
        let lct = if gct {
            r.next() % 3 == 0
        } else {
            r.next() % 10 != 0
        };
        let lsize = r.next() % 8;
        let mut lflags = lsize;
        if lct {
            lflags |= 0x80;
        }
        if r.next() % 3 == 0 {
            lflags |= 0x40;
        }
        f.push(0x2C);
        p16le(f, x);
        p16le(f, y);
        p16le(f, w);
        p16le(f, h);
        f.push(lflags as u8);
        if lct {
            for _ in 0..(2u32 << lsize) * 3 {
                f.push((r.next() >> 9) as u8);
            }
        }
        let mut mincs = (if lct { lsize } else { gsize }) + 1;
        if mincs < 2 {
            mincs = 2;
        }
        if r.next() % 30 == 0 {
            mincs = r.next() % 14;
        }
        if mincs > 12 {
            f.push(mincs as u8);
            f.push(0);
            continue;
        }
        let colours = (1u32 << mincs).min(256);
        let n = (w * h) as usize;
        let style = r.next() % 3;
        let mut px = vec![0u8; n];
        for i in 0..n {
            px[i] = if style == 0 {
                (r.next() % colours) as u8
            } else if style == 1 {
                ((i as u32 / 7) % colours) as u8
            } else if r.next() % 4 == 0 || i == 0 {
                (r.next() % colours) as u8
            } else {
                px[i - 1]
            };
        }
        let block_max = if r.next() % 4 == 0 {
            1 + r.next() % 254
        } else {
            255
        };
        lzw_encode(f, &px, mincs, block_max);
    }
    if r.next() % 10 != 0 {
        f.push(0x3B);
    }
}

/// Whitespace between the numbers of a PNM file, now and then with a comment.
fn pnm_space(r: &mut Random, f: &mut Vec<u8>) {
    const SPACE: [u8; 6] = [b' ', b'\t', b'\n', b'\r', 0x0b, 0x0c];
    if r.next() % 6 == 0 {
        f.extend_from_slice(b"\n# made up\n");
    }
    let n = 1 + r.next() % 3;
    for _ in 0..n {
        f.push(SPACE[(r.next() % 6) as usize]);
    }
}

/// A synthetic PNM file: grey or colour, binary or text, of every kind of maximum value.
pub fn make_pnm(r: &mut Random, f: &mut Vec<u8>) {
    const KINDS: [u8; 4] = *b"2356";
    let t = KINDS[(r.next() % 4) as usize];
    let ascii = t == b'2' || t == b'3';
    let comp = if t == b'3' || t == b'6' { 3 } else { 1 };
    let w = 1 + r.next() % 40;
    let h = 1 + r.next() % 40;
    let maxv = match r.next() % 8 {
        0 => 255,
        1 => 1 + r.next() % 255,
        2 => 65535,
        3 => 256 + r.next() % 65280,
        4 => 1 + r.next() % 15,
        5 => {
            if r.next() % 10 == 0 {
                0
            } else {
                255
            }
        }
        6 => {
            if r.next() % 10 == 0 {
                70000
            } else {
                1000
            }
        }
        _ => 255,
    };
    f.push(b'P');
    f.push(t);
    pnm_space(r, f);
    f.extend_from_slice(w.to_string().as_bytes());
    pnm_space(r, f);
    f.extend_from_slice(h.to_string().as_bytes());
    pnm_space(r, f);
    f.extend_from_slice(maxv.to_string().as_bytes());
    let n = w * h * comp;
    if ascii {
        for _ in 0..n {
            pnm_space(r, f);
            let v = if r.next() % 20 == 0 {
                r.next() % (maxv + 100)
            } else {
                r.next() % (maxv + 1)
            };
            f.extend_from_slice(v.to_string().as_bytes());
        }
        f.push(b'\n');
    } else {
        f.push(if r.next() % 2 != 0 { b'\n' } else { b' ' });
        for _ in 0..n {
            let v = r.next();
            if maxv > 255 {
                f.push((v >> 8) as u8);
            }
            f.push(v as u8);
        }
    }
}

/// A byte of an RGBE pixel: a colour, or an exponent near 1 (and now and then 0).
fn hdr_byte(r: &mut Random, k: u32) -> u8 {
    if k < 3 {
        return (r.next() >> 24) as u8;
    }
    if r.next() % 8 == 0 {
        0
    } else {
        (120 + r.next() % 20) as u8
    }
}

/// A synthetic Radiance HDR file: a header with lines to skip, then scanlines flat or run-length
/// encoded.
pub fn make_hdr(r: &mut Random, f: &mut Vec<u8>) {
    let w = if r.next() % 2 != 0 {
        1 + r.next() % 40
    } else {
        1 + r.next() % 300
    };
    let h = 1 + r.next() % 20;
    let rle = r.next() % 4 != 0;
    f.extend_from_slice(if r.next() % 4 == 0 {
        b"#?RGBE\n"
    } else {
        b"#?RADIANCE\n"
    });
    let lines = r.next() % 3;
    for _ in 0..lines {
        f.extend_from_slice(if r.next() % 2 != 0 {
            b"# made up\n"
        } else {
            b"EXPOSURE=1.0\n"
        });
    }
    if r.next() % 30 != 0 {
        f.extend_from_slice(b"FORMAT=32-bit_rle_rgbe\n");
    }
    if r.next() % 4 == 0 {
        f.extend_from_slice(b"GAMMA=1.0\n");
    }
    f.extend_from_slice(b"\n");
    f.extend_from_slice(if r.next() % 20 == 0 { b"+Y " } else { b"-Y " });
    f.extend_from_slice(h.to_string().as_bytes());
    f.extend_from_slice(if r.next() % 5 == 0 { b"  +X " } else { b" +X " });
    f.extend_from_slice(w.to_string().as_bytes());
    f.extend_from_slice(b"\n");
    for _ in 0..h {
        if rle && (8..32768).contains(&w) && r.next() % 20 != 0 {
            f.extend_from_slice(&[2, 2, (w >> 8) as u8, (w & 255) as u8]);
            for k in 0..4 {
                let mut x = 0;
                while x < w {
                    let left = w - x;
                    if r.next() % 2 != 0 {
                        let n = 1 + r.next() % left.min(127);
                        f.push((128 + n) as u8);
                        let value = hdr_byte(r, k);
                        f.push(value);
                        x += n;
                    } else {
                        let n = 1 + r.next() % left.min(128);
                        f.push(n as u8);
                        for _ in 0..n {
                            let value = hdr_byte(r, k);
                            f.push(value);
                        }
                        x += n;
                    }
                }
            }
        } else {
            for _ in 0..w {
                for k in 0..4 {
                    let value = hdr_byte(r, k);
                    f.push(value);
                }
            }
        }
    }
}

/// The `stb_image_more` dump: synthetic `.tga`, `.psd`, `.gif`, `.pnm` and `.hdr` files, a third
/// of them damaged; each asked its size and whether it is HDR, read at 8 bits a channel, as
/// floats and every other at 16, with a synthetic gamma and scale; GIFs and every fourth file
/// also read as an animation.
pub fn stb_image_more(seed: u64, count: u32) -> String {
    use stb_image_nrp::{LoadOptions, info, is_16_bit, is_hdr, load, load_16, load_f32, load_gif};
    const GAMMA: [f32; 4] = [2.2, 1.0, 1.8, 2.4];
    const SCALE: [f32; 4] = [1.0, 0.5, 2.0, 4.0];
    let mut r = Random { state: seed };
    let mut hash = Hash::new();
    let (mut loaded, mut refused, mut too_large) = (0u32, 0u32, 0u32);
    let mut kinds = [0u32; 9];
    let mut file_total = 0u64;
    let mut out = String::new();
    for i in 0..count {
        let mut file = Vec::new();
        match r.next() % 5 {
            0 => make_tga(&mut r, &mut file),
            1 => make_psd(&mut r, &mut file),
            2 => make_gif(&mut r, &mut file),
            3 => make_pnm(&mut r, &mut file),
            _ => make_hdr(&mut r, &mut file),
        }
        damage_stb_file(&mut r, &mut file);
        hash.add(&(file.len() as u32).to_le_bytes());
        hash.add(&file);
        file_total += file.len() as u64;

        let info = info(&file);
        let mut large = false;
        let mut number = 0;
        match &info {
            Ok(info) => {
                number = info.format.code() as u32;
                for value in [
                    number,
                    info.width,
                    info.height,
                    info.channels as u32,
                    u32::from(info.is_16_bit),
                    u32::from(info.is_hdr),
                    u32::from(info.is_iphone_png),
                ] {
                    hash.add(&value.to_le_bytes());
                }
                large = u64::from(info.width) * u64::from(info.height) > 1 << 20;
            }
            Err(_) => hash.add(&0u32.to_le_bytes()),
        }
        hash.add(&u32::from(is_16_bit(&file)).to_le_bytes());
        hash.add(&u32::from(is_hdr(&file)).to_le_bytes());
        kinds[number as usize] += 1;

        let req = (r.next() % 5) as i32;
        let flip = r.next() % 4 == 0;
        let req16 = (r.next() % 5) as i32;
        let reqf = (r.next() % 5) as i32;
        let reqg = (r.next() % 5) as i32;
        let gamma = GAMMA[(r.next() % 4) as usize];
        let scale = SCALE[(r.next() % 4) as usize];
        // (A damaged size can ask for more memory and time than there is; those are left.)
        if large {
            too_large += 1;
        } else {
            let options = LoadOptions {
                flip_vertically: flip,
                hdr_to_ldr_gamma: gamma,
                hdr_to_ldr_scale: scale,
                ldr_to_hdr_gamma: gamma,
                ldr_to_hdr_scale: scale,
                ..LoadOptions::default()
            };
            if add_result(
                &mut hash,
                &load(&file, channels(req), &options),
                |value: u8| [value],
            ) {
                loaded += 1;
            } else {
                refused += 1;
            }
            if i % 2 == 0 {
                add_result(
                    &mut hash,
                    &load_16(&file, channels(req16), &options),
                    u16::to_le_bytes,
                );
            }
            add_result(
                &mut hash,
                &load_f32(&file, channels(reqf), &options),
                f32::to_le_bytes,
            );
            if number == 3 || i % 4 == 0 {
                match load_gif(&file, channels(reqg), &options) {
                    Ok(gif) => {
                        // (As the original gives them: the size of the last frame, and 4
                        // channels when the header gives the size, which stand when there
                        // is none.)
                        let (x, y) = if gif.frames.is_empty() {
                            (0, 0)
                        } else {
                            (gif.width, gif.height)
                        };
                        let in_file = if gif.width != 0 { 4 } else { 0 };
                        for value in [x, y, gif.frames.len() as u32, in_file] {
                            hash.add(&value.to_le_bytes());
                        }
                        for frame in &gif.frames {
                            hash.add(&frame.data);
                        }
                        for frame in gif.frames {
                            hash.add(&frame.delay_ms.to_le_bytes());
                        }
                    }
                    Err(error) => {
                        hash.add(&0xffffffffu32.to_le_bytes());
                        hash.add(error.reason().as_bytes());
                        hash.add(&[0]);
                    }
                }
            }
        }

        if (i + 1) % 200 == 0 || i + 1 == count {
            out += &format!("stb_image_more {} {:016x}\n", i + 1, hash.value);
        }
    }
    out + &format!(
        "stb_image_more bytes of files: {file_total}; read: {loaded}, refused: {refused}, left as too large: {too_large}; found to be .jpg: {}, .png: {}, .gif: {}, .bmp: {}, .psd: {}, .pnm: {}, .hdr: {}, .tga: {}, nothing: {}\n",
        kinds[1], kinds[2], kinds[3], kinds[4], kinds[5], kinds[6], kinds[7], kinds[8], kinds[0]
    )
}
