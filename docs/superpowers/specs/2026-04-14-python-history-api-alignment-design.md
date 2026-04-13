# Python History API Alignment Design

> Historical design note:
> This document captures the approved breaking migration that aligns `tqsdk-rs`
> history/serial terminology with the official Python SDK.
> For the current public API after implementation, prefer `README.md`,
> `docs/architecture.md`, and `AGENTS.md`.

## Summary

Align the Rust market-data history API with Python by making the public split explicit:

- `serial` means recent in-memory/updating sequence data, capped at `10000` rows.
- `data_series` means one-shot time-range historical download.
- `DataDownloader` remains the batch/history export workflow.

This migration is intentionally breaking. Existing Rust naming that treats
`get_*_data_series` as the main history entry point will be replaced by Python-style
terminology and docs.

## Approved Scope

- Break public market-data history naming to match Python semantics.
- Promote `serial` terminology to the canonical Rust public surface.
- Keep time-range historical download support, but demote it to explicit download APIs.
- Update examples, README, architecture docs, migration docs, and tests accordingly.

## Goals

- Make Python users immediately understand which Rust API corresponds to
  `get_kline_serial` / `get_tick_serial`.
- Stop using `history` as an ambiguous label that currently mixes
  recent-window sequence access with time-range historical download.
- Preserve the current technical capabilities:
  recent updating windows, one-shot time-range download, and CSV-oriented batch download.

## Non-Goals

- No attempt to preserve source compatibility for old naming.
- No change to underlying pagination protocol or `tq_dl` permission semantics.
- No change to `ReplaySession` API naming in this migration.
- No redesign of `SeriesSubscription` update mechanics.

## Current Problem

Today the Rust SDK exposes:

- `Client::kline` / `Client::tick` for updating windows
- `Client::get_kline_data_series` / `Client::get_tick_data_series` for time-range download

This technically maps to Python, but the Rust docs and examples emphasize the download path
as the main "history" interface. That causes two problems:

1. Python users look for `serial` and do not find it.
2. Users run `examples/history.rs`, which currently uses the download path and requires
   `tq_dl`, even when they only want a recent bounded sequence.

## Python Reference Model

The target semantic mapping is:

- `get_kline_serial(symbol, dur, data_length<=10000)` -> recent Kline sequence, auto-updating
- `get_tick_serial(symbol, data_length<=10000)` -> recent Tick sequence, auto-updating
- `get_kline_data_series(symbol, dur, start_dt, end_dt)` -> one-shot historical Kline download
- `get_tick_data_series(symbol, start_dt, end_dt)` -> one-shot historical Tick download
- `DataDownloader(...)` -> explicit batch/history export workflow

## Decision

Adopt a breaking public API migration rather than a compatibility layer.

Rust will expose Python-aligned names directly and stop treating current names as canonical.

## Public API Changes

### Serial APIs Become Canonical

Add and document these as the main public history/sequence surface:

```rust
impl Client {
    pub async fn get_kline_serial<T>(
        &self,
        symbols: T,
        duration: std::time::Duration,
        data_length: usize,
    ) -> Result<Arc<SeriesSubscription>>
    where
        T: Into<KlineSymbols>;

    pub async fn get_tick_serial(
        &self,
        symbol: &str,
        data_length: usize,
    ) -> Result<Arc<SeriesSubscription>>;
}
```

Semantics:

- same underlying behavior as the current `Client::kline` / `Client::tick`
- updating sequence, not one-shot snapshot
- `data_length` remains capped to `10000`

### Old Window APIs Are Removed

The current methods:

- `Client::kline`
- `Client::tick`

are removed in the same change.

They are not retained as compatibility aliases and are not kept as deprecated shims.
The user explicitly approved the breaking migration in favor of Python semantic clarity.

### Download APIs Stay Explicitly Download-Oriented

The current methods:

- `Client::get_kline_data_series`
- `Client::get_tick_data_series`

keep their time-range download semantics and `tq_dl` requirement.

However, they are reclassified in docs and examples as historical download APIs rather than
general "history sequence" APIs.

### Downloader Remains Separate

