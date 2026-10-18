Adapter: AMD Radeon RX 7800 XT (Vulkan), 4x MSAA, 240 measured frames per row (update + render + GPU)

| Scene | Size | T&L | Frame avg (ms) | p99 (ms) | CPU T&L (ms) | Draws | On GPU | Draw calls |
|---|---|---|---:|---:|---:|---:|---:|---:|
| castle64 hub | 1920x1080 | CPU | 0.79 | 0.96 | 0.03 | 26 | 0 | 6 |
| castle64 hub | 1920x1080 | GPU | 0.75 | 1.00 | 0.00 | 26 | 26 | 17 |
| castle64 hub | 3840x2160 | CPU | 2.08 | 2.25 | 0.03 | 26 | 0 | 6 |
| castle64 hub | 3840x2160 | GPU | 2.07 | 2.26 | 0.00 | 26 | 26 | 17 |
| Solid Eleven match | 1920x1080 | CPU | 1.78 | 2.44 | 0.67 | 234 | 0 | 10 |
| Solid Eleven match | 1920x1080 | GPU | 0.76 | 0.99 | 0.00 | 234 | 234 | 39 |
| Solid Eleven match | 3840x2160 | CPU | 3.34 | 3.78 | 0.73 | 234 | 0 | 10 |
| Solid Eleven match | 3840x2160 | GPU | 1.97 | 2.17 | 0.00 | 234 | 234 | 39 |
| 2000 robots (instanced) | 1920x1080 | CPU | 28.13 | 47.80 | 6.68 | 2001 | 0 | 1 |
| 2000 robots (instanced) | 1920x1080 | GPU | 0.46 | 0.64 | 0.00 | 2001 | 1990 | 2 |
| 2000 robots (instanced) | 3840x2160 | CPU | 25.16 | 46.50 | 6.24 | 2001 | 0 | 1 |
| 2000 robots (instanced) | 3840x2160 | GPU | 0.69 | 0.77 | 0.00 | 2001 | 1990 | 2 |
