# Evidence for PR #116 (issue #67)

- `net-pc-host.png`, `net-pc-guest.png`, `net-pc-*.log`: `cargo xtask net local` (two Solid Eleven instances over UDP, 40 ms / 10 ms jitter / 3 % loss), both at `TRINO_NET_HASH:760:0xaeadc42f3bbe07f9`.
- `net-web-first.png`, `net-web-second.png`, `net-web-results.json`: `cargo xtask net web` (two Chromium pages over WebRTC), same hash.
- `replay-pc-400.png`, `replay-n64-400.png`: castle64's PC recording replayed to frame 400 on the PC and in ares (hash `0x63c8df1c9755648a` on both).
- `replay-pc-last.png`: the recording's last frame on the PC.
- `editor-replay.png`, `editor-net-tab.png`: the editor's Play here replay (scrubber, two players, hashes) and the Net tab.
- `n64-both.log`, `3ds-both.log`, `web.log`: `cargo xtask test n64 --expansion-pak both`, `test 3ds --new3ds both`, `test web` on the final branch.
