//! The zlib decoder of `stb_image.h`, which its PNG reader uses: "public domain zlib decode
//! v0.2, Sean Barrett 2006-11-18".
//!
//! - all input must be provided in an upfront buffer
//! - all output is written to a single output buffer
//!
//! It checks no Adler-32 checksum. (It is here so that a PNG is read as the original reads it,
//! the damaged ones too.)

use alloc::vec::Vec;

use super::{Failed, LoadError};

// fast-way is faster to check than jpeg huffman, but slow way is slower
const STBI_ZFAST_BITS: usize = 9; // accelerate all cases in default tables
const STBI_ZFAST_MASK: u32 = (1 << STBI_ZFAST_BITS) - 1;
const STBI_ZNSYMS: usize = 288; // number of symbols in literal/length alphabet

/// `stbi__zhuffman`: zlib-style huffman encoding (jpegs packs from left, zlib from right, so
/// can't share code)
#[derive(Clone, Copy)]
struct ZHuffman {
    fast: [u16; 1 << STBI_ZFAST_BITS],
    firstcode: [u16; 16],
    maxcode: [i32; 17],
    firstsymbol: [u16; 16],
    size: [u8; STBI_ZNSYMS],
    value: [u16; STBI_ZNSYMS],
}

impl ZHuffman {
    const fn new() -> ZHuffman {
        ZHuffman {
            fast: [0; 1 << STBI_ZFAST_BITS],
            firstcode: [0; 16],
            maxcode: [0; 17],
            firstsymbol: [0; 16],
            size: [0; STBI_ZNSYMS],
            value: [0; STBI_ZNSYMS],
        }
    }
}

/// `stbi__bitreverse16`
const fn bitreverse16(mut n: i32) -> i32 {
    n = ((n & 0xAAAA) >> 1) | ((n & 0x5555) << 1);
    n = ((n & 0xCCCC) >> 2) | ((n & 0x3333) << 2);
    n = ((n & 0xF0F0) >> 4) | ((n & 0x0F0F) << 4);
    n = ((n & 0xFF00) >> 8) | ((n & 0x00FF) << 8);
    n
}

/// `stbi__bit_reverse`
const fn bit_reverse(v: i32, bits: i32) -> i32 {
    debug_assert!(bits <= 16);
    // to bit reverse n bits, reverse 16 and shift
    // e.g. 11 bits, bit reverse and shift away 5
    bitreverse16(v) >> (16 - bits)
}

/// Compile-time builder for the RFC 1951 fixed literal/length and distance Huffman tables.
const fn build_fixed_huffman<const N: usize>(sizelist: &[u8; N]) -> ZHuffman {
    let mut table = ZHuffman::new();
    let mut symbol_offset = 0i32;
    let mut next_code = [0i32; 16];
    let mut sizes = [0i32; 17];

    let mut idx = 0;
    while idx < N {
        sizes[sizelist[idx] as usize] += 1;
        idx += 1;
    }
    sizes[0] = 0;

    let mut code = 0i32;
    let mut bits = 1;
    while bits < 16 {
        next_code[bits] = code;
        table.firstcode[bits] = code as u16;
        table.firstsymbol[bits] = symbol_offset as u16;
        code += sizes[bits];
        table.maxcode[bits] = code << (16 - bits);
        code <<= 1;
        symbol_offset += sizes[bits];
        bits += 1;
    }
    table.maxcode[16] = 0x10000;

    let mut sym = 0;
    while sym < N {
        let code_len = sizelist[sym] as usize;
        if code_len != 0 {
            let slot = (next_code[code_len] - table.firstcode[code_len] as i32
                + table.firstsymbol[code_len] as i32) as usize;
            let fastv = ((code_len << 9) | sym) as u16;
            table.size[slot] = code_len as u8;
            table.value[slot] = sym as u16;
            if code_len <= STBI_ZFAST_BITS {
                let mut fast_idx = bit_reverse(next_code[code_len], code_len as i32) as usize;
                while fast_idx < (1 << STBI_ZFAST_BITS) {
                    table.fast[fast_idx] = fastv;
                    fast_idx += 1 << code_len;
                }
            }
            next_code[code_len] += 1;
        }
        sym += 1;
    }
    table
}

