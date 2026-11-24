# ADR 0045: What Bongo Kart's study asked of the platform: a wall clock, rumble, remapping, images

Status: accepted (November 2026). Bongo Kart's study (its epic #24) ranked the engine
changes the game needs. This ADR covers the platform's: GitHub Bongo-Kart #182 (§1),
#180 (§2) and #183 (§3, §4, §5). ADR 0043 covers the renderer's and ADR 0044 the audio's.

Everything here is a new `Platform` method with a default, or a new core type, so there is
no `GAME_API_VERSION` bump. A backend that doesn't implement a method answers "none":
`unix_seconds` is `None`, `rumble_support` is `Unsupported`, `rebind_key` is `false` and
`save_image` is `Err(Unsupported)`. A game asks instead of guessing from the platform.

| | PC | Web | N64 | 3DS |
|---|---|---|---|---|
| `unix_seconds` | system clock (UTC), `TRINO_UNIX_TIME` pins it | `Date.now()`, `?unix=` pins it | the cartridge's Joybus RTC (local time); `None` without one | console clock (local time) |
| `rumble` | gilrs force feedback, strong + weak motors | `vibrationActuator` `dual-rumble` | Rumble Pak, strength as a duty cycle | — (`Unsupported`) |
| `rebind_key` | yes | yes | — | — |
| `ActionMap` | yes | yes | yes | yes |
| tap shorter than a frame | latched | latched | — (polled pads) | — |
| `save_image` | `<save folder>/images/<name>.png` | a PNG download | — | — |

## 1. Wall-clock time (Bongo-Kart #182)

### Context

A daily challenge, a "best lap today" board and a dated photo need the date.
`Platform::now_micros` is monotonic from an arbitrary start: right for timing, useless for
dates.

### Decisions

- **`Platform::unix_seconds() -> Option<u64>`**: seconds since 1970-01-01. It can jump,
  because the player sets the clock. Use it for dates, never for timing, and never read it
  inside a simulation that a replay or a netplay peer must repeat. Read it once and keep
  the value in the game's state or its recording.
