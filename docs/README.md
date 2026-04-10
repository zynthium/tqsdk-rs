# Docs Index

This directory mixes current product documentation with historical engineering notes.

## Current Docs

- [architecture.md](./architecture.md)
  Current module boundaries, data flow, and architecture notes.
- [migration-marketdata-state.md](./migration-marketdata-state.md)
  Migration guide for the state-driven marketdata model.
- [migration-remove-legacy-compat.md](./migration-remove-legacy-compat.md)
  Migration guide for removed legacy and compatibility surfaces.

## Historical Docs

- [superpowers/README.md](./superpowers/README.md)
  Historical implementation plans and design notes from the April 2026 cleanup.
- [archive/](./archive/)
  Older archived notes that are kept for context, not as canonical API documentation.

## Reading Order

If you are trying to understand the repository as it exists today, start with:

1. `README.md` at the repository root
2. `docs/architecture.md`
3. the relevant migration guide in this directory
4. `AGENTS.md` if you are making changes in the repo
