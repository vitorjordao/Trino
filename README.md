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
  Before: the wave and flush water and the wet road as model strips, each in its own plane
  of the road. After: one dynamic mesh per body of water and per wet patch. Same fixed
  camera, same frame after the event's warning, 1920×1080 crops.
  - `wave_beach.jpg` and `wave_beach.gif`: the beach wave rising, running back, and the
    wet road it leaves.
  - `wet_drying.jpg`: the wet road drying. Before, in bands where the strips overlap;
    after, as a gradient from the border in.
  - `flush_sewer.jpg`: the acid flush on the sewer track.
  - `wave_moat.jpg`: the moat wave, which spills up to the wall.
