Adapter: AMD Radeon(TM) Graphics (Vulkan), 4x MSAA, 240 measured frames per row (update + render + GPU)

| Scene | Size | T&L | Frame avg (ms) | p99 (ms) | CPU T&L (ms) | Draws | On GPU | Draw calls |
|---|---|---|---:|---:|---:|---:|---:|---:|
| castle64 hub | 1920x1080 | CPU | 14.16 | 14.98 | 0.05 | 26 | 0 | 6 |
| castle64 hub | 1920x1080 | GPU | 13.91 | 14.64 | 0.00 | 26 | 26 | 17 |
| castle64 hub | 3840x2160 | CPU | 95.00 | 96.65 | 0.05 | 26 | 0 | 6 |
| castle64 hub | 3840x2160 | GPU | 92.89 | 97.81 | 0.01 | 26 | 26 | 17 |
| Solid Eleven match | 1920x1080 | CPU | 17.00 | 18.26 | 0.80 | 234 | 0 | 10 |
| Solid Eleven match | 1920x1080 | GPU | 15.87 | 16.97 | 0.00 | 234 | 234 | 39 |
| Solid Eleven match | 3840x2160 | CPU | 91.81 | 94.40 | 0.85 | 234 | 0 | 10 |
| Solid Eleven match | 3840x2160 | GPU | 91.30 | 93.10 | 0.00 | 234 | 234 | 39 |
| 2000 robots (instanced) | 1920x1080 | CPU | 40.50 | 63.16 | 7.37 | 2001 | 0 | 1 |
| 2000 robots (instanced) | 1920x1080 | GPU | 5.96 | 6.36 | 0.00 | 2001 | 1990 | 2 |
| 2000 robots (instanced) | 3840x2160 | CPU | 69.19 | 85.55 | 6.57 | 2001 | 0 | 1 |
| 2000 robots (instanced) | 3840x2160 | GPU | 15.23 | 15.68 | 0.00 | 2001 | 1990 | 2 |
