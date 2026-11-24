# Evidence: Bongo-Kart epic #24, renderer (Trino PR "mipmaps, additive blending, sky/ground + rim light, two-LUT mix")

- `sheets/`: labeled side-by-side sheets used in the PR body.
- `raw/`: the PNGs the tests write (`target/evidence/*` in a Trino tree):
  - `mipmaps/` from `crates/platform-pc/tests/mipmaps.rs`;
  - `additive/` from `crates/platform-pc/tests/additive.rs`;
  - `light-terms/` from `crates/platform-pc/tests/light_terms.rs`;
  - `pc-gpu-tnl/light_terms_*` from the GPU T&L oracle (`crates/platform-pc/tests/gpu_tnl.rs`);
  - `lut-mix/` from `crates/platform-pc/tests/post.rs` (`TRINO_POST_SHOTS=<dir>`).
