//! The Softimage `.pic` reader: `stbi__pic_*`, from `stb_image.h` v2.32.
//!
//! Reads 8-bit RGB and RGBA Softimage PIC files with uncompressed (`0`), pure run-length (`1`),
//! and mixed run-length (`2`) channel packets.

use alloc::vec::Vec;

use super::{
    Context, Failed, ImageFormat, RawInfo, STBI_MAX_DIMENSIONS, convert_rgba_in_place,
    mad3sizes_valid, try_alloc_elem,
};

#[derive(Clone, Copy, Default)]
struct PicPacket {
    kind: u8,
    channel: u8,
}

/// `stbi__pic_is4`
fn pic_is4(s: &mut Context, magic: [u8; 4]) -> bool {
    for expected in magic {
        if s.get8() != expected {
            return false;
        }
    }
    true
}

/// `stbi__pic_test_core`
fn pic_test_core(s: &mut Context) -> bool {
    if !pic_is4(s, *b"\x53\x80\xF6\x34") {
        return false;
    }
    for _ in 0..84 {
        s.get8();
    }
    pic_is4(s, *b"PICT")
}

/// `stbi__pic_test`
pub(crate) fn pic_test(s: &mut Context) -> bool {
    let r = pic_test_core(s);
    s.rewind();
    r
}

/// `stbi__readval`: reads active channels according to `channel` mask (`0x80` R, `0x40` G,
/// `0x20` B, `0x10` A) into `dest[..4]`.
fn pic_readval(s: &mut Context, channel: u8, dest: &mut [u8]) -> Result<(), Failed> {
    let mut mask = 0x80u8;
    for out in &mut dest[..4] {
        if channel & mask != 0 {
            if s.at_eof() {
                return s.fail("bad file");
            }
            *out = s.get8();
        }
        mask >>= 1;
    }
    Ok(())
}

/// `stbi__copyval`
fn pic_copyval(channel: u8, dest: &mut [u8], src: [u8; 4]) {
    let mut mask = 0x80u8;
    for i in 0..4 {
        if channel & mask != 0 {
            dest[i] = src[i];
        }
        mask >>= 1;
    }
}

fn pic_decode_packet(
    s: &mut Context,
    packet: PicPacket,
    width: usize,
    row: &mut [u8],
) -> Option<()> {
    match packet.kind {
        // Uncompressed
        0 => {
            for x in 0..width {
                pic_readval(s, packet.channel, &mut row[x * 4..x * 4 + 4]).ok()?;
            }
        }
        // Pure RLE
        1 => {
            let mut left = width;
            let mut at = 0usize;
            while left > 0 {
                if s.at_eof() {
                    return s.errp("bad file");
                }
                let mut count = usize::from(s.get8());
                if count == 0 {
                    return s.errp("bad file");
                }
                if count > left {
                    count = left;
                }
                let mut value = [0u8; 4];
                pic_readval(s, packet.channel, &mut value).ok()?;
                for _ in 0..count {
                    pic_copyval(packet.channel, &mut row[at..at + 4], value);
                    at += 4;
                }
                left -= count;
            }
        }
        // Mixed RLE
        2 => {
            let mut left = width;
            let mut at = 0usize;
            while left > 0 {
                let count_byte = s.get8();
                if s.at_eof() {
                    return s.errp("bad file");
                }
                let mut count = usize::from(count_byte);
                if count >= 128 {
                    count = if count == 128 {
                        s.get16be() as usize
                    } else {
                        count - 127
                    };
                    if count > left || count == 0 {
                        return s.errp("bad file");
                    }
                    let mut value = [0u8; 4];
                    pic_readval(s, packet.channel, &mut value).ok()?;
                    for _ in 0..count {
                        pic_copyval(packet.channel, &mut row[at..at + 4], value);
                        at += 4;
                    }
                } else {
                    count += 1;
                    if count > left {
                        return s.errp("bad file");
                    }
                    for _ in 0..count {
                        pic_readval(s, packet.channel, &mut row[at..at + 4]).ok()?;
                        at += 4;
                    }
                }
                left -= count;
            }
        }
        _ => return s.errp("bad format"),
    }
    Some(())
}

