---
name: localization
description: Translate a Trino game and keep it translated — string tables as `assets/i18n/<lang>.ron` (plurals, number/money/date formats, fallback), fonts baked with exactly the glyphs each language uses (`chars = "auto"`, per-language atlases within the N64's TMEM), `cargo xtask i18n check|pseudo|export|import|stats|fonts`, the editor's Localization tab, and the runtime `Localizer` (`tr`, `tr_fmt`, system language). Use for "add a language", "translate", "a string is missing", "plural", "format a price/date", "the font lacks ç", "pseudo-localize", "CSV for translators", or moving a game's hand-rolled strings to the engine.
---

# localization (string tables as text)

Format and design: ADR 0026, `docs/ASSETS.md` › Localization. The runtime is
`trino_core::i18n`; the host model `trino_scene::i18n`; the bake
`trino_asset_pipeline::i18n`; the CLI `xtask/src/i18n.rs`; the editor
`crates/editor/src/localization.rs` + `panels/localization.rs`.

## Add a language (the whole loop)

1. Copy the default table: `assets/i18n/en-US.ron` → `assets/i18n/<tag>.ron`. Set `lang:`
   (the file name must match it), `fallback: "en-US"`, `name:` (the language's own name,
   for a language menu), then translate the `entries`. The Localization tab
   (`TRINO_EDITOR_TAB=localization`) or `i18n export --csv` / `import --csv` do the same.
2. `cargo xtask i18n check --json` (add `--dir examples/<game>` for an example with its own
   `assets/`). Exit 0 = no errors. It reports:
   - `missing_key`: an **error** when no fallback has the key (the game shows nothing), a
     warning when the fallback's text shows;
   - `placeholder`: the language renamed or dropped `{name}` (a plural's `one` may drop
     `{n}`); `plural_form`: the language's rule needs `few`/`many` (`ru`, `pl`, `cs`);
   - `missing_glyph`: per target, through the font that language draws with — make the
     font `chars = "auto"` or add the characters;
   - `overflow`: a UI box too small in that language on that target (same layout as `ui
     check`); `font_budget`: a warning from 75 % of the N64's TMEM
     for the atlas's texels (`TMEM_WARN_PERCENT`: a few glyphs from not fitting) and past
     it (it no longer stays resident: every glyph re-uploads, ~50 µs). TMEM is 4 KB for
     I4/I8, **2 KB beside a palette** for CI4/CI8 (the shim's `trino_text_begin` limit;
     before GitHub #99 the report compared CI texels with 4 KB);
   - `extra_key` (warning) and `unused_key` (info: no screen key, no string literal in the
     project's `.rs` files names it).
   `--strict` fails on warnings too.
3. `cargo xtask ui shot <screen> --lang <tag> --platform n64` and look.
4. Bake/test as usual: every platform bakes `i18n/<tag>` and lists it in `i18n/languages`.

## Entries

```ron
"menu.play": "Jogar",
"guests.count": Plural(one: "{n} convidado", other: "{n} convidados"),
"money": Format("{amount:currency}"),   // only placeholders: translators keep it
```

- Placeholder kinds: `{n}` / `{n:int}` (grouped by the locale), `{y:raw}` (no grouping:
  years), `{x:num}` (a `Dec`'s decimals), `{amount:currency}`, `{d:date}`, `{s:str}`.
  Write money as `:currency`: the glyph subset adds the currency symbol only for it.
- Plural forms by CLDR rule (`PluralRule::for_tag`): `OneOther` (en, es, de…),
  `OneUpToOne` (pt, fr: **0 is singular**, "0 convidado"), `Other` (ja, zh, ko),
  `Slavic`, `Polish`, `Czech`. Override with `plural: "Slavic"`.
- Locale defaults by tag (`Locale::for_tag`); override any field bare:
  `locale: (currency: "R$", min_group: 1, date_order: "Dmy")` — never `Some(...)`.
- A `qps-*` tag is a pseudo-locale: `cargo xtask i18n pseudo` writes `qps-ploc.ron`
  (accented, `[ ]`, +40%). PC/Web builds bake it; consoles never; it adds no glyphs. Its
  overflow is a warning. Delete it before shipping if you like; it never ships to consoles.

## Fonts

```toml
[i18n]
default = "en-US"
check_fonts = ["fonts/ui"]        # fonts that draw localized text (default: the UI's + auto fonts)
extra_chars = "0123456789"        # dynamic text every auto font needs
groups = true                     # per-language atlases in group i18n-<lang> (lowercase)

[fonts.ui]
file = "fonts/Roboto.ttf"
size = 11
chars = "auto"                    # TTF/OTF only
extra_chars = "#*"                # names, scores
per_language = true               # fonts/ui (default) + fonts/ui@pt-BR ...
```

- `cargo xtask i18n fonts --platform n64` lists each atlas, its glyphs, bytes and TMEM
  share. Per-language atlases keep each language small (the engine's `fonts/i18n`: three
  I4 atlases of 1.4–1.6 KB, 34–39 % of TMEM).
- TTF atlases are shelf-packed tight (`font::pack_shelves`, GitHub #99): the width (16 to
  256 in 16-texel steps) with the fewest texels, widest glyphs first, the height in
  4-texel steps (not a power of two: the N64 clamps text rectangles and `tex3ds` pads
  the 3DS copy itself). The old 256-wide rows rounded to a power of two put castle64's
  pt-BR and es-ES atlases at 256×32, 100 % of TMEM. Glyph cells are still a full line
  tall (TFNT has no y offset); that is the next lever if an atlas nears 75 % again.
  Both sides stay ≤ 256 (the N64's `max_texture_dim`): by area alone the football
  template's `se_ui` packed 16×332, and the N64 drew its glyphs below row 256 wrong
  (the clock's colon; caught by the template's N64 golden in CI, not by the engine's).
- `auto` always bakes ` ` and `?` (the runtime draws `?` for a missing character).
- A font for one language only (CJK on PC/Web): `[i18n.overrides.ja-JP] "fonts/ui" =
  { pc = "fonts/ui-ja", web = "fonts/ui-ja" }`.

## In game code

```rust
use trino_core::i18n::{Arg, Localizer, StrKey};
const PLAY: StrKey = StrKey::new("menu.play");
// update_with (blobs are 'static):
let n = self.loc.load(platform);                       // i18n/languages, default first
self.loc.pick(self.save.lang, platform.system_language()); // saved > system > default
// anywhere:
let s = self.loc.tr(PLAY);
let p = self.loc.tr_fmt(StrKey::new("money"), &[Arg::money("amount", 120_000)], &mut buf);
let font = self.loc.font(UI_FONT);                     // the language's atlas
ui.draw(r, &screen, &env.with_strings(&self.loc));     // the UI swaps the font itself
```

- Save `loc.language()` (a `LangId`, 4 bytes). The N64 has no system language (`None`).
- With `[i18n] groups = true`, `load_group(loc.group())` before switching on a console.
- A game that keeps strings in code: `[i18n] blobs = false`, then `cargo xtask i18n export
  --rust <out.rs> --dir <project>` and `include!` it (Solid Eleven:
  `examples/solid-eleven/src/ui/strings_gen.rs`; `i18n_cli` fails on a stale export).

## Pitfalls

- An `Option` field in a hand-written RON format reads `Some(x)` unless the struct uses a
  bare deserializer (`deserialize_with = "bare"` in `trino_scene::i18n::LocaleDef`).
- Asset group names are lowercase: the per-language group is `i18n-pt-br`.
- A pseudo-locale measures missing accents as `?`; a font without `?` measures them as
  nothing and hides the overflow it was meant to show.
- Test strings by the table, not by the code: the Solid Eleven test formats every `S`
  through `tr_fmt` and checks the characters against `ui::FONT_CHARS`.
- `TRINO_LANG=pt-BR` forces the PC's system language (the editor's Play passes its preview
  language that way); `?lang=` does it on the Web.
