# Evidence for PR #127

Issues #119, #121 and #125. Every file comes from a local run on the PR's final commit.

| file | what it shows |
|---|---|
| `strict-tmem/strict_tmem.txt` | #125: 15 textures on both sides of each N64 format's TMEM room; the PC's strict mode draws or panics exactly where `N64Format::fits_tmem` says (`crates/runner-pc/tests/strict_tmem.rs`) |
| `ui/ui_save_notice_{pc,n64,3ds}.png` | #121: the starter template's `save_notice` screen (a save recovered from the backup) per target; the goldens in `tests/golden/pc/` |
| `editor/saves-tab.png`, `saves-tab-n64.png`, `saves-tab-3ds.png` | #121: the editor's Saves tab with its PC, N64 (ares `.eeprom`) and 3DS (Azahar SD folders) sources |
| `logs/soak3ds.log` | #121: `cargo xtask save soak 3ds --cycles 8`; Azahar killed with SIGKILL mid-save 8 times, every boot read the last finished save or a later one |
| `logs/test3ds-both.log` | #119: `cargo xtask test 3ds --new3ds both --no-build`; the `save folder` step reads the seeded shared save (`seed=1`) and the shared folder stays untouched; goldens within tolerance on both tiers |
| `logs/test_n64_both.log` | `cargo xtask test n64 --expansion-pak both` (ares in Docker): both tiers pass, the four ares goldens pixel-exact |
| `logs/workspace-tests.log` | `cargo test --workspace --no-fail-fast`: 221 binaries, 1810 passed, 0 failed |
| `libdragon/` | #121's upstream item. libdragon moved from the archived `DragonMinded/libdragon` to `n64brew/libdragon`; filed there as issue n64brew/libdragon#8 and PR n64brew/libdragon#9. `UPSTREAM_ISSUE.md` / `UPSTREAM_PR.md` are the texts as filed, the `.patch` is the fix (branch `eeprom-write-bytes-rmw` on `vitorjordao/libdragon-n64brew`), rerun on n64brew's `preview` f26e1c161, the `testrom_*` logs are libdragon's own test ROM in ares before (220/222, `test_eeprom.c:134` fails) and after the fix (221/222; `test_timer_ticks` fails on both and is unrelated), `COMMANDS.txt` how to reproduce |
