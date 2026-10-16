# Platform guides

Every command goes through `cargo xtask` (an alias in `.cargo/config.toml`), so the steps
are the same on Linux, Windows and macOS. Setup notes are also in
[CONTRIBUTING.md](../CONTRIBUTING.md); [HARDWARE.md](HARDWARE.md) covers real consoles.

## PC

Needs only Rust (stable, picked by `rust-toolchain.toml`). On Linux, install `pkg-config`
and the ALSA headers (`libasound2-dev`).

```sh
cargo xtask run pc            # build and launch castle64
cargo xtask editor            # the editor
cargo xtask watch pc          # live reload: game code as a dylib, assets rebaked on save
                              #   --game <crate> picks the game to rebuild (default castle64)
cargo xtask test              # the workspace suite (same as CI)
```

Run modes and switches for `trino-app-pc`:

| Variable | Effect |
|---|---|
| `TRINO_SIM=native\|n64\|3ds` | Console simulation profile (default `n64`): internal resolution, filtering, color depth, the 3DS's two screens |
| `TRINO_LOOK=off\|n64` | Turns the N64 output emulation off or on, independently of the profile |
| `TRINO_STRICT=1` | Strict mode: panic when content exceeds the profile's `Caps` |
| `TRINO_SCENE=scenes/x.scene.ron` | Play an editor scene instead of the game; it reloads when the scene, its prefabs or materials change |
| `TRINO_GAME=showcase` | Run `examples/showcase`, the demo scene exported to code |
| `TRINO_SMOKE_FRAMES=N` | Exit after N frames (CI smoke runs) |

Keyboard mapping: A/B = Z/X, X/Y = C/V, L/R = Q/E, Start = Enter, Select = Right Shift,
D-pad = arrows, stick = WASD. The mouse feeds `InputState::pointer`. Gamepads go through
gilrs behind the `gamepad` feature of `trino-platform-pc` and `trino-runner-pc`; on Linux
it links libudev (`libudev-dev`), so it is opt-in there. Keys and pad buttons can be
rebound from the game (`Platform::rebind`).

The editor has its own guide in the [editor](../.agents/skills/editor/SKILL.md) skill,
including hooks that let a script or an agent screenshot the window.

## Nintendo 64

**Toolchain:** Docker. `cargo xtask build n64` builds the pinned `trino-n64` image the first
time (libdragon pinned by commit, about ten minutes), then the C shim, the RSP overlay,
the assets and the ROM. Only rustc runs on the host, on nightly with `-Zbuild-std`
(`rustup toolchain install nightly --component rust-src`) against
`platforms/n64/mips-nintendo64-none.json`. On Windows, Docker must be reachable from WSL2;
a plain dockerd inside WSL is enough.

**Emulator:** [ares](https://ares-emu.net) v148, found at `$TRINO_ARES` or `ares-v148/` in
the repo root (gitignored). `cargo xtask emulators setup ares` builds a headless ares image
(Xvfb, Mesa lavapipe); `TRINO_ARES=docker` uses it, which is what CI does.

```sh
cargo xtask build n64         # target/n64/trino.z64
cargo xtask run n64           # build and open in ares
cargo xtask test n64          # self-test ROM: every step, the golden frames, the perf gates
cargo xtask test n64 --bless  # rewrite the goldens and the N64 perf ceilings (review them)
cargo xtask watch n64         # rebuild and relaunch on save
```

`test n64` reads the ROM's ISViewer output (`TRINO_TEST_PASS`/`TRINO_TEST_FAIL`), compares
framebuffer dumps with `tests/golden/n64/` pixel for pixel, and checks the emulated frame
cost per scene and stage against `tests/perf/castle64_n64.toml`. Measuring and improving
that cost is the [n64-performance](../.agents/skills/n64-performance/SKILL.md) skill.

Things to know:

- Rust is compiled as o32 and libdragon as o64. Every C call goes through the shim with at
  most four scalar or pointer arguments, and no scalar float after an integer argument
  ([ADR 0002](adr/0002-n64-abi-o32-staticlib.md)).
- The depth buffer is 16-bit. Draw wide worlds at a reduced scale (1/128 works) and keep
  coplanar surfaces apart.
- The main stack is 64 KB, and an overflow is silent. Build large structs in place on the
  heap; the self-test fails above 60 KB.
- Troubleshooting: the [build-n64](../.agents/skills/build-n64/SKILL.md) and
  [run-emulator](../.agents/skills/run-emulator/SKILL.md) skills.

## Nintendo 3DS

**Toolchain:** a local [devkitPro](https://devkitpro.org/wiki/Getting_Started) install with
the `3ds-dev` packages, found at `$DEVKITPRO`, `C:\devkitPro` or `/opt/devkitpro`. No
Docker. Rust uses the built-in `armv6k-nintendo-3ds` target on nightly with
`-Zbuild-std`. The shim covers libctru, citro2d (sprites) and citro3d (all 3D, with
per-vertex depth).

**Emulator:** [Azahar](https://azahar-emu.org), found at `$TRINO_AZAHAR`, in
`target/emulators/azahar/` (`cargo xtask emulators setup azahar` fetches the pinned
AppImage on Linux x86_64), in the default install directory or on `PATH`. Headless runs
use Xvfb.

```sh
cargo xtask build 3ds         # target/3ds/trino.3dsx
cargo xtask run 3ds           # build and open in Azahar
cargo xtask test 3ds          # self-test app: every step and the golden frames
cargo xtask test 3ds --bless  # rewrite the 3DS goldens (review the PNGs)
cargo xtask build 3ds --selftest && cargo xtask test 3ds --no-build   # build now, boot later (CI's split)
cargo xtask watch 3ds         # rebuild and relaunch on save
```

`test 3ds` reads the `TRINO_TEST_*` lines from Azahar's log (`svcOutputDebugString`) and
compares the top and bottom screens with `tests/golden/3ds/` within a tolerance, because
Azahar's pixels depend on the host's OpenGL.

Things to know:

- The bottom screen is 320x240 and touch-enabled. Draw to it after
  `renderer.on_screen(Screen::Bottom)`, and set a camera for it every frame.
  `InputState::pointer_on(Screen::Bottom)` gives touches in its own pixels.
- Never draw 3D with `C2D_DrawTriangle`: it has one depth per triangle. All 3D goes
  through the shim's citro3d path.
- Troubleshooting: the [build-3ds](../.agents/skills/build-3ds/SKILL.md) and
  [run-emulator](../.agents/skills/run-emulator/SKILL.md) skills.

## External projects

A game in its own repository builds for the consoles with the same commands
([ADR 0008](adr/0008-external-console-projects.md)). It needs a `trino.toml` and a local
engine checkout, usually an `engine/` git submodule:

```sh
cargo xtask build|run|test|watch n64|3ds --project <dir>   # from the engine checkout
cargo trino build n64                                       # from a standalone project (alias)
```

xtask also picks the project from `$TRINO_PROJECT` or a `trino.toml` in the current
directory. `cargo xtask new <name> --standalone <dir>` scaffolds such a project; the
[new-project](../.agents/skills/new-project/SKILL.md) skill describes the `trino.toml` keys
and the `n64_main!`/`n3ds_main!` hosts.

## Real hardware

`cargo xtask run n64 --device sc64|everdrive` and `cargo xtask run 3ds --device network
--ip <IP>` send the build to a console and print its debug log in the terminal. They have
been tested with stub loaders, not yet on physical devices. See
[HARDWARE.md](HARDWARE.md).
