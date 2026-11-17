//! The zlib decoder of `stb_image.h`, which its PNG reader uses: "public domain zlib decode
//! v0.2, Sean Barrett 2006-11-18".
//!
//! - all input must be provided in an upfront buffer
//! - all output is written to a single output buffer
//!
//! It checks no Adler-32 checksum. (It is here so that a PNG is read as the original reads it,
//! the damaged ones too.)

use super::LoadError;

// fast-way is faster to check than jpeg huffman, but slow way is slower
const STBI_ZFAST_BITS: usize = 9; // accelerate all cases in default tables
const STBI_ZFAST_MASK: u32 = (1 << STBI_ZFAST_BITS) - 1;
const STBI_ZNSYMS: usize = 288; // number of symbols in literal/length alphabet

/// `stbi__zhuffman`: zlib-style huffman encoding (jpegs packs from left, zlib from right, so
/// can't share code)
struct ZHuffman {
    fast: [u16; 1 << STBI_ZFAST_BITS],
    firstcode: [u16; 16],
    maxcode: [i32; 17],
    firstsymbol: [u16; 16],
    size: [u8; STBI_ZNSYMS],
    value: [u16; STBI_ZNSYMS],
}

impl ZHuffman {
    fn new() -> ZHuffman {
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
fn bitreverse16(mut n: i32) -> i32 {
    n = ((n & 0xAAAA) >> 1) | ((n & 0x5555) << 1);
    n = ((n & 0xCCCC) >> 2) | ((n & 0x3333) << 2);
    n = ((n & 0xF0F0) >> 4) | ((n & 0x0F0F) << 4);
    n = ((n & 0xFF00) >> 8) | ((n & 0x00FF) << 8);
    n
}

/// `stbi__bit_reverse`
fn bit_reverse(v: i32, bits: i32) -> i32 {
    debug_assert!(bits <= 16);
    // to bit reverse n bits, reverse 16 and shift
    // e.g. 11 bits, bit reverse and shift away 5
    bitreverse16(v) >> (16 - bits)
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

impl Zbuf<'_> {
    /// `stbi__err`
    fn err(&mut self, reason: &'static str) -> bool {
        self.reason = Some(reason);
        false
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

    /// `stbi__zexpand`: need to make room for n bytes
    fn zexpand(&mut self, n: i32) -> bool {
        if !self.z_expandable {
            return self.err("output buffer limit");
        }
        let cur = self.zout.len() as u32;
        let mut limit = self.limit;
        if limit == 0 {
            limit = 1;
        }
        if u32::MAX - cur < n as u32 {
            return self.err("outofmem");
        }
        while cur + n as u32 > limit {
            if limit > u32::MAX / 2 {
                return self.err("outofmem");
            }
            limit *= 2;
        }
        self.limit = limit;
        true
    }

    /// `stbi__parse_huffman_block`
    fn parse_huffman_block(&mut self) -> bool {
        loop {
            let mut z = zhuffman_decode(self, false);
            if z < 256 {
                if z < 0 {
                    return self.err("bad huffman code"); // error in huffman codes
                }
                if self.zout.len() as u32 >= self.limit && !self.zexpand(1) {
                    return false;
                }
                self.zout.push(z as u8);
            } else {
                if z == 256 {
                    if self.hit_zeof_once && self.num_bits < 16 {
                        // The first time we hit zeof, we inserted 16 extra zero bits into our bit
                        // buffer so the decoder can just do its speculative decoding. But if we
                        // actually consumed any of those bits
                        // (which is the case when num_bits < 16),
                        // the stream actually read past the end so it is malformed.
                        return self.err("unexpected end");
                    }
                    return true;
                }
                if z >= 286 {
                    // per DEFLATE, length codes 286 and 287 must not appear in compressed data
                    return self.err("bad huffman code");
                }
                z -= 257;
                let mut len = STBI_ZLENGTH_BASE[z as usize];
                if STBI_ZLENGTH_EXTRA[z as usize] != 0 {
                    len += self.zreceive(STBI_ZLENGTH_EXTRA[z as usize]) as i32;
                }
                z = zhuffman_decode(self, true);
                if !(0..30).contains(&z) {
                    // per DEFLATE, distance codes 30 and 31 must not appear in compressed data
                    return self.err("bad huffman code");
                }
                let mut dist = STBI_ZDIST_BASE[z as usize];
                if STBI_ZDIST_EXTRA[z as usize] != 0 {
                    dist += self.zreceive(STBI_ZDIST_EXTRA[z as usize]) as i32;
                }
                if (self.zout.len() as i64) < i64::from(dist) {
                    return self.err("bad dist");
                }
                if i64::from(len) > i64::from(self.limit) - self.zout.len() as i64
                    && !self.zexpand(len)
                {
                    return false;
                }
                let from = self.zout.len() - dist as usize;
                if dist == 1 {
                    // run of one byte; common in images.
                    let v = self.zout[from];
                    self.zout.resize(self.zout.len() + len as usize, v);
                } else {
                    for i in 0..len as usize {
                        let v = self.zout[from + i];
                        self.zout.push(v);
                    }
                }
            }
        }
    }

    /// `stbi__compute_huffman_codes`
    fn compute_huffman_codes(&mut self) -> bool {
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
        if !zbuild_huffman(&mut z_codelength, &codelength_sizes, &mut self.reason) {
            return false;
        }

        let mut n = 0i32;
        while n < ntot {
            let mut c = zhuffman_decode_with(self, &z_codelength);
            if !(0..19).contains(&c) {
                return self.err("bad codelengths");
            }
            if c < 16 {
                lencodes[n as usize] = c as u8;
                n += 1;
            } else {
                let mut fill = 0u8;
                if c == 16 {
                    c = self.zreceive(2) as i32 + 3;
                    if n == 0 {
                        return self.err("bad codelengths");
                    }
                    fill = lencodes[n as usize - 1];
                } else if c == 17 {
                    c = self.zreceive(3) as i32 + 3;
                } else if c == 18 {
                    c = self.zreceive(7) as i32 + 11;
                } else {
                    return self.err("bad codelengths");
                }
                if ntot - n < c {
                    return self.err("bad codelengths");
                }
                lencodes[n as usize..(n + c) as usize].fill(fill);
                n += c;
            }
        }
        if n != ntot {
            return self.err("bad codelengths");
        }
        if !zbuild_huffman(
            &mut self.z_length,
            &lencodes[..hlit as usize],
            &mut self.reason,
        ) {
            return false;
        }
        if !zbuild_huffman(
            &mut self.z_distance,
            &lencodes[hlit as usize..(hlit + hdist) as usize],
            &mut self.reason,
        ) {
            return false;
        }
        true
    }

    /// `stbi__parse_uncompressed_block`
    fn parse_uncompressed_block(&mut self) -> bool {
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
            return self.err("zlib corrupt");
        }
        // now fill header the normal way
        while k < 4 {
            header[k] = self.zget8();
            k += 1;
        }
        let len = usize::from(header[1]) * 256 + usize::from(header[0]);
        let nlen = usize::from(header[3]) * 256 + usize::from(header[2]);
        if nlen != (len ^ 0xffff) {
            return self.err("zlib corrupt");
        }
        if self.at + len > self.zbuffer.len() {
            return self.err("read past buffer");
        }
        if self.zout.len() as u64 + len as u64 > u64::from(self.limit) && !self.zexpand(len as i32)
        {
            return false;
        }
        self.zout
            .extend_from_slice(&self.zbuffer[self.at..self.at + len]);
        self.at += len;
        true
    }

    /// `stbi__parse_zlib_header`
    fn parse_zlib_header(&mut self) -> bool {
        let cmf = i32::from(self.zget8());
        let cm = cmf & 15;
        /* int cinfo = cmf >> 4; */
        let flg = i32::from(self.zget8());
        if self.zeof() {
            return self.err("bad zlib header"); // zlib spec
        }
        if (cmf * 256 + flg) % 31 != 0 {
            return self.err("bad zlib header"); // zlib spec
        }
        if flg & 32 != 0 {
            return self.err("no preset dict"); // preset dictionary not allowed in png
        }
        if cm != 8 {
            return self.err("bad compression"); // DEFLATE required for png
        }
        // window = 1 << (8 + cinfo)... but who cares, we fully buffer output
        true
    }

    /// `stbi__parse_zlib`
    fn parse_zlib(&mut self, parse_header: bool) -> bool {
        if parse_header && !self.parse_zlib_header() {
            return false;
        }
        self.num_bits = 0;
        self.code_buffer = 0;
        self.hit_zeof_once = false;
        loop {
            let last = self.zreceive(1);
            let kind = self.zreceive(2);
            if kind == 0 {
                if !self.parse_uncompressed_block() {
                    return false;
                }
            } else if kind == 3 {
                return false;
            } else {
                if kind == 1 {
                    // use fixed code lengths
                    if !zbuild_huffman(&mut self.z_length, &STBI_ZDEFAULT_LENGTH, &mut self.reason)
                    {
                        return false;
                    }
                    if !zbuild_huffman(
                        &mut self.z_distance,
                        &STBI_ZDEFAULT_DISTANCE,
                        &mut self.reason,
                    ) {
                        return false;
                    }
                } else if !self.compute_huffman_codes() {
                    return false;
                }
                if !self.parse_huffman_block() {
                    return false;
                }
            }
            if last != 0 {
                break;
            }
        }
        true
    }
}

/// `stbi__zbuild_huffman`
fn zbuild_huffman(z: &mut ZHuffman, sizelist: &[u8], reason: &mut Option<&'static str>) -> bool {
    let mut k = 0i32;
    let mut next_code = [0i32; 16];
    let mut sizes = [0i32; 17];

    // DEFLATE spec for generating codes
    z.fast.fill(0);
    for &size in sizelist {
        sizes[usize::from(size)] += 1;
    }
    sizes[0] = 0;
    for i in 1..16 {
        if sizes[i] > (1 << i) {
            *reason = Some("bad sizes");
            return false;
        }
    }
    let mut code = 0i32;
    for i in 1..16 {
        next_code[i] = code;
        z.firstcode[i] = code as u16;
        z.firstsymbol[i] = k as u16;
        code += sizes[i];
        if sizes[i] != 0 && code - 1 >= (1 << i) {
            *reason = Some("bad codelengths");
            return false;
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
    true
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
            if !a.hit_zeof_once {
                // This is the first time we hit eof, insert 16 extra padding btis
                // to allow us to keep going; if we actually consume any of them
                // though, that is invalid data. This is caught later.
                a.hit_zeof_once = true;
                a.num_bits += 16; // add 16 implicit zero bits
            } else {
                // We already inserted our extra 16 padding bits and are again
                // out, this stream is actually prematurely terminated.
                return false;
            }
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
static STBI_ZDEFAULT_LENGTH: [u8; STBI_ZNSYMS] = {
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
static STBI_ZDEFAULT_DISTANCE: [u8; 32] = [5; 32];

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
    if a.parse_zlib(parse_header) {
        Ok(a.zout)
    } else {
        Err(a.reason)
    }
}

/// `stbi_zlib_decode_malloc_guesssize_headerflag`, as the PNG reader calls it: the reason is
/// none where the original sets none.
pub(crate) fn decode_for_png(
    buffer: &[u8],
    initial_size: u32,
    parse_header: bool,
) -> Result<Vec<u8>, Option<&'static str>> {
    // (The original takes the size as an int, and has no memory to start with when that is
    // below zero.)
    if (initial_size as i32) < 0 {
        return Err(None);
    }
    do_zlib(buffer, initial_size, true, parse_header)
}

fn reason(result: Result<Vec<u8>, Option<&'static str>>) -> Result<Vec<u8>, LoadError> {
    result.map_err(|reason| LoadError(reason.unwrap_or("")))
}

/// A zlib stream decoded: `stbi_zlib_decode_malloc`.
pub fn zlib_decode_malloc(buffer: &[u8]) -> Result<Vec<u8>, LoadError> {
    reason(do_zlib(buffer, 16384, true, true))
}

/// A zlib stream decoded, or with `parse_header` false a deflate stream with no zlib header:
/// `stbi_zlib_decode_malloc_guesssize_headerflag`. (The size the original starts from does
/// not change what comes out, so it is not asked for.)
pub fn zlib_decode_malloc_headerflag(
    buffer: &[u8],
    parse_header: bool,
) -> Result<Vec<u8>, LoadError> {
    reason(do_zlib(buffer, 16384, true, parse_header))
}

/// A zlib stream decoded into no more than `olen` bytes: `stbi_zlib_decode_buffer`. A stream
/// that holds more is refused.
pub fn zlib_decode_buffer(olen: usize, buffer: &[u8]) -> Result<Vec<u8>, LoadError> {
    reason(do_zlib(
        buffer,
        olen.min(i32::MAX as usize) as u32,
        false,
        true,
    ))
}

/// A deflate stream with no zlib header decoded: `stbi_zlib_decode_noheader_malloc`.
pub fn zlib_decode_noheader_malloc(buffer: &[u8]) -> Result<Vec<u8>, LoadError> {
    reason(do_zlib(buffer, 16384, true, false))
}

/// A deflate stream with no zlib header decoded into no more than `olen` bytes:
/// `stbi_zlib_decode_noheader_buffer`.
pub fn zlib_decode_noheader_buffer(olen: usize, buffer: &[u8]) -> Result<Vec<u8>, LoadError> {
    reason(do_zlib(
        buffer,
        olen.min(i32::MAX as usize) as u32,
        false,
        false,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use miniz_nrp as miniz;

    #[test]
    fn streams_of_every_kind_of_block_come_back() {
        let text: Vec<u8> = (0..30000u32)
            .map(|i| b"the quick brown fox "[(i % 20) as usize] ^ (i / 997) as u8)
            .collect();
        for level in [0, 1, 6, 9] {
            let packed = miniz::compress(&text, level);
            assert_eq!(zlib_decode_malloc(&packed).as_deref(), Ok(&text[..]));
            assert_eq!(
                zlib_decode_buffer(text.len(), &packed).as_deref(),
                Ok(&text[..])
            );
            assert_eq!(
                zlib_decode_noheader_malloc(&packed[2..]).as_deref(),
                Ok(&text[..])
            );
            assert_eq!(
                zlib_decode_noheader_buffer(text.len(), &packed[2..]).as_deref(),
                Ok(&text[..])
            );
            assert_eq!(
                zlib_decode_malloc_headerflag(&packed[2..], false).as_deref(),
                Ok(&text[..])
            );
            // Into too little room.
            assert_eq!(
                zlib_decode_buffer(text.len() - 1, &packed),
                Err(LoadError("output buffer limit"))
            );
        }
        // The fixed codes.
        let fixed = stb_image_write_nrp::zlib_compress(&text, 8);
        assert_eq!(zlib_decode_malloc(&fixed).as_deref(), Ok(&text[..]));
    }

    #[test]
    fn what_is_no_stream_is_refused() {
        assert_eq!(zlib_decode_malloc(&[]), Err(LoadError("bad zlib header")));
        assert_eq!(
            zlib_decode_malloc(&[0x78, 0x9d, 0]),
            Err(LoadError("bad zlib header"))
        );
        assert_eq!(
            zlib_decode_malloc(&[0x78, 0xbb, 0]),
            Err(LoadError("no preset dict"))
        );
        assert_eq!(
            zlib_decode_malloc(&[0x79, 0x18, 0]),
            Err(LoadError("bad compression"))
        );
        // A block of the kind there is none of: refused with no reason.
        assert_eq!(zlib_decode_noheader_malloc(&[0x07]), Err(LoadError("")));
        // A stored block whose two lengths disagree, and one longer than the stream.
        assert_eq!(
            zlib_decode_noheader_malloc(&[0x01, 5, 0, 5, 0]),
            Err(LoadError("zlib corrupt"))
        );
        assert_eq!(
            zlib_decode_noheader_malloc(&[0x01, 5, 0, 0xfa, 0xff, 1, 2]),
            Err(LoadError("read past buffer"))
        );
        // A copy from before the beginning (fixed codes: a length, then distance 1).
        assert_eq!(
            zlib_decode_noheader_malloc(&[0x03, 0x02, 0x00]),
            Err(LoadError("bad dist"))
        );
        // Cut short.
        let packed = miniz::compress(&[7u8; 5000], 6);
        assert!(zlib_decode_malloc(&packed[..packed.len() - 6]).is_err());
    }
}
