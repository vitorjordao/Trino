# Evidence for PR #137 (issue #136)

- `editor_mask3d.png`: the Animation tab's Graph mode on `anim/strider_layers`, the `upper` layer's mask (`torso/*`, 4 of 11 joints) picked in the 3D preview: lit dots on the masked joints.
- `kick_layers_pc.png` / `kick_layers_n64.png`: Solid Eleven's shot through six moments (60 Hz frames 4–34), from the side, on the PC body and the N64 one (`cargo test -p xtask --test solid_eleven_kick_shots -- --ignored`). Rows: (1) before #136, the shot clip replaces the whole body; (2) the shot as layers over a standstill; (3) over a jog; (4) over a sprint: hips, legs, torso and head kick while the arms keep the run's swing (the `arms` layer's weight falls with speed).
