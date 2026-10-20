# ADR 0028: Gamepad families and button glyphs

Status: accepted (October 2026, GitHub #86 item 7). Implemented in `trino-core`,
`trino-platform-pc`, `trino-platform-web` and Solid Eleven's menus and controls screen.

## Context

On the PC and the Web a player may hold an Xbox pad, a DualShock 4 or DualSense, a Switch
Pro Controller, an 8BitDo or a no-name pad. The engine maps every one of them to the same
positional buttons (gilrs' SDL database, the W3C standard mapping: `Button::A` is the
bottom face button), but games printed Xbox letters for all of them. A PlayStation player
was told to press "A", and a Nintendo player pressed the button labelled A, which is the
right one. `Platform::gamepad_name` gave the system's name and nothing to act on.

## Decisions

### 1. The family is core data: `trino_core::pad`

- `PadFamily { Xbox, PlayStation, Nintendo, EightBitDo, Generic }`. Generic keeps the
  standard positions with Xbox letters, uncolored (Steam's convention; a Steam Deck is
  Generic).
- `PadSlot` names a button by position (South, East, West, North, the shoulders, the
  triggers, the stick clicks, Start, Select, Home, the D-pad). `PadSlot::standard(Button)`
  is the PC's and the Web's default binding.
- `PadFamily::glyph(slot) -> PadGlyph { short, name, color, shape }`:
  - Xbox: A/B/X/Y in green, red, blue and yellow.
  - PlayStation: cross, circle, square and triangle as shapes (`GlyphShape`), with no
    letters (`short` empty, `name` "CROSS"), in their colors.
  - Nintendo: the letters in swapped places, so the bottom button prints "B".
  - 8BitDo: Nintendo's places in SNES colors.
  - Shoulders and the rest: LB/RB/LT/RT, L1/R1/L2/R2, L/R/ZL/ZR, and "+"/"-" for
    Nintendo's Start and Select.

  Every label is ASCII (uppercase, digits, `+`, `-`), so any bitmap font prints it.
  `button_glyph(Button)` is the shortcut a menu calls.
- It is all `no_std` with no allocation, like the rest of the core. A console never
  calls it, and it costs the consoles nothing.

### 2. Detection: pure functions, ids first

- `PadFamily::from_usb(vendor, product)` maps the vendors that make one family:
  Microsoft `045e`, Sony `054c`, Nintendo `057e` and 8BitDo `2dc8`. Vendors that make
  several families (PDP, PowerA, Hori, Logitech) give `None`.
- `PadFamily::from_name(&str)` matches keywords case-insensitively, 8BitDo first, so
  "8BitDo Pro 2" never reads as a Nintendo pad.
- `PadFamily::detect(name, usb)` takes the ids, then the name, then Generic.
- Ids win on purpose. An 8BitDo pad in XInput mode reports `045e:028e`, and in Switch mode
  `057e:2009`. In those modes the system maps its buttons as that family, so that family's
  glyphs are the right ones.
- The **PC**:
  - The gilrs glue only copies `name()`, `vendor_id()` and `product_id()` of the first
    connected pad into a `PadIdent` (`PcInput::note_pad`, every poll). This follows the
    pitfall "keep gamepad logic out of the gilrs glue".
  - `PadIdent::family` runs the detection.
  - Tests call `note_pad` directly, with no pad and no `gamepad` feature.
- The **Web**: `keys::parse_pad_id` splits `Gamepad.id` in each browser's form:
  - Chrome: `Name (STANDARD GAMEPAD Vendor: 054c Product: 0ce6)`, or `Name (XInput
    STANDARD GAMEPAD)`;
  - Firefox: `054c-0ce6-Name`, sometimes without the leading zeros, or a bare `xinput` on
    Windows;
  - Safari: the name alone.

  `keys::pad_family` then detects. Both functions build off wasm, so the host tests cover
  them.

### 3. The platform: a default method, no ABI bump

`Platform::gamepad_family(&self) -> Option<PadFamily>`, default `None`, beside
`gamepad_name`:

- `PcPlatform::publish_input` copies it from `PcInput`;
- the Web host copies it from `WebInput` each poll;
- the consoles keep the default. Their buttons are fixed and the game knows them (the
  N64's A/B/C, the 3DS's Nintendo layout).

A new `InputInfo` field was rejected because it would break the struct literals in three
hosts. `GAME_API_VERSION` stays at 4: like every defaulted method since `second_screen`, it
changes nothing the dylib boundary carries (`crates/game-api/src/lib.rs`).

### 4. What stays the game's

- Rebinding: a pad button the player moved is labelled by `Platform::binding_label`
  (Xbox names). Mapping a rebound `PadButton` to a family's glyph would need a
  `Platform::binding_slot`; nobody needs it yet.
- Drawing: Solid Eleven's `ui/glyphs.rs` turns the glyph into badge colors and the
  PlayStation symbols into `fill_rect` runs (`spans`), and holds a diagram layout per
  family (`chassis`, `spot`). These are game data, not engine API. Another game may draw
  sprites instead.

## Consequences

- One menu line, `platform.gamepad_family().map(|f| f.button_glyph(Button::A))`, prints
  the right glyph on every pad, and keeps the game's own labels on the consoles.
- Detection is tested on 14 PC identities and 14 browser ids (the table in the evidence
  of #86). It has not been tried with physical pads on the development machine.
- Follow-ups:
  - `binding_slot` for glyphs of rebound buttons;
  - Generic pads with numbered buttons (DirectInput's 1–4) if a game asks.
