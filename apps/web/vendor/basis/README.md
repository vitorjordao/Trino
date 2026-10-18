# Basis Universal transcoder (vendored)

Binomial's Basis Universal transcoder compiled to WebAssembly, the files
three.js ships for its `KTX2Loader`. The Web backend loads them at boot
(`trino_platform_web`, `glue.js`) to transcode the game's KTX2 textures to
BC, ETC2, ASTC or RGBA8 — whichever the browser's GPU samples
(ADR 0017 §6). `cargo xtask build web` copies this folder to
`target/web/basis/`.

| File | SHA-256 |
|---|---|
| `basis_transcoder.js` | `8478b5b6d6b74e7d3082b89f6417321d8d1dc0307f2b30d4484bb11b441696a1` |
| `basis_transcoder.wasm` | `6cf17dc889352c42e9acf8897107978d127005fe3386c36a0e3845e27967630a` |

- **Source:** the npm package `three@0.180.0`,
  `examples/jsm/libs/basis/` (`npm pack three@0.180.0`).
- **Upstream:** <https://github.com/BinomialLLC/basis_universal>
  (`webgl/transcoder`).
- **License:** Apache License 2.0 (Binomial LLC). The engine's own code is
  MIT OR Apache-2.0.

The host side (the bake, `texture inspect`, the editor, the tests) uses the
same transcoder natively through the `basis-universal` crate, so the PSNR a
bake measures is what a browser shows: `crates/asset-pipeline/src/texture/
ktx2.rs` and the browser self-test's `ktx2 transcode` step check that both
decode a texture to the same pixels.

To update: `npm pack three@<version>`, copy the two files, update the hashes
above and run `cargo xtask test web`.
