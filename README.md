# Evidence for PR #106 (issue #101, ADR 0031)

Measured on an AMD Renoir iGPU (RADV, Vulkan), 16 logical CPUs, with the driver's disk cache off. Regenerate:

    cargo test --release -p xtask --test precompile_shots -- --ignored --nocapture
    cargo test --release -p trino-platform-pc --test precompile_bench -- --ignored --nocapture
    python3 tools/evidence/precompile_charts.py target/evidence/precompile
