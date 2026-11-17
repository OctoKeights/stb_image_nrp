//! The Rust half of the comparison with stb_image.h: runs the port over made-up data and
//! prints a running hash of what comes out, as `compare/dump_stb_image.c` does with the original.
//! `compare/compare.sh` compares the two.
//!
//!   dump stb_image <seed> <count>        made-up .png, .bmp and .jpg files of every kind, a third
//!                                        of them damaged, asked their size and read at 8 and 16
//!                                        bits a channel; and made-up data through the zlib
//!                                        decoder, whole and damaged
//!   dump stb_image_more <seed> <count>   made-up .tga, .psd, .gif, .pnm and .hdr files, a third
//!                                        of them damaged, read at 8 and 16 bits a channel, as
//!                                        floats and as animations

#[path = "../tests/made_up/mod.rs"]
mod made_up;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (Some(what), Some(seed), Some(count)) = (
        args.get(1),
        args.get(2).and_then(|s| s.parse::<u64>().ok()),
        args.get(3).and_then(|s| s.parse::<u32>().ok()),
    ) else {
        eprintln!("usage: dump <what> <seed> <count>   (see the top of dump.rs)");
        std::process::exit(2);
    };
    let _ = seed;
    match what.as_str() {
        "stb_image" => print!("{}", made_up::stb_image(seed, count)),
        "stb_image_more" => print!("{}", made_up::stb_image_more(seed, count)),
        _ => {
            eprintln!("unknown dump {what}");
            std::process::exit(2);
        }
    }
}
