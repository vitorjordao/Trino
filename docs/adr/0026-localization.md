# ADR 0026: Localization — string tables, glyph subsets per language, plurals and locale formats

- Status: accepted (2026-10-19, GitHub #59)
- Builds on: ADR 0024 (UI system: `UiStrings`, `TSTR` v1), ADR 0022 (asset groups),
  ADR 0012 (the N64's resident text atlas), ADR 0005 §3 (format versioning)

## Context

Both shipped games built their own localization. Solid Eleven kept 184 strings in a
`match` per language, plus a few ad hoc ones (`"P1: CASA"`, `"PORTUGUÊS"`), and a
unit test that walked the enum to check the font. Plan & Party did the same with its own
glyph set. Font atlases (`TFNT`) were baked from a hand-kept `chars` list, so adding a
language meant guessing glyphs. On the N64 every glyph costs TMEM (4 KB; an atlas that
fits stays resident, ADR 0012), and a superset atlas grows ROM and RAM without saying
so. Nothing checked that a key exists in every language, that placeholders agree, that a
font covers what a language writes, or that a translation fits its box on 320×240.
Numbers, money and plurals were formatted by hand.

## Decision

### 1. Data: one RON file per language

`assets/i18n/<lang>.ron` (the file must be named after its `lang`):

```ron
Strings(
    lang: "pt-BR",
    fallback: "en-US",
    name: "Português (Brasil)",
    plural: "OneUpToOne",                 // optional: the tag's CLDR rule otherwise
    locale: (currency: "R$", min_group: 1), // optional overrides of the tag's defaults
    entries: {
        "menu.play": "Jogar",
        "guests.count": Plural(one: "{n} convidado", other: "{n} convidados"),
        "money": Format("{amount:currency}"),
    },
)
```

- An entry is a string, a `Plural(zero:, one:, two:, few:, many:, other:)` or a
  `Format("…")` (only placeholders: translators keep it). Unknown fields are errors.
  Optional fields are written bare (`currency: "R$"`, not `Some("R$")`).
- Placeholders: `{name}` / `{name:kind}`, kinds `int` (grouped), `raw` (no grouping),
  `num`, `currency`, `date`, `str`; `{{`/`}}` are braces. An unknown argument prints the
  placeholder itself, so QA sees it.
- A tag starting `qps-` is a pseudo-locale: PC and Web builds bake it, consoles never do,
  and it adds no glyph to any font.
- `[i18n]` in the manifest: `default`, `languages` (shipped; default every table but
  pseudo-locales), `extra_chars`, `groups`, `blobs`, `check_fonts`,
  `[i18n.overrides.<lang>]` (`"fonts/ui" = "fonts/ui-ja"`, or per platform:
  `{ pc = …, web = … }`). The issue sketched the locale rules in the manifest. They live
  in each language's file instead, next to the strings they format, with defaults by tag.

### 2. Runtime: `trino_core::i18n` (`no_std`, no allocation)

- `StrKey` (`const`, `asset_id` of the key) and `LangId` (`asset_id` of the tag; saves
  store it in 4 bytes); `LangTag` (≤ 15 bytes, what a platform reports).
- `Localizer<'a, N>`: up to `N` `StringTable`s (`'static` blobs, or static code). It has:
  - `load(platform)` (reads `i18n/languages`, the default first), `add` (a live reload
    replaces the language), `set_language`, `language`, `tag`, `best_match` (exact,
    then primary subtag: `pt` → `pt-BR`), and `pick(saved, system)`;
  - `tr` (the fallback chain, at most `N` hops: a cycle ends) and `get`/`entry`;
  - `plural(key, n)`, `tr_fmt(key, args, &mut TextBuf)` (`_in` variants for a given
    language), and `format` (a pattern from code);
  - `locale()`, `font(FontId)` and `group()`.
- `Arg`/`Value`: `Int`, `Dec { value, decimals }` (fixed point), `Money` (minor units),
  `Str` and `Date`. These are integers only, so every target prints the same digits.
  Plural forms come from the argument named `n` or `count`, else the first number, by the
  language's CLDR rule: `OneOther`, `OneUpToOne` (pt, fr: 0 and 1 are singular),
  `Other`, `Slavic`, `Polish`, `Czech`.
- `Locale`: decimal and thousands separators, `min_group` (es and pl write `1200` but
  `12.000`), currency symbol, position, space and decimals (0 for the yen), date order
  and separator, plural rule. `Locale::for_tag` holds the defaults (en-US, en-GB, pt-BR,
  pt-PT, es-ES, es-419, fr, de, it, nl, ru, uk, pl, ja, zh, ko).
- `UiStrings::font` (a default method): the UI measures and draws a node's font through
  it, in one function the layout and the draw share (`ui::draw::font_of`). Overflow
  checks and pixels therefore never disagree. `Localizer`, `StringTable`, `Fallback` and
  the host's `HostStrings` implement it.
- `Platform::system_language()` (a default method, so `GAME_API_VERSION` stays):
  - PC: `TRINO_LANG`, else the OS locale (`sys-locale`).
  - Web: `?lang=`, else `navigator.language`.
  - 3DS: the system setting (`CFGU_GetSystemLanguage` through the shim, a bare language
    such as `pt`, logged `TRINO_LANG <tag>` at boot).
  - N64: `None`, documented: the game's default, then the player's saved choice.

### 3. Format: `TSTR` version 2

Version 2 has an 80-byte header:
- string refs to the pool: the language, its fallback, display name, decimal and
  thousands separators, currency, date separator;
- the plural rule, date order, currency decimals and flags, and `min_group`;
- the font remap count and the language's group hash.

After the header come the font remaps (`u32 font, u32 replacement`), then the records
sorted by key hash (`hash, offset, length`; bit 31 of the length marks a plural, whose
six forms are joined by U+001F), then the UTF-8 pool. Version 1 (#58's) still parses,
with its tag's default locale: a migration, no rebake needed. `StringTable::from_parts`
is `const`, so `cargo xtask i18n export --rust` emits the bake's own bytes as static
code (one pool literal per text) for a game that keeps its strings in code.

### 4. Bake: glyph subsets, per language, TMEM-aware

`Manifest::load` resolves `[i18n]`, so every platform's bake, the memory plan and every
report see concrete fonts:
- `chars = "auto"` (TTF/OTF only; a PNG grid's cells are its glyphs) bakes the union of
  the following. A glyph nothing uses is not baked.
  - every shipped language's glyphs: its texts without placeholders and those its fallback
    chain lends it (they draw with its atlas too), its name, and what
    its placeholders print in its locale (digits and separators; the currency symbol
    only for `:currency`; the date separator for `:date`);
  - every UI screen's literal `text`;
  - `[i18n] extra_chars` and the font's `extra_chars`;
  - the space and `?`, the glyph the runtime draws for a character the atlas lacks.
- `per_language = true` bakes `fonts/<name>` with the default language's glyphs and a
  `fonts/<name>@<lang>` per other language. Each language's `TSTR` maps the font to its
  atlas, so `Localizer::font` and the UI swap it with no game code.
- With `[i18n] groups = true` the other languages' atlases sit in asset group
  `i18n-<lang>` (lowercase: group names are), loaded on demand (#56), and the `TSTR`
  names the group (`Localizer::group`).
- `[i18n] blobs = false` skips the `i18n/*` blobs for a game whose tables are static code
  (Solid Eleven).

### 5. Checks and tools (AI ↔ editor parity)

- `cargo xtask i18n check [--platform p] [--strict] [--dir <project>] [--json]` reports:
  - the tables: missing keys (an error when no fallback has them) and extra keys,
    placeholders that differ from the default's (a plural's `one` may drop `{n}`), plural
    forms the rule needs, and bad locales, rules or fallbacks;
  - keys no UI screen and no string literal in the project's Rust names (info);
  - glyphs a checked font lacks, per target, through the font each language draws with;
  - text that overflows a UI box, per screen, target and language (`ui::audit` with the
    language's font remaps, the same layout the runtime does);
  - atlases past the N64's TMEM.
  Errors fail it; `--strict` fails on warnings too.
- `i18n pseudo` (accented Latin-1, `[ ]`, +40% `~`, placeholders kept), `i18n export|
  import --csv` (a `key` column then one per language; plurals as `key#one` rows; an
  empty cell removes the key so the language falls back), `i18n export --rust`,
  `i18n stats`, `i18n fonts` (each atlas, its glyphs, bytes and TMEM / texture memory
  per platform, its group) and `i18n list`, all with `--json`.
- The editor's Localization tab edits the same files: a keys × languages grid with
  fallback and missing cells marked, add or remove keys and languages, a pseudo-locale,
  CSV, the issue list, and glyph coverage plus N64 TMEM per font and language. Its
  preview language drives the UI tab and Play (`TRINO_LANG`). External edits reload live.
  Hooks: `TRINO_EDITOR_TAB=localization` and `TRINO_EDITOR_LANG`.

## Consequences

- The engine's own tables (`assets/i18n/{en-US,pt-BR,es-ES}.ron`) and a subset Roboto
  Condensed font (`fonts/i18n`, three atlases of 41–49 glyphs, 2 KB each as N64 I4: half
  of TMEM) back the self-test's `i18n` step on PC, ares, Azahar and the browsers. The
  step:
  - loads every language from `i18n/languages` and picks by the system language;
  - in each language, reads a key, a fallback, a plural and a price
    (`$1,200.00`, `R$ 1.200,00`, `1200,00 €`);
  - checks that the language draws with its own atlas;
  - reads pixels back to check that the drawn price ends where that language's ends.
- Solid Eleven moved its 187 strings (with the ad hoc ones) to `en-US.ron` / `pt-BR.ron`,
  read through static code. Its three fonts are `chars = "auto"` whose extra characters
  (names, scores) make the same set as before, so the atlases and the screenshots are
  byte-identical. `i18n check` replaces the hand-written font test.
- `GAME_API_VERSION` does not move: `Platform::system_language` and `UiStrings::font`
  are default methods.
- Costs:
  - a lookup is a binary search over 12-byte records, plus the fallback hops;
  - formatting writes digit by digit into a `TextBuf`.
  
  Nothing is per vertex or per frame unless a game formats per frame.

## Not done (follow-ups)

- Plan & Party lives in another repository; it can migrate the same way (its `pnp8`
  glyph set becomes `chars = "auto"` + `extra_chars`).
- Gender and grammatical case (Fluent's selectors). The entry enum can grow a `Select`
  variant within `TSTR` (a new record flag).
- Right-to-left scripts and shaping. The text renderer lays glyphs out left to right.
- Comments in a table are lost when the editor or `i18n import` rewrites the file.
- The editor's preview language drives the UI tab and Play (`TRINO_LANG`). Play here and
  the Scene and Game views draw scenes, which hold no text, so it doesn't reach them.
- The per-language groups are loaded at run time by a PC test
  (`a_languages_atlas_loads_and_unloads_with_its_group`). The consoles load them through
  #56's generic loader, which its `asset groups` step covers; the engine's own
  `fonts/i18n` stays in boot, so the console self-test switches languages without a load.

## Addendum (2026-10-29, GitHub #99): atlases packed tight, a warning from 75 % of TMEM

castle64's achievement strings (#61) pushed the pt-BR and es-ES atlases from 256×16 to
256×32: 4 KB of I4, the N64's whole TMEM, one glyph from not fitting, and nothing said
so. Measured choices:

- **Packer** (`font::pack_shelves`): the width, 16 to 256 in 16-texel steps (whole TMEM
  lines in I4), with the fewest texels; widest glyphs first into the first shelf with
  room; the height in 4-texel steps instead of a power of two (the N64 clamps text
  rectangles and `tex3ds` pads the 3DS copy), and no side past 256 (the N64's
  `max_texture_dim`: a 16×332 atlas drew its lower glyphs wrong there). `fonts/i18n` en-US 256×16 → 64×44
  (2 048 → 1 408 B, 34 %), es-ES 256×32 → 16×188 (4 096 → 1 504 B, 37 %), pt-BR
  256×32 → 32×100 (4 096 → 1 600 B, 39 %); the 3DS copies pad to 4 096 texels each
  (8 192 before for es-ES and pt-BR). Glyph pixels are unchanged, only where they sit.
- **Splitting the font** (the self-test's strings apart from the game's) was not needed
  once the packer brought every atlas under 40 %.
- **`i18n check`** warns from 75 % of the atlas's room (`TMEM_WARN_PERCENT`) and past
  it, both warnings: past TMEM the atlas still draws, a glyph at a time, which big
  display fonts accept (Solid Eleven's 22 and 44 px). The room is the texels' (4 KB, or
  2 KB beside a CI palette: `N64Format::tmem_texels`), as the shim's `trino_text_begin`
  decides; the reports compared CI texels with 4 KB before.
- Next lever if an atlas nears 75 % again: per-glyph vertical crop (a y offset in TFNT,
  a format bump); cells are still a full line tall.