/// `stbi__zbuf`: zlib-from-memory implementation for PNG reading. Because PNG allows
/// splitting the zlib stream arbitrarily, and it's annoying structurally to have PNG call
/// ZLIB call PNG, we require PNG read all the IDATs and combine them into a single memory
/// buffer.
struct Zbuf<'a> {
    zbuffer: &'a [u8],
    /// (How far into `zbuffer` the decoder is.)
    at: usize,
    num_bits: i32,
    hit_zeof_once: bool,
    code_buffer: u32,

    /// (The output so far, and how much room the original would have for it.)
    zout: Vec<u8>,
    limit: u32,
    z_expandable: bool,

    z_length: ZHuffman,
    z_distance: ZHuffman,

    /// (The failure reason, a global in the original.)
    reason: Option<&'static str>,
}

const MAX_ZOUT_CAP: usize = if (u32::MAX as usize) < (isize::MAX as usize) {
    u32::MAX as usize
} else {
    isize::MAX as usize
};

impl Zbuf<'_> {
    /// `stbi__err`, for a step that fails: records `reason` as the failure's.
    fn fail<T>(&mut self, reason: &'static str) -> Result<T, Failed> {
        self.reason = Some(reason);
        Err(Failed)
    }

    /// `stbi__zeof`
    fn zeof(&self) -> bool {
        self.at >= self.zbuffer.len()
    }

    /// `stbi__zget8`
    fn zget8(&mut self) -> u8 {
        if self.zeof() {
            0
        } else {
            self.at += 1;
            self.zbuffer[self.at - 1]
        }
    }

    /// `stbi__fill_bits`
    fn fill_bits(&mut self) {
        loop {
            if self.code_buffer >= 1u32.wrapping_shl(self.num_bits as u32) {
                self.at = self.zbuffer.len(); /* treat this as EOF so we fail. */
                return;
            }
            self.code_buffer |= u32::from(self.zget8()).wrapping_shl(self.num_bits as u32);
            self.num_bits += 8;
            if self.num_bits > 24 {
                break;
            }
        }
    }

    /// `stbi__zreceive`
    fn zreceive(&mut self, n: i32) -> u32 {
        if self.num_bits < n {
            self.fill_bits();
        }
        let k = self.code_buffer & ((1 << n) - 1);
        self.code_buffer >>= n;
        self.num_bits -= n;
        k
    }

    /// `stbi__zexpand`: need to make room for `n` bytes, capped at `isize::MAX` (for 32-bit
    /// targets) and `u32::MAX` (as in `stb_image.h`), using fallible `try_reserve_exact`.
    fn zexpand(&mut self, n: i32) -> Result<(), Failed> {
        if !self.z_expandable {
            return self.fail("output buffer limit");
        }
        if n < 0 {
            return self.fail("outofmem");
        }
        let cur = self.zout.len();
        let add = n as usize;
        if add > MAX_ZOUT_CAP.saturating_sub(cur) {
            return self.fail("outofmem");
        }
        let needed = cur + add;
        let mut limit = (self.limit as usize).min(MAX_ZOUT_CAP);
        if limit == 0 {
            limit = 1;
        }
        while needed > limit {
            if limit > (u32::MAX as usize) / 2 {
                return self.fail("outofmem");
            }
            limit = limit.saturating_mul(2).min(MAX_ZOUT_CAP);
        }
        if self.zout.try_reserve_exact(limit - cur).is_err() {
            return self.fail("outofmem");
        }
        self.limit = limit as u32;
        Ok(())
    }

    /// Ensures `self.zout` has capacity for `additional` more bytes without panicking on OOM or
    /// 32-bit `isize::MAX` overflow.
    #[inline]
    fn ensure_capacity(&mut self, additional: usize) -> Result<(), Failed> {
        if additional > self.zout.capacity() - self.zout.len()
            && self.zout.try_reserve(additional).is_err()
        {
            return self.fail("outofmem");
        }
        Ok(())
    }

    /// `stbi__parse_huffman_block`
    fn parse_huffman_block(&mut self) -> Result<(), Failed> {
        loop {
            let mut z = zhuffman_decode(self, false);
            if z < 256 {
                if z < 0 {
                    return self.fail("bad huffman code"); // error in huffman codes
                }
                if self.zout.len() as u32 >= self.limit {
                    self.zexpand(1)?;
                }
                self.ensure_capacity(1)?;
                self.zout.push(z as u8);
            } else {
                if z == 256 {
                    if self.hit_zeof_once && self.num_bits < 16 {
                        // The first time we hit zeof, we inserted 16 extra zero bits into our bit
                        // buffer so the decoder can just do its speculative decoding. But if we
                        // actually consumed any of those bits
                        // (which is the case when num_bits < 16),
                        // the stream actually read past the end so it is malformed.
                        return self.fail("unexpected end");
                    }
                    return Ok(());
                }
                if z >= 286 {
                    // per DEFLATE, length codes 286 and 287 must not appear in compressed data
                    return self.fail("bad huffman code");
                }
                z -= 257;
                let mut len = STBI_ZLENGTH_BASE[z as usize];
                if STBI_ZLENGTH_EXTRA[z as usize] != 0 {
                    len += self.zreceive(STBI_ZLENGTH_EXTRA[z as usize]) as i32;
                }
                z = zhuffman_decode(self, true);
                if !(0..30).contains(&z) {
                    // per DEFLATE, distance codes 30 and 31 must not appear in compressed data
                    return self.fail("bad huffman code");
                }
                let mut dist = STBI_ZDIST_BASE[z as usize];
                if STBI_ZDIST_EXTRA[z as usize] != 0 {
                    dist += self.zreceive(STBI_ZDIST_EXTRA[z as usize]) as i32;
                }
                if (self.zout.len() as i64) < i64::from(dist) {
                    return self.fail("bad dist");
                }
                if i64::from(len) > i64::from(self.limit) - self.zout.len() as i64 {
                    self.zexpand(len)?;
                }
                let from = self.zout.len() - dist as usize;
                let len = len as usize;
                self.ensure_capacity(len)?;
                if dist == 1 {
                    // run of one byte; common in images.
                    let v = self.zout[from];
                    self.zout.resize(self.zout.len() + len, v);
                } else if dist as usize >= len {
                    // (A copy that does not overlap what it writes: all at once.)
                    self.zout.extend_from_within(from..from + len);
                } else {
                    for i in from..from + len {
                        let v = self.zout[i];
                        self.zout.push(v);
                    }
                }
            }
        }
    }

    /// `stbi__compute_huffman_codes`
    fn compute_huffman_codes(&mut self) -> Result<(), Failed> {
        static LENGTH_DEZIGZAG: [u8; 19] = [
            16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
        ];
        let mut z_codelength = ZHuffman::new();
        let mut lencodes = [0u8; 286 + 32 + 137]; //padding for maximum single op
        let mut codelength_sizes = [0u8; 19];

        let hlit = self.zreceive(5) as i32 + 257;
        let hdist = self.zreceive(5) as i32 + 1;
        let hclen = self.zreceive(4) as i32 + 4;
        let ntot = hlit + hdist;

        for i in 0..hclen as usize {
            let s = self.zreceive(3);
            codelength_sizes[usize::from(LENGTH_DEZIGZAG[i])] = s as u8;
        }
        zbuild_huffman(&mut z_codelength, &codelength_sizes).or_else(|reason| self.fail(reason))?;

        let mut n = 0i32;
        while n < ntot {
            let mut c = zhuffman_decode_with(self, &z_codelength);
            if !(0..19).contains(&c) {
                return self.fail("bad codelengths");
            }
            if c < 16 {
                lencodes[n as usize] = c as u8;
                n += 1;
            } else {
                let mut fill = 0u8;
                if c == 16 {
                    c = self.zreceive(2) as i32 + 3;
                    if n == 0 {
                        return self.fail("bad codelengths");
                    }
                    fill = lencodes[n as usize - 1];
                } else if c == 17 {
                    c = self.zreceive(3) as i32 + 3;
                } else if c == 18 {
                    c = self.zreceive(7) as i32 + 11;
                } else {
                    return self.fail("bad codelengths");
                }
                if ntot - n < c {
                    return self.fail("bad codelengths");
                }
                lencodes[n as usize..(n + c) as usize].fill(fill);
                n += c;
            }
        }
        if n != ntot {
            return self.fail("bad codelengths");
        }
        zbuild_huffman(&mut self.z_length, &lencodes[..hlit as usize])
            .or_else(|reason| self.fail(reason))?;
        zbuild_huffman(
            &mut self.z_distance,
            &lencodes[hlit as usize..(hlit + hdist) as usize],
        )
        .or_else(|reason| self.fail(reason))?;
        Ok(())
    }

    /// `stbi__parse_uncompressed_block`
    fn parse_uncompressed_block(&mut self) -> Result<(), Failed> {
        let mut header = [0u8; 4];
        if self.num_bits & 7 != 0 {
            self.zreceive(self.num_bits & 7); // discard
        }
        // drain the bit-packed data into header
        let mut k = 0;
        while self.num_bits > 0 {
            header[k] = (self.code_buffer & 255) as u8; // suppress MSVC run-time check
            k += 1;
            self.code_buffer >>= 8;
            self.num_bits -= 8;
        }
        if self.num_bits < 0 {
            return self.fail("zlib corrupt");
        }
        // now fill header the normal way
        while k < 4 {
            header[k] = self.zget8();
            k += 1;
        }
        let len = usize::from(header[1]) * 256 + usize::from(header[0]);
        let nlen = usize::from(header[3]) * 256 + usize::from(header[2]);
        if nlen != (len ^ 0xffff) {
            return self.fail("zlib corrupt");
        }
        if self.at + len > self.zbuffer.len() {
            return self.fail("read past buffer");
        }
        if self.zout.len() as u64 + len as u64 > u64::from(self.limit) {
            self.zexpand(len as i32)?;
        }
        self.ensure_capacity(len)?;
        self.zout
            .extend_from_slice(&self.zbuffer[self.at..self.at + len]);
        self.at += len;
        Ok(())
    }

    /// `stbi__parse_zlib_header`
    fn parse_zlib_header(&mut self) -> Result<(), Failed> {
        let cmf = i32::from(self.zget8());
        let cm = cmf & 15;
        /* int cinfo = cmf >> 4; */
        let flg = i32::from(self.zget8());
        if self.zeof() {
            return self.fail("bad zlib header"); // zlib spec
        }
        if (cmf * 256 + flg) % 31 != 0 {
            return self.fail("bad zlib header"); // zlib spec
        }
        if flg & 32 != 0 {
            return self.fail("no preset dict"); // preset dictionary not allowed in png
        }
        if cm != 8 {
            return self.fail("bad compression"); // DEFLATE required for png
        }
        // window = 1 << (8 + cinfo)... but who cares, we fully buffer output
        Ok(())
    }

    /// `stbi__parse_zlib`
    fn parse_zlib(&mut self, parse_header: bool) -> Result<(), Failed> {
        if parse_header {
            self.parse_zlib_header()?;
        }
        self.num_bits = 0;
        self.code_buffer = 0;
        self.hit_zeof_once = false;
        loop {
            let last = self.zreceive(1);
            let kind = self.zreceive(2);
            if kind == 0 {
                self.parse_uncompressed_block()?;
            } else if kind == 3 {
                return Err(Failed);
            } else {
                if kind == 1 {
                    // Precomputed RFC 1951 fixed Huffman tables (`const` at compile time).
                    self.z_length = FIXED_LENGTH;
                    self.z_distance = FIXED_DISTANCE;
                } else {
                    self.compute_huffman_codes()?;
                }
                self.parse_huffman_block()?;
            }
            if last != 0 {
                break;
            }
        }
        Ok(())
    }
}

