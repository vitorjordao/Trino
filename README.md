# Evidence: render targets (Bongo-Kart #184, Trino ADR 0046)

From the render target PR on `feat/render-targets`.

| Path | What it shows | Made by |
|---|---|---|
| `images/frame-*.png` | castle64's hub at 1920×1080, as the runner draws it (4× MSAA, SSAO, GPU T&L), with its targets shown as framed sprites: a rear-view mirror from the player, a live map from above, monitors with the screen's own view (one 480×270, one 960×540, four 256×144) | `cargo test --release -p xtask --test target_bench -- --ignored --nocapture` |
| `images/target-*.png` | Each layout's first target, read back alone (`PcRenderer::read_target`) | the same |
| `bench/radeon-renoir-run{1..4}.md` | Four runs of the cost bench on the Ryzen 7 5825U's Radeon (RADV RENOIR) | the same |

What the bench found (ADR 0046 §4):
- **One target:** +0.05 to +0.17 ms a frame when the iGPU is in its fast clock state, +0.25 to +0.52 ms in its slow one.
- **Four monitors:** +0.53 to +1.07 ms.
- **The cost is per pass, not per pixel:** four 256×144 monitors cost twice one 960×540.
