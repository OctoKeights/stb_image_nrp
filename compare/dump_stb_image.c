/* The C half of the comparison with stb_image.h (https://github.com/nothings/stb): reads made-up
   image files and prints a running hash of the pixels that come out, or of the reason the file
   was refused. examples/dump.rs prints the same from the Rust port, and compare/compare.sh
   compares the two.

     dump stb_image <seed> <count>   made-up files: pictures written by stb_image_write.h, and
                                     files put together here with every kind of header the
                                     three readers take and made-up bytes for the picture
                                     (PNG of every depth and colour type, with palettes,
                                     transparent colours and interlacing; BMP of every header
                                     size and depth, with palettes and bit masks; JPEG baseline
                                     and progressive, of one to four components sampled every
                                     way, with restart markers); a third of them then damaged.
                                     Each is asked its size, and read at 8 bits a channel with
                                     a made-up number of channels asked for, and every third at
                                     16 bits. And for every fourth, made-up data through the
                                     zlib decoder, whole and damaged.

     dump stb_image_more <seed> <count>
                                     made-up files of the other formats: TGA of every kind
                                     (grey, colour and palette, 8 to 32 bits, with and without
                                     run-length encoding, either way up and round); PSD of
                                     one to five channels, 8 and 16 bits, raw and run-length
                                     encoded; GIF of one to three frames, with global and local
                                     colour tables, interlacing, transparency and every way of
                                     disposing of a frame, LZW encoded here; PNM binary and
                                     text, of every kind of maximum value; and Radiance HDR,
                                     flat and run-length encoded. A third of them then
                                     damaged. Each is asked its size and whether it is HDR, read
                                     at 8 bits a channel, as floats and every other at 16 bits,
                                     with a made-up gamma and scale; GIFs and every fourth file
                                     also read as an animation, every frame with its delay.

   stb_image.h is built with every reader, with its plain C in place of SSE2 (define WITH_SIMD
   for the other), and with memory that starts as zeros, so that a damaged file that has the
   original read what it never wrote gives the same answer every time. */
#define STBI_NO_STDIO
#ifndef WITH_SIMD
#define STBI_NO_SIMD
#endif
#include <stdlib.h>
#define STBI_MALLOC(sz) calloc(1, sz)
#define STBI_REALLOC(p, newsz) realloc(p, newsz)
#define STBI_FREE(p) free(p)
#define STB_IMAGE_IMPLEMENTATION
#include "stb_image.h"
#define STB_IMAGE_WRITE_IMPLEMENTATION
#include "stb_image_write.h"
#include "miniz.h"

#include "dump_stb_common.h"

static void P8(Out *f, uint32_t value)
{
    uint8_t byte = (uint8_t)value;
    Put(f, &byte, 1);
}
static void P16be(Out *f, uint32_t value)
{
    P8(f, value >> 8);
    P8(f, value);
}
static void P32be(Out *f, uint32_t value)
{
    P16be(f, value >> 16);
    P16be(f, value);
}
static void P16le(Out *f, uint32_t value)
{
    P8(f, value);
    P8(f, value >> 8);
}
static void P32le(Out *f, uint32_t value)
{
    P16le(f, value);
    P16le(f, value >> 16);
}

/* A chunk of a PNG file. (No checksum: the reader does not look at it.) */
static void Chunk(Out *f, const char *name, const uint8_t *bytes, size_t size)
{
    P32be(f, (uint32_t)size);
    Put(f, (void *)name, 4);
    if (size)
        Put(f, (void *)bytes, (int)size);
    P32be(f, 0);
}

/* A made-up PNG file: a header of any of the fifteen kinds, and made-up bytes as its rows. */
static void MakePng(Out *f)
{
    static const uint8_t kSig[8] = { 137, 80, 78, 71, 13, 10, 26, 10 };
    static const uint8_t kDepth[16] = { 1, 2, 4, 8, 16, 8, 16, 1, 2, 4, 8, 8, 16, 8, 16, 8 };
    static const uint8_t kColor[16] = { 0, 0, 0, 0, 0, 2, 2, 3, 3, 3, 3, 4, 4, 6, 6, 2 };
    static const uint8_t kChannels[7] = { 1, 0, 3, 1, 2, 0, 4 };
    static const uint32_t kXOrig[7] = { 0, 4, 0, 2, 0, 1, 0 }, kYOrig[7] = { 0, 0, 4, 0, 2, 0, 1 }, kXSpc[7] = { 8, 8, 4, 4, 2, 2, 1 }, kYSpc[7] = { 8, 8, 8, 4, 4, 2, 2 };
    static const uint8_t kZeros[8] = { 0 };
    const uint32_t kind = Next() % 16;
    const uint32_t depth = kDepth[kind], color = kColor[kind], channels = kChannels[color];
    uint32_t w, h, interlace, style, iphone, pass, x, y, how, pal_len = 0, parts, k;
    uint8_t *z, bytes[768];
    size_t zsize, at;
    Out raw = { NULL, 0, 0 }, header = { NULL, 0, 0 };
    if (Next() % 8 == 0)
    {
        w = 1 + Next() % 200;
        h = 1 + Next() % 120;
    }
    else
    {
        w = 1 + Next() % 40;
        h = 1 + Next() % 40;
    }
    interlace = Next() % 3 == 0;
    style = Next() % 3;
    iphone = Next() % 16 == 0;

    /* The rows, each behind its filter: of the whole image, or of each of the seven passes. */
    for (pass = 0; pass < (interlace ? 7u : 1u); ++pass)
    {
        uint32_t pw = w, ph = h, row_bytes;
        if (interlace)
        {
            pw = (w - kXOrig[pass] + kXSpc[pass] - 1) / kXSpc[pass];
            ph = (h - kYOrig[pass] + kYSpc[pass] - 1) / kYSpc[pass];
        }
        if (pw == 0 || ph == 0)
            continue;
        row_bytes = (pw * channels * depth + 7) / 8;
        for (y = 0; y < ph; ++y)
        {
            uint32_t filter = Next() % 5;
            if (Next() % 4000 == 0)
                filter = 5 + Next() % 3;
            P8(&raw, filter);
            for (x = 0; x < row_bytes; ++x)
                P8(&raw, style == 0 ? Next() >> 13 : style == 1 ? Next() % 4 : x * 3 + y);
        }
    }
    /* Now and then too few of them, or more than are needed. */
    how = Next() % 40;
    if (how == 0)
        raw.size -= 1 + Next() % raw.size;
    else if (how == 1)
        Put(&raw, (void *)kZeros, 5);

    how = Next() % 4;
    if (how == 0)
    {
        int n = 0;
        z = stbi_zlib_compress(raw.bytes, (int)raw.size, &n, 5 + (int)(Next() % 8));
        zsize = (size_t)n;
    }
    else
    {
        mz_ulong n = mz_compressBound((mz_ulong)raw.size);
        z = (uint8_t *)malloc(n);
        mz_compress2(z, &n, raw.bytes, (mz_ulong)raw.size, (int)(Next() % 10));
        zsize = (size_t)n;
    }

    Put(f, (void *)kSig, 8);
    if (iphone)
        Chunk(f, "CgBI", kZeros, 4);
    P32be(&header, w);
    P32be(&header, h);
    P8(&header, depth);
    P8(&header, color);
    P8(&header, 0);
    P8(&header, 0);
    P8(&header, interlace);
    Chunk(f, "IHDR", header.bytes, header.size);
    if (Next() % 4 == 0)
        Chunk(f, "tEXt", (const uint8_t *)"made up\0text", 12);
    /* A palette: nearly always where one is needed, now and then where none is. */
    if (color == 3 ? Next() % 20 != 0 : Next() % 8 == 0)
    {
        pal_len = 1 + Next() % 256;
        for (k = 0; k < pal_len * 3; ++k)
            bytes[k] = (uint8_t)(Next() >> 12);
        Chunk(f, "PLTE", bytes, pal_len * 3);
    }
    /* Transparency: alphas for a palette, or the one colour that is to be clear. */
    if (Next() % 3 == 0)
    {
        uint32_t n = 0;
        if (color == 3)
        {
            n = 1 + Next() % (pal_len ? pal_len : 1);
            if (Next() % 30 == 0)
                n = pal_len + 1;
        }
        else if (color == 0)
            n = 2;
        else if (color == 2)
            n = 6;
        else if (Next() % 10 == 0)
            n = 2;
        if (n > 256)
            n = 256;
        for (k = 0; k < n; ++k)
            bytes[k] = (uint8_t)(color == 3 ? Next() >> 9 : Next() % 4);
        if (n)
            Chunk(f, "tRNS", bytes, n);
    }
    parts = 1 + Next() % 3;
    if (Next() % 16 == 0)
        Chunk(f, "IDAT", z, 0);
    /* (An iPhone file's stream has no zlib header.) */
    at = iphone && zsize > 2 ? 2 : 0;
    for (k = 0; k < parts; ++k)
    {
        const size_t n = k + 1 == parts ? zsize - at : Next() % (zsize - at + 1);
        Chunk(f, "IDAT", z + at, n);
        at += n;
    }
    Chunk(f, "IEND", kZeros, 0);
    free(z);
    free(raw.bytes);
    free(header.bytes);
}