/// `stbi__zbuild_huffman`
// (The names are stb_image's, so that the code reads against the original.)
#[allow(clippy::many_single_char_names)]
fn zbuild_huffman(z: &mut ZHuffman, sizelist: &[u8]) -> Result<(), &'static str> {
    let mut k = 0i32;
    let mut next_code = [0i32; 16];
    let mut sizes = [0i32; 17];

    // DEFLATE spec for generating codes
    z.fast.fill(0);
    for &size in sizelist {
        sizes[usize::from(size)] += 1;
    }
    sizes[0] = 0;
    if sizes
        .iter()
        .enumerate()
        .skip(1)
        .take(15)
        .any(|(i, &n)| n > 1 << i)
    {
        return Err("bad sizes");
    }
    let mut code = 0i32;
    for i in 1..16 {
        next_code[i] = code;
        z.firstcode[i] = code as u16;
        z.firstsymbol[i] = k as u16;
        code += sizes[i];
        if sizes[i] != 0 && code > 1 << i {
            return Err("bad codelengths");
        }
        z.maxcode[i] = code << (16 - i); // preshift for inner loop
        code <<= 1;
        k += sizes[i];
    }
    z.maxcode[16] = 0x10000; // sentinel
    for (i, &size) in sizelist.iter().enumerate() {
        let s = usize::from(size);
        if s != 0 {
            let c =
                (next_code[s] - i32::from(z.firstcode[s]) + i32::from(z.firstsymbol[s])) as usize;
            let fastv = ((s << 9) | i) as u16;
            z.size[c] = s as u8;
            z.value[c] = i as u16;
            if s <= STBI_ZFAST_BITS {
                let mut j = bit_reverse(next_code[s], s as i32) as usize;
                while j < (1 << STBI_ZFAST_BITS) {
                    z.fast[j] = fastv;
                    j += 1 << s;
                }
            }
            next_code[s] += 1;
        }
    }
    Ok(())
}

