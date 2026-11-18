//! The JPEG reader of `stb_image.h`: baseline and progressive, 8 bits a channel, with the
//! original's plain C kernels (its `STBI_NO_SIMD` build).

use super::{
    Context, Failed, ImageFormat, RawInfo, STBI_MAX_DIMENSIONS, Scan, addints_valid, compute_y,
    mad2sizes_valid, mad3sizes_valid, mul2shorts_valid,
};

// huffman decoding acceleration
const FAST_BITS: usize = 9; // larger handles more cases; smaller stomps less cache

/// `stbi__huffman`
#[derive(Clone)]
struct Huffman {
    fast: [u8; 1 << FAST_BITS],
    // weirdly, repacking this into AoS is a 10% speed loss, instead of a win
    code: [u16; 256],
    values: [u8; 256],
    size: [u8; 257],
    maxcode: [u32; 18],
    delta: [i32; 17], // old 'firstsymbol' - old 'firstcode'
}

impl Huffman {
    fn new() -> Huffman {
        Huffman {
            fast: [0; 1 << FAST_BITS],
            code: [0; 256],
            values: [0; 256],
            size: [0; 257],
            maxcode: [0; 18],
            delta: [0; 17],
        }
    }
}

/// A component of the image: `img_comp` of `stbi__jpeg`.
#[derive(Clone, Default)]
struct Component {
    id: i32,
    h: i32,
    v: i32,
    tq: i32,
    hd: i32,
    ha: i32,
    dc_pred: i32,

    x: i32,
    y: i32,
    w2: i32,
    h2: i32,
    data: Vec<u8>,
    linebuf: Vec<u8>,
    coeff: Vec<i16>, // progressive only
    coeff_w: i32,    // number of 8x8 coefficient blocks
}

/// The part of `stbi__jpeg` that reads the entropy-coded bits, with the scan's settings.
struct Entropy {
    code_buffer: u32, // jpeg entropy-coded buffer
    code_bits: i32,   // number of valid bits
    marker: u8,       // marker seen while filling entropy buffer
    nomore: bool,     // flag if we saw a marker so must stop

    spec_start: i32,
    spec_end: i32,
    succ_high: i32,
    succ_low: i32,
    eob_run: i32,
}

/// `stbi__jpeg`
struct Jpeg {
    huff_dc: [Huffman; 4],
    huff_ac: [Huffman; 4],
    dequant: [[u16; 64]; 4],
    fast_ac: [[i16; 1 << FAST_BITS]; 4],

    // sizes for components, interleaved MCUs
    img_h_max: i32,
    img_v_max: i32,
    img_mcu_x: i32,
    img_mcu_y: i32,
    img_mcu_w: i32,
    img_mcu_h: i32,

    // definition of jpeg image component
    img_comp: [Component; 4],

    e: Entropy,

    progressive: bool,
    jfif: bool,
    app14_color_transform: i32, // Adobe APP14 tag
    rgb: i32,

    scan_n: i32,
    order: [i32; 4],
    restart_interval: i32,
    todo: i32,
}

impl Jpeg {
    /// (All zero, as the original's is after its `memset`.)
    fn new() -> Box<Jpeg> {
        Box::new(Jpeg {
            huff_dc: [
                Huffman::new(),
                Huffman::new(),
                Huffman::new(),
                Huffman::new(),
            ],
            huff_ac: [
                Huffman::new(),
                Huffman::new(),
                Huffman::new(),
                Huffman::new(),
            ],
            dequant: [[0; 64]; 4],
            fast_ac: [[0; 1 << FAST_BITS]; 4],
            img_h_max: 0,
            img_v_max: 0,
            img_mcu_x: 0,
            img_mcu_y: 0,
            img_mcu_w: 0,
            img_mcu_h: 0,
            img_comp: Default::default(),
            e: Entropy {
                code_buffer: 0,
                code_bits: 0,
                marker: 0,
                nomore: false,
                spec_start: 0,
                spec_end: 0,
                succ_high: 0,
                succ_low: 0,
                eob_run: 0,
            },
            progressive: false,
            jfif: false,
            app14_color_transform: 0,
            rgb: 0,
            scan_n: 0,
            order: [0; 4],
            restart_interval: 0,
            todo: 0,
        })
    }
}

/// `stbi__build_huffman`
// (The names are stb_image's, so that the code reads against the original.)
#[allow(clippy::many_single_char_names)]
fn build_huffman(s: &mut Context, h: &mut Huffman, count: &[i32; 16]) -> Result<(), Failed> {
    let mut k = 0usize;
    // build size list for each symbol (from JPEG spec)
    for (i, &n) in count.iter().enumerate() {
        for _ in 0..n {
            h.size[k] = (i + 1) as u8;
            k += 1;
            if k >= 257 {
                return s.fail("bad size list");
            }
        }
    }
    h.size[k] = 0;

    // compute actual symbols (from jpeg spec)
    let mut code = 0u32;
    k = 0;
    for j in 1..=16usize {
        // compute delta to add to code to compute symbol id
        h.delta[j] = (k as u32).wrapping_sub(code) as i32;
        if usize::from(h.size[k]) == j {
            while usize::from(h.size[k]) == j {
                h.code[k] = code as u16;
                k += 1;
                code += 1;
            }
            if code > 1u32 << j {
                return s.fail("bad code lengths");
            }
        }
        // compute largest code + 1 for this size, preshifted as needed later
        h.maxcode[j] = code << (16 - j);
        code <<= 1;
    }
    h.maxcode[17] = 0xffff_ffff;

    // build non-spec acceleration table; 255 is flag for not-accelerated
    h.fast.fill(255);
    for i in 0..k {
        let s = usize::from(h.size[i]);
        if s <= FAST_BITS {
            let c = usize::from(h.code[i]) << (FAST_BITS - s);
            let m = 1 << (FAST_BITS - s);
            for j in 0..m {
                h.fast[c + j] = i as u8;
            }
        }
    }
    Ok(())
}

/// `stbi__build_fast_ac`: build a table that decodes both magnitude and value of small ACs in
/// one go.
fn build_fast_ac(fast_ac: &mut [i16; 1 << FAST_BITS], h: &Huffman) {
    for (i, (fast_ac, &fast)) in fast_ac.iter_mut().zip(&h.fast).enumerate() {
        *fast_ac = 0;
        if fast < 255 {
            let rs = i32::from(h.values[usize::from(fast)]);
            let run = (rs >> 4) & 15;
            let magbits = rs & 15;
            let len = i32::from(h.size[usize::from(fast)]);

            if magbits != 0 && len + magbits <= FAST_BITS as i32 {
                // magnitude code followed by receive_extend code
                let mut k =
                    (((i as i32) << len) & ((1 << FAST_BITS) - 1)) >> (FAST_BITS as i32 - magbits);
                let m = 1 << (magbits - 1);
                if k < m {
                    k += ((!0u32 << magbits) as i32) + 1;
                }
                // if the result is small enough, we can fit it in fast_ac table
                if (-128..=127).contains(&k) {
                    *fast_ac = ((k * 256) + (run * 16) + (len + magbits)) as i16;
                }
            }
        }
    }
}

/// `stbi__grow_buffer_unsafe`
fn grow_buffer_unsafe(j: &mut Entropy, s: &mut Context) {
    loop {
        let b = if j.nomore { 0 } else { u32::from(s.get8()) };
        if b == 0xff {
            let mut c = s.get8();
            while c == 0xff {
                c = s.get8(); // consume fill bytes
            }
            if c != 0 {
                j.marker = c;
                j.nomore = true;
                return;
            }
        }
        j.code_buffer |= b.wrapping_shl((24 - j.code_bits) as u32);
        j.code_bits += 8;
        if j.code_bits > 24 {
            break;
        }
    }
}

// (1 << n) - 1
static STBI_BMASK: [u32; 17] = [
    0, 1, 3, 7, 15, 31, 63, 127, 255, 511, 1023, 2047, 4095, 8191, 16383, 32767, 65535,
];

/// `stbi__jpeg_huff_decode`: decode a jpeg huffman value from the bitstream
// (The names are stb_image's, so that the code reads against the original.)
#[allow(clippy::many_single_char_names)]
fn jpeg_huff_decode(j: &mut Entropy, s: &mut Context, h: &Huffman) -> i32 {
    if j.code_bits < 16 {
        grow_buffer_unsafe(j, s);
    }

    // look at the top FAST_BITS and determine what symbol ID it is,
    // if the code is <= FAST_BITS
    let c = ((j.code_buffer >> (32 - FAST_BITS)) & ((1 << FAST_BITS) - 1)) as usize;
    let k = usize::from(h.fast[c]);
    if k < 255 {
        let s = i32::from(h.size[k]);
        if s > j.code_bits {
            return -1;
        }
        j.code_buffer <<= s;
        j.code_bits -= s;
        return i32::from(h.values[k]);
    }

    // naive test is to shift the code_buffer down so k bits are
    // valid, then test against maxcode. To speed this up, we've
    // preshifted maxcode left so that it has (16-k) 0s at the
    // end; in other words, regardless of the number of bits, it
    // wants to be compared against something shifted to have 16;
    // that way we don't need to shift inside the loop.
    let temp = j.code_buffer >> 16;
    let mut k = FAST_BITS + 1;
    loop {
        if temp < h.maxcode[k] {
            break;
        }
        k += 1;
    }
    if k == 17 {
        // error! code not found
        j.code_bits -= 16;
        return -1;
    }

    if k as i32 > j.code_bits {
        return -1;
    }

    // convert the huffman code to the symbol id
    let c = (((j.code_buffer >> (32 - k)) & STBI_BMASK[k]) as i32).wrapping_add(h.delta[k]);
    if !(0..256).contains(&c) {
        // symbol id out of bounds!
        return -1;
    }

    // convert the id to a symbol
    j.code_bits -= k as i32;
    j.code_buffer <<= k;
    i32::from(h.values[c as usize])
}

