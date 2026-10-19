# Accessibility

Trino gives a game the player's accessibility settings and the pieces that honor them on
every target, the N64 included: color vision through semantic palettes, a second channel
of icons, text scale and high contrast in the UI, captions for sounds, damped camera
motion, a flash guard and hold-to-toggle. Each one is a text file a person, an agent or
the editor writes, checked by `cargo xtask access check` before it ships. The design and
its trade-offs are in [ADR 0027](adr/0027-accessibility.md).

| Setting | What the engine does | N64 | 3DS | PC / Web |
|---|---|---|---|---|
| Color vision (`cvd`) | semantic colors swap to the mode's alternative; UI nodes and sprites show their icon | same draw, another color | same | same, plus a simulation view for review (`TRINO_CVD`) |
| Text scale (100–200 %) | every UI text grows; Fit boxes grow with it | whole glyph pixels: 100 % or 200 % | same | 5, 6, 7… pixels a glyph pixel at 1080p |
| High contrast | the UI draws with the high-contrast theme, controls outlined; `Outline` rings key 3D objects | yes | yes | yes |
| Captions (off / important / all) | sounds with a `caption` show a box with their text | yes | yes, optionally on the touch screen | yes |
| Reduce motion | `CameraFx` keeps 20 % of a shake or punch, no FOV kick | yes | yes | yes |
| Flash guard (on by default) | `ScreenFlash` shows at most 3 flashes a second, softer | yes | yes | yes; strict mode warns on raw strobes |
| Hold to toggle | `HoldToggle` latches a held action | yes | yes | yes |
| Where they are kept | | RAM (the game may store 3 bytes in its save) | RAM (same) | `<save>.access` / `localStorage` |

## The settings

```rust
// update_with:
let s = platform.access();          // AccessSettings
platform.set_access(AccessSettings { captions: CaptionMode::All, ..s });
```

The PC and the Web keep them for the player; a console keeps them for the session. To
keep them across power cycles, a console game stores `s.pack()` (3 bytes) in its own save
and `AccessSettings::unpack(bytes)` restores them (`None` for a blank or damaged save).
`TRINO_ACCESS=cvd=deuteranopia,text_scale=200,high_contrast=on` forces settings in a PC
run (and `?access=...` in a browser), for screenshots and tests.

New projects ship an Accessibility screen (`ui/accessibility.ui.ron`, from Options) that
reads and sets them; the template game shows how (`templates/new-game/src/lib.rs`).

## Color vision: semantic palettes

Name your colors by what they mean and give each mode an alternative where the base fails:

```ron
// assets/palettes.ron
(
    version: 1,
    colors: {
        "team.home": (base: (200, 40, 40), protanopia: (240, 160, 0), deuteranopia: (240, 160, 0)),
        "team.away": (base: (40, 160, 60), protanopia: (40, 90, 220), deuteranopia: (40, 90, 220),
                      grayscale: (20, 20, 20), note: "the visitors"),
        "hud.text": (base: (255, 255, 255)),
        "hud.panel": (base: (16, 16, 24)),
    },
    distinct: [["team.home", "team.away"]],          // must look apart in every mode
    contrast: [(fg: "hud.text", bg: "hud.panel", min: 7.0)],   // readable in every mode
    // min_delta_e: 20.0,                             // how far apart (CIE76 ΔE)
)
```

The bake turns it into the `access/data` blob. In the game:

```rust
// fields: access: AccessBlob, settings: AccessSettings
self.access.get(platform);                       // update_with; re-parses after a live edit
self.settings = platform.access();
// render: the tint (or palette) the draw already takes
let home = self.access.data()
    .and_then(|d| d.color("team.home", self.settings.cvd))
    .unwrap_or(HOME_RED);
renderer.draw_model(KIT, &t, Material::VertexLit, &ModelParams::tinted(home));
// a slotted mesh: four semantic colors as its palette
if let Some(d) = self.access.data() {
    let palette = d.palette(["team.home", "", "", "team.trim"], self.settings.cvd);
    // ModelParams { palette: Some(palette), ..ModelParams::DEFAULT }
}
```