/// `stbi__zhuffman_decode_slowpath`
fn zhuffman_decode_slowpath(code_buffer: &mut u32, num_bits: &mut i32, z: &ZHuffman) -> i32 {
    // not resolved by fast table, so compute it the slow way
    // use jpeg approach, which requires MSbits at top
    let k = bit_reverse(*code_buffer as i32, 16);
    let mut s = STBI_ZFAST_BITS + 1;
    loop {
        if k < z.maxcode[s] {
            break;
        }
        s += 1;
    }
    if s >= 16 {
        return -1; // invalid code!
    }
    // code size is s, so:
    let b = (k >> (16 - s)) - i32::from(z.firstcode[s]) + i32::from(z.firstsymbol[s]);
    if b >= STBI_ZNSYMS as i32 {
        return -1; // some data was corrupt somewhere!
    }
    // (Below zero cannot be: the codes shorter than this one end where it begins. The
    // original reads before its table there.)
    if b < 0 || usize::from(z.size[b as usize]) != s {
        return -1; // was originally an assert, but report failure instead.
    }
    *code_buffer >>= s;
    *num_bits -= s as i32;
    i32::from(z.value[b as usize])
}

/// `stbi__zhuffman_decode`, with the lengths' table or the distances'.
fn zhuffman_decode(a: &mut Zbuf, distance: bool) -> i32 {
    if !zhuffman_ready(a) {
        return -1;
    }
    let z = if distance { &a.z_distance } else { &a.z_length };
    zhuffman_look_up(&mut a.code_buffer, &mut a.num_bits, z)
}