// bias[n] = (-1<<n) + 1
static STBI_JBIAS: [i32; 16] = [
    0, -1, -3, -7, -15, -31, -63, -127, -255, -511, -1023, -2047, -4095, -8191, -16383, -32767,
];

/// `stbi__extend_receive`: combined JPEG 'receive' and JPEG 'extend', since baseline always
/// extends everything it receives.
fn extend_receive(j: &mut Entropy, s: &mut Context, n: i32) -> i32 {
    if j.code_bits < n {
        grow_buffer_unsafe(j, s);
    }
    if j.code_bits < n {
        return 0; // ran out of bits from stream, return 0s intead of continuing
    }

    // sign bit always in MSB; 0 if MSB clear (positive), 1 if MSB set (negative)
    let sgn = (j.code_buffer >> 31) as i32;
    let mut k = j.code_buffer.rotate_left(n as u32);
    j.code_buffer = k & !STBI_BMASK[n as usize];
    k &= STBI_BMASK[n as usize];
    j.code_bits -= n;
    (k as i32).wrapping_add(STBI_JBIAS[n as usize] & (sgn - 1))
}

/// `stbi__jpeg_get_bits`: get some unsigned bits
fn jpeg_get_bits(j: &mut Entropy, s: &mut Context, n: i32) -> i32 {
    if j.code_bits < n {
        grow_buffer_unsafe(j, s);
    }
    if j.code_bits < n {
        return 0; // ran out of bits from stream, return 0s intead of continuing
    }
    let mut k = j.code_buffer.rotate_left(n as u32);
    j.code_buffer = k & !STBI_BMASK[n as usize];
    k &= STBI_BMASK[n as usize];
    j.code_bits -= n;
    k as i32
}

/// `stbi__jpeg_get_bit`
fn jpeg_get_bit(j: &mut Entropy, s: &mut Context) -> bool {
    if j.code_bits < 1 {
        grow_buffer_unsafe(j, s);
    }
    if j.code_bits < 1 {
        return false; // ran out of bits from stream, return 0s intead of continuing
    }
    let k = j.code_buffer;
    j.code_buffer <<= 1;
    j.code_bits -= 1;
    k & 0x8000_0000 != 0
}

// given a value that's at position X in the zigzag stream,
// where does it appear in the 8x8 matrix coded as row-major?
#[rustfmt::skip]
static STBI_JPEG_DEZIGZAG: [u8; 64 + 15] = [
     0,  1,  8, 16,  9,  2,  3, 10,
    17, 24, 32, 25, 18, 11,  4,  5,
    12, 19, 26, 33, 40, 48, 41, 34,
    27, 20, 13,  6,  7, 14, 21, 28,
    35, 42, 49, 56, 57, 50, 43, 36,
    29, 22, 15, 23, 30, 37, 44, 51,
    58, 59, 52, 45, 38, 31, 39, 46,
    53, 60, 61, 54, 47, 55, 62, 63,
    // let corrupt input sample past end
    63, 63, 63, 63, 63, 63, 63, 63,
    63, 63, 63, 63, 63, 63, 63,
];

/// The tables one component's blocks are decoded with.
struct BlockTables<'a> {
    hdc: &'a Huffman,
    hac: &'a Huffman,
    fac: &'a [i16; 1 << FAST_BITS],
    dequant: &'a [u16; 64],
}

/// `stbi__jpeg_decode_block`: decode one 64-entry block--
// (The names are stb_image's, so that the code reads against the original.)
#[allow(clippy::many_single_char_names)]
fn jpeg_decode_block(
    j: &mut Entropy,
    s: &mut Context,
    data: &mut [i16; 64],
    tables: &BlockTables,
    dc_pred: &mut i32,
) -> Result<(), Failed> {
    let BlockTables {
        hdc,
        hac,
        fac,
        dequant,
    } = *tables;
    if j.code_bits < 16 {
        grow_buffer_unsafe(j, s);
    }
    let t = jpeg_huff_decode(j, s, hdc);
    if !(0..=15).contains(&t) {
        return s.fail("bad huffman code");
    }

    // 0 all the ac values now so we can do it 32-bits at a time
    *data = [0; 64];

    let diff = if t != 0 { extend_receive(j, s, t) } else { 0 };
    if !addints_valid(*dc_pred, diff) {
        return s.fail("bad delta");
    }
    let dc = *dc_pred + diff;
    *dc_pred = dc;
    if !mul2shorts_valid(dc, i32::from(dequant[0])) {
        return s.fail("can't merge dc and ac");
    }
    data[0] = (dc * i32::from(dequant[0])) as i16;

    // decode AC components, see JPEG spec
    let mut k = 1usize;
    loop {
        if j.code_bits < 16 {
            grow_buffer_unsafe(j, s);
        }
        let c = ((j.code_buffer >> (32 - FAST_BITS)) & ((1 << FAST_BITS) - 1)) as usize;
        let r = i32::from(fac[c]);
        if r != 0 {
            // fast-AC path
            k += ((r >> 4) & 15) as usize; // run
            let sz = r & 15; // combined length
            if sz > j.code_bits {
                return s.fail("bad huffman code");
            }
            j.code_buffer <<= sz;
            j.code_bits -= sz;
            // decode into unzigzag'd location
            let zig = usize::from(STBI_JPEG_DEZIGZAG[k]);
            k += 1;
            data[zig] = (r >> 8).wrapping_mul(i32::from(dequant[zig])) as i16;
        } else {
            let rs = jpeg_huff_decode(j, s, hac);
            if rs < 0 {
                return s.fail("bad huffman code");
            }
            let sz = rs & 15;
            let r = rs >> 4;
            if sz == 0 {
                if rs != 0xf0 {
                    break; // end block
                }
                k += 16;
            } else {
                k += r as usize;
                // decode into unzigzag'd location
                let zig = usize::from(STBI_JPEG_DEZIGZAG[k]);
                k += 1;
                data[zig] = extend_receive(j, s, sz).wrapping_mul(i32::from(dequant[zig])) as i16;
            }
        }
        if k >= 64 {
            break;
        }
    }
    Ok(())
}

/// `stbi__jpeg_decode_block_prog_dc`
fn jpeg_decode_block_prog_dc(
    j: &mut Entropy,
    s: &mut Context,
    data: &mut [i16],
    hdc: &Huffman,
    dc_pred: &mut i32,
) -> Result<(), Failed> {
    if j.spec_end != 0 {
        return s.fail("can't merge dc and ac");
    }

    if j.code_bits < 16 {
        grow_buffer_unsafe(j, s);
    }

    if j.succ_high == 0 {
        // first scan for DC coefficient, must be first
        let t = jpeg_huff_decode(j, s, hdc);
        if !(0..=15).contains(&t) {
            return s.fail("can't merge dc and ac");
        }
        let diff = if t != 0 { extend_receive(j, s, t) } else { 0 };

        if !addints_valid(*dc_pred, diff) {
            return s.fail("bad delta");
        }
        let dc = *dc_pred + diff;
        *dc_pred = dc;
        if !mul2shorts_valid(dc, 1 << j.succ_low) {
            return s.fail("can't merge dc and ac");
        }
        data[0] = (dc * (1 << j.succ_low)) as i16;
    } else {
        // refinement scan for DC coefficient
        if jpeg_get_bit(j, s) {
            data[0] = data[0].wrapping_add((1i32 << j.succ_low) as i16);
        }
    }
    Ok(())
}