- **`trino_core::clock`** does the calendar math without a C library: `Civil`,
  `unix_from_civil`, `civil_from_unix` (Howard Hinnant's algorithms) and `day_index`, the
  seed of a daily challenge that every player gets on the same date. Fields out of range
  and dates before 1970 are `None`. `PinnedClock` is a pinned value that keeps running.
- **Each target's source:**
  - **PC:** `SystemTime`. `TRINO_UNIX_TIME=<seconds>` pins it for tests, goldens and
    recordings (`PcPlatform::pin_clock`).
  - **Web:** `Date.now()`; `?unix=<seconds>` pins it.
  - **3DS:** the shim's `trino_unix_seconds` turns `osGetTime()` (milliseconds since 1900)
    into a `u32`, because no 64-bit value crosses the FFI. The console has no time zone,
    so the value is the player's local time counted as UTC: a day starts at the player's
    midnight.
  - **N64:** the shim's `trino_rtc_read` sends two raw Joybus commands to the
    cartridge port with `joybus_exec_cmd`: status (`0x06`, identifier `0x0010` for a
    clock), then a read of block 2 (`0x07`: BCD second, minute, hour, day, weekday,
    month, year, century). Rust converts the fields with `unix_from_civil` and reads the
    chip at most once a second; between reads the value runs on the tick counter.
    libdragon's `rtc.c` was the first version. Its `time()` hook links newlib's `mktime`,
    `tzset` and `sscanf`, about 14 KB of code, which on the base console comes out of the
    heap, and the base self-test ran out of it.
  - **The N64 header:** only cartridges with a clock have one (an emulator or a flash cart
    that provides it), and the ROM header declares it (`ed64romconfig --rtc`). In
    `trino.toml`, `[n64] rtc` defaults to on unless the save is an EEPROM, because
    ed64romconfig warns that "the combination of EEPROM + RTC does not work on
    EverDrive". A self-test ROM declares the clock unless `rtc = false`, so ares exercises
    the clock: it answers with the host's time. Without a clock the value is `None`.

## 2. Rumble with a setting and no placebo (Bongo-Kart #180)

### Context

Bumps, boosts and item hits want a jolt in the hands. The issue asked for strength as
0–100 %, for an option the player can turn down or off, and for "no placebo": an option
that does nothing on the player's hardware must not be shown.

### Decisions

- **`Platform::rumble(player, low, high, ms)`**: the low-frequency (strong) and
  high-frequency (weak) motors at 0..=1 for `ms` milliseconds. A new request replaces the
  last one. A zero strength or a zero duration stops the motors.
- **`Platform::rumble_support(player) -> RumbleSupport`**:
  - `Unsupported`: this build can never rumble. Hide the option.
  - `Disconnected`: it could, but no pad that rumbles is seated at that player now. Show
    the option and say so.
  - `Ready`: a pad that rumbles is seated there.

  A request on an `Unsupported` target is dropped and `rumble_state` stays off.
- **`Platform::rumble_state(player)`** is what the motors were asked to play now. Tests
  and the self-test read it, and so does a game that draws a rumble indicator.
- **The player's setting:** `AccessSettings::rumble` (0..=100 %, default 100) scales every
  request (`rumble_factor`), and 0 turns rumble off. It packs into the three accessibility
  bytes: byte 2's six spare bits hold `(100 − rumble) / 2`, so an old save, whose bits are
  0, reads 100 %. Its text form is `rumble=40` or `vibration=40%`.
- **`trino_core::HostedRumble`** keeps the per-player state, the expiry and the request
  the host forwards (`take_sent`). Every backend shares it.
- **Each target:**
  - **PC:** gilrs force feedback, built with the `gamepad` feature. A `Strong` effect
    plays the low strength and a `Weak` effect the high one, on every pad seated at that
    player. Dropping the effect stops it. `PcInput::rumble_support` reads
    `is_ff_supported`. The host calls `PcPlatform::publish_rumble(&mut input)` each frame.
  - **Web:** `Gamepad.vibrationActuator.playEffect("dual-rumble", …)` and `reset()`.
    Chrome, Edge and Safari have it. Firefox has no actuator in the standard API, so its
    pads read as `Disconnected`.
  - **N64:** the Rumble Pak's motor only turns on or off. `N64Platform::tick`, called once
    a frame by the host, runs it as a duty cycle of the strength (`trino_core::DutyCycle`,
    tested on the host): 1.0 runs every frame, 0.5 every other frame, and a request starts
    with the motor on, so a short bump is felt at once. `trino_rumble_supported(port)` reads the pak. The ROM header
    declares a Rumble Pak in all four ports (`[n64] pak = "rumble"`, the default; the
    other values are `"controller"` and `"none"`), which ares and flash carts read.
  - **3DS:** it has no motor, so it is `Unsupported`.

## 3. Rebinding: the console buttons and a game's own actions (Bongo-Kart #183)

### Context

Bongo Kart's controls are actions: accelerate, brake, drift, item, horn, look back. Each
has several keys, and some of those keys no console button has. The PC already rebound
console buttons by press (`rebind_player`), but a menu that sets "drift = Space" in one
call couldn't, and the Web didn't rebind at all.

### Decisions

- **`Platform::rebind_key(player, button, key) -> bool`** binds a console button to a key
  at once. If another button had that key, it loses it. Keys with a fixed role (WASD,
  Escape, F1–F8, …) are refused (`false`), as they are for `rebind_player`. Shift binds
  the left Shift. The PC and the Web implement it. On the PC it needs the host's input
  published (`publish_input`).
- **The Web rebinds now:** `rebind_player` (the next bindable key), `rebind_key`,
  `cancel_rebind`, `reset_bindings` and labels from the live bindings. The bindings are
  stored in `localStorage` under `trino-input:<save>`, except in a self-test or a smoke
  run. The key table and its labels are the PC's (`keys::bindable`).
- **`trino_core::ActionMap<N>`** holds a game's own actions. An `ActionBinding` has two
  keys and two pad buttons. `down`, `pressed` and `released` read an `InputState`, so the
  same map and the same remapping screen work on the PC, in a browser and with a
  console's pad, with no backend code.
  - **Remapping:** `pressed_key` / `pressed_button` catch the next key or button the
    player presses, skipping the game's reserved ones. `rebind_key` / `rebind_button` put
    it in a slot. A key belongs to one action: taking it from another action gives that
    action this slot's old key (a swap and its index come back), so nothing is unbound
    by accident.
  - **Saving:** `write_save` / `read_save` use `2 + 4 × N` bytes (version, count, then
    each action's keys and buttons). A damaged or foreign block is refused and the map
    keeps its bindings.
  - **One copy of the code:** the methods are thin over slice functions, so map sizes
    don't duplicate the code (the `Ui<N>` lesson on the N64).
- **`Key::label()`**: a key's short uppercase ASCII name ("SPACE", "LEFT", "J") for a
  remapping screen, the same names the PC's `binding_label` uses.

## 4. A tap shorter than a frame (Bongo-Kart #183)

### Context

At 30 fps, or on a slow frame, a key pressed and released between two polls never read as
down, so a quick tap on "use item" was lost. The PC and the Web kept only the held state.

### Decision

The keyboard latches presses: a key that went down since the last poll reads as down for
that poll even if it is up again, then is forgotten. Its console button, its key bit and
its WASD stick direction read down too. Held keys are unaffected. This is
`PcInput::handle_key` / `keyboard_state` on the PC and `keys::Keyboard::state` on the Web.
The consoles poll their pads once a frame and have no events to latch.

## 5. Saving an image (Bongo-Kart #183)

### Context

A photo mode, a race's result card and a ghost's thumbnail are pictures the player keeps.

### Decisions

- **`Platform::save_image(name, &RgbaImage) -> Result<(), SaveError>`**. `RgbaImage` holds
  8-bit RGBA texels, row-major from the top-left, with no padding, `width × height × 4`
  bytes. Each side is 1..=8192. A struct built by hand is checked again by every host.
- **Names stay plain:** 1..=64 ASCII letters, digits, `-` and `_` (`image_name_ok`). A
  name never becomes a folder or an extension. A bad name or an inconsistent image is
  `Io`.
- **PC:** a PNG in `<save folder>/images/<name>.png`, written to a `.tmp` file and renamed.
  Without a save folder the result is `Unsupported`.
- **Web:** a PNG download (`<a download>` on a `blob:` URL). The URL is revoked at the
  next download, because revoking it at once cancels the download in some browsers.
- **Consoles:** `Unsupported`. There is no file a player could take off the console.

## Tests

- `crates/core/tests/actions.rs`: the swaps, the next press and the save block.
- Unit tests in `rumble.rs`, `clock.rs`, `image.rs` and `access`: the pack round trips and
  old saves at 100 %.
- `crates/platform-pc/tests/clock_rumble_keys.rs`: the clock and its pin, rumble scaled
  and forwarded, `rebind_key`, the tap latch and `save_image`'s PNG. They pass with and
  without the `gamepad` feature.
- `crates/platform-web/src/keys.rs`: the Web's tap latch, rebinding by press and at once,
  and the bindings' text round trip.
- `xtask` `project::tests`: the `[n64] rtc` and `pak` keys and the header flags.
- **Self-test step `clock rumble input`** (`castle64::selftest::clock_rumble_input`) on
  PC, ares, Azahar and the browsers:
  - the calendar math on that CPU, and the wall clock when there is one (after 2000, not
    running backwards, a round trip);
  - rumble at the player's 50 % where the target can rumble, and nothing where it can't;
  - an `ActionMap` swap and save block, checking the bytes;
  - `rebind_key` where there are bindings, undone at once;
  - `save_image` kept or `Unsupported`.

  The step prints `TRINO_STEP:clock rumble input ok clock=<date|none> rumble=<support>
  rebind=<0|1> image=<0|1>`. On PC the test is `engine_checks`'
  `the_clock_rumble_input_step_passes_on_pc`.
