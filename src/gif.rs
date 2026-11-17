//! The GIF reader: `stbi__gif_*`, public domain by Jean-Marc Lienher, simplified and shrunk by
//! stb. Every frame of an animation, or the first.

use super::{
    Context, ImageType, Info, STBI_MAX_DIMENSIONS, addints_valid, convert_format, mad3sizes_valid,
    vertical_flip,
};

/// Every frame of a GIF file: [`super::load_gif_from_memory`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoadedGif {
    /// The size of every frame.
    pub x: i32,
    pub y: i32,
    /// How many frames there are.
    pub frames: i32,
    /// How many channels the file has: always 4.
    pub channels_in_file: i32,
    /// How long each frame is shown, in thousandths of a second.
    pub delays: Vec<i32>,
    /// The frames one after another, each `x` by `y` pixels from the top row, of as many values
    /// as were asked for, or of 4 when none were.
    pub data: Vec<u8>,
}

/// Which colour table the frame being read uses.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Table {
    Global,
    Local,
}

/// `stbi__gif`
struct Gif {
    w: i32,
    h: i32,
    out: Vec<u8>, // output buffer (always 4 components)
    flags: i32,
    bgindex: i32,
    transparent: i32,
    eflags: i32,
    color_table: Table,
    parse: i32,
    step: i32,
    lflags: i32,
    start_x: i32,
    start_y: i32,
    max_x: i32,
    max_y: i32,
    cur_x: i32,
    cur_y: i32,
    line_size: i32,
    delay: i32,
    suffix: [u8; 4096 + 1],  // one extra slot as sentinel
    prefix: [u16; 4096 + 1], // one extra slot as sentinel
    pal: [[u8; 3]; 256],
    lpal: [[u8; 3]; 256],
}

/// The one slot past the end of `suffix` and `prefix` the original reaches: with a code size of
/// 12, its first free code is 4098. Where those writes land is the struct's layout on a 64-bit
/// machine: `suffix[4098]` is the low byte of `prefix[0]` (which every raster sets to 0 before
/// it starts), and `prefix[4098]` is the third byte of the global palette and the fourth, which
/// is to say the blue of colour 0 and the red of colour 1.
const PAST_THE_END: usize = 4098;

impl Gif {
    /// `g->prefix[code]`
    fn prefix(&self, code: usize) -> u16 {
        if code == PAST_THE_END {
            u16::from(self.pal[0][2]) | u16::from(self.pal[1][0]) << 8
        } else {
            self.prefix[code]
        }
    }

    /// `g->prefix[code] = value`
    fn set_prefix(&mut self, code: usize, value: u16) {
        if code == PAST_THE_END {
            self.pal[0][2] = value as u8;
            self.pal[1][0] = (value >> 8) as u8;
        } else {
            self.prefix[code] = value;
        }
    }

    /// `g->suffix[code]`
    fn suffix(&self, code: usize) -> u8 {
        if code == PAST_THE_END {
            self.prefix[0] as u8
        } else {
            self.suffix[code]
        }
    }

    /// `g->suffix[code] = value`
    fn set_suffix(&mut self, code: usize, value: u8) {
        if code == PAST_THE_END {
            self.prefix[0] = (self.prefix[0] & 0xff00) | u16::from(value);
        } else {
            self.suffix[code] = value;
        }
    }
}

impl Gif {
    /// All zeros, as the original's `memset`.
    fn new() -> Gif {
        Gif {
            w: 0,
            h: 0,
            out: Vec::new(),
            flags: 0,
            bgindex: 0,
            transparent: 0,
            eflags: 0,
            color_table: Table::Global,
            parse: 0,
            step: 0,
            lflags: 0,
            start_x: 0,
            start_y: 0,
            max_x: 0,
            max_y: 0,
            cur_x: 0,
            cur_y: 0,
            line_size: 0,
            delay: 0,
            suffix: [0; 4096 + 1],
            prefix: [0; 4096 + 1],
            pal: [[0; 3]; 256],
            lpal: [[0; 3]; 256],
        }
    }
}

/// `stbi__gif_test_raw`
fn gif_test_raw(s: &mut Context) -> bool {
    if s.get8() != b'G' || s.get8() != b'I' || s.get8() != b'F' || s.get8() != b'8' {
        return false;
    }
    let sz = s.get8();
    if sz != b'9' && sz != b'7' {
        return false;
    }
    if s.get8() != b'a' {
        return false;
    }
    true
}

