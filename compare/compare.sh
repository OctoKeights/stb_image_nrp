#!/bin/sh
# Compares stb_image_nrp with stb_image.h of stb (https://github.com/nothings/stb).
#
#   MINIZ=<checkout of miniz> compare/compare.sh <checkout of stb> [count] [seeds] [dumps]
#
# compare/dump_stb_image.c is built against its stb_image.h, with every reader, with STBI_NO_SIMD
# (the plain C, which the port is of), with memory that starts as zeros and with overflow that
# wraps (-fwrapv), so that the damaged files among those it reads have one answer; it uses its
# stb_image_write.h to make some of the files and miniz (https://github.com/richgel999/miniz, in
# $MINIZ) to compress others. examples/dump.rs is built from the port (with stb_image_write_nrp
# and miniz_nrp for the same); both run the same synthetic files for each seed and what they print
# is compared byte for byte. Dumps: stb_image and stb_image_more, each with a tenth of [count]
# (200,000 by default) as the number of synthetic files, each asked its size and read once or
# more.
set -u
if [ $# -lt 1 ]; then
    echo "usage: $0 <checkout of stb> [count] [seeds] [dumps]" >&2
    exit 2
fi
here=$(cd "$(dirname "$0")/.." && pwd)
source=$1
blocks=${2:-200000}
seeds=${3:-3}
dumps=${4:-stb_image stb_image_more}
tmp=${TMPDIR:-/tmp}/stb_image_nrp-compare
mkdir -p "$tmp" || exit 2
if ! [ -f "$source/stb_image.h" ] && [ -f "$source/stb_image_write.h" ]; then
    echo "$source does not look like checkout of stb" >&2
    exit 2
fi

(cd "$here" && cargo build --release -q --ignore-rust-version --example dump) || exit 2
rust=$(cd "$here" && cargo metadata --format-version 1 --no-deps 2>/dev/null | sed 's/.*"target_directory":"\([^"]*\)".*/\1/')/release/examples/dump

if [ -z "${MINIZ:-}" ] || [ ! -f "$MINIZ/miniz_tdef.c" ]; then
    echo "no miniz: put its checkout in \$MINIZ" >&2
    exit 2
fi
# (The header miniz's build would make, and no .zip code.)
mkdir -p "$tmp/miniz" && echo "#define MINIZ_EXPORT" > "$tmp/miniz/miniz_export.h" || exit 2
${CC:-cc} -O2 -fwrapv -ffp-contract=off -DMINIZ_NO_ARCHIVE_APIS -DMINIZ_NO_ZLIB_COMPATIBLE_NAMES -I "$tmp/miniz" -I "$MINIZ" -I "$source" "$here/compare/dump_stb_image.c" "$MINIZ/miniz.c" "$MINIZ/miniz_tdef.c" "$MINIZ/miniz_tinfl.c" -lm -o "$tmp/dump-stb-image" || exit 2

bad=0
for what in $dumps; do
    extra=
    case $what in
    stb_image | stb_image_more)
        reference=$tmp/dump-stb-image
        count=$((blocks / 10))
        ;;
    *)
        echo "unknown dump $what" >&2
        exit 2
        ;;
    esac
    seed=1
    while [ "$seed" -le "$seeds" ]; do
        # shellcheck disable=SC2086
        "$reference" "$what" "$seed" "$count" $extra > "$tmp/reference.txt" 2> /dev/null
        # shellcheck disable=SC2086
        "$rust" "$what" "$seed" "$count" $extra > "$tmp/rust.txt"
        if [ ! -s "$tmp/reference.txt" ]; then
            echo "$what, seed $seed: NOT COMPARED, the original's side printed nothing"
            bad=$((bad + 1))
        elif cmp -s "$tmp/reference.txt" "$tmp/rust.txt"; then
            echo "$what, seed $seed: the same ($(tail -1 "$tmp/rust.txt"))"
        else
            echo "$what, seed $seed: DIFFERENT"
            bad=$((bad + 1))
        fi
        seed=$((seed + 1))
    done
done
[ "$bad" -eq 0 ]