/// `stbi__jpeg_decode_block_prog_ac`
// @OPTIMIZE: store non-zigzagged during the decode passes,
// and only de-zigzag when dequantizing
// (One pass, step for step as stb_image's, its state shared throughout; the names are stb_image's,
// so that the code reads against the original.)
#[allow(clippy::too_many_lines, clippy::many_single_char_names)]
fn jpeg_decode_block_prog_ac(
    j: &mut Entropy,
    s: &mut Context,
    data: &mut [i16],
    hac: &Huffman,
    fac: &[i16; 1 << FAST_BITS],
) -> Result<(), Failed> {
    if j.spec_start == 0 {
        return s.fail("can't merge dc and ac");
    }

    if j.succ_high == 0 {
        let shift = j.succ_low;

        if j.eob_run != 0 {
            j.eob_run -= 1;
            return Ok(());
        }

        let mut k = j.spec_start;
        loop {
            if j.code_bits < 16 {
                grow_buffer_unsafe(j, s);
            }
            let c = ((j.code_buffer >> (32 - FAST_BITS)) & ((1 << FAST_BITS) - 1)) as usize;
            let r = i32::from(fac[c]);
            if r != 0 {
                // fast-AC path
                k += (r >> 4) & 15; // run
                let sz = r & 15; // combined length
                if sz > j.code_bits {
                    return s.fail("bad huffman code");
                }
                j.code_buffer <<= sz;
                j.code_bits -= sz;
                let zig = usize::from(STBI_JPEG_DEZIGZAG[k as usize]);
                k += 1;
                data[zig] = ((r >> 8) * (1 << shift)) as i16;
            } else {
                let rs = jpeg_huff_decode(j, s, hac);
                if rs < 0 {
                    return s.fail("bad huffman code");
                }
                let sz = rs & 15;
                let r = rs >> 4;
                if sz == 0 {
                    if r < 15 {
                        j.eob_run = 1 << r;
                        if r != 0 {
                            j.eob_run += jpeg_get_bits(j, s, r);
                        }
                        j.eob_run -= 1;
                        break;
                    }
                    k += 16;
                } else {
                    k += r;
                    let zig = usize::from(STBI_JPEG_DEZIGZAG[k as usize]);
                    k += 1;
                    data[zig] = extend_receive(j, s, sz).wrapping_mul(1 << shift) as i16;
                }
            }
            if k > j.spec_end {
                break;
            }
        }
    } else {
        // refinement scan for these AC coefficients

        let bit = (1i32 << j.succ_low) as i16;

        if j.eob_run != 0 {
            j.eob_run -= 1;
            for k in j.spec_start..=j.spec_end {
                let p = &mut data[usize::from(STBI_JPEG_DEZIGZAG[k as usize])];
                if *p != 0 && jpeg_get_bit(j, s) && (*p & bit) == 0 {
                    if *p > 0 {
                        *p = p.wrapping_add(bit);
                    } else {
                        *p = p.wrapping_sub(bit);
                    }
                }
            }
        } else {
            let mut k = j.spec_start;
            loop {
                // @OPTIMIZE see if we can use the fast path here, advance-by-r is so slow, eh
                let rs = jpeg_huff_decode(j, s, hac);
                if rs < 0 {
                    return s.fail("bad huffman code");
                }
                let mut sz = rs & 15;
                let mut r = rs >> 4;
                if sz == 0 {
                    if r < 15 {
                        j.eob_run = (1 << r) - 1;
                        if r != 0 {
                            j.eob_run += jpeg_get_bits(j, s, r);
                        }
                        r = 64; // force end of block
                    } else {
                        // r=15 s=0 should write 16 0s, so we just do
                        // a run of 15 0s and then write s (which is 0),
                        // so we don't have to do anything special here
                    }
                } else {
                    if sz != 1 {
                        return s.fail("bad huffman code");
                    }
                    // sign bit
                    if jpeg_get_bit(j, s) {
                        sz = i32::from(bit);
                    } else {
                        sz = -i32::from(bit);
                    }
                }

                // advance by r
                while k <= j.spec_end {
                    let p = &mut data[usize::from(STBI_JPEG_DEZIGZAG[k as usize])];
                    k += 1;
                    if *p != 0 {
                        if jpeg_get_bit(j, s) && (*p & bit) == 0 {
                            if *p > 0 {
                                *p = p.wrapping_add(bit);
                            } else {
                                *p = p.wrapping_sub(bit);
                            }
                        }
                    } else {
                        if r == 0 {
                            *p = sz as i16;
                            break;
                        }
                        r -= 1;
                    }
                }
                if k > j.spec_end {
                    break;
                }
            }
        }
    }
    Ok(())
}

/// `stbi__clamp`: take a -128..127 value and `stbi__clamp` it and convert to 0..255
fn clamp(x: i32) -> u8 {
    // trick to use a single test to catch both cases
    if x as u32 > 255 {
        if x < 0 {
            return 0;
        }
        if x > 255 {
            return 255;
        }
    }
    x as u8
}

/// `stbi__f2f`
const fn f2f(x: f32) -> i32 {
    (x * 4096.0 + 0.5) as i32
}

/// `stbi__fsh`
fn fsh(x: i32) -> i32 {
    x.wrapping_mul(4096)
}

/// `STBI__IDCT_1D`: derived from jidctint -- `DCT_ISLOW`. Gives (x0, x1, x2, x3, t0, t1, t2,
/// t3). (The sums wrap where the original's overflow, which only values no encoder writes
/// bring about.)
#[allow(clippy::excessive_precision)]
fn idct_1d([s0, s1, s2, s3, s4, s5, s6, s7]: [i32; 8]) -> [i32; 8] {
    const C0_5411961: i32 = f2f(0.541_196_1);
    const CN1_847759065: i32 = f2f(-1.847_759_065);
    const C0_765366865: i32 = f2f(0.765_366_865);
    const C1_175875602: i32 = f2f(1.175_875_602);
    const C0_298631336: i32 = f2f(0.298_631_336);
    const C2_053119869: i32 = f2f(2.053_119_869);
    const C3_072711026: i32 = f2f(3.072_711_026);
    const C1_501321110: i32 = f2f(1.501_321_110);
    const CN0_899976223: i32 = f2f(-0.899_976_223);
    const CN2_562915447: i32 = f2f(-2.562_915_447);
    const CN1_961570560: i32 = f2f(-1.961_570_560);
    const CN0_390180644: i32 = f2f(-0.390_180_644);
    let add = i32::wrapping_add;
    let sub = i32::wrapping_sub;
    let mul = i32::wrapping_mul;

    let mut p2 = s2;
    let mut p3 = s6;
    let mut p1 = mul(add(p2, p3), C0_5411961);
    let mut t2 = add(p1, mul(p3, CN1_847759065));
    let mut t3 = add(p1, mul(p2, C0_765366865));
    p2 = s0;
    p3 = s4;
    let mut t0 = fsh(add(p2, p3));
    let mut t1 = fsh(sub(p2, p3));
    let x0 = add(t0, t3);
    let x3 = sub(t0, t3);
    let x1 = add(t1, t2);
    let x2 = sub(t1, t2);
    t0 = s7;
    t1 = s5;
    t2 = s3;
    t3 = s1;
    p3 = add(t0, t2);
    let mut p4 = add(t1, t3);
    p1 = add(t0, t3);
    p2 = add(t1, t2);
    let p5 = mul(add(p3, p4), C1_175875602);
    t0 = mul(t0, C0_298631336);
    t1 = mul(t1, C2_053119869);
    t2 = mul(t2, C3_072711026);
    t3 = mul(t3, C1_501321110);
    p1 = add(p5, mul(p1, CN0_899976223));
    p2 = add(p5, mul(p2, CN2_562915447));
    p3 = mul(p3, CN1_961570560);
    p4 = mul(p4, CN0_390180644);
    t3 = add(t3, add(p1, p4));
    t2 = add(t2, add(p2, p3));
    t1 = add(t1, add(p2, p4));
    t0 = add(t0, add(p1, p3));
    [x0, x1, x2, x3, t0, t1, t2, t3]
}

/// `stbi__idct_block`
fn idct_block(out: &mut [u8], out_stride: usize, data: &[i16; 64]) {
    let mut val = [0i32; 64];
    let add = i32::wrapping_add;
    let sub = i32::wrapping_sub;

    // columns
    for i in 0..8 {
        let d = |row: usize| i32::from(data[i + row * 8]);
        // if all zeroes, shortcut -- this avoids dequantizing 0s and IDCTing
        if d(1) == 0 && d(2) == 0 && d(3) == 0 && d(4) == 0 && d(5) == 0 && d(6) == 0 && d(7) == 0 {
            //    no shortcut                 0     seconds
            //    (1|2|3|4|5|6|7)==0          0     seconds
            //    all separate               -0.047 seconds
            //    1 && 2|3 && 4|5 && 6|7:    -0.047 seconds
            let dcterm = d(0) * 4;
            for row in 0..8 {
                val[i + row * 8] = dcterm;
            }
        } else {
            let [mut x0, mut x1, mut x2, mut x3, t0, t1, t2, t3] =
                idct_1d([d(0), d(1), d(2), d(3), d(4), d(5), d(6), d(7)]);
            // constants scaled things up by 1<<12; let's bring them back
            // down, but keep 2 extra bits of precision
            x0 = add(x0, 512);
            x1 = add(x1, 512);
            x2 = add(x2, 512);
            x3 = add(x3, 512);
            val[i] = add(x0, t3) >> 10;
            val[i + 56] = sub(x0, t3) >> 10;
            val[i + 8] = add(x1, t2) >> 10;
            val[i + 48] = sub(x1, t2) >> 10;
            val[i + 16] = add(x2, t1) >> 10;
            val[i + 40] = sub(x2, t1) >> 10;
            val[i + 24] = add(x3, t0) >> 10;
            val[i + 32] = sub(x3, t0) >> 10;
        }
    }

    for i in 0..8 {
        let v = &val[i * 8..i * 8 + 8];
        let o = &mut out[i * out_stride..i * out_stride + 8];
        // no fast case since the first 1D IDCT spread components out
        let [mut x0, mut x1, mut x2, mut x3, t0, t1, t2, t3] = idct_1d(v.try_into().unwrap());
        // constants scaled things up by 1<<12, plus we had 1<<2 from first
        // loop, plus horizontal and vertical each scale by sqrt(8) so together
        // we've got an extra 1<<3, so 1<<17 total we need to remove.
        // so we want to round that, which means adding 0.5 * 1<<17,
        // aka 65536. Also, we'll end up with -128 to 127 that we want
        // to encode as 0..255 by adding 128, so we'll add that before the shift
        x0 = add(x0, 65536 + (128 << 17));
        x1 = add(x1, 65536 + (128 << 17));
        x2 = add(x2, 65536 + (128 << 17));
        x3 = add(x3, 65536 + (128 << 17));
        // tried computing the shifts into temps, or'ing the temps to see
        // if any were out of range, but that was slower
        o[0] = clamp(add(x0, t3) >> 17);
        o[7] = clamp(sub(x0, t3) >> 17);
        o[1] = clamp(add(x1, t2) >> 17);
        o[6] = clamp(sub(x1, t2) >> 17);
        o[2] = clamp(add(x2, t1) >> 17);
        o[5] = clamp(sub(x2, t1) >> 17);
        o[3] = clamp(add(x3, t0) >> 17);
        o[4] = clamp(sub(x3, t0) >> 17);
    }
}

