# Evidence for PR (issue #112): animation layers

- `tiers.png`: one layered pose (running, aiming up-left) pushed into each tier's renderer: PC · 3DS (2 layers / 6 clips) · N64 + Expansion Pak (2/6) · base N64 (fold: 1 layer / 3 clips, the arms swing with the run).
- `aim_2d.png`: the upper layer's 2D blend space at 3 × 3 aims (yaw × pitch).
- `inertial.png`: the hop inertialized (top) vs crossfaded (bottom), five frames each.
- `slope.png`: feet on a 20° slope — IK height only (#65), then turned onto the ground (`FootSlope`).
- `editor_layers_pc.png`, `editor_layers_n64.png`: the editor's Graph mode on the `upper` layer (mask on the skeleton), PC look vs the N64 look's fold.

Regenerate: `cargo test -p xtask --test anim_layers_shots -- --ignored`.
