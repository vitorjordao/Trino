// Opus wav64 support (compression level 3), linked by `cargo xtask build
// n64` only when the bake found an Opus asset (`target/n64/opus`): it pulls
// in libopus, which the shim's weak `trino_audio_opus_init` doesn't.
#include <stdint.h>
#include <libdragon.h>

uint32_t trino_audio_opus_init(void)
{
    static int done;
    if (!done) {
        wav64_init_compression(3);
        // audioconv64 always resamples Opus to 48 kHz, above the default
        // channel limit (the output rate); the limits size each channel's
        // sample buffer, so this costs ~9% more of it.
        for (int ch = 0; ch < 16; ch++)
            mixer_ch_set_limits(ch, 16, 48000, 0);
        done = 1;
    }
    return 1;
}
