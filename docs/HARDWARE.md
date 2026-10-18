# Running on real hardware

`cargo xtask run n64|3ds` opens the game in an emulator. With `--device`, the same
command sends it to a console instead, and the game's debug log appears in your
terminal:

```
cargo xtask run n64 --device sc64                  # SummerCart64 over USB
cargo xtask run n64 --device everdrive             # EverDrive 64 X7 / 3.0 over USB
cargo xtask run 3ds --device network --ip <IP>     # a 3DS on the LAN, via 3dslink
cargo xtask run 3ds --device network --ip <IP> --gdb [--gdb-port 4003]
                                                   # + a gdb script for Luma3DS
```

- **Same build as the emulator.** xtask builds the ROM or `.3dsx` first, as `run` always
  does. Flags that change this:
  - `--no-build` sends the file already in `target/`, whichever build that is;
  - `--selftest` builds and sends the self-test build, whose `TRINO_STEP`/
    `TRINO_PERF_*`/`TRINO_TEST_*` lines then print on the terminal.

  The two don't combine: xtask refuses `--selftest --no-build`, because nothing would
  be built. To send a self-test build later, run `cargo xtask build n64|3ds --selftest`
  first, then `run … --no-build`.
- **Flags after `--`** are not xtask's. `run pc -- --gdb` passes `--gdb` to the PC
  app.
- **Other spellings and projects.** `run --target n64 --device sc64` is the same
  command, and `n3ds` works wherever `3ds` does. `--project <dir>` (or `$TRINO_PROJECT`, or a `trino.toml` in the current
  directory) deploys an external project's ROM or app (ADR 0008), so from such a
  project `cargo trino run n64 --device sc64` works.
- **What you see.** Every line the game logs reaches the terminal unchanged:
  - `runtime::log`;
  - `N64Host`/`N3dsHost` reports such as `TRINO_FRAME:<scene> avg_us= max_us= …`;
  - panics (`TRINO_PANIC: …`).

  On the N64 these times are real console time. ares does not time the RDP.
- **Stopping and the emulator.** Ctrl+C stops the log. Without `--device` nothing
  changes: the emulator runs as before.

## Before the 3DS: Azahar's GPU timing

Azahar has a setting, **Simulate 3DS GPU timings** (Emulation › Configure ›
Graphics), off by default since Azahar #2774. As issue #43 describes that change,
with it off the emulated GPU runs as fast as the host's GPU allows, decoupled from the
PICA200's real limits, so a frame bound by the GPU can look cheaper than it is. With
it on, Azahar simulates the 3DS GPU's timing. xtask turns it on for one session:

```
cargo xtask run 3ds --accurate-timing     # also: test 3ds, watch 3ds, test all
TRINO_AZAHAR_GPU_TIMING=1 cargo xtask test 3ds   # the same (1, true, on or yes)
```

- **Keep it off** while you iterate on gameplay and for the golden comparisons.
  The frames come out the same, but the emulator runs as fast as your PC allows.
- **Turn it on** to check frame pacing (`fps=` in `TRINO_FRAME`), waits on the GPU and
  30/60 fps targets before you deploy with `--device network`. It has not been compared
  with a real 3DS yet, so a real console is still the reference.

What xtask does with Azahar's `qt-config.ini`:
- **Without the flag**, it does not touch the setting, whatever you chose in Azahar.
- **With the flag**, `[Renderer]` gets both lines for the session:

  ```ini
  simulate_3ds_gpu_timings=true
  simulate_3ds_gpu_timings\default=false
  ```

  Qt writes `\default=true` while a value is the default, and Azahar then ignores
  the value, so both lines are needed. The key is `simulate_3ds_gpu_timings`
  (Azahar 2126.2, the pinned build). Issue #43 calls it `simulate_gpu_timing`, a name
  Azahar does not read.
- **After the session**, xtask puts your own two lines back, or removes them if you
  had none:
  - `test` puts them back once it stops the emulator;
  - `run` waits until you close Azahar. Azahar writes `qt-config.ini` while it runs,
    so the restore comes after it exits, where nothing Azahar saves can undo it;
  - `watch` puts them back when the loop ends.

  Until then a copy waits in `qt-config.ini.trino-gpu-timing`. If xtask is stopped
  first (Ctrl+C, a crash), the next `run`, `test` or `watch 3ds` restores it from that
  copy before it starts Azahar.
