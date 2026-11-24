> Run 4: the final bench, the table in ADR 0046 §4. The images in `images/` are from this run.

Adapter: AMD Radeon Graphics (RADV RENOIR) (Vulkan); castle64 hub at 1920x1080, 4x MSAA, the game's SSAO, GPU T&L. Every layout warmed for 120 frames, then 8 interleaved rounds of 40 frames each (update + renders + the GPU's work, waited for).

| Frame | Targets | Median (ms) | p95 (ms) | Added (ms) | CPU T&L (ms) | Draws | Triangles |
|---|---:|---:|---:|---:|---:|---:|---:|
| screen only | 0 | 8.69 | 9.10 | +0.00 | 0.006 | 27 | 629 |
| rear-view mirror 480x160 | 1 | 8.97 | 9.33 | +0.29 | 0.005 | 54 | 653 |
| live map 256x256 | 1 | 8.96 | 9.46 | +0.27 | 0.010 | 54 | 1224 |
| monitor 480x270, the same view | 1 | 9.04 | 9.49 | +0.35 | 0.010 | 54 | 1258 |
| monitor 960x540, the same view | 1 | 9.21 | 9.67 | +0.52 | 0.010 | 54 | 1258 |
| 4 monitors 256x144, the same view | 4 | 9.76 | 10.33 | +1.07 | 0.024 | 135 | 3145 |
