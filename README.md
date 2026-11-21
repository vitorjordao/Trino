# Evidence for PR #155 (radial speed blur)

Images and the clip referenced by the PR body.

- `castle64_speed_blur_grid.jpg`: castle64's hub at 1080p (`FILMIC` + `dusk`, 4× MSAA), the same
  moment at strength 0, 0.15, 0.3 and 0.8, center (0.5, 0.45)
  (`xtask/tests/post_bench.rs::speed_blur_cost_and_shots`).
- `castle64_speed_blur_corner.jpg`: its top-right corner at 1:1, strength 0, 0.3 and 0.8.
- `castle64_speed_blur_clip.mp4` / `.gif`: 120 frames, strength 0 → 0.3 → 0
  (`speed_blur_clip_frames`), 30 fps (the gif at 20 fps, 640 px).
- `post_test_gpu_vs_cpu.jpg`: `crates/platform-pc/tests/post.rs::speed_blur_streaks_toward_the_center_like_the_cpu_reference`'s
  frame: GPU, the CPU reference (`SpeedBlur::gather`, 256 taps) and |difference| ×4, at 0.3 and 0.8.