/* A made-up BMP file: a header of any of the five sizes, and made-up bytes as its rows. */
static void MakeBmp(Out *f)
{
    static const uint32_t kHsz[7] = { 12, 40, 40, 40, 56, 108, 124 };
    static const uint32_t kBpp[6] = { 1, 4, 8, 16, 24, 32 };
    const uint32_t hsz = kHsz[Next() % 7];
    uint32_t bpp = kBpp[Next() % 6];
    uint32_t compress = 0, w, h, top_down, masks[4] = { 0, 0, 0, 0 }, psize = 0, extra = 0, row_bytes, mask_style, k, offset, pixels_size;
    if (Next() % 40 == 0)
        bpp = Next() % 2 ? 2 : 20;
    if ((bpp == 16 || bpp == 32) && Next() % 2)
        compress = 3;
    if (Next() % 50 == 0)
        compress = 1 + Next() % 4;
    w = 1 + Next() % 40;
    h = 1 + Next() % 40;
    top_down = hsz != 12 && Next() % 4 == 0;
    mask_style = Next() % 3;
    if (mask_style == 0)
    {
        masks[0] = bpp == 16 ? 0xf800u : 0xff0000u;
        masks[1] = bpp == 16 ? 0x07e0u : 0xff00u;
        masks[2] = bpp == 16 ? 0x001fu : 0xffu;
        masks[3] = bpp == 16 ? 0u : 0xff000000u;
    }
    else if (mask_style == 1)
    {
        masks[0] = bpp == 16 ? 0x7c00u : 0xffu;
        masks[1] = bpp == 16 ? 0x03e0u : 0xff00u;
        masks[2] = bpp == 16 ? 0x001fu : 0xff0000u;
        masks[3] = bpp == 16 ? 0x8000u : 0xff000000u;
    }
    else
    {
        for (k = 0; k < 4; ++k)
        {
            const uint32_t count = 1 + Next() % 8;
            const uint32_t top = (bpp == 16 ? 16u : 32u) - count;
            const uint32_t shift = Next() % (top + 1);
            masks[k] = ((1u << count) - 1) << shift;
        }
    }
    if (bpp < 16)
    {
        psize = 1u << bpp;
        if (Next() % 4 == 0)
            psize = 1 + Next() % psize;
    }
    if (Next() % 10 == 0)
        extra = 1 + Next() % 20;
    offset = 14 + hsz + (hsz == 40 && compress == 3 ? 12u : 0u) + psize * (hsz == 12 ? 3u : 4u) + extra;

    P8(f, 'B');
    P8(f, 'M');
    P32le(f, 0);
    P32le(f, 0);
    P32le(f, offset);
    P32le(f, hsz);
    if (hsz == 12)
    {
        P16le(f, w);
        P16le(f, h);
        P16le(f, 1);
        P16le(f, bpp);
    }
    else
    {
        P32le(f, w);
        P32le(f, top_down ? 0u - h : h);
        P16le(f, 1);
        P16le(f, bpp);
        P32le(f, compress);
        for (k = 0; k < 5; ++k)
            P32le(f, 0);
        if (hsz == 40 && compress == 3)
            for (k = 0; k < 3; ++k)
                P32le(f, masks[k]);
        if (hsz >= 56)
            for (k = 0; k < 4; ++k)
                P32le(f, masks[k]);
        if (hsz >= 108)
            for (k = 0; k < 13; ++k)
                P32le(f, 0);
        if (hsz == 124)
            for (k = 0; k < 4; ++k)
                P32le(f, 0);
    }
    for (k = 0; k < psize * (hsz == 12 ? 3u : 4u) + extra; ++k)
        P8(f, Next() >> 10);
    row_bytes = ((w * bpp + 31) / 32) * 4;
    pixels_size = row_bytes * h;
    /* Now and then cut short. */
    if (Next() % 10 == 0)
        pixels_size -= Next() % (pixels_size / 2 + 1);
    for (k = 0; k < pixels_size; ++k)
        P8(f, Next() >> 13);
}

/* Made-up bytes for a JPEG scan: none of them the 0xFF that begins a marker, unless followed
   by the zero that says it is none (and, once in a while, by whatever comes next). */
static void EntropyBytes(Out *f, uint32_t n, uint32_t style)
{
    uint32_t i;
    for (i = 0; i < n; ++i)
    {
        uint32_t b = (Next() >> 11) & 255;
        if (style == 1 && Next() % 4 != 0)
            b = 0;
        if (style == 2)
            b &= 0x0f;
        P8(f, b);
        if (b == 0xff && Next() % 64 != 0)
            P8(f, 0);
    }
}

/* The bytes of a scan. With a restart interval, in stretches with a restart marker after each:
   no more of them than the scan has intervals, and each short enough that the reader has
   come to its marker by the time the interval is over. */
