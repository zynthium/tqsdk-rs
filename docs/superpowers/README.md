# Historical Planning Docs

`docs/superpowers/` stores historical implementation plans and design notes captured during the April 2026 cleanup work.

These files are useful for understanding the engineering path that led to the current architecture, but they are not canonical API documentation. Some of them intentionally mention surfaces that have already been removed, renamed, or collapsed.

Typical examples include:

- `BacktestHandle`
- `BacktestExecutionAdapter`
- `compat::TargetPosTask`
- quote / series callback and channel fan-out APIs
- transitional migration steps that have already been completed

When a `docs/superpowers/` document conflicts with the current repository state, follow these documents instead:

- `README.md`
- `docs/architecture.md`
- `docs/migration-marketdata-state.md`
- `docs/migration-remove-legacy-compat.md`
- `AGENTS.md`

If a historical plan is still worth keeping but no longer matches the current code, prefer marking it as historical instead of silently rewriting the whole document as if it described present-day state.