/// `stbi__gif_test`
pub(crate) fn gif_test(s: &mut Context) -> bool {
    let r = gif_test_raw(s);
    s.rewind();
    r
}

/// `stbi__gif_parse_colortable`
fn gif_parse_colortable(s: &mut Context, pal: &mut [[u8; 3]; 256], num_entries: i32) {
    for entry in pal.iter_mut().take(num_entries as usize) {
        entry[0] = s.get8();
        entry[1] = s.get8();
        entry[2] = s.get8();
    }
}

/// `stbi__gif_header`
fn gif_header(s: &mut Context, g: &mut Gif, is_info: bool) -> bool {
    if s.get8() != b'G' || s.get8() != b'I' || s.get8() != b'F' || s.get8() != b'8' {
        return s.err("not GIF");
    }

    let version = s.get8();
    if version != b'7' && version != b'9' {
        return s.err("not GIF");
    }
    if s.get8() != b'a' {
        return s.err("not GIF");
    }

    s.reason = "";
    g.w = s.get16le();
    g.h = s.get16le();
    g.flags = i32::from(s.get8());
    g.bgindex = i32::from(s.get8());
    let _ratio = s.get8();
    g.transparent = -1;

    if g.w > STBI_MAX_DIMENSIONS as i32 || g.h > STBI_MAX_DIMENSIONS as i32 {
        return s.err("too large");
    }
    if g.w == 0 || g.h == 0 {
        return s.err("zero pixels");
    }

    // (The channels: always 4, can't actually tell whether it's 3 or 4 until we parse the
    // comments.)

    if is_info {
        return true;
    }

    if g.flags & 0x80 != 0 {
        gif_parse_colortable(s, &mut g.pal, 2 << (g.flags & 7));
    } else {
        g.bgindex = -1; // no global color table -> bg color ignored
    }

    true
}

/// `stbi__gif_info_raw`
fn gif_info_raw(s: &mut Context) -> Option<(i32, i32)> {
    let mut g = Gif::new();
    if !gif_header(s, &mut g, true) {
        s.rewind();
        return None;
    }
    Some((g.w, g.h))
}

/// `stbi__process_gif_raster`: false where the original gives NULL.
fn process_gif_raster(s: &mut Context, g: &mut Gif) -> bool {
    let lzw_cs = s.get8();
    if lzw_cs > 12 {
        return false;
    }
    let clear = 1i32 << lzw_cs;
    let mut first = true;
    let mut codesize = i32::from(lzw_cs) + 1;
    let mut codemask = (1i32 << codesize) - 1;
    let mut bits = 0i32;
    let mut valid_bits = 0i32;
    for init_code in 0..clear as usize {
        g.prefix[init_code] = 0;
        g.suffix[init_code] = init_code as u8;
    }

    // support no starting clear code
    let mut avail = clear + 2;
    let mut oldcode = -1i32;

    let mut revbuf = [0u8; 4096];
    let mut len = 0i32;
    loop {
        while valid_bits < codesize {
            if len == 0 {
                len = i32::from(s.get8()); // start new block
                if len == 0 {
                    return true;
                }
            }
            len -= 1;
            bits |= i32::from(s.get8()) << valid_bits;
            valid_bits += 8;
        }

        let code = bits & codemask;
        bits >>= codesize;
        valid_bits -= codesize;
        // @OPTIMIZE: is there some way we can accelerate the non-clear path?
        if code == clear {
            // clear code
            codesize = i32::from(lzw_cs) + 1;
            codemask = (1 << codesize) - 1;
            avail = clear + 2;
            oldcode = -1;
            first = false;
        } else if code == clear + 1 {
            // end of stream code
            s.skip(len);
            loop {
                len = i32::from(s.get8());
                if len <= 0 {
                    break;
                }
                s.skip(len);
            }
            return true;
        } else if code <= avail {
            if first {
                return s.err("no clear code");
            }

            let mut revind = 4096usize;
            let mut cur = code;
            if code == avail {
                // self-reference case
                if oldcode < 0 {
                    return s.err("illegal code in raster");
                }
                // chase oldcode to find its first character
                cur = oldcode;
                let mut steps = 0;
                while cur >= clear {
                    cur = i32::from(g.prefix(cur as usize));
                    // (With a code size of 12 the slot past the end can be its own prefix, and
                    // the original goes round for ever; here the file is refused.)
                    steps += 1;
                    if steps > 4096 {
                        return s.err("illegal code in raster");
                    }
                }
                // add the first character at the end of revbuf; consumes 1 slot
                revind -= 1;
                revbuf[revind] = cur as u8;
                cur = oldcode;
            }
            // walk linked list of prefixes to emit bytes in reverse order to revbuf
            // safe: longest chain possible can't contain
            // more than 4096 - (clear + 2) <= 4094 bytes,
            // since every code below clear+2 terminates the chain and any code above has a
            // "prefix" link to a lower-numbered code
            while cur >= clear {
                if revind <= 1 {
                    return s.err("illegal code in raster"); // (as above)
                }
                revind -= 1;
                revbuf[revind] = g.suffix(cur as usize);
                cur = i32::from(g.prefix(cur as usize));
            }

            // one final slot in revbuf: still guaranteed to
            // be <=4096 bytes, so revind >= 0 at the end
            revind -= 1;
            revbuf[revind] = cur as u8;
            if oldcode >= 0 {
                g.set_prefix(avail as usize, oldcode as u16);
                g.set_suffix(avail as usize, cur as u8);
                if avail < 4096 {
                    // increment avail unless dict is already full
                    avail += 1;
                    if avail > codemask && avail < 4096 {
                        codesize += 1;
                        codemask = (1 << codesize) - 1;
                    }
                }
            }
            oldcode = code;

            // turn decoded bytes into pixels
            let table = if g.color_table == Table::Local {
                &g.lpal
            } else {
                &g.pal
            };
            while revind < 4096 && g.cur_y < g.max_y {
                let col = revbuf[revind];
                if i32::from(col) != g.transparent {
                    let c = table[col as usize];
                    let p = (g.cur_x + g.cur_y) as usize;
                    g.out[p..p + 4].copy_from_slice(&[c[0], c[1], c[2], 255]);
                }
                g.cur_x += 4;

                if g.cur_x >= g.max_x {
                    g.cur_x = g.start_x;
                    g.cur_y += g.step;

                    while g.cur_y >= g.max_y && g.parse > 0 {
                        g.step = (1 << g.parse) * g.line_size;
                        g.cur_y = g.start_y + (g.step >> 1);
                        g.parse -= 1;
                    }
                }
                revind += 1;
            }
        } else {
            return s.err("illegal code in raster");
        }
    }
}