const STBI_MARKER_NONE: u8 = 0xff;

/// `stbi__get_marker`: if there's a pending marker from the entropy stream, return that
/// otherwise, fetch from the stream and get a marker. if there's no marker, return 0xff,
/// which is never a valid marker value
fn get_marker(j: &mut Entropy, s: &mut Context) -> u8 {
    if j.marker != STBI_MARKER_NONE {
        let x = j.marker;
        j.marker = STBI_MARKER_NONE;
        return x;
    }
    let mut x = s.get8();
    if x != 0xff {
        return STBI_MARKER_NONE;
    }
    while x == 0xff {
        x = s.get8(); // consume repeated 0xff fill bytes
    }
    x
}

// in each scan, we'll have scan_n components, and the order
// of the components is specified by order[]
/// `STBI__RESTART`
fn is_restart(x: u8) -> bool {
    (0xd0..=0xd7).contains(&x)
}

/// `stbi__jpeg_reset`: after a restart interval, `stbi__jpeg_reset` the entropy decoder and the
/// dc prediction
fn jpeg_reset(j: &mut Jpeg) {
    j.e.code_bits = 0;
    j.e.code_buffer = 0;
    j.e.nomore = false;
    for comp in &mut j.img_comp {
        comp.dc_pred = 0;
    }
    j.e.marker = STBI_MARKER_NONE;
    j.todo = if j.restart_interval != 0 {
        j.restart_interval
    } else {
        0x7fff_ffff
    };
    j.e.eob_run = 0;
    // no more than 1<<31 MCUs if no restart_interal? that's plenty safe,
    // since we don't even allow 1<<30 pixels
}

/// The end of each of the loops of `stbi__parse_entropy_coded_data`: counts a data unit
/// against the restart interval. True when the scan is to stop here.
fn count_down(z: &mut Jpeg, s: &mut Context) -> bool {
    z.todo -= 1;
    if z.todo <= 0 {
        if z.e.code_bits < 24 {
            grow_buffer_unsafe(&mut z.e, s);
        }
        // if it's NOT a restart, then just bail, so we get corrupt data
        // rather than no data
        if !is_restart(z.e.marker) {
            return true;
        }
        jpeg_reset(z);
    }
    false
}

/// `stbi__parse_entropy_coded_data`
// (One pass, step for step as stb_image's, its state shared throughout; the names are stb_image's,
// so that the code reads against the original.)
#[allow(clippy::too_many_lines, clippy::many_single_char_names)]
fn parse_entropy_coded_data(z: &mut Jpeg, s: &mut Context) -> Result<(), Failed> {
    jpeg_reset(z);
    if !z.progressive {
        if z.scan_n == 1 {
            let mut data = [0i16; 64];
            let n = z.order[0] as usize;
            // non-interleaved data, we just need to process one block at a time,
            // in trivial scanline order
            // number of blocks to do just depends on how many actual "pixels" this
            // component has, independent of interleaved MCU blocking and such
            let w = (z.img_comp[n].x + 7) >> 3;
            let h = (z.img_comp[n].y + 7) >> 3;
            for j in 0..h {
                for i in 0..w {
                    let comp = &mut z.img_comp[n];
                    let ha = comp.ha as usize;
                    if jpeg_decode_block(
                        &mut z.e,
                        s,
                        &mut data,
                        &BlockTables {
                            hdc: &z.huff_dc[comp.hd as usize],
                            hac: &z.huff_ac[ha],
                            fac: &z.fast_ac[ha],
                            dequant: &z.dequant[comp.tq as usize],
                        },
                        &mut comp.dc_pred,
                    )
                    .is_err()
                    {
                        return Err(Failed);
                    }
                    let w2 = comp.w2 as usize;
                    idct_block(
                        &mut comp.data[w2 * j as usize * 8 + i as usize * 8..],
                        w2,
                        &data,
                    );
                    // every data block is an MCU, so countdown the restart interval
                    if count_down(z, s) {
                        return Ok(());
                    }
                }
            }
            Ok(())
        } else {
            // interleaved
            let mut data = [0i16; 64];
            for j in 0..z.img_mcu_y {
                for i in 0..z.img_mcu_x {
                    // scan an interleaved mcu... process scan_n components in order
                    for k in 0..z.scan_n as usize {
                        let n = z.order[k] as usize;
                        // scan out an mcu's worth of this component; that's just determined
                        // by the basic H and V specified for the component
                        for y in 0..z.img_comp[n].v {
                            for x in 0..z.img_comp[n].h {
                                let comp = &mut z.img_comp[n];
                                let x2 = ((i * comp.h + x) * 8) as usize;
                                let y2 = ((j * comp.v + y) * 8) as usize;
                                let ha = comp.ha as usize;
                                if jpeg_decode_block(
                                    &mut z.e,
                                    s,
                                    &mut data,
                                    &BlockTables {
                                        hdc: &z.huff_dc[comp.hd as usize],
                                        hac: &z.huff_ac[ha],
                                        fac: &z.fast_ac[ha],
                                        dequant: &z.dequant[comp.tq as usize],
                                    },
                                    &mut comp.dc_pred,
                                )
                                .is_err()
                                {
                                    return Err(Failed);
                                }
                                let w2 = comp.w2 as usize;
                                idct_block(&mut comp.data[w2 * y2 + x2..], w2, &data);
                            }
                        }
                    }
                    // after all interleaved components, that's an interleaved MCU,
                    // so now count down the restart interval
                    if count_down(z, s) {
                        return Ok(());
                    }
                }
            }
            Ok(())
        }
    } else if z.scan_n == 1 {
        let n = z.order[0] as usize;
        // non-interleaved data, we just need to process one block at a time,
        // in trivial scanline order
        // number of blocks to do just depends on how many actual "pixels" this
        // component has, independent of interleaved MCU blocking and such
        let w = (z.img_comp[n].x + 7) >> 3;
        let h = (z.img_comp[n].y + 7) >> 3;
        for j in 0..h {
            for i in 0..w {
                let comp = &mut z.img_comp[n];
                let at = 64 * (i + j * comp.coeff_w) as usize;
                let data = &mut comp.coeff[at..at + 64];
                // if we're out of data, just skip decoding this MCU
                // progressive coeffs got initialized at setup so this is fine
                if !z.e.nomore {
                    if z.e.spec_start == 0 {
                        jpeg_decode_block_prog_dc(
                            &mut z.e,
                            s,
                            data,
                            &z.huff_dc[comp.hd as usize],
                            &mut comp.dc_pred,
                        )?;
                    } else {
                        let ha = comp.ha as usize;
                        jpeg_decode_block_prog_ac(
                            &mut z.e,
                            s,
                            data,
                            &z.huff_ac[ha],
                            &z.fast_ac[ha],
                        )?;
                    }
                }
                // every data block is an MCU, so countdown the restart interval
                if count_down(z, s) {
                    return Ok(());
                }
            }
        }
        Ok(())
    } else {
        // interleaved
        for j in 0..z.img_mcu_y {
            for i in 0..z.img_mcu_x {
                // scan an interleaved mcu... process scan_n components in order
                for k in 0..z.scan_n as usize {
                    let n = z.order[k] as usize;
                    // scan out an mcu's worth of this component; that's just determined
                    // by the basic H and V specified for the component
                    for y in 0..z.img_comp[n].v {
                        for x in 0..z.img_comp[n].h {
                            let comp = &mut z.img_comp[n];
                            let x2 = i * comp.h + x;
                            let y2 = j * comp.v + y;
                            let at = 64 * (x2 + y2 * comp.coeff_w) as usize;
                            let data = &mut comp.coeff[at..at + 64];
                            jpeg_decode_block_prog_dc(
                                &mut z.e,
                                s,
                                data,
                                &z.huff_dc[comp.hd as usize],
                                &mut comp.dc_pred,
                            )?;
                        }
                    }
                }
                // after all interleaved components, that's an interleaved MCU,
                // so now count down the restart interval
                if count_down(z, s) {
                    return Ok(());
                }
            }
        }
        Ok(())
    }
}

