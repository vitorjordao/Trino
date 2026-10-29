// The 3DS shim's `T3AU` header check (`crates/platform-3ds/shim/
// t3au_header.h`, the file the shim includes), compiled for the host so the
// `t3au_shim` fuzz target can hold it to the asset pipeline's
// `T3au::parse` (issue #121). Built by `crates/fuzz/build.rs`; under
// `cargo fuzz` with AddressSanitizer too.

#include <stddef.h>
#include "../../platform-3ds/shim/t3au_header.h"

// `file` is a whole `.t3au` file. 1 and `out` filled when the shim would
// load it.
int trino_fuzz_t3au_parse(const uint8_t* file, size_t len, trino_t3au_header_t* out)
{
    if (len < TRINO_T3AU_HEADER) return 0;
    return trino_t3au_parse(file, (uint64_t)len, out);
}

// The shim's read of context `c` (8 bytes): predictor/scale and history.
void trino_fuzz_t3au_context(const uint8_t* c, uint16_t* index, int16_t* history0,
                             int16_t* history1)
{
    trino_t3au_context(c, index, history0, history1);
}

// TRINO_T3AU_BUSES, pinned to trino_core::Bus::COUNT by the tests.
uint32_t trino_fuzz_t3au_buses(void)
{
    return TRINO_T3AU_BUSES;
}
