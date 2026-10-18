# Evidence for PR #83 (issue #55: quality presets)

- `pc/`: castle64 at each preset, the Graphics page, dynamic resolution and frame pacing,
  measured on an AMD Radeon integrated GPU (Vulkan, Windows) with
  `cargo test --release -p xtask --test quality_shots -- --ignored`; `discrete_gpu.txt`
  is the same run on an RX 7800 XT.
- `editor/`: the Quality tab and the Game view at Low (dynamic resolution) and Ultra.
- `web/`: the Graphics page in Chromium, and the self-test logs of the four browser runs.
- `logs/`: the PC quality smokes (CI's step, run locally), `quality bench --json`, and the
  consoles' `quality` self-test step in ares and Azahar.