/// `stbi__zhuffman_decode`, with a table that is not one of the decoder's two.
fn zhuffman_decode_with(a: &mut Zbuf, z: &ZHuffman) -> i32 {
    if !zhuffman_ready(a) {
        return -1;
    }
    zhuffman_look_up(&mut a.code_buffer, &mut a.num_bits, z)
}

/// The first half of `stbi__zhuffman_decode`: the bits it needs. False when there are none.
fn zhuffman_ready(a: &mut Zbuf) -> bool {
    if a.num_bits < 16 {
        if a.zeof() {
            if a.hit_zeof_once {
                // We already inserted our extra 16 padding bits and are again
                // out, this stream is actually prematurely terminated.
                return false;
            }
            // This is the first time we hit eof, insert 16 extra padding btis
            // to allow us to keep going; if we actually consume any of them
            // though, that is invalid data. This is caught later.
            a.hit_zeof_once = true;
            a.num_bits += 16; // add 16 implicit zero bits
        } else {
            a.fill_bits();
        }
    }
    true
}

/// The second half of `stbi__zhuffman_decode`.
fn zhuffman_look_up(code_buffer: &mut u32, num_bits: &mut i32, z: &ZHuffman) -> i32 {
    let b = i32::from(z.fast[(*code_buffer & STBI_ZFAST_MASK) as usize]);
    if b != 0 {
        let s = b >> 9;
        *code_buffer >>= s;
        *num_bits -= s;
        return b & 511;
    }
    zhuffman_decode_slowpath(code_buffer, num_bits, z)
}