static void Entropy(Out *f, uint32_t blocks, uint32_t mcus, uint32_t restart)
{
    const uint32_t style = Next() % 3;
    if (restart)
    {
        const uint32_t segments = Next() % ((mcus + restart - 1) / restart);
        uint32_t k;
        for (k = 0; k < segments; ++k)
        {
            EntropyBytes(f, Next() % (restart / 2 + 2), style);
            P8(f, 0xff);
            P8(f, 0xd0 + (k & 7));
        }
        EntropyBytes(f, Next() % (restart / 2 + 2), style);
    }
    else
        EntropyBytes(f, blocks * (1 + Next() % 6), style);
}

/* A made-up JPEG file: headers of every kind the reader takes, Huffman tables in which most
   bits mean something, and made-up bytes as the picture. */
static void MakeJpeg(Out *f)
{
    static const uint32_t kComps[5] = { 1, 3, 3, 3, 4 };
    static const uint8_t kSampling[8] = { 0x11, 0x22, 0x21, 0x12, 0x41, 0x22, 0x11, 0x42 };
    static const uint8_t kDc[3][16] = { { 0, 1, 2, 3 }, { 0, 2, 1, 5 }, { 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15 } };
    static const uint8_t kDcCount[3] = { 4, 4, 16 }, kDcLength[3] = { 2, 2, 4 }, kDcPick[5] = { 0, 0, 1, 1, 2 };
    static const uint8_t kAc[2][16] = { { 0x00, 0x01, 0x11, 0x21, 0x10, 0xF0, 0x31, 0x20 }, { 0x00, 0x01, 0x02, 0x03, 0x11, 0x12, 0x21, 0x31, 0x41, 0x10, 0x20, 0xF0, 0x04, 0x13, 0x51, 0x30 } };
    const uint32_t progressive = Next() % 2;
    const uint32_t ncomp = kComps[Next() % 5];
    const uint32_t w = 1 + Next() % 64;
    const uint32_t h = 1 + Next() % 64;
    const uint32_t y_sampling = kSampling[Next() % 8];
    const uint32_t rgb_ids = Next() % 8 == 0;
    uint32_t dc_kind[2], ac_kind[2], ac_offset[2], restart = 0, sixteen, range, c, t, k, hv[4], id[4], blocks, mcus, hmax, vmax;
    for (t = 0; t < 2; ++t)
    {
        dc_kind[t] = kDcPick[Next() % 5];
        ac_kind[t] = Next() % 3;
        ac_offset[t] = Next() % 2 ? 0 : Next() & 255;
    }
    if (Next() % 3 == 0)
        restart = 1 + Next() % 8;

    P16be(f, 0xffd8);
    if (Next() % 2)
    {
        P16be(f, 0xffe0);
        P16be(f, 16);
        Put(f, (void *)"JFIF\0\1\1\0\0\1\0\1\0\0", 14);
    }
    if (ncomp == 4 ? Next() % 4 != 0 : Next() % 3 == 0)
    {
        P16be(f, 0xffee);
        P16be(f, 14);
        Put(f, (void *)"Adobe\0\144\0\0\0\0", 11);
        P8(f, Next() % 3);
    }
    if (Next() % 4 == 0)
    {
        const uint32_t n = Next() % 20;
        P16be(f, 0xfffe);
        P16be(f, 2 + n);
        for (k = 0; k < n; ++k)
            P8(f, Next() >> 10);
    }

    /* Two tables of divisors, of one byte a value or of two. */
    sixteen = Next() % 6 == 0;
    range = Next() % 3 == 0 ? 64 : 4;
    P16be(f, 0xffdb);
    P16be(f, 2 + 2 * (sixteen ? 129u : 65u));
    for (t = 0; t < 2; ++t)
    {
        P8(f, (sixteen << 4) | t);
        for (k = 0; k < 64; ++k)
        {
            const uint32_t v = 1 + Next() % range;
            if (sixteen)
                P16be(f, v);
            else
                P8(f, v);
        }
    }
    if (restart)
    {
        P16be(f, 0xffdd);
        P16be(f, 4);
        P16be(f, restart);
    }

    /* The frame. */
    P8(f, 0xff);
    P8(f, progressive ? 0xc2 : Next() % 4 == 0 ? 0xc1 : 0xc0);
    P16be(f, 8 + 3 * ncomp);
    P8(f, 8);
    P16be(f, h);
    P16be(f, w);
    P8(f, ncomp);
    for (c = 0; c < ncomp; ++c)
    {
        id[c] = rgb_ids && ncomp == 3 ? (uint32_t) "RGB"[c] : c + 1;
        hv[c] = c == 0 ? y_sampling : c == 3 && Next() % 2 ? y_sampling : 0x11;
        P8(f, id[c]);
        P8(f, hv[c]);
        P8(f, c == 0 ? 0 : 1);
    }
    hmax = y_sampling >> 4;
    vmax = y_sampling & 15;
    mcus = ((w + 8 * hmax - 1) / (8 * hmax)) * ((h + 8 * vmax - 1) / (8 * vmax));
    blocks = mcus * (hmax * vmax + ncomp - 1);

    /* The Huffman tables: two for the first values of blocks, two for the rest. */
    {
        uint32_t size = 2;
        for (t = 0; t < 2; ++t)
            size += 17 + kDcCount[dc_kind[t]] + 17 + (ac_kind[t] == 0 ? 192u : ac_kind[t] == 1 ? 8u : 16u);
        P16be(f, 0xffc4);
        P16be(f, size);
    }
    for (t = 0; t < 2; ++t)
    {
        P8(f, t);
        for (k = 1; k <= 16; ++k)
            P8(f, k == kDcLength[dc_kind[t]] ? kDcCount[dc_kind[t]] : 0);
        Put(f, (void *)kDc[dc_kind[t]], kDcCount[dc_kind[t]]);
    }
    for (t = 0; t < 2; ++t)
    {
        P8(f, 0x10 | t);
        if (ac_kind[t] == 0)
        {
            /* 64 codes of seven bits and 128 of eight, which between them use every run of
               bits, for 192 made-up symbols. */
            for (k = 1; k <= 16; ++k)
                P8(f, k == 7 ? 64 : k == 8 ? 128 : 0);
            for (k = 0; k < 192; ++k)
                P8(f, k * 167 + ac_offset[t]);
        }
        else
        {
            const uint32_t count = ac_kind[t] == 1 ? 8u : 16u;
            for (k = 1; k <= 16; ++k)
                P8(f, k == (ac_kind[t] == 1 ? 3u : 4u) ? count : 0);
            Put(f, (void *)kAc[ac_kind[t] - 1], (int)count);
        }
    }

    /* The scans. */
    if (!progressive)
    {
        if (ncomp > 1 && Next() % 3 == 0)
        {
            for (c = 0; c < ncomp; ++c)
            {
                P16be(f, 0xffda);
                P16be(f, 8);
                P8(f, 1);
                P8(f, id[c]);
                P8(f, c == 0 ? 0x00 : 0x11);
                P8(f, 0);
                P8(f, 63);
                P8(f, 0);
                Entropy(f, blocks, mcus, restart);
                if (c == 0 && Next() % 16 == 0)
                {
                    const uint32_t lines = Next() % 8 == 0 ? h + 1 : h;
                    P16be(f, 0xffdc);
                    P16be(f, 4);
                    P16be(f, lines);
                }
            }
        }
        else
        {
            P16be(f, 0xffda);
            P16be(f, 6 + 2 * ncomp);
            P8(f, ncomp);
            for (c = 0; c < ncomp; ++c)
            {
                P8(f, id[c]);
                P8(f, c == 0 ? 0x00 : 0x11);
            }
            P8(f, 0);
            P8(f, 63);
            P8(f, 0);
            Entropy(f, blocks, mcus, restart);
        }
    }
    else
    {
        const uint32_t scans = 1 + Next() % 6;
        uint32_t scan;
        for (scan = 0; scan < scans; ++scan)
        {
            const uint32_t what = scan == 0 ? 0 : Next() % 4;
            uint32_t ah = 0, al, ss = 0, se = 0, first = 0, n = ncomp;
            if (Next() % 3 == 0)
                ah = 1 + Next() % 2;
            al = ah ? ah - 1 : Next() % 2;
            if (what == 0)
            {
                /* The first values of blocks: of all the components together, or of one. */
                if (Next() % 4 == 0)
                {
                    first = Next() % ncomp;
                    n = 1;
                }
            }
            else
            {
                /* A stretch of the rest, of one component. */
                first = Next() % ncomp;
                n = 1;
                ss = 1 + Next() % 10;
                se = ss + Next() % (64 - ss);
            }
            P16be(f, 0xffda);
            P16be(f, 6 + 2 * n);
            P8(f, n);
            for (c = first; c < first + n; ++c)
            {
                P8(f, id[c]);
                P8(f, c == 0 ? 0x00 : 0x11);
            }
            P8(f, ss);
            P8(f, se);
            P8(f, (ah << 4) | al);
            Entropy(f, blocks, mcus, restart);
        }
    }

    /* The end: now and then bytes that mean nothing before it, or no end. */
    if (Next() % 10 == 0)
    {
        const uint32_t n = Next() % 6;
        for (k = 0; k < n; ++k)
            P8(f, (Next() >> 10) & 0x7f);
    }
    if (Next() % 30 != 0)
        P16be(f, 0xffd9);
}