`Client::spawn_data_downloader*` stays unchanged and remains the explicit multi-page /
CSV-oriented download workflow.

## Example Changes

### `examples/history.rs`

This example must stop using time-range download.

New behavior:

- read `TQ_HISTORY_BAR_SECONDS`
- derive bounded `data_length` from `TQ_HISTORY_LOOKBACK_MINUTES`
- clamp to `10000`
- call `Client::get_kline_serial(...)`
- wait until initial snapshot is ready
- print the current bounded window summary

This makes the example runnable without `tq_dl` when the account has ordinary market-data grants.

### Download Example

If the repository still wants to demonstrate time-range historical download, add or rename an example
to something explicit such as:

- `examples/data_series.rs`
- or `examples/download_history.rs`

That example should continue to require `tq_dl`.

## Documentation Changes

Update all public-facing docs to use this split consistently.

### README

Change the canonical description from:

- `Client::{kline,tick}` = realtime windows
- `Client::{get_kline_data_series,get_tick_data_series}` = history

to:

- `Client::{get_kline_serial,get_tick_serial}` = Python-aligned recent serial sequence APIs
- `Client::{get_kline_data_series,get_tick_data_series}` = one-shot time-range download APIs
- `Client::spawn_data_downloader*` = batch/history export workflow

Also update the examples table so `history.rs` no longer claims `tq_dl` is required.

### Architecture

Revise `docs/architecture.md` so the Series section is framed around:

- serial subscriptions / updating windows
- time-range download
- downloader/export

### Migration Docs

Update `docs/migration-remove-legacy-compat.md` to reflect the new canonical mapping and
to explicitly call out the breaking rename from `kline/tick` to `get_*_serial`.

### Agent Guidance

If the public shape changes, update:

- `AGENTS.md`
- `CLAUDE.md`
- any `skills/tqsdk-rs/` memory/doc package if present in this repo tree

so future agents do not keep generating the old API.

## Behavioral Rules

### Serial APIs

- always bounded
- `data_length` normalized to `1..=10000`
- initial snapshot fetched through the same live window mechanism as current `kline/tick`
- subsequent updates continue through `SeriesSubscription`

### Data-Series Download APIs

- still one-shot `[start_dt, end_dt)` history download
- still require `tq_dl`
- still allowed to use disk cache and paginated fetch path

### Error Semantics

- serial APIs should fail only on normal market-data grant issues or transport issues
- serial APIs must not trigger `permission_denied_history()`
- download APIs must continue to use `permission_denied_history()` when `tq_dl` is absent

## Test Plan

### New / Updated API Tests

- facade tests for `Client::get_kline_serial` and `Client::get_tick_serial`
- verify they map to updating `SeriesSubscription` semantics
- verify data length normalization still caps at `10000`

### Download Guard Tests

Retain and update existing tests that assert:

- `kline_data_series` requires `tq_dl`
- `tick_data_series` requires `tq_dl`
- replay history loading bypasses that guard

### Example Verification

- `cargo check --examples`
- `cargo test`
- rerun `cargo run --example history`

The updated `history` example should no longer fail with professional history permission errors.

## Risks

### Breaking Surface

Any existing user code calling:

- `Client::kline`
- `Client::tick`

as the documented/canonical live sequence API will need migration.

### Naming Churn

Because Rust currently already has `get_*_data_series`, introducing `get_*_serial` while also
removing or deprecating `kline/tick` changes the public mental model and documentation in many places.

### Partial Migration Risk

The most dangerous failure mode is landing only the new functions without rewriting examples and docs.
That would leave two competing histories in the repo and preserve user confusion.

## Recommendation for Implementation Order

1. Add `get_*_serial` facade and tests.
2. Remove `kline/tick` from the public `Client` facade and migrate call sites.
3. Rewrite `examples/history.rs` around serial semantics.
4. Update README, architecture, migration docs, `AGENTS.md`, `CLAUDE.md`, and any repo-local skill docs in one pass.
5. Run full verification and re-run the example.

## Open Decision Locked By User

The user explicitly chose the breaking path.

That means this migration should optimize for Python semantic clarity, not backward compatibility.
