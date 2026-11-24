> Run 1: the mirror looked back from the camera's eye (the hub's edge: only sky), and there was no map row yet. The other rows are the same layouts as run 4.

Adapter: AMD Radeon Graphics (RADV RENOIR) (Vulkan); castle64 hub at 1920x1080, 4x MSAA, the game's SSAO, GPU T&L. Every layout warmed for 120 frames, then 8 interleaved rounds of 40 frames each (update + renders + the GPU's work, waited for).

| Frame | Targets | Median (ms) | p95 (ms) | Added (ms) | CPU T&L (ms) | Draws | Triangles |
|---|---:|---:|---:|---:|---:|---:|---:|
| screen only | 0 | 7.98 | 8.49 | +0.00 | 0.003 | 27 | 629 |
| rear-view mirror 480x160 | 1 | 8.13 | 8.58 | +0.15 | 0.003 | 54 | 641 |
| monitor 480x270, the same view | 1 | 8.09 | 8.52 | +0.11 | 0.006 | 54 | 1258 |
| monitor 960x540, the same view | 1 | 8.15 | 8.65 | +0.17 | 0.006 | 54 | 1258 |
| 4 monitors 256x144, the same view | 4 | 8.54 | 9.05 | +0.56 | 0.012 | 135 | 3145 |
