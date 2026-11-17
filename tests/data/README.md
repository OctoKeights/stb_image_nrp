# Recorded results

`stb_image_expected.txt` holds the hashes `compare/dump_stb_image.c` prints when built
against `stb_image.h` v2.32, for synthetic PNG, BMP and JPEG files; and
`stb_image_more_expected.txt` the same for synthetic TGA, PSD, GIF, PNM and HDR files.
`tests/golden.rs` checks that stb_image_nrp prints the same.

To record them again, run the comparison once, which builds the original's side, then run
that program directly:

    MINIZ=<checkout of miniz> compare/compare.sh <checkout of stb> 3000 1
    ref="${TMPDIR:-/tmp}/stb_image_nrp-compare/dump-stb-image"
    { "$ref" stb_image 4 600; "$ref" stb_image 10 600; } > tests/data/stb_image_expected.txt
    { "$ref" stb_image_more 4 600; "$ref" stb_image_more 10 600; } > tests/data/stb_image_more_expected.txt

All test data is generated from seeds; no external files are used.