/// What `stbi__gif_load_next` gives: a frame (in `g.out`), the end of the file, or an error.
enum Next {
    Frame,
    End,
    Failed,
}

/// `stbi__gif_load_next`: this function is designed to support animated gifs, although
/// stb_image doesn't support it. `saved` is the frame to go back to, for a very specific
/// disposal format.
fn gif_load_next(s: &mut Context, g: &mut Gif, saved: Option<&[u8]>) -> Next {
    // on first frame, any non-written pixels get the background colour (non-transparent)
    let mut first_frame = false;
    if g.out.is_empty() {
        if !gif_header(s, g, false) {
            return Next::Failed; // stbi__g_failure_reason set by stbi__gif_header
        }
        if !mad3sizes_valid(4, g.w, g.h, 0) {
            s.err("too large");
            return Next::Failed;
        }
        g.out = vec![0; 4 * g.w as usize * g.h as usize]; // clear to transparent
        first_frame = true;
    } else {
        // second frame - how do we dispose of the previous one?
        let mut dispose = (g.eflags & 0x1C) >> 2;
        if dispose == 3 && saved.is_none() {
            // if I don't have an image to revert back to, default to the old background
            dispose = 2;
        }

        if dispose == 3 {
            // use previous graphic
            let saved = saved.unwrap_or_default();
            let mut y = g.start_y;
            while y < g.max_y {
                let (from, to) = ((y + g.start_x) as usize, (y + g.max_x) as usize);
                g.out[from..to].copy_from_slice(&saved[from..to]);
                y += g.line_size;
            }
        } else if dispose == 2 {
            // use background color
            let mut y = g.start_y;
            while y < g.max_y {
                let mut bg = [0u8; 4];
                if g.bgindex >= 0 && g.transparent < 0 {
                    bg[..3].copy_from_slice(&g.pal[g.bgindex as usize]);
                    bg[3] = 255;
                }
                for x in g.start_x..g.max_x {
                    g.out[(y + x) as usize] = bg[(x & 3) as usize];
                }
                y += g.line_size;
            }
        } else {
            // This is a non-disposal case either way, just leave the pixels as they are.
        }
    }

    loop {
        let tag = s.get8();
        match tag {
            0x2C => {
                /* Image Descriptor */
                let x = s.get16le();
                let y = s.get16le();
                let w = s.get16le();
                let h = s.get16le();
                if x + w > g.w || y + h > g.h {
                    s.err("bad Image Descriptor");
                    return Next::Failed;
                }

                g.line_size = g.w * 4;
                g.start_x = x * 4;
                g.start_y = y * g.line_size;
                g.max_x = g.start_x + w * 4;
                g.max_y = g.start_y + h * g.line_size;
                g.cur_x = g.start_x;
                g.cur_y = g.start_y;

                // if the width of the specified rectangle is 0, that means
                // we may not see *any* pixels or the image is malformed;
                // to make sure this is caught, move the current y down to
                // max_y (which is what out_gif_code checks).
                if w == 0 {
                    g.cur_y = g.max_y;
                }

                g.lflags = i32::from(s.get8());

                if g.lflags & 0x40 != 0 {
                    g.step = 8 * g.line_size; // first interlaced spacing
                    g.parse = 3;
                } else {
                    g.step = g.line_size;
                    g.parse = 0;
                }

                if g.lflags & 0x80 != 0 {
                    gif_parse_colortable(s, &mut g.lpal, 2 << (g.lflags & 7));
                    g.color_table = Table::Local;
                } else if g.flags & 0x80 != 0 {
                    g.color_table = Table::Global;
                } else {
                    s.err("missing color table");
                    return Next::Failed;
                }

                // if we have a background color set and no transparent
                // color, first frame should clear to background
                // first frame needs to initialize the image
                if first_frame && g.bgindex >= 0 && g.transparent < 0 {
                    let mut bg = [0u8, 0, 0, 255];
                    bg[..3].copy_from_slice(&g.pal[g.bgindex as usize]);
                    for (x, value) in g.out[..(g.h * g.line_size) as usize].iter_mut().enumerate() {
                        *value = bg[x & 3];
                    }
                }

                return if process_gif_raster(s, g) {
                    Next::Frame
                } else {
                    Next::Failed
                };
            }

            0x21 => {
                // Comment Extension.
                let ext = s.get8();
                if ext == 0xF9 {
                    // Graphic Control Extension.
                    let len = i32::from(s.get8());
                    if len == 4 {
                        g.eflags = i32::from(s.get8());
                        // delay - 1/100th of a second, saving as 1/1000ths.
                        g.delay = 10 * s.get16le();

                        g.transparent = i32::from(s.get8());
                        if g.eflags & 0x01 == 0 {
                            g.transparent = -1;
                        }
                    } else {
                        s.skip(len);
                        continue;
                    }
                }
                loop {
                    let len = i32::from(s.get8());
                    if len == 0 {
                        break;
                    }
                    s.skip(len);
                }
            }

            0x3B => {
                // gif stream termination code
                return Next::End;
            }

            _ => {
                s.err("unknown code");
                return Next::Failed;
            }
        }
    }
}

