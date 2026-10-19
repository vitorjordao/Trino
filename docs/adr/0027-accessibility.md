# ADR 0027: Accessibility on every target

Status: accepted (October 2026, GitHub #60). Every phase of the issue is implemented: the
settings and their persistence (§1), semantic palettes with color-vision alternatives and
`access check` (§2–§3, §8), captions (§4), the UI's text scale, high contrast and icons
(§5), motion, flashes and held input (§6), the editor's simulation and checklist (§9),
and the adopters (§10). The PC/Web correction filter (daltonization) stays with #64.

## Context

Trino had input rebinding (PC) and per-draw palettes for a mesh's color slots
(`ModelParams::palette`). The rest of an accessibility checklist (the Xbox Accessibility
Guidelines, the Game Accessibility Guidelines) was each game's to reinvent: Solid Eleven's
red and green teams, castle64's red, green and blue doors and Plan & Party's status colors
read as one color to a red-green deficiency; no text grew; no sound had a caption; nothing
damped camera shake or held back a strobe; a held action could not become a toggle; and
nothing could check any of it.

The N64 is the ceiling. Its fixed-function pipeline has no post filter, so the portable
mechanism for color vision is the one the designer controls: the colors themselves.

## Decisions

### 1. The settings: `trino_core::access::AccessSettings`

`cvd` (`Off`, `Protanopia`, `Deuteranopia`, `Tritanopia`, `Grayscale`), `text_scale`
(100–200 %), `high_contrast`, `captions` (`Off`, `Important`, `All`), `reduce_motion`,
`flash_guard` (on by default: WCAG 2.3.1's three flashes) and `hold_to_toggle`.

- `Platform::access()` / `set_access()` are new default methods (the defaults keep
  nothing, as a test platform needs), so `GAME_API_VERSION` does not move. Every engine
  backend holds a `HostedAccess`: `set_access` applies at once (clamped) and records a
  change for the host to persist.
- **PC:** `<save>.access` next to `.display` and `.input` (`key=value` lines, a player
  can edit it), written after the frame on a change, never on smoke runs;
  `TRINO_ACCESS=cvd=deuteranopia,text_scale=200,...` forces settings for a run (and then
  nothing is written). `apps/pc` (castle64) persists to `target/save/castle64.access`.
- **Web:** `localStorage` `trino-access:<name>`; `?access=...` forces them; self-test and
  `?frames` runs neither read nor write.
- **Consoles:** RAM, for the session. A game that wants them across power cycles stores
  `AccessSettings::pack()` — 3 bytes, refused by `unpack` when blank or damaged — in its
  own save (the N64 EEPROM has 512 B or 2 KB). Explicit, documented, never a silent
  difference.

### 2. Color vision without shaders: semantic palettes

`assets/palettes.ron` names colors by meaning (`team.home`, `castle64.door.green`) with a
base value and an alternative per mode (a mode left out keeps the base), the `distinct`
groups players must tell apart and the `contrast` pairs text is drawn with. The bake turns
it into the `access/data` blob (§7). A game asks `AccessData::color(name, mode)` or
`palette([..4 names], mode)` and passes the answer as the tint or the `Palette` it already
draws with: **the same draw, no extra triangle, no shader, every target.** A draw that
takes a palette leaves the N64's RSP (`RspReject::Palette`, BACKLOG P1 #12) as before;
games that only change a tint (castle64's doors, Solid Eleven's kits) stay on it — the
"zero cost" claim is per draw, not per path.

The simulation behind the checks is Machado, Oliveira and Fernandes (2009) at severity
1.0, applied in linear RGB (sRGB through two 256-entry tables, so it is deterministic
and `no_std`); grayscale is the Rec. 709 luminance. Distance is CIE76 ΔE in CIELAB (D65);
`min_delta_e` defaults to 20. Contrast is WCAG's ratio. Unit tests hold the simulation to
an independent float64 implementation's values (±1).

### 3. A second channel: icons

A sprite (`[sprites.kit] cvd_icon = "sprites/stripes"`) and a UI node (`cvd_icon:
"sprites/stripes"`) name an icon — a pattern, a shape — shown while a mode is on: the UI
draws it at the node's right end at the layout's scale; a game asks `AccessData::icon`.
TUIL goes to version 2 (records 94 → 102 bytes: the icon's sprite and size); version 1
blobs still load, without icons. `ui export` writes the field only when set, so existing
exports stay byte-identical.

### 4. Captions

A sound or a music track declares `caption = "cap.whistle"` (a string-table key, #59's
tables), `important`, `caption_priority` (0–255; 100 when important), `direction` and
`caption_seconds` (default 3). `Captions::wrap(&mut dyn Audio)` returns an `Audio` that
forwards **every** method (a test calls each one) and queues the caption of every sound,
voice, track and stinger it starts, when the player's mode shows it (`Important` shows
only `important` sounds). The queue holds 4: a repeat refreshes and counts `(x2)`, a full
queue drops the lowest priority (the oldest among equals) and never for a lower one, an
arrow points at a panned sound (`|pan| ≥ 0.25`). `Captions::draw` puts a near-opaque box
per line above the bottom of the safe area, in the UI's whole-pixel scale times the text
scale; `CaptionPlace::SecondScreen` draws on the 3DS touch screen (the main screen where
there is none), so it is drawn last — the screen order (top, then bottom) is a contract.

### 5. Text scale, high contrast, icons in the UI

`UiEnv::with_access(&settings)` carries a `UiAccess` into layout and draw.

- **Text scale** multiplies every text node's size; Fit boxes grow, fixed boxes overflow
  (and `access check` says so). Where the screen scales by whole pixels (the default) the
  glyph's pixel size is rounded to a whole number: bitmap fonts stay crisp and the N64
  keeps its fast text path (`draw_text` with an integer scale). At ×1 (the consoles) 125 %
  draws at 100 % and 150 % at 200 %; at 1080p (×4) 125 % is 5 pixels a glyph pixel.
- **High contrast** swaps the screen's colors for `Theme::HIGH_CONTRAST` (white on black,
  a yellow focus, 7:1 for text) or the project's `ui/themes/high_contrast.ron` (a theme
  that leaves fields out takes the engine's); node colors give way (images and 3D models
  keep theirs), borders take the accent, unfocused controls get a white outline.