/// `stbi__pic_load_core`
fn pic_load_core(s: &mut Context, width: usize, height: usize, result: &mut [u8]) -> Option<i32> {
    let mut act_comp = 0u8;
    let mut num_packets = 0usize;
    let mut packets = [PicPacket::default(); 10];

    loop {
        if num_packets == packets.len() {
            return s.errp("bad format");
        }
        let chained = s.get8();
        let size = s.get8();
        let kind = s.get8();
        let channel = s.get8();
        packets[num_packets] = PicPacket { kind, channel };
        num_packets += 1;
        act_comp |= channel;

        if s.at_eof() {
            return s.errp("bad file");
        }
        if size != 8 {
            return s.errp("bad format");
        }
        if chained == 0 {
            break;
        }
    }

    let comp = if act_comp & 0x10 != 0 { 4 } else { 3 };

    for y in 0..height {
        let row = &mut result[y * width * 4..(y + 1) * width * 4];
        for &packet in &packets[..num_packets] {
            pic_decode_packet(s, packet, width, row)?;
        }
    }

    Some(comp)
}

/// `stbi__pic_load`: the pixels and how many channels the file has; the size goes into the
/// context.
pub(crate) fn pic_load(s: &mut Context, req_comp: i32) -> Option<(Vec<u8>, i32)> {
    if !pic_test_core(s) {
        return s.errp("not PIC");
    }

    let x = s.get16be();
    let y = s.get16be();
    if s.at_eof() {
        return s.errp("bad file");
    }
    if x <= 0 || y <= 0 {
        return s.errp("0-pixel image");
    }
    if (1 << 28) / x < y
        || x > STBI_MAX_DIMENSIONS as i32
        || y > STBI_MAX_DIMENSIONS as i32
        || !mad3sizes_valid(x, y, 4, 0)
    {
        return s.errp("too large");
    }

    // skip `ratio`, `fields`, `pad`
    s.get32be();
    s.get16be();
    s.get16be();

    let total_rgba = x as usize * y as usize * 4;
    if s.check_limits(x as u32, y as u32, total_rgba).is_err() {
        return None;
    }

    // intermediate buffer is RGBA filled with 0xff (opaque)
    let Some(mut result) = try_alloc_elem(0xffu8, total_rgba) else {
        return s.errp("outofmem");
    };

    let comp = pic_load_core(s, x as usize, y as usize, &mut result)?;
    s.img_x = x as u32;
    s.img_y = y as u32;

    let target = if req_comp == 0 { comp } else { req_comp };
    if target != 4 {
        if !(1..=4).contains(&target) {
            return s.errp("bad req_comp");
        }
        convert_rgba_in_place(&mut result, target as usize);
    }

    Some((result, comp))
}

/// `stbi__pic_info`
pub(crate) fn pic_info(s: &mut Context) -> Option<RawInfo> {
    if !pic_test_core(s) {
        s.rewind();
        return None;
    }

    let x = s.get16be();
    let y = s.get16be();
    if s.at_eof() {
        s.rewind();
        return None;
    }
    if x <= 0 || y <= 0 || (1 << 28) / x < y {
        s.rewind();
        return None;
    }
    s.skip(8);

    let mut act_comp = 0u8;
    let mut num_packets = 0usize;
    loop {
        if num_packets == 10 {
            s.rewind();
            return None;
        }
        num_packets += 1;
        let chained = s.get8();
        let size = s.get8();
        let _kind = s.get8();
        let channel = s.get8();
        act_comp |= channel;

        if s.at_eof() || size != 8 {
            s.rewind();
            return None;
        }
        if chained == 0 {
            break;
        }
    }

    let comp = if act_comp & 0x10 != 0 { 4 } else { 3 };
    Some(RawInfo {
        kind: ImageFormat::Pic,
        x,
        y,
        comp,
        is_16_bit: false,
        is_hdr: false,
        is_iphone_png: false,
    })
}

#[cfg(test)]
mod tests {
    use alloc::vec::Vec;

    use crate::{Channels, ImageFormat, LoadOptions, info, load, load_16, load_f32};

    fn make_pic_header(w: u16, h: u16, packets: &[(u8, u8, u8, u8)], body: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(b"\x53\x80\xF6\x34");
        out.extend_from_slice(&[0u8; 84]); // version + 80-byte comment
        out.extend_from_slice(b"PICT");
        out.extend_from_slice(&w.to_be_bytes());
        out.extend_from_slice(&h.to_be_bytes());
        out.extend_from_slice(&[0x3f, 0x80, 0x00, 0x00, 0x00, 0x03, 0x00, 0x00]); // ratio, fields, pad
        for &(chained, size, kind, channel) in packets {
            out.extend_from_slice(&[chained, size, kind, channel]);
        }
        out.extend_from_slice(body);
        out
    }