/* Adds what a read came to: the size, the channels and the values, or the reason there are
   none. */
static int AddResult(const void *data, int x, int y, int comp, int req, size_t bytes_per_value)
{
    if (data)
    {
        AddNumber((uint32_t)x);
        AddNumber((uint32_t)y);
        AddNumber((uint32_t)comp);
        Add(data, (size_t)x * (size_t)y * (size_t)(req ? req : comp) * bytes_per_value);
        return 1;
    }
    else
    {
        const char *reason = stbi_failure_reason();
#ifdef SHOW_REASONS
        fprintf(stderr, "%s\n", reason);
#endif
        AddNumber(0xffffffffu);
        Add(reason, strlen(reason) + 1);
        return 0;
    }
}

/* ---------- The second dump: .tga, .psd, .gif, .pnm and .hdr files ---------- */

static void PutText(Out *f, const char *text)
{
    Put(f, (void *)text, (int)strlen(text));
}
static void PutUnsigned(Out *f, uint32_t value)
{
    char text[16];
    snprintf(text, sizeof text, "%u", value);
    PutText(f, text);
}

/* A pixel of a TGA file: an index into its palette (now and then one past its end), or bytes. */
static void TgaPixel(Out *f, uint32_t indexed, uint32_t pal_len, uint32_t bytes)
{
    uint32_t k;
    if (indexed)
    {
        uint32_t index;
        if (Next() % 16 == 0)
            index = Next() % 400;
        else
            index = Next() % pal_len;
        P8(f, index);
        if (bytes == 2)
            P8(f, index >> 8);
    }
    else
        for (k = 0; k < bytes; ++k)
            P8(f, Next() >> 9);
}

/* A made-up TGA file: palette or not, grey or colour, of every depth, raw or run-length
   encoded, either way up and round. */
static void MakeTga(Out *f)
{
    static const uint32_t kPaletteBits[5] = { 8, 15, 16, 24, 32 };
    static const uint32_t kBits[4] = { 15, 16, 24, 32 };
    uint32_t indexed, rle, w, h, desc, id_length, bytes_per_pixel, pal_len = 0, i, k, done;
    indexed = Next() % 3 == 0;
    rle = Next() % 2;
    w = 1 + Next() % 40;
    h = 1 + Next() % 40;
    desc = Next() % 2 ? 0x20u : 0u;
    if (Next() % 4 == 0)
        desc |= 0x10u;
    id_length = Next() % 3;
    P8(f, id_length);
    if (indexed)
    {
        uint32_t pal_bits, index_bits, pal_start, entry;
        pal_bits = kPaletteBits[Next() % 5];
        index_bits = Next() % 4 == 0 ? 16 : 8;
        pal_start = Next() % 4 == 0 ? Next() % 3 : 0;
        entry = pal_bits == 15 ? 2 : pal_bits / 8;
        pal_len = 1 + Next() % (index_bits == 8 ? 256 : 300);
        P8(f, 1);
        P8(f, rle ? 9 : 1);
        P16le(f, pal_start);
        P16le(f, pal_len);
        P8(f, pal_bits);
        P16le(f, 0);
        P16le(f, 0);
        P16le(f, w);
        P16le(f, h);
        P8(f, index_bits);
        P8(f, desc);
        for (i = 0; i < id_length + pal_start + pal_len * entry; ++i)
            P8(f, Next() >> 7);
        bytes_per_pixel = index_bits / 8;
    }
    else
    {
        uint32_t grey, bits;
        grey = Next() % 3 == 0;
        if (grey)
            bits = Next() % 2 ? 16 : 8;
        else
            bits = kBits[Next() % 4];
        P8(f, 0);
        P8(f, (grey ? 3 : 2) + (rle ? 8 : 0));
        P16le(f, 0);
        P16le(f, 0);
        P8(f, 0);
        P16le(f, 0);
        P16le(f, 0);
        P16le(f, w);
        P16le(f, h);
        P8(f, bits);
        P8(f, desc);
        for (i = 0; i < id_length; ++i)
            P8(f, Next() >> 7);
        bytes_per_pixel = (bits + 7) / 8;
    }
    done = 0;
    while (done < w * h)
    {
        if (!rle)
        {
            TgaPixel(f, indexed, pal_len, bytes_per_pixel);
            ++done;
        }
        else
        {
            uint32_t count, repeat;
            count = 1 + Next() % 128;
            repeat = Next() % 2;
            P8(f, (repeat << 7) | (count - 1));
            if (repeat)
                TgaPixel(f, indexed, pal_len, bytes_per_pixel);
            else
                for (k = 0; k < count; ++k)
                    TgaPixel(f, indexed, pal_len, bytes_per_pixel);
            done += count;
        }
    }
}