/// `stbi__jpeg_dequantize`
fn jpeg_dequantize(data: &mut [i16; 64], dequant: &[u16; 64]) {
    for i in 0..64 {
        data[i] = i32::from(data[i]).wrapping_mul(i32::from(dequant[i])) as i16;
    }
}

/// `stbi__jpeg_finish`
fn jpeg_finish(z: &mut Jpeg, s: &Context) {
    if z.progressive {
        // dequantize and idct the data
        for n in 0..s.img_n as usize {
            let comp = &mut z.img_comp[n];
            let w = (comp.x + 7) >> 3;
            let h = (comp.y + 7) >> 3;
            let w2 = comp.w2 as usize;
            for j in 0..h {
                for i in 0..w {
                    let at = 64 * (i + j * comp.coeff_w) as usize;
                    let mut data = [0i16; 64];
                    data.copy_from_slice(&comp.coeff[at..at + 64]);
                    jpeg_dequantize(&mut data, &z.dequant[comp.tq as usize]);
                    idct_block(
                        &mut comp.data[w2 * j as usize * 8 + i as usize * 8..],
                        w2,
                        &data,
                    );
                }
            }
        }
    }
}

/// `stbi__process_marker`
// (One pass, step for step as stb_image's, its state shared throughout; the names are stb_image's,
// so that the code reads against the original.)
#[allow(clippy::too_many_lines, clippy::many_single_char_names)]
fn process_marker(z: &mut Jpeg, s: &mut Context, m: u8) -> Result<(), Failed> {
    match m {
        STBI_MARKER_NONE => return s.fail("expected marker"), // no marker found

        0xDD => {
            // DRI - specify restart interval
            if s.get16be() != 4 {
                return s.fail("bad DRI len");
            }
            z.restart_interval = s.get16be();
            return Ok(());
        }

        0xDB => {
            // DQT - define quantization table
            let mut l = s.get16be() - 2;
            while l > 0 {
                let q = i32::from(s.get8());
                let p = q >> 4;
                let sixteen = p != 0;
                let t = (q & 15) as usize;
                if p != 0 && p != 1 {
                    return s.fail("bad DQT type");
                }
                if t > 3 {
                    return s.fail("bad DQT table");
                }

                for &at in &STBI_JPEG_DEZIGZAG[..64] {
                    z.dequant[t][usize::from(at)] = if sixteen {
                        s.get16be() as u16
                    } else {
                        u16::from(s.get8())
                    };
                }
                l -= if sixteen { 129 } else { 65 };
            }
            return if l == 0 { Ok(()) } else { Err(Failed) };
        }

        0xC4 => {
            // DHT - define huffman table
            let mut l = s.get16be() - 2;
            while l > 0 {
                let mut sizes = [0i32; 16];
                let mut n = 0;
                let q = i32::from(s.get8());
                let tc = q >> 4;
                let th = (q & 15) as usize;
                if tc > 1 || th > 3 {
                    return s.fail("bad DHT header");
                }
                for size in &mut sizes {
                    *size = i32::from(s.get8());
                    n += *size;
                }
                if n > 256 {
                    // Loop over i < n would write past end of values!
                    return s.fail("bad DHT header");
                }
                l -= 17;
                let table = if tc == 0 {
                    &mut z.huff_dc[th]
                } else {
                    &mut z.huff_ac[th]
                };
                build_huffman(s, table, &sizes)?;
                for value in &mut table.values[..n as usize] {
                    *value = s.get8();
                }
                if tc != 0 {
                    build_fast_ac(&mut z.fast_ac[th], &z.huff_ac[th]);
                }
                l -= n;
            }
            return if l == 0 { Ok(()) } else { Err(Failed) };
        }
        _ => {}
    }

    // check for comment block or APP blocks
    if (0xE0..=0xEF).contains(&m) || m == 0xFE {
        let mut l = s.get16be();
        if l < 2 {
            if m == 0xFE {
                return s.fail("bad COM len");
            }
            return s.fail("bad APP len");
        }
        l -= 2;

        if m == 0xE0 && l >= 5 {
            // JFIF APP0 segment
            static TAG: [u8; 5] = [b'J', b'F', b'I', b'F', 0];
            let mut ok = true;
            for byte in TAG {
                if s.get8() != byte {
                    ok = false;
                }
            }
            l -= 5;
            if ok {
                z.jfif = true;
            }
        } else if m == 0xEE && l >= 12 {
            // Adobe APP14 segment
            static TAG: [u8; 6] = [b'A', b'd', b'o', b'b', b'e', 0];
            let mut ok = true;
            for byte in TAG {
                if s.get8() != byte {
                    ok = false;
                }
            }
            l -= 6;
            if ok {
                s.get8(); // version
                s.get16be(); // flags0
                s.get16be(); // flags1
                z.app14_color_transform = i32::from(s.get8()); // color transform
                l -= 6;
            }
        }

        s.skip(l);
        return Ok(());
    }

    s.fail("unknown marker")
}

/// `stbi__process_scan_header`: after we see SOS
fn process_scan_header(z: &mut Jpeg, s: &mut Context) -> Result<(), Failed> {
    let ls = s.get16be();
    z.scan_n = i32::from(s.get8());
    if z.scan_n < 1 || z.scan_n > 4 || z.scan_n > s.img_n {
        return s.fail("bad SOS component count");
    }
    if ls != 6 + 2 * z.scan_n {
        return s.fail("bad SOS len");
    }
    for i in 0..z.scan_n as usize {
        let id = i32::from(s.get8());
        let q = i32::from(s.get8());
        let mut which = 0;
        while which < s.img_n as usize {
            if z.img_comp[which].id == id {
                break;
            }
            which += 1;
        }
        if which == s.img_n as usize {
            return Err(Failed); // no match
        }
        z.img_comp[which].hd = q >> 4;
        if z.img_comp[which].hd > 3 {
            return s.fail("bad DC huff");
        }
        z.img_comp[which].ha = q & 15;
        if z.img_comp[which].ha > 3 {
            return s.fail("bad AC huff");
        }
        z.order[i] = which as i32;
    }

    {
        z.e.spec_start = i32::from(s.get8());
        z.e.spec_end = i32::from(s.get8()); // should be 63, but might be 0
        let aa = i32::from(s.get8());
        z.e.succ_high = aa >> 4;
        z.e.succ_low = aa & 15;
        if z.progressive {
            if z.e.spec_start > 63
                || z.e.spec_end > 63
                || z.e.spec_start > z.e.spec_end
                || z.e.succ_high > 13
                || z.e.succ_low > 13
            {
                return s.fail("bad SOS");
            }
        } else {
            if z.e.spec_start != 0 {
                return s.fail("bad SOS");
            }
            if z.e.succ_high != 0 || z.e.succ_low != 0 {
                return s.fail("bad SOS");
            }
            z.e.spec_end = 63;
        }
    }

    Ok(())
}