    #[test]
    fn pic_uncompressed_pure_rle_and_mixed_rle() {
        let opts = LoadOptions::default();

        // 1. Uncompressed RGB (type 0, channel 0xE0 = R|G|B), 2x1
        let rgb_pic = make_pic_header(2, 1, &[(0, 8, 0, 0xE0)], &[10, 20, 30, 40, 50, 60]);
        let inf = info(&rgb_pic).unwrap();
        assert_eq!(
            (inf.format, inf.width, inf.height, inf.channels),
            (ImageFormat::Pic, 2, 1, Channels::Rgb)
        );
        assert_eq!(inf.format.code(), 9);
        let img = load(&rgb_pic, None, &opts).unwrap();
        assert_eq!(img.channels, Channels::Rgb);
        assert_eq!(img.channels_in_file, Channels::Rgb);
        assert_eq!(img.data, [10, 20, 30, 40, 50, 60]);

        // Requested RGBA on RGB PIC fills alpha = 255
        let rgba = load(&rgb_pic, Some(Channels::Rgba), &opts).unwrap();
        assert_eq!(rgba.data, [10, 20, 30, 255, 40, 50, 60, 255]);

        // 16-bit and f32 paths also work on PIC
        let img16 = load_16(&rgb_pic, None, &opts).unwrap();
        assert_eq!(img16.data[0], 0x0a0a);
        let imgf = load_f32(&rgb_pic, None, &opts).unwrap();
        assert_eq!(imgf.data.len(), 6);

        // 2. Chained packets: Pure RLE RGB (type 1, 0xE0) + Mixed RLE Alpha (type 2, 0x10), 4x1
        let mut body = Vec::new();
        // Packet 0 (Pure RLE RGB): run of 3 [100, 110, 120], run of 1 [200, 210, 220]
        body.extend_from_slice(&[3, 100, 110, 120, 1, 200, 210, 220]);
        // Packet 1 (Mixed RLE Alpha): literal 2 (count=1 -> 2 pixels: 50, 60), repeated 2 (127+2=129: 70)
        body.extend_from_slice(&[1, 50, 60, 129, 70]);
        let chained_pic = make_pic_header(4, 1, &[(1, 8, 1, 0xE0), (0, 8, 2, 0x10)], &body);
        let inf = info(&chained_pic).unwrap();
        assert_eq!(
            (inf.format, inf.channels),
            (ImageFormat::Pic, Channels::Rgba)
        );
        let img = load(&chained_pic, None, &opts).unwrap();
        assert_eq!(
            img.data,
            [
                100, 110, 120, 50, // pixel 0
                100, 110, 120, 60, // pixel 1
                100, 110, 120, 70, // pixel 2
                200, 210, 220, 70, // pixel 3
            ]
        );

        // 3. Mixed RLE with count == 128 (16-bit big-endian extended run count = 130), 130x1
        let mut long_body = Vec::new();
        long_body.extend_from_slice(&[128, 0x00, 130, 9, 8, 7]);
        let long_pic = make_pic_header(130, 1, &[(0, 8, 2, 0xE0)], &long_body);
        let long_img = load(&long_pic, None, &opts).unwrap();
        assert_eq!(long_img.width, 130);
        assert_eq!(long_img.data, [9, 8, 7].repeat(130));
    }

    #[test]
    fn pic_rejects_corrupt_and_truncated_files() {
        let opts = LoadOptions::default();

        // Zero dimensions
        let zero = make_pic_header(0, 4, &[(0, 8, 0, 0xE0)], &[]);
        assert!(info(&zero).is_err());
        assert_eq!(
            load(&zero, None, &opts).unwrap_err().reason(),
            "0-pixel image"
        );

        // Bad packet bit depth (size != 8)
        let bad_depth = make_pic_header(1, 1, &[(0, 16, 0, 0xE0)], &[1, 2, 3]);
        assert!(info(&bad_depth).is_err());
        assert_eq!(
            load(&bad_depth, None, &opts).unwrap_err().reason(),
            "bad format"
        );

        // Unknown packet kind (3)
        let bad_kind = make_pic_header(1, 1, &[(0, 8, 3, 0xE0)], &[1, 2, 3]);
        assert_eq!(
            load(&bad_kind, None, &opts).unwrap_err().reason(),
            "bad format"
        );

        // Truncated pixel data (CVE-2023-43898 regression: must fail cleanly without null/OOB)
        let truncated = make_pic_header(2, 2, &[(0, 8, 0, 0xE0)], &[1, 2]);
        assert_eq!(
            load(&truncated, None, &opts).unwrap_err().reason(),
            "bad file"
        );

        // Too many chained packets (> 10)
        let eleven = [(1u8, 8u8, 0u8, 0x80u8); 11];
        let too_many = make_pic_header(1, 1, &eleven, &[0; 11]);
        assert!(info(&too_many).is_err());
        assert_eq!(
            load(&too_many, None, &opts).unwrap_err().reason(),
            "bad format"
        );
    }
}
