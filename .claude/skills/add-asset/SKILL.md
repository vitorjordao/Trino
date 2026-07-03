---
name: add-asset
description: Add a new asset (sprite, sound, music, model) to a Trino game — where to put masters, how to write the per-platform manifest, and how to rebake.
---

# add-asset

> The asset pipeline lands in **Fase 2** (see PLANO_EXECUCAO_TRINO.md). Until then this
> skill documents the layout so files land in the right place.

## Layout

- Master files go in `assets/shared/<category>/<name>.<ext>` (PNG, WAV, GLTF).
- A platform-specific replacement goes in `assets/<platform>/<same relative path>` and
  **wins over** the shared master for that platform only.
- Each asset directory may have a `manifest.toml` declaring per-platform formats:

```toml
[sprites.player]
n64 = { format = "CI4" }      # 4-bit palettized, fits TMEM
3ds = { format = "RGBA8" }
# pc defaults to RGBA8
```

## Rules

- N64 texture budget is 4 KB TMEM. `Caps::N64.validate_texture` is the check the
  pipeline applies; prefer CI4/CI8 formats and small sprites.
- An asset that a platform cannot represent is a **build error**, never a silent
  fallback.
- Handles (`SpriteId` etc.) derive from the logical path (`sprites/player`), so renaming
  a file is a breaking change for scenes referencing it.

## After adding (from Fase 2 on)

```
cargo xtask assets <platform>   # bake
cargo xtask test                # pipeline snapshot tests must still pass
```