The same draw, no shader, the same on every target. (A draw with a palette leaves the
N64's RSP as any palette draw does; a tint stays on it.) When the colors are fixed
content — kits a club owns — compare them as the player sees them instead:
`trino_core::access::delta_e_seen(cvd, a, b)` (Solid Eleven's `dress_for`).

**Icons** add a second channel: `[sprites.kit] cvd_icon = "sprites/stripes"` in the
manifest (`AccessData::icon(sprite)`), `cvd_icon: "sprites/stripes"` on a UI node (drawn
at its right end while a mode is on).

## Captions

```toml
# assets/manifest.toml
[sounds.whistle]
file = "sounds/whistle.wav"
caption = "cap.whistle"      # a key of assets/i18n/<lang>.ron
important = true             # shown in the "important" mode too, and must have a caption
caption_priority = 200       # a full queue drops the lowest first (default 0, 100 when important)
direction = true             # an arrow toward a panned sound
caption_seconds = 1.5        # default 3
```

```rust
// update_with: everything the game plays goes through the queue
self.captions.configure(self.access.get(platform), &platform.access());
let mut captions = self.captions;          // Captions is Copy
captions.update(dt);
self.update(input, &mut captions.wrap(audio), dt);
self.captions = captions;
// render, last
self.captions.draw(renderer, strings, &CaptionStyle { font: HUD, ..CaptionStyle::DEFAULT });
```

`CaptionPlace::SecondScreen` puts them on the 3DS touch screen (the main screen
elsewhere); draw them after everything else on the top screen. `Captions::texts` gives the
lines for a game that draws captions its own way; `Captions::push` queues a caption the
game makes itself (a voice line).

## Text scale and high contrast in the UI

```rust
let env = UiEnv::of(renderer)
    .with_access(&settings)                          // text scale, high contrast, icons
    .with_contrast_theme(access.or_empty().contrast_theme()); // the project's theme, if any
ui.draw(renderer, screen, &env.with_strings(table));
```

Design screens to grow: give controls `min: (w, h)` and `padding` instead of fixed
`size: (Px(w), Px(h))`, and put long lists in a `Scroll` with a `max` height so 200 % text
scrolls instead of leaving the N64's safe area. `cargo xtask access check` lays every
screen out at 200 % on every target and language.

The high-contrast theme is the engine's `Theme::HIGH_CONTRAST` (white on black, a yellow
focus, 7:1) or the project's `ui/themes/high_contrast.ron`:

```ron
// ui/themes/high_contrast.ron: a screen theme; what it leaves out is the engine's
(focus: Some((0, 200, 255, 255)), focus_text: Some((0, 0, 0, 255)))
```

## Outlines on 3D objects

In high contrast, ring the objects a player must not lose (the player, enemies, the ball)
with `Outline`: a second, unlit draw of the same model, a few pixels larger and just
behind it, so only a solid ring around the silhouette shows. No backend code; one extra
draw per part, on the N64's RSP like any unlit draw.

```rust
use trino_core::access::{self, OUTLINE_COLOR};
let ring = settings.high_contrast.then_some(
    Outline::new(OUTLINE_COLOR, center, radius), // the object's bounding sphere, world space
);
// draw_model plus its ring (exactly the plain draw when `ring` is None):
access::draw_outlined(renderer, &camera, model, &transform, Material::VertexLit, &params, ring.as_ref());
// a part drawn with draw_model_matrix: ring.draw(renderer, &camera, model, &m, material)
```

- Give every part of a multi-part character the same sphere: the ring hugs the whole body
  and draws a thin line where one part stands in front of another.
- `with_pixels(n)` sets the ring's width in a 240-line picture's pixels (2 by default); a
  taller picture scales it, so it looks as wide on a 1080p PC as on a console.
- What lies right behind the object (the floor under its feet, a wall it touches) hides
  the ring there.

castle64 rings its player in yellow and its boars in red-orange; Solid Eleven rings the
ball (`cargo test -p xtask --test access_shots -- --ignored` writes
`castle64_outline_{off,on}_{n64,3ds,pc}.png`).

## Motion, flashes, held input

```rust
// a hit: the camera shakes (20 % of it under reduce_motion), the screen flashes
// (at most 3 a second under flash_guard)
self.fx.shake(0.3, 0.4);
self.flash.trigger(Color::rgba(255, 255, 255, 200), 0.15, &settings);
// every frame
self.fx.update(dt);
self.fx.apply(&mut camera, &settings);
self.flash.update(dt);
// render, after the world
self.flash.draw(renderer);
// sprint: held, or toggled
let sprinting = self.sprint.update(input, &self.prev, Button::B, &settings);
```

Strict mode on the PC (`TRINO_STRICT=1`) warns when raw full-screen fills strobe faster
than three times a second.

## Checking and reviewing

```
cargo xtask access check [dir] [--platform n64|3ds|pc|web] [--json]
cargo xtask access shot <screen|scenes/x.scene.ron> --cvd deuteranopia [--view off] \
    [--text 200] [--contrast] [--platform n64] [--out x.png]
TRINO_CVD=protanopia cargo run -p trino-app-pc        # a game window as a protanope sees it
                                                       # (a browser: ?cvd=protanopia)
```

`access check` fails on any finding (warnings too): groups too close or pairs too dim in
a mode (it names the alternative to write), a theme under 7:1, a caption key a language
lacks, an `important` sound without a caption, a screen whose text overflows or that
leaves the safe area at 200 %.

In the editor, the **Accessibility** tab shows the simulation (applied to the Scene, Game
and UI views; also View › Accessibility simulation), the semantic colors with each mode's
value and how that viewer sees it (click to edit; `assets/palettes.ron` is written and
follows outside edits), and the same checklist. `TRINO_EDITOR_ACCESS=cvd=deuteranopia,
text=200,contrast=on` and `TRINO_EDITOR_TAB=accessibility` open it in that state.