/// `stbi__process_frame_header`
// (One pass, step for step as stb_image's, its state shared throughout; the names are stb_image's,
// so that the code reads against the original.)
#[allow(clippy::too_many_lines, clippy::many_single_char_names)]
fn process_frame_header(z: &mut Jpeg, s: &mut Context, scan: Scan) -> Result<(), Failed> {
    let (mut h_max, mut v_max) = (1, 1);
    let lf = s.get16be();
    if lf < 11 {
        return s.fail("bad SOF len"); // JPEG
    }
    let p = s.get8();
    if p != 8 {
        return s.fail("only 8-bit"); // JPEG baseline
    }
    s.img_y = s.get16be() as u32;
    if s.img_y == 0 {
        return s.fail("no header height"); // Legal, but we don't handle it--but neither does IJG
    }
    s.img_x = s.get16be() as u32;
    if s.img_x == 0 {
        return s.fail("0 width"); // JPEG requires
    }
    if s.img_y > STBI_MAX_DIMENSIONS {
        return s.fail("too large");
    }
    if s.img_x > STBI_MAX_DIMENSIONS {
        return s.fail("too large");
    }
    let c = i32::from(s.get8());
    if c != 3 && c != 1 && c != 4 {
        return s.fail("bad component count");
    }
    s.img_n = c;
    for comp in z.img_comp.iter_mut().take(c as usize) {
        comp.data = Vec::new();
        comp.linebuf = Vec::new();
    }

    if lf != 8 + 3 * s.img_n {
        return s.fail("bad SOF len");
    }

    z.rgb = 0;
    let n = s.img_n as usize;
    for (i, comp) in z.img_comp[..n].iter_mut().enumerate() {
        comp.id = i32::from(s.get8());
        if n == 3 && comp.id == i32::from(b"RGB"[i]) {
            z.rgb += 1;
        }
        let q = i32::from(s.get8());
        comp.h = q >> 4;
        if comp.h == 0 || comp.h > 4 {
            return s.fail("bad H");
        }
        comp.v = q & 15;
        if comp.v == 0 || comp.v > 4 {
            return s.fail("bad V");
        }
        comp.tq = i32::from(s.get8());
        if comp.tq > 3 {
            return s.fail("bad TQ");
        }
    }

    if scan != Scan::Load {
        return Ok(());
    }

    if !mad3sizes_valid(s.img_x as i32, s.img_y as i32, s.img_n, 0) {
        return s.fail("too large");
    }

    for comp in &z.img_comp[..s.img_n as usize] {
        h_max = h_max.max(comp.h);
        v_max = v_max.max(comp.v);
    }

    // check that plane subsampling factors are integer ratios;
    // our resamplers can't deal with fractional ratios
    // and I've never seen a non-corrupted JPEG file actually use them
    for comp in &z.img_comp[..s.img_n as usize] {
        if h_max % comp.h != 0 {
            return s.fail("bad H");
        }
        if v_max % comp.v != 0 {
            return s.fail("bad V");
        }
    }

    // compute interleaved mcu info
    z.img_h_max = h_max;
    z.img_v_max = v_max;
    z.img_mcu_w = h_max * 8;
    z.img_mcu_h = v_max * 8;
    // these sizes can't be more than 17 bits
    z.img_mcu_x = s.img_x.div_ceil(z.img_mcu_w as u32) as i32;
    z.img_mcu_y = s.img_y.div_ceil(z.img_mcu_h as u32) as i32;

    for i in 0..s.img_n as usize {
        let comp = &mut z.img_comp[i];
        // number of effective pixels (e.g. for non-interleaved MCU)
        comp.x = (s.img_x * comp.h as u32).div_ceil(h_max as u32) as i32;
        comp.y = (s.img_y * comp.v as u32).div_ceil(v_max as u32) as i32;
        // to simplify generation, we'll allocate enough memory to decode
        // the bogus oversized data from using interleaved MCUs and their
        // big blocks (e.g. a 16x16 iMCU on an image of width 33); we won't
        // discard the extra data until colorspace conversion
        //
        // img_mcu_x, img_mcu_y: <=17 bits; comp[i].h and .v are <=4 (checked earlier)
        // so these muls can't overflow with 32-bit ints (which we require)
        comp.w2 = z.img_mcu_x * comp.h * 8;
        comp.h2 = z.img_mcu_y * comp.v * 8;
        comp.coeff = Vec::new();
        comp.linebuf = Vec::new();
        // (The original has no memory for a size that does not fit an int.)
        if !mad2sizes_valid(comp.w2, comp.h2, 15) {
            return s.fail("outofmem");
        }
        // (What the original leaves as it finds it, and the port has as zeros.)
        comp.data = vec![0; comp.w2 as usize * comp.h2 as usize];
        if z.progressive {
            // w2, h2 are multiples of 8 (see above)
            comp.coeff_w = comp.w2 / 8;
            // (The original checks that twice the size, for values of two bytes, fits an
            // int, and has no memory when it does not.)
            if !mad3sizes_valid(comp.w2, comp.h2, 2, 15) {
                return s.fail("outofmem");
            }
            // 0 all the coeffs now so they're guaranteed to be initialized
            comp.coeff = vec![0; comp.w2 as usize * comp.h2 as usize];
        }
    }

    Ok(())
}

// use comparisons since in some cases we handle more than one case (e.g. SOF)
fn is_dnl(x: u8) -> bool {
    x == 0xdc
}
fn is_soi(x: u8) -> bool {
    x == 0xd8
}
fn is_eoi(x: u8) -> bool {
    x == 0xd9
}
fn is_sof(x: u8) -> bool {
    x == 0xc0 || x == 0xc1 || x == 0xc2
}
fn is_sos(x: u8) -> bool {
    x == 0xda
}
fn is_sof_progressive(x: u8) -> bool {
    x == 0xc2
}

/// `stbi__decode_jpeg_header`
fn decode_jpeg_header(z: &mut Jpeg, s: &mut Context, scan: Scan) -> Result<(), Failed> {
    z.jfif = false;
    z.app14_color_transform = -1; // valid values are 0,1,2
    z.e.marker = STBI_MARKER_NONE; // initialize cached marker to empty
    let mut m = get_marker(&mut z.e, s);
    if !is_soi(m) {
        return s.fail("no SOI");
    }
    if scan == Scan::Type {
        return Ok(());
    }
    m = get_marker(&mut z.e, s);
    while !is_sof(m) {
        process_marker(z, s, m)?;
        m = get_marker(&mut z.e, s);
        while m == STBI_MARKER_NONE {
            // some files have extra padding after their blocks, so ok, we'll scan
            if s.at_eof() {
                return s.fail("no SOF");
            }
            m = get_marker(&mut z.e, s);
        }
    }
    z.progressive = is_sof_progressive(m);
    process_frame_header(z, s, scan)
}

/// `stbi__skip_jpeg_junk_at_end`
fn skip_jpeg_junk_at_end(s: &mut Context) -> u8 {
    // some JPEGs have junk at end, skip over it but if we find what looks
    // like a valid marker, resume there
    while !s.at_eof() {
        let mut x = s.get8();
        while x == 0xff {
            // might be a marker
            if s.at_eof() {
                return STBI_MARKER_NONE;
            }
            x = s.get8();
            if x != 0x00 && x != 0xff {
                // not a stuffed zero or lead-in to another marker, looks
                // like an actual marker, return it
                return x;
            }
            // stuffed zero has x=0 now which ends the loop, meaning we go
            // back to regular scan loop.
            // repeated 0xff keeps trying to read the next byte of the marker.
        }
    }
    STBI_MARKER_NONE
}

/// `stbi__decode_jpeg_image`: decode image to YCbCr format
fn decode_jpeg_image(j: &mut Jpeg, s: &mut Context) -> Result<(), Failed> {
    for comp in &mut j.img_comp {
        comp.data = Vec::new();
        comp.coeff = Vec::new();
    }
    j.restart_interval = 0;
    decode_jpeg_header(j, s, Scan::Load)?;
    let mut m = get_marker(&mut j.e, s);
    while !is_eoi(m) {
        if is_sos(m) {
            process_scan_header(j, s)?;
            parse_entropy_coded_data(j, s)?;
            if j.e.marker == STBI_MARKER_NONE {
                j.e.marker = skip_jpeg_junk_at_end(s);
                // if we reach eof without hitting a marker, stbi__get_marker()
                // below will fail and we'll eventually return 0
            }
            m = get_marker(&mut j.e, s);
            if is_restart(m) {
                m = get_marker(&mut j.e, s);
            }
        } else if is_dnl(m) {
            let ld = s.get16be();
            let nl = s.get16be() as u32;
            if ld != 4 {
                return s.fail("bad DNL len");
            }
            if nl != s.img_y {
                return s.fail("bad DNL height");
            }
            m = get_marker(&mut j.e, s);
        } else {
            process_marker(j, s, m)?;
            m = get_marker(&mut j.e, s);
        }
    }
    if j.progressive {
        jpeg_finish(j, s);
    }
    Ok(())
}

// static jfif-centered resampling (across block boundaries)

/// `stbi__div4`
fn div4(x: i32) -> u8 {
    (x >> 2) as u8
}

/// `stbi__resample_row_v_2`
fn resample_row_v_2(out: &mut [u8], in_near: &[u8], in_far: &[u8], w: usize) {
    // need to generate two samples vertically for every one in input
    for i in 0..w {
        out[i] = div4(3 * i32::from(in_near[i]) + i32::from(in_far[i]) + 2);
    }
}

/// `stbi__resample_row_h_2`
fn resample_row_h_2(out: &mut [u8], input: &[u8], w: usize) {
    // need to generate two samples horizontally for every one in input
    let int = |i: usize| i32::from(input[i]);

    if w == 1 {
        // if only one sample, can't do any interpolation
        out[0] = input[0];
        out[1] = input[0];
        return;
    }

    out[0] = input[0];
    out[1] = div4(int(0) * 3 + int(1) + 2);
    for i in 1..w - 1 {
        let n = 3 * int(i) + 2;
        out[i * 2] = div4(n + int(i - 1));
        out[i * 2 + 1] = div4(n + int(i + 1));
    }
    let i = w - 1;
    out[i * 2] = div4(int(w - 2) * 3 + int(w - 1) + 2);
    out[i * 2 + 1] = input[w - 1];
}

/// `stbi__div16`
fn div16(x: i32) -> u8 {
    (x >> 4) as u8
}