/// `stbi__load_gif_main`
pub(crate) fn load_gif_main(s: &mut Context, req_comp: i32) -> Option<LoadedGif> {
    if !gif_test(s) {
        return s.errp("not GIF");
    }

    let mut g = Gif::new();
    let mut ok = true;
    let mut totalbytes = 0i32;
    let mut frames: Vec<(i32, Vec<u8>)> = Vec::new();
    let mut saved: Option<usize> = None;
    let (mut x, mut y) = (0, 0);

    loop {
        match gif_load_next(s, &mut g, saved.map(|frame| frames[frame].1.as_slice())) {
            Next::Failed => {
                ok = false;
                break;
            }
            Next::End => break, // end of animated gif marker
            Next::Frame => {}
        }

        x = g.w;
        y = g.h;
        let stride = g.w * g.h * 4;

        // we ensure g.w > 0, g.h > 0; that means totalbytes > 0 and this check
        // also guarantees that layers doesn't overflow
        if !addints_valid(totalbytes, stride) {
            ok = s.err("toobig");
            break;
        }
        totalbytes += stride;

        frames.push((g.delay, g.out[..stride as usize].to_vec()));

        // if this frame is not in "restore to previous" disposal mode (which "un-paints" it),
        // it becomes the next reference frame
        if (g.eflags & 0x1c) != 0x0c {
            saved = Some(frames.len() - 1);
        }
    }

    if !ok {
        return None;
    }

    let layers = frames.len() as i32;
    let mut out = Vec::with_capacity(totalbytes as usize);
    let mut delays = Vec::with_capacity(frames.len());
    for (delay, pixels) in frames {
        delays.push(delay);
        let at = out.len();
        out.extend_from_slice(&pixels);
        if s.options.flip_vertically_on_load {
            vertical_flip(&mut out[at..], g.w as usize, g.h as usize, 4);
        }
    }

    // do the final conversion after loading everything
    if req_comp != 0 && req_comp != 4 {
        out = convert_format(s, out, 4, req_comp, g.w as u32, (layers * g.h) as u32)?;
    }

    let channels_in_file = if g.w != 0 { 4 } else { 0 };
    Some(LoadedGif {
        x,
        y,
        frames: layers,
        channels_in_file,
        delays,
        data: out,
    })
}

