# Evidence: dynamic meshes (`Renderer::draw_mesh`, ADR 0042)

Branch `feat/dynamic-mesh`. Files:

- `same_pixels_gpu.png`, `same_pixels_cpu.png`: written by
  `crates/platform-pc/tests/dynamic_mesh.rs::a_dynamic_mesh_draws_the_pixels_of_the_baked_model`.
  Top: baked models drawn with `draw_model_matrix`; bottom: the same vertices drawn
  with `draw_mesh`. The surfaces are lit, flat, tinted, translucent, textured and water.
  Each pair is byte for byte identical: GPU T&L in the first file, CPU T&L in the second.
- `oracle.png`: written by `dynamic_meshes_on_the_gpu_match_the_cpu_path`. Rippling
  sheets with per-vertex color and alpha. Left: GPU path; middle: CPU path; right: the
  difference ×8. This is `tnl_oracle::compare`, held to the same tolerance as every native
  golden.
- `bongo-kart/`: the game that asked for it (Bongo Kart, branch `wt/onda-malha`).
  Before: the wave water as model strips. After: one dynamic mesh per body of water and
  per wet patch. Same camera, same simulation frame, 1920×1080.