- **Proof.** Azahar logs its configuration at boot. `test 3ds` echoes
  `Renderer_Simulate3DSGPUTimings: true|false`. With the flag, the test fails if
  Azahar booted with it off. `run` prints the line after Azahar closes.
- **Not with `--device`.** A real 3DS has real timing, so xtask refuses the flag
  there, and on the N64 as well.

## Hardware tiers: the Expansion Pak and the New 3DS

One ROM and one `.3dsx` run on both variants of each console
([ADR 0020](adr/0020-console-hardware-tiers.md)). The boot line says which one it found,
before anything else the game logs:

```
TRINO_HW n64 ram=8M tier=expanded                         # Expansion Pak (4M: base)
TRINO_HW 3ds new=1 speedup=1 app_ram=96M tier=expanded    # New 3DS, 804 MHz
TRINO_HW 3ds new=1 speedup=0 app_ram=96M tier=base        # New 3DS, [3ds] speedup = false
TRINO_HW 3ds new=0 speedup=0 app_ram=96M tier=base        # old 3DS / 2DS
```

- **N64:** the boot reads libdragon's `get_memory_size()`; 8 MB means the Pak is in.
  The game must still fit and run in 4 MB: the tier adds RAM, never CPU time.
- **3DS:** `APT_CheckNew3DS`, then `osSetSpeedupEnable(true)` (804 MHz and the L2 cache)
  unless the project set `[3ds] speedup = false` in `trino.toml` (the bake writes a
  `no_speedup` file into the RomFS). A New 3DS without the speedup runs on the base tier,
  so the game's timing is the old 3DS's. Azahar boots both builds but runs the ARM11 at
  one clock either way (castle64 and Solid Eleven measure the same CPU time, ADR 0020
  §7): the speedup's gain is measured here, on the console. `app_ram` is the
  application memory region the system gave the app, reported, not gated on: Azahar
  2126.2 gives a `.3dsx` 96 MB in both modes (the lines above); a console's depends on
  its model and how the app was launched.
- **Real hardware is the console it is:** `--expansion-pak` and `--new3ds` only set the
  emulators' switches, and xtask refuses them with `--device`. `--speedup on|off` changes
  the build, so it works with `--device network` too.
- **The second core** on the 3DS (`trino_platform_3ds::worker`): core 2 on a New 3DS,
  core 1 at 30% (`APT_SetAppCpuTimeLimit(30)`) on an old one. The self-test's
  `TRINO_STEP:worker core=N queued=8 us=…` line shows which one the jobs ran on.

## How the log gets out

| Console | Game side | Host side |
|---|---|---|
| N64 | The shim calls libdragon's `debug_init_usblog()` as well as `debug_init_isviewer()`, so `debugf` (every `trino_log`) goes to the flashcart's USB port with the UNFLoader protocol. libdragon's `debug.h` lists 64drive, EverDrive64 and SC64. In ares, USB logging finds no cart and only ISViewer is used. | `sc64deployer debug` or `UNFLoader -d` prints it. |
| 3DS | 3dslink passes the host's address as a launch argument and libctru stores it in `__3dslink_host`. When it is set, the shim calls `socInit` and `link3dsStdio()` (stdout and stderr become a TCP connection to the host). `trino_log` and panics are then also written to stdout, besides `svcOutputDebugString`. Under Azahar, or launched from the menu, `__3dslink_host` is 0 and nothing changes. | `3dslink -s` keeps a server on TCP 17491 and prints what arrives. |

The loader runs with this terminal's stdin, stdout and stderr. A pipe would make
UNFLoader and 3dslink buffer the log in blocks. `sc64deployer debug` also reads
stdin, so it can send input to the console.

## The commands xtask runs