static STBI_ZLENGTH_BASE: [i32; 31] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131,
    163, 195, 227, 258, 0, 0,
];

static STBI_ZLENGTH_EXTRA: [i32; 31] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0, 0, 0,
];

static STBI_ZDIST_BASE: [i32; 32] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537,
    2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577, 0, 0,
];

static STBI_ZDIST_EXTRA: [i32; 32] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13,
    13, 0, 0,
];

/// `stbi__zdefault_length`
const STBI_ZDEFAULT_LENGTH: [u8; STBI_ZNSYMS] = {
    let mut lengths = [8u8; STBI_ZNSYMS];
    let mut i = 144;
    while i <= 255 {
        lengths[i] = 9;
        i += 1;
    }
    while i <= 279 {
        lengths[i] = 7;
        i += 1;
    }
    lengths
};

/// `stbi__zdefault_distance`
const STBI_ZDEFAULT_DISTANCE: [u8; 32] = [5; 32];

const FIXED_LENGTH: ZHuffman = build_fixed_huffman(&STBI_ZDEFAULT_LENGTH);
const FIXED_DISTANCE: ZHuffman = build_fixed_huffman(&STBI_ZDEFAULT_DISTANCE);

/// `stbi__do_zlib`: the decoded bytes, or the reason there are none (which is none itself
/// where the original sets none).
fn do_zlib(
    buffer: &[u8],
    olen: u32,
    expandable: bool,
    parse_header: bool,
) -> Result<Vec<u8>, Option<&'static str>> {
    let mut a = Zbuf {
        zbuffer: buffer,
        at: 0,
        num_bits: 0,
        hit_zeof_once: false,
        code_buffer: 0,
        zout: Vec::new(),
        limit: olen,
        z_expandable: expandable,
        z_length: ZHuffman::new(),
        z_distance: ZHuffman::new(),
        reason: None,
    };
    if a.parse_zlib(parse_header).is_ok() {
        Ok(a.zout)
    } else {
        Err(a.reason)
    }
}

/// `stbi_zlib_decode_malloc_guesssize_headerflag`, as the PNG reader calls it: the reason is
/// none where the original sets none. When the `fast-zlib` feature is enabled, valid RFC 1950
/// zlib streams are accelerated via `libdeflate_nrp`, falling back seamlessly to `do_zlib` on
/// raw-DEFLATE (`CgBI`) or damaged streams.
pub(crate) fn decode_for_png(
    buffer: &[u8],
    initial_size: u32,
    expected_out_max: Option<usize>,
    parse_header: bool,
) -> Result<Vec<u8>, Option<&'static str>> {
    // (The original takes the size as an int, and has no memory to start with when that is
    // below zero.)
    if (initial_size as i32) < 0 {
        return Err(None);
    }
    #[cfg(feature = "fast-zlib")]
    if parse_header
        && let Some(expected) = expected_out_max
        && expected <= buffer.len().saturating_mul(1032).saturating_add(64)
        && let Some(mut out) = super::try_alloc_zeroed::<u8>(expected.saturating_add(16))
        && let Ok(res) = libdeflate_nrp::zlib_decompress(buffer, &mut out)
    {
        out.truncate(res.bytes_written);
        return Ok(out);
    }
    #[cfg(not(feature = "fast-zlib"))]
    let _ = expected_out_max;
    do_zlib(buffer, initial_size, true, parse_header)
}

