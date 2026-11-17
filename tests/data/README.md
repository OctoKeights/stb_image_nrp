# Recorded results

`stb_image_expected.txt` is what stb_image.h makes of made-up `.png`, `.bmp` and `.jpg` files,
as `compare/dump_stb_image.c` prints it, and `stb_image_more_expected.txt` the same for made-up
`.tga`, `.psd`, `.gif`, `.pnm` and `.hdr` files, read at 8 and 16 bits a channel, as floats and
as animations; `tests/golden.rs` checks that the port prints the same. To make them again, run
the comparison once (it builds the original's side), then that by itself:

    MINIZ=<checkout of miniz> compare/compare.sh <checkout of stb> 3000 1
    ref="${TMPDIR:-/tmp}/stb_image_nrp-compare/dump-stb-image"
    { "$ref" stb_image 4 600; "$ref" stb_image 10 600; } > tests/data/stb_image_expected.txt
    { "$ref" stb_image_more 4 600; "$ref" stb_image_more 10 600; } > tests/data/stb_image_more_expected.txt

No picture or file from anywhere is in this repository: every file and stream the tests and
the comparison use is made up from a seed.
