---
name: test-all
description: Run Trino's full verification suite (fmt, clippy, workspace tests, dep-graph check) exactly like CI. Use before committing, after refactors, or when asked to "run the tests".
---

# test-all

Run the three CI gates in order. Stop at the first failure and fix it before rerunning.

```
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Notes:
- `cargo test --workspace` includes `xtask/tests/dep_graph.rs`, which enforces the
  dependency direction (core has zero deps; games depend only on core/game-api). If it
  fails, fix the Cargo.toml that violated the rule — do not weaken the test.
- If `cargo fmt --all --check` fails, run `cargo fmt --all` and re-verify.
- From Fase 4/5 on, console suites run via `cargo xtask test n64|3ds` (emulators
  required; CI covers them if you cannot run them locally).
