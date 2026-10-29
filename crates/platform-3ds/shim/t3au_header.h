// The `T3AU` header check of the 3DS shim's `trino_wav_load` (issue #121),
// without libctru: `<stdint.h>` only, so `crates/fuzz` compiles this same
// file on the host and fuzzes it against the asset pipeline's
// `T3au::parse` (the `t3au_shim` target, differential). The rules are the
// pipeline's, one for one: a header the Rust parser refuses, the shim
// refuses, and what both accept they read the same way.
//
// Layout (little-endian), written by `cargo xtask assets 3ds`
// (trino_asset_pipeline::audio::n3ds_container):
//    0 "T3AU"   4 u8 codec (0 PCM16, 1 DSP-ADPCM)   5 u8 bus
//    6 u8 flags (bit 0 loop, bit 1 stream)   7 u8 channels (1)
//    8 u32 rate   12 u32 samples   16 u32 data bytes
//   20 u32 chunk samples (streamed: samples per wave buffer, 0 = whole)
//   24 u32 context count   28 16 x u16 DSP-ADPCM coefficients
//   60 a context per chunk (u8 predictor/scale, u8 0, s16 last sample,
//      s16 the one before, u16 0), then the data.

#ifndef TRINO_T3AU_HEADER_H
#define TRINO_T3AU_HEADER_H

#include <stdint.h>

#if defined(_MSC_VER) && !defined(__cplusplus)
#define TRINO_T3AU_FN static __inline
#else
#define TRINO_T3AU_FN static inline
#endif

#define TRINO_T3AU_HEADER 60
// trino_core::Bus::COUNT (the fuzz crate's tests pin it).
#define TRINO_T3AU_BUSES 6
// Bounds on what a header may ask the shim to allocate: contexts (8 bytes
// each, malloc'd) and a stream's wave buffers (a chunk's bytes, three
// linearAlloc'd copies; the bake writes 14 336-sample chunks).
#define TRINO_T3AU_MAX_CONTEXTS (1u << 20)
#define TRINO_T3AU_MAX_CHUNK (1u << 24)

typedef struct {
    uint8_t adpcm;   // 0 PCM16, 1 DSP-ADPCM
    uint8_t bus;     // trino_core::Bus index
    uint8_t looped;  // baked to loop
    uint8_t stream;  // played from the file in chunks (flag, chunk and samples)
    uint32_t sample_rate;
    uint32_t nsamples;
    uint32_t data_bytes;
    uint32_t chunk_samples;
    uint32_t ncontexts;
    uint32_t data_offset;  // where the data starts in the file
    int16_t coefs[16];
} trino_t3au_header_t;

TRINO_T3AU_FN uint32_t trino_t3au_rd32(const uint8_t* p)
{
    return (uint32_t)p[0] | (uint32_t)p[1] << 8 | (uint32_t)p[2] << 16 | (uint32_t)p[3] << 24;
}

TRINO_T3AU_FN int16_t trino_t3au_rd16(const uint8_t* p)
{
    return (int16_t)((uint16_t)p[0] | (uint16_t)p[1] << 8);
}

// Decode and check the first TRINO_T3AU_HEADER bytes of a file of
// `file_size` bytes. 1 when the file holds exactly what the header says:
// its contexts, then data enough for every sample (whole DSP-ADPCM frames;
// a chunked sound one context per chunk), nothing after. 0 otherwise
// (`out` is then undefined). 64-bit sums: no product wraps.
TRINO_T3AU_FN int trino_t3au_parse(const uint8_t* h, uint64_t file_size, trino_t3au_header_t* out)
{
    if (file_size < TRINO_T3AU_HEADER || h[0] != 'T' || h[1] != '3' || h[2] != 'A' ||
        h[3] != 'U' || h[4] > 1 || h[5] >= TRINO_T3AU_BUSES || h[7] != 1)
        return 0;
    out->adpcm = h[4];
    out->bus = h[5];
    out->looped = h[6] & 1;
    out->sample_rate = trino_t3au_rd32(h + 8);
    out->nsamples = trino_t3au_rd32(h + 12);
    out->data_bytes = trino_t3au_rd32(h + 16);
    out->chunk_samples = trino_t3au_rd32(h + 20);
    out->ncontexts = trino_t3au_rd32(h + 24);
    for (int i = 0; i < 16; i++) out->coefs[i] = trino_t3au_rd16(h + 28 + i * 2);
    uint64_t n = out->nsamples, chunk = out->chunk_samples, ctx = out->ncontexts;
    if (ctx > TRINO_T3AU_MAX_CONTEXTS || chunk > TRINO_T3AU_MAX_CHUNK) return 0;
    uint64_t data_offset = TRINO_T3AU_HEADER + ctx * 8;
    if (data_offset + out->data_bytes != file_size) return 0;
    out->data_offset = (uint32_t)data_offset;
    if (out->adpcm) {
        // A chunk is whole frames, and each one starts at its context.
        if (chunk && (chunk % 14 != 0 || ctx != (n + chunk - 1) / chunk)) return 0;
        if ((n + 13) / 14 * 8 > out->data_bytes) return 0;
    } else if (n * 2 > out->data_bytes) {
        return 0;
    }
    out->stream = (h[6] & 2) && chunk && n;
    return 1;
}

// One chunk's DSP-ADPCM context (8 bytes): what `ndspAdpcmData` holds.
TRINO_T3AU_FN void trino_t3au_context(const uint8_t* c, uint16_t* index, int16_t* history0,
                                      int16_t* history1)
{
    *index = c[0];
    *history0 = trino_t3au_rd16(c + 2);
    *history1 = trino_t3au_rd16(c + 4);
}

#endif