/* A made-up PSD file: none to five channels, 8 or 16 bits, raw or run-length encoded, with
   now and then mode data, resources and reserved bytes to skip. */
static void MakePsd(Out *f)
{
    uint32_t channels, h, w, depth, compression, pixels, i, k, c, section;
    channels = Next() % 6;
    h = 1 + Next() % 30;
    w = 1 + Next() % 30;
    depth = Next() % 2 ? 16 : 8;
    compression = Next() % 2;
    pixels = w * h;
    PutText(f, "8BPS");
    P16be(f, Next() % 20 == 0 ? 2 : 1);
    for (i = 0; i < 6; ++i)
        P8(f, 0);
    P16be(f, channels);
    P32be(f, h);
    P32be(f, w);
    P16be(f, depth);
    P16be(f, Next() % 20 == 0 ? 1 : 3);
    for (section = 0; section < 3; ++section)
    {
        uint32_t n;
        n = Next() % 3 == 0 ? Next() % 20 : 0;
        P32be(f, n);
        for (i = 0; i < n; ++i)
            P8(f, Next() >> 5);
    }
    P16be(f, Next() % 20 == 0 ? 2 : compression);
    if (!compression)
    {
        for (c = 0; c < channels; ++c)
            for (i = 0; i < pixels; ++i)
            {
                uint32_t v;
                if (c == 3 && Next() % 2)
                    v = Next() % 2 ? 0xffffffffu : 0;
                else
                    v = Next();
                P8(f, v >> 8);
                if (depth == 16)
                    P8(f, v);
            }
    }
    else
    {
        for (i = 0; i < h * channels; ++i)
            P16be(f, Next() % 200);
        for (c = 0; c < channels; ++c)
        {
            uint32_t done = 0;
            while (done < pixels)
            {
                const uint32_t left = pixels - done;
                const uint32_t how = Next() % 8;
                if (how == 0)
                    P8(f, 128);
                else if (how < 4 && left >= 2)
                {
                    uint32_t n;
                    n = 2 + Next() % ((left < 128 ? left : 128) - 1);
                    P8(f, 257 - n);
                    P8(f, Next() >> 3);
                    done += n;
                }
                else
                {
                    uint32_t n;
                    n = 1 + Next() % (left < 128 ? left : 128);
                    P8(f, n - 1);
                    for (k = 0; k < n; ++k)
                        P8(f, Next() >> 3);
                    done += n;
                }
            }
        }
    }
}

/* LZW as a GIF has it, written here: the codes in blocks, and the code size as the reader works
   it out. */
static uint32_t g_dict[4096][256];
static uint32_t g_generation;
typedef struct
{
    Out *f;
    uint32_t bits, nbits, block_max, blen, mincs, cs, avail, hasold;
    uint8_t block[255];
} Lzw;
static void LzwByte(Lzw *z, uint8_t b)
{
    z->block[z->blen++] = b;
    if (z->blen == z->block_max)
    {
        P8(z->f, z->blen);
        Put(z->f, z->block, (int)z->blen);
        z->blen = 0;
    }
}
static void LzwCode(Lzw *z, uint32_t code)
{
    const uint32_t clear = 1u << z->mincs;
    z->bits |= code << z->nbits;
    z->nbits += z->cs;
    while (z->nbits >= 8)
    {
        LzwByte(z, (uint8_t)z->bits);
        z->bits >>= 8;
        z->nbits -= 8;
    }
    if (code == clear)
    {
        z->cs = z->mincs + 1;
        z->avail = clear + 2;
        z->hasold = 0;
    }
    else
    {
        if (z->hasold && z->avail < 4096)
        {
            ++z->avail;
            if (z->avail > (1u << z->cs) - 1 && z->avail < 4096)
                ++z->cs;
        }
        z->hasold = 1;
    }
}
static void LzwEncode(Out *f, const uint8_t *px, uint32_t n, uint32_t mincs, uint32_t block_max)
{
    Lzw z;
    const uint32_t clear = 1u << mincs;
    uint32_t next, prefix, i;
    memset(&z, 0, sizeof z);
    z.f = f;
    z.block_max = block_max;
    z.mincs = mincs;
    z.cs = mincs + 1;
    z.avail = clear + 2;
    P8(f, mincs);
    ++g_generation;
    LzwCode(&z, clear);
    next = clear + 2;
    if (n > 0)
    {
        prefix = px[0];
        for (i = 1; i < n; ++i)
        {
            const uint32_t k = px[i], e = g_dict[prefix][k];
            if ((e >> 12) == g_generation)
            {
                prefix = e & 4095;
                continue;
            }
            LzwCode(&z, prefix);
            if (next < 4096)
                g_dict[prefix][k] = (g_generation << 12) | next++;
            else
            {
                LzwCode(&z, clear);
                ++g_generation;
                next = clear + 2;
            }
            prefix = k;
        }
        LzwCode(&z, prefix);
    }
    LzwCode(&z, clear + 1);
    if (z.nbits > 0)
        LzwByte(&z, (uint8_t)z.bits);
    if (z.blen)
    {
        P8(f, z.blen);
        Put(f, z.block, (int)z.blen);
    }
    P8(f, 0);
}

/* A made-up GIF file: one to three frames, each with or without a graphic control extension
   (a delay, a transparent colour, a way of disposing of it) and a comment, at a made-up place,
   with a local colour table or the global one, interlaced or not. */