Every flag below comes from the tool's own documentation or source:
- SummerCart64: [`docs/00_quick_startup_guide.md`](https://github.com/Polprzewodnikowy/SummerCart64/blob/main/docs/00_quick_startup_guide.md)
  and [`sw/deployer/src/main.rs`](https://github.com/Polprzewodnikowy/SummerCart64/blob/main/sw/deployer/src/main.rs);
- UNFLoader: [`UNFLoader/README.md`](https://github.com/buu342/N64-UNFLoader/blob/master/UNFLoader/README.md)
  and [`UNFLoader/main.cpp`](https://github.com/buu342/N64-UNFLoader/blob/master/UNFLoader/main.cpp);
- 3dslink: `3dslink --help` and [`host/src/main.c`](https://github.com/devkitPro/3dslink/blob/master/host/src/main.c);
- Luma3DS: [README](https://github.com/LumaTeam/Luma3DS#readme), "GDB ports".

| `--device` | Command lines | Notes |
|---|---|---|
| `sc64` | `sc64deployer upload <rom>`, then `sc64deployer debug` | `upload` writes the ROM and picks the save type from its header. It **does not boot the console**: power the N64 on, or press reset, once xtask says so. `--reboot` is left out because it needs support in the running game. `--isv` is left out because the ROM already logs over USB, and ISViewer would print every line twice. |
| `everdrive` | `UNFLoader -b -f 3 -r <rom> -d` | `-b`: plain stdout, no ncurses UI. Without it the log stays inside UNFLoader's window. `-f 3`: the "EverDrive 3.0 or X7" cart type, so autodetection is skipped. `-r`: upload, then boot (UNFLoader sends the cart's PIFboot command after the upload, `device_everdrive.cpp`). `-d`: debug mode. `-d` must stay last, because it takes an optional log-file name. |
| `network` | `3dslink -a <ip> -s <app.3dsx>` | `-a`: the 3DS's address. Without `--ip`, 3dslink broadcasts to find the 3DS. `-s`: after the upload, serve on TCP 17491 and print the app's output until it exits. |

## Prerequisites

### Tools

xtask looks for each tool in this order:
1. the environment variable;
2. `PATH`;
3. for 3dslink only, `tools/bin` of the devkitPro install that `build 3ds` uses. That
   is the first of `$DEVKITPRO`, `C:\devkitPro` and `/opt/devkitpro` that holds
   `libctru`. A Windows installer's msys-style `DEVKITPRO=/opt/devkitpro` is
   skipped when that path doesn't exist.

| Tool | Override | Get it |
|---|---|---|
| `sc64deployer` | `TRINO_SC64DEPLOYER` | `sc64-deployer-<os>-<version>` from the [SummerCart64 releases](https://github.com/Polprzewodnikowy/SummerCart64/releases) |
| `UNFLoader` | `TRINO_UNFLOADER` | The [N64-UNFLoader releases](https://github.com/buu342/N64-UNFLoader/releases). On Linux, run `UNFLoader/installer_linux.sh`, which installs the FTDI library and sets up USB access; otherwise UNFLoader has to run with `sudo`. |
| `3dslink` | `TRINO_3DSLINK` | devkitPro's 3DS tools: `dkp-pacman -S 3dstools`, which is part of `3ds-dev`. Already present if `cargo xtask build 3ds` works. |
| `arm-none-eabi-gdb` | — | devkitARM (`$DEVKITPRO/devkitARM/bin`), for `--gdb` |

### Device detection

- **SummerCart64.** Connect the USB-C cable. `sc64deployer list` shows the cart and
  `sc64deployer info` shows its firmware.
- **EverDrive 64.** Connect the cart's USB port. UNFLoader prints the cart it found.
  `-f 3` skips autodetection, but the FTDI device still has to be reachable.
- **3DS.**
  - The console needs custom firmware (Luma3DS) and the Homebrew Launcher.
  - It must be on the same LAN as the computer.
  - In the Homebrew Launcher press **Y** to start the netloader. It shows the IP to
    pass as `--ip`.

## Debugging a 3DS with gdb (Luma3DS)

Luma3DS's Rosalina has a GDB stub. Its README gives the ports:
- **4000–4002** are the normal ports. Use them in `extended-remote` mode with
  `info os processes` and `attach <pid>`.
- **4003** is the break-on-start port, used without extended-remote.

1. On the 3DS, open Rosalina (L + Down + Select) › Debugger options › **Enable
   debugger**.
2. Run `cargo xtask run 3ds --device network --ip <IP> --gdb`. It writes
   `target/3ds/<name>.gdb`, which loads the unstripped ELF
   (`target/armv6k-nintendo-3ds/release/<bin>.elf`) and connects to `<IP>:<port>`. It
   also prints the gdb command line:
   ```
   $DEVKITPRO/devkitARM/bin/arm-none-eabi-gdb -x target/3ds/<name>.gdb
   ```
3. Run that in a second terminal; the first one keeps printing the log.
   - **Attach: the default, port 4000.** Once the app runs, gdb connects in
     extended-remote mode. Then run `info os processes` and `attach <pid>`.
   - **Break on start: `--gdb-port 4003`.** Pick Rosalina › Debugger options ›
     "Force-debug next application at launch" *before* the upload. The app stops
     before its first instruction, and `continue` runs it.

     **Unverified:** we have not confirmed that a `.3dsx` started by the Homebrew
     Launcher counts as that "next application". If gdb never connects, use attach.

The release ELF has symbols (function names, backtraces) but no line tables.

## Troubleshooting

- **`sc64deployer` / `UNFLoader` / `3dslink` not found.** Each error names the
  install link and the `TRINO_*` override.
- **SC64: nothing happens after `upload`.** Upload never starts the game. Power the
  N64 on, or press reset, while `sc64deployer debug` is running. Run
  `sc64deployer list` to check that the cart is seen at all.
- **SC64 or EverDrive: no log lines.**
  - The ROM must be built without `NDEBUG`. xtask's build never defines it, but
    with it `debugf` compiles to nothing.
  - Only one program can own the USB port. Close other loaders first.
- **UNFLoader on Linux: permission denied, or no device.**
  - Run `installer_linux.sh`, which sets up the FTDI driver and permissions, or
    run UNFLoader with `sudo`.
  - Supported carts, per UNFLoader's README: EverDrive X7 with OS 3.04 or
    later, and EverDrive 3.0 with OS 3.04 to 3.06. OS 3.07 and later does not
    work on the 3.0.
- **3dslink: "No response from 3DS!"** The netloader isn't running (press Y in the
  Homebrew Launcher), or broadcasts don't reach it. Pass `--ip`.
- **3dslink uploads, but the log stays empty.**
  - The 3DS connects *back* to the computer on TCP 17491. Allow that port in the
    computer's firewall.
  - The app writes `TRINO_WARN: 3dslink: …` to `svcOutputDebugString`, which Luma
    shows in gdb, when it cannot open the socket.
  - `TRINO_WARN: 3dslink: no host server` means the app connected before
    `3dslink -s` was listening. The server only starts once the upload has
    finished. Run the command again.
- **`--selftest` on an N64 floods the terminal with hex.** The self-test dumps its
  golden frames as `TRINO_FBR` rows. Over USB that is several thousand lines. The
  goldens are only compared in emulators.
- **3DS: the app runs out of memory.** Under 3dslink only, the socket service takes
  a 1 MiB buffer from the heap (the libctru 3dslink demo's size).
- **Frame numbers differ from ares.** Expected. ares does not time the RDP, so
  real hardware is the reference (BACKLOG P0 1a).

## What is verified

Without hardware, `xtask/tests/hardware.rs` runs the real xtask binary against an
external project. Stub loaders take the place of `sc64deployer`, `UNFLoader` and
`3dslink`. The tests check:
- the exact command lines;
- the order (upload, then debug);
- that a failed upload stops the deploy;
- the tool lookup, through the override variables and through `PATH`;
- the gdb scripts;
- that the loaders' output reaches xtask's stdout.

The 3DS shim's 3dslink path compiles (`cargo xtask build 3ds`), and the ares and
Azahar self-tests stay green with the same goldens.

Not yet run on a physical SummerCart64, EverDrive or 3DS: `docs/BACKLOG.md`.