/// `stbi__resample_row_hv_2`
fn resample_row_hv_2(out: &mut [u8], in_near: &[u8], in_far: &[u8], w: usize) {
    // need to generate 2x2 samples for every one in input
    if w == 1 {
        out[0] = div4(3 * i32::from(in_near[0]) + i32::from(in_far[0]) + 2);
        out[1] = out[0];
        return;
    }

    let mut t1 = 3 * i32::from(in_near[0]) + i32::from(in_far[0]);
    out[0] = div4(t1 + 2);
    for i in 1..w {
        let t0 = t1;
        t1 = 3 * i32::from(in_near[i]) + i32::from(in_far[i]);
        out[i * 2 - 1] = div16(3 * t0 + t1 + 8);
        out[i * 2] = div16(3 * t1 + t0 + 8);
    }
    out[w * 2 - 1] = div4(t1 + 2);
}

/// `stbi__resample_row_generic`
fn resample_row_generic(out: &mut [u8], in_near: &[u8], w: usize, hs: usize) {
    // resample with nearest-neighbor
    for i in 0..w {
        for j in 0..hs {
            out[i * hs + j] = in_near[i];
        }
    }
}

// this is a reduced-precision calculation of YCbCr-to-RGB introduced
// to make sure the code produces the same results in both SIMD and scalar
/// `stbi__float2fixed`
const fn float2fixed(x: f32) -> i32 {
    ((x * 4096.0 + 0.5) as i32) << 8
}

/// `stbi__YCbCr_to_RGB_row`
fn ycbcr_to_rgb_row(out: &mut [u8], y: &[u8], pcb: &[u8], pcr: &[u8], count: usize, step: usize) {
    let pixels = y[..count].iter().zip(&pcb[..count]).zip(&pcr[..count]);
    let rgb = |((&y, &cb), &cr): ((&u8, &u8), &u8)| {
        let y_fixed = (i32::from(y) << 20) + (1 << 19); // rounding
        let cr = i32::from(cr) - 128;
        let cb = i32::from(cb) - 128;
        let r = y_fixed + cr * float2fixed(1.40200);
        let g = y_fixed
            + (cr * -float2fixed(0.71414))
            + ((cb * -float2fixed(0.34414)) & 0xffff_0000_u32 as i32);
        let b = y_fixed + cb * float2fixed(1.77200);
        [r, g, b].map(|v| (v >> 20).clamp(0, 255) as u8)
    };
    if step == 4 {
        for (out, pixel) in out.as_chunks_mut::<4>().0.iter_mut().zip(pixels) {
            let [r, g, b] = rgb(pixel);
            *out = [r, g, b, 255];
        }
    } else {
        // (The original writes a fourth byte here too, which the next pixel overwrites.)
        debug_assert_eq!(step, 3);
        for (out, pixel) in out.as_chunks_mut::<3>().0.iter_mut().zip(pixels) {
            *out = rgb(pixel);
        }
    }
}

/// How a component is brought up to the image's size: the `resample` of `stbi__resample`.
#[derive(Clone, Copy, Default, PartialEq)]
enum Resample {
    #[default]
    Row1,
    V2,
    H2,
    Hv2,
    Generic,
}

/// `stbi__resample`
#[derive(Clone, Copy, Default)]
struct ResampleState {
    resample: Resample,
    /// (Where in the component's data the two rows begin.)
    line0: usize,
    line1: usize,
    hs: i32,
    vs: i32,      // expansion factor in each axis
    w_lores: i32, // horizontal pixels pre-expansion
    ystep: i32,   // how far through vertical expansion we are
    ypos: i32,    // which pre-expansion row we're on
}

/// `stbi__blinn_8x8`: fast 0..255 * 0..255 => 0..255 rounded multiplication
fn blinn_8x8(x: u8, y: u8) -> u8 {
    let t = u32::from(x) * u32::from(y) + 128;
    ((t + (t >> 8)) >> 8) as u8
}

/// `load_jpeg_image`: the pixels and how many channels the file has.
// (One pass, step for step as stb_image's, its state shared throughout; the names are stb_image's,
// so that the code reads against the original.)
#[allow(clippy::too_many_lines, clippy::many_single_char_names)]
fn load_jpeg_image(z: &mut Jpeg, s: &mut Context, req_comp: i32) -> Option<(Vec<u8>, i32)> {
    s.img_n = 0; // make stbi__cleanup_jpeg safe

    // validate req_comp
    if !(0..=4).contains(&req_comp) {
        return s.errp("bad req_comp");
    }

    // load a jpeg image from whichever source, but leave in YCbCr format
    if decode_jpeg_image(z, s).is_err() {
        return None;
    }

    // determine actual number of components to generate
    let n = if req_comp != 0 {
        req_comp
    } else if s.img_n >= 3 {
        3
    } else {
        1
    } as usize;

    let is_rgb = s.img_n == 3 && (z.rgb == 3 || (z.app14_color_transform == 0 && !z.jfif));

    let decode_n = if s.img_n == 3 && n < 3 && !is_rgb {
        1
    } else {
        s.img_n as usize
    };

    // nothing to do if no components requested; check this now to avoid
    // accessing uninitialized coutput[0] later
    if decode_n == 0 {
        return None;
    }

    // resample and color-convert
    let img_x = s.img_x as usize;
    let mut res_comp = [ResampleState::default(); 4];

    let (h_max, v_max) = (z.img_h_max, z.img_v_max);
    for (r, comp) in res_comp.iter_mut().zip(&mut z.img_comp).take(decode_n) {
        // allocate line buffer big enough for upsampling off the edges
        // with upsample factor of 4
        comp.linebuf = vec![0; img_x + 3];

        r.hs = h_max / comp.h;
        r.vs = v_max / comp.v;
        r.ystep = r.vs >> 1;
        r.w_lores = s.img_x.div_ceil(r.hs as u32) as i32;
        r.ypos = 0;
        r.line0 = 0;
        r.line1 = 0;

        r.resample = match (r.hs, r.vs) {
            (1, 1) => Resample::Row1,
            (1, 2) => Resample::V2,
            (2, 1) => Resample::H2,
            (2, 2) => Resample::Hv2,
            _ => Resample::Generic,
        };
    }

    // can't error after this so, this is safe
    if !mad3sizes_valid(n as i32, s.img_x as i32, s.img_y as i32, 1) {
        return s.errp("outofmem");
    }
    let mut output = vec![0u8; n * img_x * s.img_y as usize + 1];

    // now go ahead and resample
    for j in 0..s.img_y as usize {
        let out = &mut output[n * img_x * j..];
        for (r, comp) in res_comp.iter_mut().zip(&mut z.img_comp).take(decode_n) {
            let y_bot = r.ystep >= (r.vs >> 1);
            let (near, far) = if y_bot {
                (r.line1, r.line0)
            } else {
                (r.line0, r.line1)
            };
            let (in_near, in_far) = (&comp.data[near..], &comp.data[far..]);
            let w = r.w_lores as usize;
            match r.resample {
                Resample::Row1 => comp.linebuf[..img_x].copy_from_slice(&in_near[..img_x]),
                Resample::V2 => resample_row_v_2(&mut comp.linebuf, in_near, in_far, w),
                Resample::H2 => resample_row_h_2(&mut comp.linebuf, in_near, w),
                Resample::Hv2 => resample_row_hv_2(&mut comp.linebuf, in_near, in_far, w),
                Resample::Generic => {
                    resample_row_generic(&mut comp.linebuf, in_near, w, r.hs as usize);
                }
            }
            r.ystep += 1;
            if r.ystep >= r.vs {
                r.ystep = 0;
                r.line0 = r.line1;
                r.ypos += 1;
                if r.ypos < comp.y {
                    r.line1 += comp.w2 as usize;
                }
            }
        }
        let coutput = |k: usize| &z.img_comp[k].linebuf[..];
        if n >= 3 {
            let y = coutput(0);
            if s.img_n == 3 {
                if is_rgb {
                    for i in 0..img_x {
                        let out = &mut out[i * n..];
                        out[0] = y[i];
                        out[1] = coutput(1)[i];
                        out[2] = coutput(2)[i];
                        out[3] = 255;
                    }
                } else {
                    ycbcr_to_rgb_row(out, y, coutput(1), coutput(2), img_x, n);
                }
            } else if s.img_n == 4 {
                if z.app14_color_transform == 0 {
                    // CMYK
                    for i in 0..img_x {
                        let m = coutput(3)[i];
                        let out = &mut out[i * n..];
                        out[0] = blinn_8x8(coutput(0)[i], m);
                        out[1] = blinn_8x8(coutput(1)[i], m);
                        out[2] = blinn_8x8(coutput(2)[i], m);
                        out[3] = 255;
                    }
                } else if z.app14_color_transform == 2 {
                    // YCCK
                    ycbcr_to_rgb_row(out, y, coutput(1), coutput(2), img_x, n);
                    for i in 0..img_x {
                        let m = coutput(3)[i];
                        let out = &mut out[i * n..];
                        out[0] = blinn_8x8(255 - out[0], m);
                        out[1] = blinn_8x8(255 - out[1], m);
                        out[2] = blinn_8x8(255 - out[2], m);
                    }
                } else {
                    // YCbCr + alpha?  Ignore the fourth channel for now
                    ycbcr_to_rgb_row(out, y, coutput(1), coutput(2), img_x, n);
                }
            } else {
                for i in 0..img_x {
                    let out = &mut out[i * n..];
                    out[0] = y[i];
                    out[1] = y[i];
                    out[2] = y[i];
                    out[3] = 255; // not used if n==3
                }
            }
        } else if is_rgb {
            if n == 1 {
                let (r, g, b) = (coutput(0), coutput(1), coutput(2));
                for (((out, &r), &g), &b) in out[..img_x].iter_mut().zip(r).zip(g).zip(b) {
                    *out = compute_y(i32::from(r), i32::from(g), i32::from(b));
                }
            } else {
                for i in 0..img_x {
                    out[i * 2] = compute_y(
                        i32::from(coutput(0)[i]),
                        i32::from(coutput(1)[i]),
                        i32::from(coutput(2)[i]),
                    );
                    out[i * 2 + 1] = 255;
                }
            }
        } else if s.img_n == 4 && z.app14_color_transform == 0 {
            for i in 0..img_x {
                let m = coutput(3)[i];
                let r = blinn_8x8(coutput(0)[i], m);
                let g = blinn_8x8(coutput(1)[i], m);
                let b = blinn_8x8(coutput(2)[i], m);
                out[i * n] = compute_y(i32::from(r), i32::from(g), i32::from(b));
                out[i * n + 1] = 255;
            }
        } else if s.img_n == 4 && z.app14_color_transform == 2 {
            for i in 0..img_x {
                out[i * n] = blinn_8x8(255 - coutput(0)[i], coutput(3)[i]);
                out[i * n + 1] = 255;
            }
        } else {
            let y = coutput(0);
            if n == 1 {
                out[..img_x].copy_from_slice(&y[..img_x]);
            } else {
                for i in 0..img_x {
                    out[i * 2] = y[i];
                    out[i * 2 + 1] = 255;
                }
            }
        }
    }
    // (The byte past the end that the last pixel's unused fourth value went to.)
    output.pop();
    Some((output, if s.img_n >= 3 { 3 } else { 1 })) // report original components, not output
}

