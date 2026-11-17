//! The port of stb_image.h against results recorded from the original, so that a change shows
//! without a C compiler at hand.
//!
//! `tests/data/stb_image_expected.txt` is what `compare/dump_stb_image.c`, built against stb
//! and miniz (see `compare/compare.sh`), prints for `stb_image 4 600` and `stb_image 10 600`:
//! a hash of what synthetic `.png`, `.bmp` and `.jpg` files of every kind, a third of them
//! damaged, are read as or refused with, and of what synthetic data comes to in its zlib
//! decoder. `compare/compare.sh` is the full comparison.

mod made_up;

#[test]
fn reads_the_files_as_stb_image_does() {
    let printed = [made_up::stb_image(4, 600), made_up::stb_image(10, 600)].concat();
    let expected = include_str!("data/stb_image_expected.txt");
    for (line, (ours, theirs)) in printed.lines().zip(expected.lines()).enumerate() {
        assert_eq!(ours, theirs, "line {}", line + 1);
    }
    assert_eq!(printed.lines().count(), expected.lines().count());
}

/// The same for the other formats: `tests/data/stb_image_more_expected.txt` is what the
/// original prints for `stb_image_more 4 600` and `stb_image_more 10 600`, synthetic `.tga`,
/// `.psd`, `.gif`, `.pnm` and `.hdr` files read at 8 and 16 bits a channel, as floats and as
/// animations.
#[test]
fn reads_the_other_formats_as_stb_image_does() {
    let printed = [
        made_up::stb_image_more(4, 600),
        made_up::stb_image_more(10, 600),
    ]
    .concat();
    let expected = include_str!("data/stb_image_more_expected.txt");
    for (line, (ours, theirs)) in printed.lines().zip(expected.lines()).enumerate() {
        assert_eq!(ours, theirs, "line {}", line + 1);
    }
    assert_eq!(printed.lines().count(), expected.lines().count());
}

/// Files damaged far worse than the comparison's: whatever is in them, the reader gives an
/// image or a reason, and does not panic. (`FILES` of them; more by hand when the reader is
/// changed.)
#[test]
fn badly_damaged_files_are_read_or_refused() {
    use stb_image_nrp::{Channels, LoadOptions, info, load, load_16, load_f32, load_gif};
    const FILES: u32 = 1000;
    let mut r = made_up::Random { state: 77 };
    let (mut read, mut refused) = (0, 0);
    for _ in 0..FILES {
        let mut file = Vec::new();
        match r.next() % 8 {
            0 => made_up::make_png(&mut r, &mut file),
            1 => made_up::make_bmp(&mut r, &mut file),
            2 => made_up::make_jpeg(&mut r, &mut file),
            3 => made_up::make_tga(&mut r, &mut file),
            4 => made_up::make_psd(&mut r, &mut file),
            5 => made_up::make_gif(&mut r, &mut file),
            6 => made_up::make_pnm(&mut r, &mut file),
            _ => made_up::make_hdr(&mut r, &mut file),
        }
        for _ in 0..4 {
            // A few bytes changed, more each time round; now and then the end cut off too.
            for _ in 0..1 + r.next() % 6 {
                let at = r.next() as usize % file.len();
                file[at] = r.next() as u8;
            }
            if r.next().is_multiple_of(8) && file.len() > 16 {
                file.truncate(16 + r.next() as usize % (file.len() - 16));
            }
            // (A size that would take more memory and time than the test has is left.)
            if info(&file)
                .is_ok_and(|info| u64::from(info.width) * u64::from(info.height) > 1 << 20)
            {
                continue;
            }
            let options = LoadOptions {
                flip_vertically: !r.next().is_multiple_of(2),
                unpremultiply: true,
                convert_iphone_png_to_rgb: true,
                ..LoadOptions::default()
            };
            let wanted = Channels::from_count((r.next() % 5) as u8);
            let size = |width: u32, height: u32, channels: Channels| {
                width as usize * height as usize * channels.count()
            };
            match load(&file, wanted, &options) {
                Ok(image) => {
                    assert_eq!(image.channels, wanted.unwrap_or(image.channels_in_file));
                    assert_eq!(
                        image.data.len(),
                        size(image.width, image.height, image.channels)
                    );
                    read += 1;
                }
                Err(_) => refused += 1,
            }
            if let Ok(image) = load_16(&file, wanted, &options) {
                assert_eq!(
                    image.data.len(),
                    size(image.width, image.height, image.channels)
                );
            }
            if let Ok(image) = load_f32(&file, wanted, &options) {
                assert_eq!(
                    image.data.len(),
                    size(image.width, image.height, image.channels)
                );
            }
            if let Ok(gif) = load_gif(&file, wanted, &options) {
                for frame in &gif.frames {
                    assert_eq!(frame.data.len(), size(gif.width, gif.height, gif.channels));
                }
            }
        }
    }
    assert!(
        read > 100 && refused > 100,
        "{read} read, {refused} refused"
    );
}