- **The default path is today's.** With `UiAccess::NONE` the multiplier is exactly 1 and
  the theme the screen's: every existing golden stayed within its tolerance, the starter
  screens' and Solid Eleven's included after their fixed sizes became `min` sizes. The
  only golden re-blessed is the starter Options screen (`ui_options_*.png`), which gained
  the Accessibility button. The settings are
  in the layout key, so a change lays the screen out again.

### 6. Motion, flashes, held input

- `CameraFx`: shake, punch, FOV kick; under `reduce_motion` a shake or a punch keeps 20 %
  and FOV kicks are dropped; deterministic (the core's sine).
- `ScreenFlash`: a full-screen flash by one `fill_rect`; under `flash_guard` a fourth flash
  within a second is dropped and the peak alpha capped at 160.
- `FlashMonitor`: counts brightness rises of 0.1 or more (relative luminance × alpha ×
  coverage) and reports a strobe at more than three a second. The PC renderer feeds it
  every frame with its full-screen (≥ 25 %) fills and, in strict mode, logs a warning per
  strobe (`PcRenderer::flash_strobes`). It warns rather than panics: a flash is content,
  not a budget.
- `HoldToggle`: a held action as held, or as a press-on/press-off toggle.

### 7. The `access/data` blob (`TACC`)

One generated blob, no manifest entry, baked beside the UI blobs by `ui::bake` — so the
PC/Web bake, the N64 and 3DS images and the memory plan get it with no new code path.
A 60-byte header (counts and the high-contrast theme), 24-byte color records (id + five
RGBA), 12-byte captions (sound id, key, priority, flags, duration), 8-byte icons, a
string pool; tables sorted by id, binary-searched, refused when damaged. `AccessBlob`
re-parses after a live reload. A project with none of it bakes no blob. A palette whose
`distinct` group a mode cannot tell apart **fails the bake**: a mode would ship colors
that don't work.

### 8. The CLI: `cargo xtask access check|shot`

- `access check [dir] [--platform p] [--json]`: every distinct group apart and every pair
  readable in every mode, the high-contrast theme at 7:1, every caption key in every
  language (a fallback is a warning, a missing key an error), every `important` sound
  captioned, and **every screen laid out at 200 % on each target and language**: no text
  past its box, no node past the safe area (the N64's overscan: a list must scroll), the
  budgets kept. Warnings fail too, as in `ui check`. The checklist and findings are
  serde-serialized for `--json` and the editor.
- `access shot <screen|scene.ron> [--cvd m] [--view m|off] [--text N] [--contrast]
  [--platform p] [--size WxH] [--lang l] [--out x.png] [--json]`: a screen (with the
  player's settings) or a scene, rendered headless and shown through the simulation view.

### 9. The simulation view and the editor

`PcRenderer::set_cvd_view(mode)` (and `TRINO_CVD` in a game window) runs a pass after the
VI filter on both screens: copy the frame aside, draw it back through the mode's matrix
in linear light. It is built on first use (a pipeline per mode, a scratch texture per
screen) and costs nothing while off. A test holds it to the CPU simulation within 2/255;
its shader translates to GLSL ES 3.00 (WebGL2). It is a **review tool**, not a game's
look: the correction filter for content without palettes (daltonization) is #64's.

The editor's **Accessibility** tab: the simulation (mode, the UI tab's text size, high
contrast) applied to the Scene, Game and UI views (also View › Accessibility simulation,
`TRINO_EDITOR_ACCESS`), the semantic colors with each mode's value and how that viewer
sees it (edits write `assets/palettes.ron`, outside edits show live), and the checklist
from the same `check_project` the CLI runs. A kittest flow and the layout audit cover it.

### 10. Adopters

- castle64: the stage doors' open tints come from `palettes.ron` per mode; the coin, star
  and hurt sounds have captions (the star and the hurt are important).
- Solid Eleven: `dress_for(clubs, people, cvd)` compares kits as the player sees them and
  changes the visitors' (then the hosts') kit until every pair keeps ΔE 20 in every mode —
  the clubs' own kits, no new colors, no draw off the RSP. Its whistle (important),
  cheers, "ooh" and post have captions in English and Portuguese. Its pause menu and the
  starter screens grow with their text (`min` sizes instead of fixed ones).
- New projects get an Accessibility screen (`ui/accessibility.ui.ron`) reached from
  Options; its rows scroll when 200 % text makes them taller than the N64's safe area.

## Consequences

- A color, a caption, a theme and a screen at 200 % are text files a person, an agent or
  the editor changes, checked per target and language before they ship; the consoles
  honor color vision at no draw cost.
- Content decides: a game without semantic colors gets no color-vision support on the
  consoles (the PC/Web filter is #64); captions show only for sounds that declare them.
- The UI's text scale is coarse at ×1: 100 % or 200 %, by design (crisp bitmap glyphs).

## Verification

- Core: `crates/core/src/access/*` unit tests (settings, pack/unpack, text, the color
  math against reference values) and `crates/core/tests/access.rs` (TACC, the blob's live
  reload, the wrapper forwarding every call, the queue, the caption box per placement,
  motion, the guard on a synthetic strobe, hold-to-toggle, the UI's text scale, contrast
  and icons, TUIL v2 and v1).
- Formats and checks: `crates/scene/src/access.rs`, `crates/asset-pipeline/tests/access_bake.rs`.
- Hosts: `platform_services.rs` (PC), `runner-pc` (the file and `TRINO_ACCESS`).
- Renderer: `xtask/tests/access_goldens.rs` — the palette swap golden
  (`access_palettes.png`), the simulated golden (`access_cvd_deuteranopia.png`), the GPU
  view against the CPU simulation, strict mode's strobe warning.
- CLI: `xtask/tests/access_cli.rs`; editor: `accessibility_tab_palettes_simulation_and_checklist`
  and the layout audit.
- Self-test step `accessibility` (castle64) on the PC profiles, ares, Azahar and the
  browsers: settings round trip, the baked data, the logic, the caption wrapper, the UI at
  200 % and its icon, the high-contrast focus color read back, the palette swap read back,
  the caption box read back (on the 3DS touch screen too).