/// `stbi__jpeg_load`
pub(crate) fn jpeg_load(s: &mut Context, req_comp: i32) -> Option<(Vec<u8>, i32)> {
    let mut j = Jpeg::new();
    load_jpeg_image(&mut j, s, req_comp)
}

/// `stbi__jpeg_test`
pub(crate) fn jpeg_test(s: &mut Context) -> bool {
    let mut j = Jpeg::new();
    let r = decode_jpeg_header(&mut j, s, Scan::Type).is_ok();
    s.rewind();
    r
}

/// `stbi__jpeg_info`
pub(crate) fn jpeg_info(s: &mut Context) -> Option<RawInfo> {
    let mut j = Jpeg::new();
    // stbi__jpeg_info_raw
    if decode_jpeg_header(&mut j, s, Scan::Header).is_err() {
        s.rewind();
        return None;
    }
    Some(RawInfo {
        kind: ImageFormat::Jpeg,
        x: s.img_x as i32,
        y: s.img_y as i32,
        comp: if s.img_n >= 3 { 3 } else { 1 },
        is_16_bit: false,
        is_hdr: false,
        is_iphone_png: false,
    })
}

#[cfg(test)]
mod tests {
    use super::super::{LoadError, LoadOptions, info, load};
    use super::*;
    use stb_image_write_nrp::{Channels, Image, WriteOptions, write_jpg};

    fn picture(x: usize, y: usize, comp: usize) -> Vec<u8> {
        // Smooth, so that it comes back near what went in.
        (0..x * y * comp)
            .map(|i| (40 + (i / comp % x) * 2 + (i / comp / x) * 2 + i % comp * 20) as u8)
            .collect()
    }

    #[test]
    fn what_the_writer_writes_comes_back_nearly() {
        let options = LoadOptions::default();
        for (x, y) in [(1, 1), (8, 8), (17, 9), (40, 33)] {
            for quality in [90, 100] {
                let pixels = picture(x, y, 3);
                let file = write_jpg(
                    &Image::new(&pixels, x as u32, y as u32, Channels::Rgb).unwrap(),
                    quality,
                    &WriteOptions::default(),
                );
                let info = info(&file).unwrap();
                assert_eq!(
                    (
                        info.format,
                        info.width as i32,
                        info.height as i32,
                        info.channels as i32
                    ),
                    (ImageFormat::Jpeg, x as i32, y as i32, 3)
                );
                let loaded = load(&file, crate::ch(0), &options).unwrap();
                assert_eq!(
                    (
                        loaded.width as i32,
                        loaded.height as i32,
                        loaded.channels_in_file as i32,
                        loaded.data.len()
                    ),
                    (x as i32, y as i32, 3, pixels.len())
                );
                let worst = loaded
                    .data
                    .iter()
                    .zip(&pixels)
                    .map(|(&a, &b)| a.abs_diff(b))
                    .max()
                    .unwrap();
                assert!(worst <= 12, "{x} by {y} at {quality}: off by {worst}");
                // As grey, with alpha, and as four channels.
                let grey = load(&file, crate::ch(1), &options).unwrap();
                assert_eq!(grey.data.len(), x * y);
                let grey_alpha = load(&file, crate::ch(2), &options).unwrap();
                assert!(
                    grey_alpha
                        .data
                        .chunks(2)
                        .zip(&grey.data)
                        .all(|(pair, &value)| pair == [value, 255])
                );
                let rgba = load(&file, crate::ch(4), &options).unwrap();
                assert!(
                    rgba.data
                        .chunks(4)
                        .zip(loaded.data.chunks(3))
                        .all(|(four, three)| &four[..3] == three && four[3] == 255)
                );
            }
        }
        // A grey file.
        let pixels = picture(20, 20, 1);
        let file = write_jpg(
            &Image::new(&pixels, 20, 20, Channels::Grey).unwrap(),
            95,
            &WriteOptions::default(),
        );
        let loaded = load(&file, crate::ch(1), &options).unwrap();
        assert_eq!(loaded.channels_in_file as i32, 3); // (the writer always writes three components)
        assert!(
            loaded
                .data
                .iter()
                .zip(&pixels)
                .all(|(&a, &b)| a.abs_diff(b) <= 6)
        );
    }

    #[test]
    fn the_inverse_transform_of_a_flat_block_is_flat() {
        let mut data = [0i16; 64];
        data[0] = 8 * 50;
        let mut out = [0u8; 64];
        idct_block(&mut out, 8, &data);
        assert_eq!(out, [178; 64]);
        // And values far out of range stop at the ends.
        data[0] = 30000;
        idct_block(&mut out, 8, &data);
        assert_eq!(out, [255; 64]);
        data[0] = -30000;
        idct_block(&mut out, 8, &data);
        assert_eq!(out, [0; 64]);
    }

    #[test]
    fn the_resamplers() {
        let (near, far) = ([10u8, 20, 30, 40], [50u8, 60, 70, 80]);
        let mut out = [0u8; 8];
        resample_row_v_2(&mut out, &near, &far, 4);
        assert_eq!(&out[..4], &[20, 30, 40, 50]);
        resample_row_h_2(&mut out, &near, 4);
        // (The last pair leans on the sample before the last, as the original's does.)
        assert_eq!(out, [10, 13, 18, 23, 28, 33, 33, 40]);
        resample_row_hv_2(&mut out, &near, &far, 4);
        assert_eq!(out, [20, 23, 28, 33, 38, 43, 48, 50]);
        resample_row_generic(&mut out, &near, 2, 4);
        assert_eq!(out, [10, 10, 10, 10, 20, 20, 20, 20]);
        resample_row_h_2(&mut out, &near, 1);
        assert_eq!(&out[..2], &[10, 10]);
    }

    #[test]
    fn damaged_files_are_refused() {
        let options = LoadOptions::default();
        let pixels = picture(24, 24, 3);
        let file = write_jpg(
            &Image::new(&pixels, 24, 24, Channels::Rgb).unwrap(),
            90,
            &WriteOptions::default(),
        );
        let refused = |file: &[u8]| load(file, crate::ch(0), &options).unwrap_err();
        // Cut short anywhere, it is refused.
        assert_eq!(refused(&file[..2]), LoadError::new("expected marker"));
        assert_eq!(
            refused(&file[..file.len() - 2]),
            LoadError::new("expected marker")
        );
        assert!(load(&file[..file.len() / 2], crate::ch(0), &options).is_err());
        // Twelve bits a sample; no width.
        let frame = 25 + 64 + 1 + 64;
        let mut bad = file.clone();
        bad[frame + 4] = 12;
        assert_eq!(refused(&bad), LoadError::new("only 8-bit"));
        let mut bad = file.clone();
        bad[frame + 7] = 0;
        bad[frame + 8] = 0;
        assert_eq!(refused(&bad), LoadError::new("0 width"));
        // A marker there is none of.
        let mut bad = file.clone();
        bad[3] = 0xC8;
        assert_eq!(refused(&bad), LoadError::new("unknown marker"));
    }
}
