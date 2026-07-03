---
name: add-asset
description: Add a new asset (sprite, sound) to a Trino game — master file placement, manifest declaration, per-platform formats, baking and live reload.
---

# add-asset

## Steps

1. Put the master file under `assets/shared/<category>/<name>.<ext>`:
   - sprites: PNG (RGB or RGBA)
   - sounds: WAV (PCM 16-bit or float, mono or stereo)
2. Declare it in `assets/manifest.toml` under a logical path:

```toml
[sprites.enemy]
file = "sprites/enemy.png"
formats = { n64 = "CI4" }   # REQUIRED for n64: CI4, CI8 or RGBA5551

[sounds.jump]
file = "sounds/jump.wav"
```

3. Reference it from game code by logical path (const, checked nowhere at
   compile time — bake validates):

```rust
pub const ENEMY: SpriteId = SpriteId::from_path("sprites/enemy");
pub const JUMP: SoundId = SoundId::from_path("sounds/jump");
```

4. Bake and verify: `cargo xtask assets pc` (and `n64`/`3ds` once those
   phases land) — any format/resolution problem is a bake **error**.
5. Run `cargo xtask test` — pipeline tests must stay green.

## Rules

- N64 textures must declare an explicit format; TMEM is 4 KB
  (`Caps::N64.validate_texture`). Prefer CI4/CI8, keep sprites small.
- A platform-specific replacement goes at `assets/<platform>/<same path>`
  and silently wins **for that platform only**. A missing source anywhere
  is a bake error, never a fallback.
- Renaming a logical path changes the handle — breaking for scenes/code
  referencing it. Renames are refactors, not edits.
- With `cargo xtask watch pc` (or `--features reload`), saving a master
  rebakes and re-uploads it live under the same handle — no restart.