static void MakeGif(Out *f)
{
    uint32_t W, H, gct, gsize, frames, frame, i, flags;
    uint8_t px[40 * 40];
    W = 1 + Next() % 40;
    H = 1 + Next() % 40;
    gct = Next() % 5 != 0;
    gsize = Next() % 8;
    frames = 1 + Next() % 3;
    PutText(f, Next() % 2 ? "GIF89a" : "GIF87a");
    P16le(f, W);
    P16le(f, H);
    flags = (Next() & 0x70u) | gsize;
    if (gct)
        flags |= 0x80u;
    P8(f, flags);
    P8(f, Next() % 256);
    P8(f, 0);
    if (gct)
        for (i = 0; i < (2u << gsize) * 3; ++i)
            P8(f, Next() >> 9);
    for (frame = 0; frame < frames; ++frame)
    {
        uint32_t x, y, w, h, lct, lsize, lflags, mincs, colours, n, style;
        if (Next() % 2)
        {
            uint32_t eflags;
            eflags = (Next() % 4) << 2;
            eflags |= Next() % 2;
            P8(f, 0x21);
            P8(f, 0xF9);
            P8(f, 4);
            P8(f, eflags);
            P16le(f, Next() % 100);
            P8(f, Next() % 256);
            P8(f, 0);
        }
        if (Next() % 5 == 0)
        {
            uint32_t blocks, b, k;
            blocks = Next() % 3;
            P8(f, 0x21);
            P8(f, 0xFE);
            for (b = 0; b < blocks; ++b)
            {
                uint32_t len;
                len = 1 + Next() % 20;
                P8(f, len);
                for (k = 0; k < len; ++k)
                    P8(f, 'a' + Next() % 26);
            }
            P8(f, 0);
        }
        x = Next() % W;
        y = Next() % H;
        w = Next() % 20 == 0 ? 0 : 1 + Next() % (W - x);
        h = 1 + Next() % (H - y);
        lct = gct ? Next() % 3 == 0 : Next() % 10 != 0;
        lsize = Next() % 8;
        lflags = lsize;
        if (lct)
            lflags |= 0x80u;
        if (Next() % 3 == 0)
            lflags |= 0x40u;
        P8(f, 0x2C);
        P16le(f, x);
        P16le(f, y);
        P16le(f, w);
        P16le(f, h);
        P8(f, lflags);
        if (lct)
            for (i = 0; i < (2u << lsize) * 3; ++i)
                P8(f, Next() >> 9);
        mincs = (lct ? lsize : gsize) + 1;
        if (mincs < 2)
            mincs = 2;
        if (Next() % 30 == 0)
            mincs = Next() % 14;
        if (mincs > 12)
        {
            P8(f, mincs);
            P8(f, 0);
            continue;
        }
        colours = 1u << mincs;
        if (colours > 256)
            colours = 256;
        n = w * h;
        style = Next() % 3;
        for (i = 0; i < n; ++i)
        {
            if (style == 0)
                px[i] = (uint8_t)(Next() % colours);
            else if (style == 1)
                px[i] = (uint8_t)((i / 7) % colours);
            else if (Next() % 4 == 0 || i == 0)
                px[i] = (uint8_t)(Next() % colours);
            else
                px[i] = px[i - 1];
        }
        LzwEncode(f, px, n, mincs, Next() % 4 == 0 ? 1 + Next() % 254 : 255);
    }
    if (Next() % 10 != 0)
        P8(f, 0x3B);
}

/* Whitespace between the numbers of a PNM file, now and then with a comment. */
static void PnmSpace(Out *f)
{
    static const char kSpace[6] = { ' ', '\t', '\n', '\r', '\v', '\f' };
    uint32_t n, i;
    if (Next() % 6 == 0)
        PutText(f, "\n# made up\n");
    n = 1 + Next() % 3;
    for (i = 0; i < n; ++i)
        P8(f, (uint32_t)kSpace[Next() % 6]);
}

/* A made-up PNM file: grey or colour, binary or text, of every kind of maximum value. */
static void MakePnm(Out *f)
{
    static const char kKinds[4] = { '2', '3', '5', '6' };
    char t;
    uint32_t ascii, comp, w, h, maxv, n, i;
    t = kKinds[Next() % 4];
    ascii = t == '2' || t == '3';
    comp = (t == '3' || t == '6') ? 3 : 1;
    w = 1 + Next() % 40;
    h = 1 + Next() % 40;
    switch (Next() % 8)
    {
        case 0: maxv = 255; break;
        case 1: maxv = 1 + Next() % 255; break;
        case 2: maxv = 65535; break;
        case 3: maxv = 256 + Next() % 65280; break;
        case 4: maxv = 1 + Next() % 15; break;
        case 5: maxv = Next() % 10 == 0 ? 0 : 255; break;
        case 6: maxv = Next() % 10 == 0 ? 70000 : 1000; break;
        default: maxv = 255; break;
    }
    P8(f, 'P');
    P8(f, (uint32_t)t);
    PnmSpace(f);
    PutUnsigned(f, w);
    PnmSpace(f);
    PutUnsigned(f, h);
    PnmSpace(f);
    PutUnsigned(f, maxv);
    n = w * h * comp;
    if (ascii)
    {
        for (i = 0; i < n; ++i)
        {
            PnmSpace(f);
            PutUnsigned(f, Next() % 20 == 0 ? Next() % (maxv + 100) : Next() % (maxv + 1));
        }
        P8(f, '\n');
    }
    else
    {
        P8(f, Next() % 2 ? '\n' : ' ');
        for (i = 0; i < n; ++i)
        {
            const uint32_t v = Next();
            if (maxv > 255)
                P8(f, v >> 8);
            P8(f, v);
        }
    }
}

/* A byte of an RGBE pixel: a colour, or an exponent near 1 (and now and then 0). */
static uint32_t HdrByte(uint32_t k)
{
    if (k < 3)
        return Next() >> 24;
    return Next() % 8 == 0 ? 0 : 120 + Next() % 20;
}

/* A made-up Radiance HDR file: a header with lines to skip, then scanlines flat or run-length
   encoded. */
static void MakeHdr(Out *f)
{
    uint32_t w, h, y, i, k, lines, rle;
    if (Next() % 2)
        w = 1 + Next() % 40;
    else
        w = 1 + Next() % 300;
    h = 1 + Next() % 20;
    rle = Next() % 4 != 0;
    PutText(f, Next() % 4 == 0 ? "#?RGBE\n" : "#?RADIANCE\n");
    lines = Next() % 3;
    for (i = 0; i < lines; ++i)
        PutText(f, Next() % 2 ? "# made up\n" : "EXPOSURE=1.0\n");
    if (Next() % 30 != 0)
        PutText(f, "FORMAT=32-bit_rle_rgbe\n");
    if (Next() % 4 == 0)
        PutText(f, "GAMMA=1.0\n");
    PutText(f, "\n");
    PutText(f, Next() % 20 == 0 ? "+Y " : "-Y ");
    PutUnsigned(f, h);
    PutText(f, Next() % 5 == 0 ? "  +X " : " +X ");
    PutUnsigned(f, w);
    PutText(f, "\n");
    for (y = 0; y < h; ++y)
    {
        if (rle && w >= 8 && w < 32768 && Next() % 20 != 0)
        {
            P8(f, 2);
            P8(f, 2);
            P8(f, w >> 8);
            P8(f, w & 255);
            for (k = 0; k < 4; ++k)
            {
                uint32_t x = 0;
                while (x < w)
                {
                    const uint32_t left = w - x;
                    if (Next() % 2)
                    {
                        uint32_t n;
                        n = 1 + Next() % (left < 127 ? left : 127);
                        P8(f, 128 + n);
                        P8(f, HdrByte(k));
                        x += n;
                    }
                    else
                    {
                        uint32_t n, j;
                        n = 1 + Next() % (left < 128 ? left : 128);
                        P8(f, n);
                        for (j = 0; j < n; ++j)
                            P8(f, HdrByte(k));
                        x += n;
                    }
                }
            }
        }
        else
            for (i = 0; i < w; ++i)
                for (k = 0; k < 4; ++k)
                    P8(f, HdrByte(k));
    }
}

