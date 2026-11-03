# Evidence for issue #117 (netplay follow-ups, ADR 0041)

| Folder | What |
|---|---|
| `uds/` | Two Azahar instances in one room playing castle64 in lockstep over the 3DS's local wireless (`uds`). `auto/` is `tools/uds-test/two-azahar.sh` run from scratch, with the two consoles' `TRINO_UDS`/`TRINO_NET_HASH` lines. Both print `TRINO_NET_HASH:600:0x94fa2260aa5f5154`. `manual/` is the first run, driven step by step. |
| `net-local-relay/` | `cargo xtask net local --lobby --route relay --watch --follow 2`: a host, a guest and a spectator through the lobby's relay. Each window's HUD follows its own seat. All three log `0xaeadc42f3bbe07f9` at frame 760. |
| `net-local-direct/` | The plain `net local` (host/join) that CI runs: direct routes, same hash. |
| `net-web/` | `cargo xtask net web --watch`: two Chromium pages plus a spectator page over WebRTC, all on `0xaeadc42f3bbe07f9`. |
| `replay-cut/` | `TREC` v2 recordings from snapshots. `replay cut` of castle64 at frame 400. A live windowed recording from frame 300 (`TRINO_RECORD_FROM`), replayed in a new process with its last frame. |
| `tables/` | The NAT matrix (`crates/net/tests/nat.rs`), the rollback soak, and Solid Eleven in lockstep (it equals ggrs's hash). |
| `logs/` | `test n64 --expansion-pak both`, `test 3ds --new3ds both`, `test web`, `net web`, and the workspace test summary. |