fn reason(result: Result<Vec<u8>, Option<&'static str>>) -> Result<Vec<u8>, LoadError> {
    result.map_err(|reason| LoadError::new(reason.unwrap_or("")))
}

/// The container of a compressed stream.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Format {
    /// A zlib stream: a two-byte header, the DEFLATE data, and an Adler-32 checksum, which
    /// is not checked (as the original does not check it).
    #[default]
    Zlib,
    /// Raw DEFLATE data.
    Raw,
}

/// Decompresses a zlib or raw DEFLATE stream (`stbi_zlib_decode_malloc` and
/// `stbi_zlib_decode_noheader_malloc`).
///
/// # Errors
///
/// A stream that is corrupt or ends early, with `stb_image`'s reason for the failure.
///
/// ```
/// use stb_image_nrp::zlib::{Format, decompress};
///
/// // "hi", stored.
/// let stream = [0x78, 0x01, 0x01, 0x02, 0x00, 0xFD, 0xFF, b'h', b'i', 0, 0, 0, 0];
/// assert_eq!(decompress(&stream, Format::Zlib).unwrap(), b"hi");
/// ```
pub fn decompress(stream: &[u8], format: Format) -> Result<Vec<u8>, LoadError> {
    reason(do_zlib(stream, 16384, true, format == Format::Zlib))
}