/* The second dump. */
static int MoreMain(uint32_t count)
{
    static const float kGamma[4] = { 2.2f, 1.0f, 1.8f, 2.4f }, kScale[4] = { 1.0f, 0.5f, 2.0f, 4.0f };
    uint32_t i, k, loaded = 0, refused = 0, too_large = 0, kinds[9] = { 0, 0, 0, 0, 0, 0, 0, 0, 0 };
    uint64_t file_total = 0;
    for (i = 0; i < count; ++i)
    {
        Out file = { NULL, 0, 0 };
        const uint32_t kind = Next() % 5;
        stbi_info_res info;
        uint32_t req, flip, req16, reqf, reqg, gamma, scale;
        int x = 0, y = 0, z = 0, comp = 0, large;
        void *pixels;
        int *delays = NULL;

        if (kind == 0)
            MakeTga(&file);
        else if (kind == 1)
            MakePsd(&file);
        else if (kind == 2)
            MakeGif(&file);
        else if (kind == 3)
            MakePnm(&file);
        else
            MakeHdr(&file);

        /* A third of them damaged: bits turned over, the end cut off, or a byte made up. */
        if (Next() % 3 == 0)
        {
            const uint32_t how = Next() % 4;
            if (how == 0)
            {
                for (k = 1 + Next() % 3; k > 0; --k)
                {
                    const uint32_t bit = Next() % (uint32_t)(file.size * 8);
                    file.bytes[bit >> 3] ^= (uint8_t)(1u << (bit & 7));
                }
            }
            else if (how == 1)
                file.size = Next() % file.size;
            else if (how == 2)
            {
                const uint32_t at = Next() % (uint32_t)file.size;
                file.bytes[at] = (uint8_t)Next();
            }
            else
            {
                const uint32_t bit = Next() % (uint32_t)((file.size < 64 ? file.size : 64) * 8);
                file.bytes[bit >> 3] ^= (uint8_t)(1u << (bit & 7));
            }
        }
        AddNumber((uint32_t)file.size);
        Add(file.bytes, file.size);
        file_total += file.size;

        info = stbi_info2_from_memory(file.bytes, (int)file.size);
        AddNumber((uint32_t)info.type);
        if (info.type != STBI_type_unsupported)
        {
            AddNumber((uint32_t)info.x);
            AddNumber((uint32_t)info.y);
            AddNumber((uint32_t)info.comp);
            AddNumber((uint32_t)info.is_16_bit);
            AddNumber((uint32_t)info.is_hdr);
            AddNumber((uint32_t)info.is_iphone_png);
        }
        AddNumber((uint32_t)stbi_is_16_bit_from_memory(file.bytes, (int)file.size));
        AddNumber((uint32_t)stbi_is_hdr_from_memory(file.bytes, (int)file.size));
        ++kinds[info.type];

        req = Next() % 5;
        flip = Next() % 4 == 0;
        req16 = Next() % 5;
        reqf = Next() % 5;
        reqg = Next() % 5;
        gamma = Next() % 4;
        scale = Next() % 4;
        /* (A damaged size can ask for more memory and time than there is; those are left.) */
        large = info.type != STBI_type_unsupported && (uint64_t)(uint32_t)info.x * (uint64_t)(info.y < 0 ? 0u - (uint32_t)info.y : (uint32_t)info.y) > (1u << 20);
        if (large)
            ++too_large;
        else
        {
            stbi_set_flip_vertically_on_load((int)flip);
            stbi_hdr_to_ldr_gamma(kGamma[gamma]);
            stbi_hdr_to_ldr_scale(kScale[scale]);
            stbi_ldr_to_hdr_gamma(kGamma[gamma]);
            stbi_ldr_to_hdr_scale(kScale[scale]);
            stbi__g_failure_reason = "";
            pixels = stbi_load_from_memory(file.bytes, (int)file.size, &x, &y, &comp, (int)req);
            if (AddResult(pixels, x, y, comp, (int)req, 1))
                ++loaded;
            else
                ++refused;
            stbi_image_free(pixels);
            if (i % 2 == 0)
            {
                stbi__g_failure_reason = "";
                pixels = stbi_load_16_from_memory(file.bytes, (int)file.size, &x, &y, &comp, (int)req16);
                AddResult(pixels, x, y, comp, (int)req16, 2);
                stbi_image_free(pixels);
            }
            stbi__g_failure_reason = "";
            pixels = stbi_loadf_from_memory(file.bytes, (int)file.size, &x, &y, &comp, (int)reqf);
            AddResult(pixels, x, y, comp, (int)reqf, 4);
            stbi_image_free(pixels);
            if (info.type == STBI_type_gif || i % 4 == 0)
            {
                stbi__g_failure_reason = "";
                pixels = stbi_load_gif_from_memory(file.bytes, (int)file.size, &delays, &x, &y, &z, &comp, (int)reqg);
                if (pixels)
                {
                    AddNumber((uint32_t)x);
                    AddNumber((uint32_t)y);
                    AddNumber((uint32_t)z);
                    AddNumber((uint32_t)comp);
                    Add(pixels, (size_t)x * (size_t)y * (size_t)z * (size_t)(reqg ? reqg : (uint32_t)comp));
                    Add(delays, (size_t)z * sizeof(int));
                }
                else
                {
                    const char *reason = stbi_failure_reason();
                    AddNumber(0xffffffffu);
                    Add(reason, strlen(reason) + 1);
                }
                stbi_image_free(pixels);
                free(delays);
                delays = NULL;
            }
        }
        free(file.bytes);

        if ((i + 1) % 200 == 0 || i + 1 == count)
            printf("stb_image_more %u %016llx\n", i + 1, (unsigned long long)g_hash);
    }
    printf("stb_image_more bytes of files: %llu; read: %u, refused: %u, left as too large: %u; found to be .jpg: %u, .png: %u, .gif: %u, .bmp: %u, .psd: %u, .pnm: %u, .hdr: %u, .tga: %u, nothing: %u\n",
           (unsigned long long)file_total, loaded, refused, too_large, kinds[1], kinds[2], kinds[3], kinds[4], kinds[5], kinds[6], kinds[7], kinds[8], kinds[0]);
    return 0;
}

