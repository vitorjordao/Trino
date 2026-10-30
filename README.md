# Evidence for PR #131 (issue #79)

- `ab/ab-table.txt`, `ab/ab.json`: `cargo xtask perf ab --base origin/main --control` over 8 paddings (0–8192) and 3 arms (base = main a023cd1, head = the branch, control = main + a no-op patch) in ares, with the Expansion Pak. The T&L work is identical in every arm and scene. The head's shifts are layout: the hot code is identical, while the Rust link order changed (see the PR).
- `reports/*.json`: `target/perf/<target>-<scene>.json` from `test n64 --trace`, `--prof rsp`, `--prof sample` and `test 3ds --trace` (`cargo xtask perf report <file>` prints them).
- `reports/rsp-count.txt`: `cargo xtask rsp count` (pinned in `tests/perf/rsp_counts.toml`).
- `reports/trace-flush-cost.txt`: ISViewer's cost per trace line in ares (`TRINO_TRACE_FLUSH:`).
- `editor/`: the Profiler's Reports view (a `--prof rsp` report against another scene's, and a `--prof sample` report's CPU samples) and its A/B view.
