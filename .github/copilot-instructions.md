# Copilot instructions

Read [`AGENTS.md`](../AGENTS.md) at the repository root — it is the canonical guide for
every AI agent working on Trino (architecture rules, commands, file formats, pitfalls).
Task-specific playbooks live in [`.agents/skills/`](../.agents/skills/) (open Agent
Skills format).

Non-negotiables: `trino-core` stays `no_std` with zero dependencies; game crates depend
only on `trino-core` + `trino-game-api`; run `cargo fmt --all --check`,
`cargo clippy --workspace --all-targets -- -D warnings` and `cargo test --workspace`
before proposing changes.