int main(int argc, char **argv)
{
    uint32_t count, i, k;
    uint8_t *data = (uint8_t *)malloc(200000 + 16), *room = (uint8_t *)malloc(200000 + 16);
    uint32_t loaded = 0, refused = 0, too_large = 0, kinds[4] = { 0, 0, 0, 0 };
    uint64_t pixel_total = 0, file_total = 0;
    if (argc < 4 || (strcmp(argv[1], "stb_image") != 0 && strcmp(argv[1], "stb_image_more") != 0))
    {
        fprintf(stderr, "usage: dump stb_image <seed> <count>\n       dump stb_image_more <seed> <count>\n");
        return 2;
    }
    g_state = strtoull(argv[2], NULL, 10);
    count = (uint32_t)strtoul(argv[3], NULL, 10);
    if (strcmp(argv[1], "stb_image_more") == 0)
    {
        free(data);
        free(room);
        return MoreMain(count);
    }
    for (i = 0; i < count; ++i)
    {
        Out file = { NULL, 0, 0 };
        const uint32_t kind = Next() % 8;
        stbi_info_res info;
        uint32_t req, flip, unpremultiply, iphone, req16;
        int x = 0, y = 0, comp = 0, large;
        void *pixels;

        if (kind < 3)
        {
            Picture p = MakePicture();
            stbi_write_png_compression_level = 5 + (int)(Next() % 8);
            stbi_write_force_png_filter = (int)(Next() % 6) - 1;
            stbi_flip_vertically_on_write(0);
            if (kind == 0)
            {
                int size = 0;
                unsigned char *png = stbi_write_png_to_mem(p.bytes, (ptrdiff_t)p.stride, p.w, p.h, p.comp, &size);
                Put(&file, png, size);
                free(png);
            }
            else if (kind == 1)
                stbi_write_bmp_strided_to_func(Put, &file, p.w, p.h, p.comp, p.bytes, (ptrdiff_t)p.stride);
            else
                stbi_write_jpg_strided_to_func(Put, &file, p.w, p.h, p.comp, p.bytes, (ptrdiff_t)p.stride, 1 + (int)(Next() % 100));
            free(p.bytes);
        }
        else if (kind < 5)
            MakePng(&file);
        else if (kind == 5)
            MakeBmp(&file);
        else
            MakeJpeg(&file);

        /* A third of them damaged: bits turned over, the end cut off, or a byte made up. */
        if (Next() % 3 == 0)
        {
            const uint32_t how = Next() % 4;
            if (how == 0)
            {
                for (k = 1 + Next() % 3; k > 0; --k)
                {
                    const uint32_t bit = Next() % (uint32_t)(file.size * 8);
                    file.bytes[bit >> 3] ^= (uint8_t)(1u << (bit & 7));
                }
            }
            else if (how == 1)
                file.size = Next() % file.size;
            else if (how == 2)
            {
                const uint32_t at = Next() % (uint32_t)file.size;
                file.bytes[at] = (uint8_t)Next();
            }
            else
            {
                const uint32_t bit = Next() % (uint32_t)((file.size < 64 ? file.size : 64) * 8);
                file.bytes[bit >> 3] ^= (uint8_t)(1u << (bit & 7));
            }
        }
        AddNumber((uint32_t)file.size);
        Add(file.bytes, file.size);
        file_total += file.size;

        info = stbi_info2_from_memory(file.bytes, (int)file.size);
        AddNumber((uint32_t)info.type);
        if (info.type != STBI_type_unsupported)
        {
            AddNumber((uint32_t)info.x);
            AddNumber((uint32_t)info.y);
            AddNumber((uint32_t)info.comp);
            AddNumber((uint32_t)info.is_16_bit);
            AddNumber((uint32_t)info.is_iphone_png);
        }
        AddNumber((uint32_t)stbi_is_16_bit_from_memory(file.bytes, (int)file.size));
        ++kinds[info.type == STBI_type_jpeg ? 1 : info.type == STBI_type_png ? 2 : info.type == STBI_type_bmp ? 3 : 0];

        req = Next() % 5;
        flip = Next() % 4 == 0;
        unpremultiply = Next() % 2;
        iphone = Next() % 2;
        req16 = Next() % 5;
        /* (A damaged size can ask for more memory and time than there is; those are left.) */
        large = info.type != STBI_type_unsupported && (uint64_t)(uint32_t)info.x * (uint64_t)(info.y < 0 ? 0u - (uint32_t)info.y : (uint32_t)info.y) > (1u << 20);
        if (large)
            ++too_large;
        else
        {
            stbi_set_flip_vertically_on_load((int)flip);
            stbi_set_unpremultiply_on_load((int)unpremultiply);
            stbi_convert_iphone_png_to_rgb((int)iphone);
            stbi__g_failure_reason = "";
            pixels = stbi_load_from_memory(file.bytes, (int)file.size, &x, &y, &comp, (int)req);
            if (AddResult(pixels, x, y, comp, (int)req, 1))
            {
                ++loaded;
                pixel_total += (uint64_t)x * (uint64_t)y;
            }
            else
                ++refused;
            stbi_image_free(pixels);
            if (i % 3 == 0)
            {
                stbi__g_failure_reason = "";
                pixels = stbi_load_16_from_memory(file.bytes, (int)file.size, &x, &y, &comp, (int)req16);
                AddResult(pixels, x, y, comp, (int)req16, 2);
                stbi_image_free(pixels);
            }
        }
        free(file.bytes);

        /* The zlib decoder by itself. */
        if (i % 4 == 0)
        {
            const size_t len = MakeData(data);
            uint8_t *z;
            size_t zsize;
            int n = 0;
            char *out;
            if (Next() % 2)
            {
                z = stbi_zlib_compress(data, (int)len, &n, 5 + (int)(Next() % 8));
                zsize = (size_t)n;
            }
            else
            {
                mz_ulong bound = mz_compressBound((mz_ulong)len);
                z = (uint8_t *)malloc(bound);
                mz_compress2(z, &bound, data, (mz_ulong)len, (int)(Next() % 10));
                zsize = (size_t)bound;
            }
            if (Next() % 3 == 0)
            {
                const uint32_t bit = Next() % (uint32_t)(zsize * 8);
                z[bit >> 3] ^= (uint8_t)(1u << (bit & 7));
            }
            stbi__g_failure_reason = "";
            out = stbi_zlib_decode_malloc((const char *)z, (int)zsize, &n);
            if (out)
            {
                AddNumber((uint32_t)n);
                Add(out, (size_t)n);
            }
            else
                Add(stbi_failure_reason(), strlen(stbi_failure_reason()) + 1);
            free(out);
            /* Into just enough room, or one byte too little. */
            stbi__g_failure_reason = "";
            n = stbi_zlib_decode_buffer((char *)room, (int)(len - (len > 0 && Next() % 2 ? 1 : 0)), (const char *)z, (int)zsize);
            AddNumber((uint32_t)n);
            if (n >= 0)
                Add(room, (size_t)n);
            else
                Add(stbi_failure_reason(), strlen(stbi_failure_reason()) + 1);
            /* Without its header. */
            stbi__g_failure_reason = "";
            out = stbi_zlib_decode_noheader_malloc((const char *)z + 2, (int)zsize - 2, &n);
            if (out)
            {
                AddNumber((uint32_t)n);
                Add(out, (size_t)n);
            }
            else
                Add(stbi_failure_reason(), strlen(stbi_failure_reason()) + 1);
            free(out);
            free(z);
        }

        if ((i + 1) % 200 == 0 || i + 1 == count)
            printf("stb_image %u %016llx\n", i + 1, (unsigned long long)g_hash);
    }
    printf("stb_image bytes of files: %llu; read: %u (%llu pixels), refused: %u, left as too large: %u; found to be .jpg: %u, .png: %u, .bmp: %u, nothing: %u\n", (unsigned long long)file_total, loaded,
           (unsigned long long)pixel_total, refused, too_large, kinds[1], kinds[2], kinds[3], kinds[0]);
    free(data);
    free(room);
    return 0;
}