/// `stbi__gif_load`: the first frame; the size goes into the context.
pub(crate) fn gif_load(s: &mut Context, req_comp: i32) -> Option<(Vec<u8>, i32)> {
    let mut g = Gif::new();
    if !matches!(gif_load_next(s, &mut g, None), Next::Frame) {
        return None; // (or the end of the file before any frame)
    }
    s.img_x = g.w as u32;
    s.img_y = g.h as u32;

    // moved conversion to after successful load so that the same
    // can be done for multiple frames.
    let mut u = g.out;
    if req_comp != 0 && req_comp != 4 {
        u = convert_format(s, u, 4, req_comp, g.w as u32, g.h as u32)?;
    }
    Some((u, 4))
}

/// `stbi__gif_info`
pub(crate) fn gif_info(s: &mut Context) -> Option<Info> {
    let (x, y) = gif_info_raw(s)?;
    Some(Info {
        kind: ImageType::Gif,
        x,
        y,
        comp: 4,
        is_16_bit: false,
        is_hdr: false,
        is_iphone_png: false,
    })
}

#[cfg(test)]
mod tests {
    use super::super::{LoadOptions, load_from_memory, load_gif_from_memory};

    /// A GIF of `w` by `h` pixels with a two-colour global table, each frame `(delay, pixels)`,
    /// written with no compression to speak of: a clear code before every pixel.
    fn gif(w: u16, h: u16, frames: &[(u16, &[u8])]) -> Vec<u8> {
        let mut file = b"GIF89a".to_vec();
        file.extend_from_slice(&w.to_le_bytes());
        file.extend_from_slice(&h.to_le_bytes());
        file.extend_from_slice(&[0x80, 0, 0, 10, 20, 30, 40, 50, 60]);
        for &(delay, pixels) in frames {
            file.extend_from_slice(&[0x21, 0xF9, 4, 0]);
            file.extend_from_slice(&delay.to_le_bytes());
            file.extend_from_slice(&[0, 0, 0x2C, 0, 0, 0, 0]);
            file.extend_from_slice(&w.to_le_bytes());
            file.extend_from_slice(&h.to_le_bytes());
            file.extend_from_slice(&[0, 2]);
            // Codes of three bits: clear (4) and a pixel, over and over, then the end (5).
            let mut codes: Vec<u32> = pixels.iter().flat_map(|&p| [4, u32::from(p)]).collect();
            codes.push(5);
            let (mut bytes, mut bits, mut n) = (Vec::new(), 0u32, 0);
            for code in codes {
                bits |= code << n;
                n += 3;
                while n >= 8 {
                    bytes.push(bits as u8);
                    bits >>= 8;
                    n -= 8;
                }
            }
            if n > 0 {
                bytes.push(bits as u8);
            }
            file.push(bytes.len() as u8);
            file.extend_from_slice(&bytes);
            file.push(0);
        }
        file.push(0x3B);
        file
    }

    #[test]
    fn the_first_frame_or_all_of_them() {
        let file = gif(2, 1, &[(5, &[0, 1]), (7, &[1, 1])]);
        let image = load_from_memory(&file, 0, &LoadOptions::default()).unwrap();
        assert_eq!((image.x, image.y, image.channels_in_file), (2, 1, 4));
        assert_eq!(image.data, [10, 20, 30, 255, 40, 50, 60, 255]);
        let all = load_gif_from_memory(&file, 3, &LoadOptions::default()).unwrap();
        assert_eq!(
            (all.x, all.y, all.frames, all.delays.clone()),
            (2, 1, 2, vec![50, 70])
        );
        assert_eq!(all.data, [10, 20, 30, 40, 50, 60, 40, 50, 60, 40, 50, 60]);
    }
}
