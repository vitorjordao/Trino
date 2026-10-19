# ADR 0025: Shadows per tier

Status: accepted (October 2026, GitHub #57)

## Context

Trino lit a draw with a directional light, an ambient term, fog, local lights (ADR 0010)
and AO (ADR 0014), but cast no shadows. Every game faked them by hand:

- Solid Eleven shipped `player_shadow.glb` and `blob.glb` and drew them under each player
  and the ball, dropping them for far players to save N64 time.
- castle64 drew a disc model under the player at a hand-tuned height. Discs like it
  z-fought the floor on the N64's 16-bit-precision depth (Plan & Party issue #18).
- Plan & Party wrote its own PC-only shadows.

The targets afford very different things. The N64 can draw a textured quad for almost
nothing but cannot render a depth map. The 3DS has a stencil buffer and a vertex shader
that can flatten a mesh. The PC and the Web can afford a shadow map.

## Decisions

### 1. One caster model, three tiers (`Caps::shadow_tier`)

`trino_core::shadow::ShadowTier` is `None < Blob < Planar < Map`:

| Target | Tier | What a caster draws |
|---|---|---|
| N64 (both RAM tiers) | `Blob` | a soft disc on the receiver under it; up to `Caps::max_planar_shadows` (2) `Mesh` casters a frame are projected instead |
| 3DS, New 3DS | `Planar` | its mesh flattened onto the receiver along the light, drawn once per pixel (stencil) |
| PC native, Web | `Map` | a depth map from the light, filtered (PCF), on every surface |

- A game describes each draw once with `ModelParams::shadow: ShadowCast`:
  - `None`: nothing (the default, so existing draws are unchanged).
  - `Blob { radius }`: a blob on every tier.
  - `Mesh`: the shape wherever the tier draws shapes. On the Blob tier, the frame's first
    two are projected and the rest cast nothing (`ShadowStats::over_budget` counts them).
  - `Auto { blob_radius }`: the shape where the tier draws shapes, otherwise a blob
    (none at radius 0, for a character's limbs when its body casts the blob).
- The effective tier is the lowest of `Caps::shadow_tier`, the game's
  `ShadowSettings::tier_max` and the PC quality preset's cap. On the native profile,
  `shadows: Off` caps the tier at `Planar`: casters stay grounded, and no map pass runs.
- The PC's console profiles draw the console's tier: blobs on `n64`, planar shadows on
  `3ds`. The editor's Game view, the goldens and the budget therefore show what the
  console shows.

### 2. API

- `Renderer::set_shadows(Option<&ShadowSettings>)` sets `tier_max`, `map_size` (0 = the
  preset's), `distance`, `softness` and `color`. Alpha is the strength. `clamped()` keeps
  the settings in range.
- `Renderer::set_shadow_receivers(&[ShadowPlane])` sets the surfaces blob and planar
  shadows land on: rectangles on planes, at most `MAX_SHADOW_PLANES` (32).
  `ShadowPlane::top_of(min, max)` is the top of a box. `ground(y)` is an endless floor.
- Both are cleared at `begin_frame`, like AO, so shadows never leak from one game or step
  into the next. Both have default no-ops. Like every earlier `ModelParams` field,
  `shadow` does not bump `GAME_API_VERSION`: a core change already needs a restart.
- A caster's shadow goes onto the nearest receiver below its bounding-sphere center whose
  rectangle it is over, and is cut to that rectangle. The Map tier ignores receivers: the
  map shades every surface.

### 3. Depth-safe placement, not hand-tuned lifts

Blob and planar triangles lie exactly on the receiver plane and are moved
`SHADOW_Z_STEPS` (4) steps of the target's depth buffer toward the camera
(`shadow::shadow_z`). The step comes from the target's `Caps`:

- `depth_bits` and `depth_mantissa` describe the buffer.
- On the N64 they describe the RDP's 18-bit compressed z (11 mantissa bits), whose step
  grows toward the far plane. `Caps::depth_step` returns that step at any z.

The offset is invisible near the camera and still beats the buffer's precision far away.
The PC test `blobs_show_at_every_distance_on_a_large_floor` checks blobs from 4 m to 120 m
on one floor. In ares, the self-test's `shadows golden frame` step dumps
`tests/golden/n64/shadows_far.png`: the same floor at 1/128 scale on the console's own
z-buffer. No blob fights the floor.

### 4. The tiers' backends

- **Blob, N64:**
  - A cast is queued (`PendingBlob`). It is built in `flush_shadows` after the batch's
    opaque draws, so the RSP draw path keeps its I-cache locality.
  - Each blob is four corners projected on the CPU and two textured triangles through
    `trino_3d_shadow` + `trino_tri_tex`. The 32×32 I8 falloff texture is made once per
    softness.
  - The mode is blended, z-tested and never z-written, so blobs need no sort among
    themselves.
  - Cost on ares (the `shadow cost` step: 22 casters, frame time less its vsync wait,
    with and without blobs): **103–113 µs per blob** over two builds and both RAM tiers
    (code layout moves it).
    `N64_COST::us_per_blob` = 111 µs. The 22-blob frame is ~14.7 ms. The step fails past
    33.3 ms, so a Solid Eleven crowd keeps 30 fps.
- **Planar, N64:**
  - At most two casters a frame. The caster's mesh goes through the CPU T&L with the
    flattening matrix (`planar_matrix`: the ray–plane intersection along the light) and
    is drawn with `trino_tri` in the shadow color.
  - Overlaps blend twice. The N64 has no stencil, and two casters rarely overlap.
- **Planar, 3DS:**
  - The GPU T&L flattens the mesh with its uniforms when the mesh has a GPU copy and the
    shadow lies wholly past the near plane. Otherwise the CPU path clips it.
  - The stencil draws every pixel once: the receiver's quad marks, the shadows draw on
    the mark, the quad unmarks. Two overlapping shadows darken once, as one shadow.
  - Through the shim's `trino_3d_material` flag bits 9–13 (a shadow, its stencil part,
    the blob texture) and `trino_shadow_blob`, four arguments at most (ADR 0002).
- **Map, PC native and Web** (`renderer/shadow_map.rs`, `shadow_map.wgsl`):
  - `shadow::map_fit` fits an orthographic box from the light over the camera's first
    `distance` meters and the casters above it. The box moves in whole texels, so
    shadows don't shimmer as the camera moves.
  - The casters' posed world triangles are drawn into a `Depth32Float` map. A
    fullscreen composite then reads the scene's depth (MSAA and a render scale
    included) and rebuilds each pixel's world position and normal.
  - The position is pushed off its surface along the normal, further the more the
    surface slopes away from the light, and compared against the map (minus a depth
    bias) with bilinear PCF taps.
  - Shadowed pixels blend toward the shadow color. The blend fades toward the shadow
    distance, the map's edge and the fog, and on surfaces turning away from the light,
    which the lighting already darkens.
  - The quality preset sizes the map (1024 on Low, 2048 above) and sets the taps (2×2,
    3×3, 4×4). The pipelines are built on the first frame with a mapped caster, so the
    console profiles and games without shadows never pay for them.
  - On the Web this is the same renderer (WebGPU, or WebGL2 through its depth sampler
    binding).

### 5. Debug views: `ShadowView` and `TRINO_SHADOWS`

- `PcRenderer::set_shadow_view(ShadowView::{Game, Off, Debug})`:
  - `Game` (the default): as the game sets them.
  - `Off`: none.
  - `Debug`: the game's settings, or the defaults when it set none, in a loud magenta at
    full strength.
- A game window reads `TRINO_SHADOWS=off|debug`. Headless renderers ignore it.

### 6. Authoring: scenes v7, prefabs v5, TSCN v5

- `Environment::shadows: Option<ShadowsDef>`. Every field defaults, so `shadows: Some(())`
  is valid. `enabled: false` keeps the tuning and draws none.
- `Entity::shadow: Option<ShadowComponent { cast, radius, receive }>`.
  - `cast: None | Blob | Mesh | Auto` (default `Auto`), `radius` 0.5 m by default.
  - `receive: true` makes the top of the entity's box a receiver. The box is its
    collider (where things stand), or else its model's bounds as drawn. A receiver with
    neither warns.
  - Prefab parts carry their own component, so a character prefab casts and its
    particles don't.
- Older files migrate with no shadows (v6 → v7, v4 → v5). A scene without the block
  writes nothing new.
- Codegen exports `pub const SHADOWS: Option<ShadowSettings>` and
  `pub const RECEIVERS: &[ShadowPlane]`, plus each node's `shadow`.
- TSCN v5 adds 16 bytes of environment and 28 bytes per node: the cast and its radius,
  the receiver flag, center and half sizes. `BakedScene::shadows()` and `receivers()`
  read them back. Games load either form with
  `set_shadows(SHADOWS.as_ref())` + `set_shadow_receivers(RECEIVERS)`. The showcase and
  `apps/pc`'s scene player do this.

### 7. Budget and CLI

`trino_scene::budget::estimate` plans every cast as the target's renderer would. That
covers the tier, the N64's planar budget and the casters past it. The estimate reports
`FrameBudget::shadows { tier, casters, blobs, planar, mapped, over_budget, receivers }`
and adds the blobs' time to the N64 CPU estimate.

- `cargo xtask scene budget` prints a `shadows:` line per platform. `--json` adds a
  `shadows` object.
- The editor's Game-view HUD and Inspector › Shadows show the same lines.
- `scene_budget.rs` checks the estimate against the PC renderer's own counters on the
  N64 profile.

### 8. Editor

- **Inspector › scene settings › Shadows:** on/off, best tier, distance, softness, color,
  map size, reset. Under them, each console's and the PC's line from the budget.
- **Inspector › entity › Shadow** (from Add component): cast, radius, receive, and what
  each tier draws for this entity. A bare collider gets a receiver; a model gets a
  caster.
- **The Game view previews each platform's tier live** through its profile buttons. The
  Scene view draws the native tier.
- **The Scene view's shadow mode** is in the toolbar (the sun-on-horizon button) and in
  View › Shadow view: "Shadows: as tier / off / debug". Debug also outlines every
  receiver's rectangle and every caster's bounds in the debug color.
- **Hooks:** `TRINO_EDITOR_SHADOWS=tier|off|debug` and `TRINO_EDITOR_INSPECT=shadows`.

### 9. The games

- **castle64:** the player's torso casts `Auto(0.4)` and its limbs cast `Auto(0)`, onto
  the top of the solid under the player. That is a blob on the N64, the body's shape on
  the 3DS, and the map on the PC. The `c64_shadow` disc and its generator are gone.
- **Solid Eleven** (`part_shadow`), onto the turf:
  - On the PC native look (`Detail::Full`), every part and the hair cast their shape
    into the map.
  - On the consoles, a near player's pelvis casts a blob. A far player casts none, as
    before, which keeps the N64 match frame where it was.
  - The ball casts its shape on the hi-fi look and a blob otherwise.
  - The hand-made `player_shadow` and `blob` models are gone.
  - The football template's N64 match frame stayed where it was: 31.0–31.7 ms on ares
    over two builds against main's 32.1 ms, within layout noise (no win is claimed).

### 10. Phases 4 and 5: evaluated, not built

- **An RSP shadow command (`TrinoCmd_DrawShadow`).**
  - An RSP draw still costs the CPU ~90 µs per call (`N64_COST::us_per_rsp_draw`, from
    Solid Eleven's parts). A blob costs 111 µs, so moving blobs to the RSP would save at
    most ~20 µs each, ~0.5 ms for 22 blobs.
  - That is not worth a new overlay command and its oracle. Only the N64's two planar
    casters would gain more, and those are rare hero draws.
  - `rsp_check` needs no change. The caster's own draw stays on the RSP fast path, and
    its shadow is a separate CPU draw: `ModelParams::shadow` plays no part in
    `rsp_eligible_mv` or the RSP memo.
  - Revisit if a game needs many projected shapes on the N64.
- **Baked static shadows** (vertex colors of static receivers, as baked AO does):
  complementary and zero-cost at run time on the consoles. It needs scene-level baking
  (receiver models per instance), which ADR 0014's follow-up already lists. It is left
  as that follow-up.

### 11. Tests

- Core unit tests:
  - the planar matrix against a ray–plane reference;
  - the depth offset against every target's precision, the RDP's compressed steps
    included;
  - blob placement and cutting, receiver clipping, the map fit, tier planning.
- PC goldens per tier: `shadows_native.png`, `shadows_n3ds.png`, `shadows_n64.png`. Probes
  check that the ground darkens under each caster on every tier. Other tests cover the
  3DS's draw-once overlap, the quality caps, MSAA and render scale, and the views.
- The `shadows` self-test step on the PC, in ares (both RAM tiers) and Azahar (both
  models). It checks a darker floor where the post's and the ball's shadows land, and an
  unchanged open floor. ares also runs `shadows golden frame` and `shadow cost`. The
  browsers run the step too; its pixel probes are skipped there (no synchronous
  `read_pixel`), and the castle64 golden frame holds the map's shadow.
- Scene v7, TSCN v5 and codegen round trips; the budget against the renderer and the
  CLI; an editor kittest plus the layout audit at 960×600 and 1600×940.

### 12. Contact hardening and distance attenuation (2026-10-19, GitHub #86 item 9)

Issue #86 asked for "smooth projected soft contact shadows with distance attenuation" on
the PC and the Web, blobs kept on the consoles. The audit found most of it done by this
ADR: the Map tier already shadowed Solid Eleven's players and ball by their shapes (PCF),
and the blob already faded and shrank with its caster's height (§4: to 60 % and nothing at
`4·radius + 1` m). What was missing was the map's dependence on height: a shadow as sharp
and dark 2 m under a ball in the air as under a boot.

- **Two fields on `ShadowSettings`, plain `f32` (no `Option`: the N64 pitfall):**
  - `soft_growth`: how much the soft edge widens per world unit between caster and
    receiver along the light. Unitless (the light's apparent size), so it holds at any
    drawing scale.
  - `fade_distance`: the caster–receiver distance at which the shadow is gone, world
    units (a game drawn at 1/64 multiplies it, as it does `distance`). Linear fade.
  - Both 0 by default: **every existing frame is unchanged**. All twelve frames
    `crates/platform-pc/tests/shadows.rs` writes (every tier, MSAA on and off, a 0.6
    render scale, the quality cap) are byte-identical to the same test built from the
    base commit in a separate worktree on the same GPU (2026-10-19). The goldens
    themselves can't prove it: they were blessed on another machine and differ from
    this one's frames by driver noise on every tier. `clamped` turns NaN and negatives
    into 0, caps `soft_growth` at 4, and lets `fade_distance` be infinite.
- **The map composite (`shadow_map.wgsl`), PCSS-lite:** only when either field is set
  (a uniform branch):
  1. *Blocker search,* after the even filter ran: nearest reads of the map's depth (no
     comparison) on a sparse 4×4 grid as wide as the filter may grow, plus, where the
     even filter found shade, at its own taps (so the two agree on thin casters). Lit
     ground far from every caster pays 16 reads and nothing else. Texels count when they are in front of the receiver by
     more than a texel's depth per texel off center (a receiver sloping up to 45° to the
     light never finds itself).
  2. Their mean depth gives the distance to the receiver along the light, world units
     (`fit.depth` converts map z).
  3. The filter radius grows by `soft_growth · dist / 2` (uv: `/ 2·fit.radius`), up to
     three times the preset's taps per side 2.5 texels apart (Low 6, Medium 9, High 12
     per side). The tap count follows the radius, so bilinear compares never leave gaps.
  4. The shadow's strength is multiplied by `1 − dist / fade_distance`.

  Without a blocker found, the pixel keeps the even filter at full strength (ADR 0025's);
  with both fields 0 the shader runs only the old arithmetic and a `× 1.0`.
- **Cost:** Solid Eleven at 1920×1080 with 4× MSAA and SSAO on the Renoir iGPU
  (`solid_eleven_shots pc_frame_time`, the day match): 9.24 ms off, 10.16 ms on in the
  quietest pair of six runs, so about +0.9 ms (the machine was loaded, runs spread
  9–17 ms). A first version that searched every pixel's even-filter taps too cost about
  twice that.
- **WebGL2:** naga has no GLSL for a `textureLoad` of a `texture_depth_2d`, and a
  sampler-less read of a depth texture reads 0 under wgpu's GL backend. `MapPass::new`
  binds the map a second time as an unfilterable float texture (binding 5) and reads it
  through the nearest sampler (binding 6), as `depth_as_float` does for the scene depth
  (`map_read_as_float`; `webgl2_shaders_translate_to_glsl_es_300` checks both the
  rewrite and that the raw read fails). Verified at run time once, with castle64's player
  shadow temporarily opted in: `cargo xtask test web --browsers chromium` drew the same
  softened shadow on WebGL2 (SwiftShader) and WebGPU (a read of 0 would have faded it
  out). MSAA needs nothing new: the composite still
  reads the scene depth's first sample, and the map is single-sampled.
- **Blobs:** `fade_distance`, when set, replaces the `4·radius + 1` reach
  (`ShadowSettings::blob_fade`); 0 keeps it bit for bit. Still one division per blob,
  no new state per frame, no `Option` read. `soft_growth` does not apply to blobs (the
  blob texture's falloff is the softness).
- **Planar (3DS):** unchanged. A per-caster alpha by height would be cheap, but it moves
  the Azahar goldens; left as a follow-up if a game asks.
- **Authoring:** `ShadowsDef` gains both fields (optional, defaulted, not written when
  0; no scene version bump) and codegen writes them. **TSCN v5 does not carry them**: a
  baked scene gets 0 and 0 (ADR 0025's look), and changing that is a TSCN version bump,
  left as a follow-up with the editor's Inspector fields.
- **Solid Eleven opts in** (`soft_growth: 0.12`, `fade_distance: 7 m × WORLD`): its ball
  in the air casts a softer, fainter shadow the higher it flies. Only
  `solid_eleven_match_native.png` moved (0.5 % of pixels past 8/255) and was re-blessed;
  castle64, the showcase and every console golden stay as they were.
- **Tests:** `a_blob_fades_and_shrinks_with_its_casters_height`,
  `contact_settings_clamp_and_default_to_the_old_look` (core);
  `contact_shadows_harden_at_contact_and_fade_with_height` (a cube on the floor, then
  0.5 and 2 up: the shadow darkens the floor by 73 / 59 / 17 luma levels and its edge
  widens 0.21 / 0.27 / 0.39 m on RADV, 0.43 m at 2 on lavapipe; with both fields 0 the
  same probes read 87 and 0.145 m at every height) and
  `contact_shadows_hold_with_msaa_and_a_render_scale` (PC GPU, RADV and lavapipe); the
  uniform layout and the WebGL2 rewrite (render-wgpu unit tests).

## Consequences

- Every caster gets grounded on every target with one component and one setting per
  frame. Games stop shipping disc models and tuning lifts.
- The PC's console profiles are honest previews: what the editor shows on `n64` is what
  ares draws.
- N64 cost is linear and known: 111 µs per blob in the budget. Projected shapes are
  capped at two a frame.
- Follow-ups:
  - baked static shadows;
  - cascades on Ultra;
  - receivers other than horizontal box tops in the editor (the API takes any plane);
  - an RSP shadow command if the N64 ever needs more projected shapes;
  - contact hardening (§12) in TSCN and the editor's Inspector, and a height fade on the
    planar tier.