/// Decompresses a zlib or raw DEFLATE stream that holds no more than `max_len` bytes
/// (`stbi_zlib_decode_buffer` and `stbi_zlib_decode_noheader_buffer`). A stream that holds
/// more fails with the reason `"output buffer limit"`.
///
/// # Errors
///
/// As [`decompress`], and a stream that holds more than `max_len` bytes.
pub fn decompress_with_limit(
    stream: &[u8],
    format: Format,
    max_len: usize,
) -> Result<Vec<u8>, LoadError> {
    reason(do_zlib(
        stream,
        max_len.min(i32::MAX as usize) as u32,
        false,
        format == Format::Zlib,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use miniz_nrp as miniz;

    #[test]
    fn const_fixed_huffman_tables_match_runtime_builder() {
        let mut runtime_len = ZHuffman::new();
        zbuild_huffman(&mut runtime_len, &STBI_ZDEFAULT_LENGTH).unwrap();
        assert_eq!(FIXED_LENGTH.fast, runtime_len.fast);
        assert_eq!(FIXED_LENGTH.firstcode, runtime_len.firstcode);
        assert_eq!(FIXED_LENGTH.maxcode, runtime_len.maxcode);
        assert_eq!(FIXED_LENGTH.firstsymbol, runtime_len.firstsymbol);
        assert_eq!(FIXED_LENGTH.size, runtime_len.size);
        assert_eq!(FIXED_LENGTH.value, runtime_len.value);

        let mut runtime_dist = ZHuffman::new();
        zbuild_huffman(&mut runtime_dist, &STBI_ZDEFAULT_DISTANCE).unwrap();
        assert_eq!(FIXED_DISTANCE.fast, runtime_dist.fast);
        assert_eq!(FIXED_DISTANCE.firstcode, runtime_dist.firstcode);
        assert_eq!(FIXED_DISTANCE.maxcode, runtime_dist.maxcode);
        assert_eq!(FIXED_DISTANCE.firstsymbol, runtime_dist.firstsymbol);
        assert_eq!(FIXED_DISTANCE.size, runtime_dist.size);
        assert_eq!(FIXED_DISTANCE.value, runtime_dist.value);
    }

    #[test]
    fn zexpand_rejects_negative_and_overflowing_requests_cleanly() {
        let mut z = Zbuf {
            zbuffer: &[],
            at: 0,
            num_bits: 0,
            hit_zeof_once: false,
            code_buffer: 0,
            zout: Vec::new(),
            limit: u32::MAX / 2 + 2,
            z_expandable: true,
            z_length: ZHuffman::new(),
            z_distance: ZHuffman::new(),
            reason: None,
        };
        assert!(z.zexpand(-1).is_err());
        assert_eq!(z.reason, Some("outofmem"));
        // Requesting expansion when `limit > u32::MAX / 2` and `cur + n > limit` fails cleanly
        // with `"outofmem"` instead of overflowing.
        z.reason = None;
        z.zout = alloc::vec![0u8; 16];
        z.limit = u32::MAX / 2 + 2;
        assert!(z.zexpand(i32::MAX).is_err());
        assert_eq!(z.reason, Some("outofmem"));
    }

    #[test]
    fn streams_of_every_kind_of_block_come_back() {
        let text: Vec<u8> = (0..30000u32)
            .map(|i| b"the quick brown fox "[(i % 20) as usize] ^ (i / 997) as u8)
            .collect();
        for level in [0, 1, 6, 9] {
            let packed = miniz::compress(&text, miniz::Level::new(level).unwrap());
            assert_eq!(decompress(&packed, Format::Zlib).as_deref(), Ok(&text[..]));
            assert_eq!(
                decompress_with_limit(&packed, Format::Zlib, text.len()).as_deref(),
                Ok(&text[..])
            );
            assert_eq!(
                decompress(&packed[2..], Format::Raw).as_deref(),
                Ok(&text[..])
            );
            assert_eq!(
                decompress_with_limit(&packed[2..], Format::Raw, text.len()).as_deref(),
                Ok(&text[..])
            );
            assert_eq!(
                decode_for_png(&packed, text.len() as u32, Some(text.len()), true).as_deref(),
                Ok(&text[..])
            );
            // Into too little room.
            assert_eq!(
                decompress_with_limit(&packed, Format::Zlib, text.len() - 1),
                Err(LoadError::new("output buffer limit"))
            );
        }
        // The fixed codes.
        let fixed = stb_image_write_nrp::zlib_compress(&text, 8);
        assert_eq!(decompress(&fixed, Format::Zlib).as_deref(), Ok(&text[..]));
        assert_eq!(
            decode_for_png(&fixed, text.len() as u32, Some(text.len()), true).as_deref(),
            Ok(&text[..])
        );
    }

    #[test]
    fn what_is_no_stream_is_refused() {
        assert_eq!(
            decompress(&[], Format::Zlib),
            Err(LoadError::new("bad zlib header"))
        );
        assert_eq!(
            decompress(&[0x78, 0x9d, 0], Format::Zlib),
            Err(LoadError::new("bad zlib header"))
        );
        assert_eq!(
            decompress(&[0x78, 0xbb, 0], Format::Zlib),
            Err(LoadError::new("no preset dict"))
        );
        assert_eq!(
            decompress(&[0x79, 0x18, 0], Format::Zlib),
            Err(LoadError::new("bad compression"))
        );
        // A block of the kind there is none of: refused with no reason.
        assert_eq!(decompress(&[0x07], Format::Raw), Err(LoadError::new("")));
        // A stored block whose two lengths disagree, and one longer than the stream.
        assert_eq!(
            decompress(&[0x01, 5, 0, 5, 0], Format::Raw),
            Err(LoadError::new("zlib corrupt"))
        );
        assert_eq!(
            decompress(&[0x01, 5, 0, 0xfa, 0xff, 1, 2], Format::Raw),
            Err(LoadError::new("read past buffer"))
        );
        // A copy from before the beginning (fixed codes: a length, then distance 1).
        assert_eq!(
            decompress(&[0x03, 0x02, 0x00], Format::Raw),
            Err(LoadError::new("bad dist"))
        );
        // Cut short.
        let packed = miniz::compress(&[7u8; 5000], miniz::Level::new(6).unwrap());
        assert!(decompress(&packed[..packed.len() - 6], Format::Zlib).is_err());
    }
}
