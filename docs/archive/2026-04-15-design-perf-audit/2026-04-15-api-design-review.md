# API Design Review - 2026-04-15

## Purpose

This document records the current review conclusion for `tqsdk-rs` public API design after multiple re-check passes.

The goal is not to list every possible improvement. The goal is to separate:

- confirmed design problems
- previous findings that were re-checked and withdrawn
- the higher-level design philosophy issue that explains the confirmed problems
- a one-shot redesign direction that can fix the retained issues coherently

This note is intended to be handed to another reviewer for cross-check before implementation work starts.

## Scope And Verification

Reviewed sources:

- `README.md`
- `docs/architecture.md`
- `docs/migration-marketdata-state.md`
- `docs/migration-remove-legacy-compat.md`
- `src/client/*`
- `src/marketdata/mod.rs`
- `src/runtime/market.rs`
- `src/trade_session/*`

Verification commands run:

- `cargo test client_ -- --nocapture`
- `cargo test marketdata::tests -- --nocapture`
- `cargo test close_invalidates_market_interfaces -- --nocapture`

These checks confirmed existing tests still pass, but they do not cover every lifecycle contract claimed by docs and design plans.

## Executive Summary

The crate's core direction is mostly coherent:

- market data is modeled as latest state, not as an event log
- trade APIs are split into snapshot state and reliable event streams
- replay/backtest remains an explicit separate path

The main design issue is higher-level:

**live market lifecycle is not modeled as a first-class public contract**

Instead, lifecycle is currently expressed partly by documentation, partly by runtime flags, and partly by which facade method happens to check initialization state.

That missing contract is the root cause behind the retained problems:

1. some market-facing wait/read paths do not become invalid after teardown
2. runtime account switching is documented as supported, but there is no atomic live-context replacement model
3. `Client` is described as the single live entry point, but there is no single live context owner shared by all live consumers

## Confirmed Findings

### 1. Market lifecycle is enforced inconsistently across public APIs

Some market-facing APIs explicitly check whether live market is active:

- `Client::subscribe_quote()` checks `market_active` and `quotes_ws`
  - `src/client/facade.rs:397`

Some other market-facing APIs bypass that gate entirely:

- `Client::quote()`
  - `src/client/mod.rs:104`
- `Client::kline_ref()`
  - `src/client/mod.rs:108`
- `Client::tick_ref()`
  - `src/client/mod.rs:112`
- `Client::wait_update()`
  - `src/client/mod.rs:116`
- `Client::wait_update_and_drain()`
  - `src/client/mod.rs:120`

Those methods delegate to `TqApi` / `MarketDataState` directly, without consulting live market lifecycle state.

`close_market()` only closes websocket-side facilities and flips `market_active`:

- `src/client/market.rs:309`

It does not close or invalidate the wait channels used by:

- `TqApi::wait_update()`
  - `src/marketdata/mod.rs:331`
- `QuoteRef::wait_update()`
  - `src/marketdata/mod.rs:450`
- `KlineRef::wait_update()`
  - `src/marketdata/mod.rs:506`
- `TickRef::wait_update()`
  - `src/marketdata/mod.rs:558`

This means teardown semantics are not uniformly represented at the API layer.

Important nuance:

- The weaker earlier claim "inactive `Client` is itself a design bug" was too broad.
- The repo does treat `init_market()` as a live market/query precondition in docs and planning notes.
- The real retained issue is narrower and stronger: **teardown is not part of the wait/read contract**.

This is particularly notable because the breaking consolidation plan explicitly says:

- `client.close()` should invalidate those interfaces
  - `docs/superpowers/plans/2026-04-11-breaking-api-consolidation.md:150-153`

But the regression test only covers query/subscription/serial APIs:

- `src/client/tests.rs:289-302`

It does not cover:

- `Client::wait_update()`
- `Client::wait_update_and_drain()`
- `QuoteRef::wait_update()`
- `KlineRef::wait_update()`
- `TickRef::wait_update()`

### 2. Runtime account switching is documented as supported, but the implementation does not provide an atomic re-auth / rebind model

README explicitly documents runtime account switching:

- `README.md:751-768`

The documented flow is:

1. create a new authenticator
2. call `client.set_auth(new_auth).await`
3. call `client.init_market().await?`

However, current implementation of `set_auth()` only replaces the `self.auth` field:

- `src/client/facade.rs:139-140`

There is no atomic rebuild of the live market context.

`init_market()` also does not first tear down an existing live market context:

- `src/client/market.rs:291-293`

It simply constructs a new websocket and replaces fields:

- `self.quotes_ws = Some(...)`
  - `src/client/market.rs:253`
- `self.series_api = Some(...)`
  - `src/client/market.rs:254-263`
- `self.ins_api = Some(...)`
  - `src/client/market.rs:269-276`

That means the public "switch account at runtime" story is not backed by a clearly modeled and atomic live-context replacement contract.

This is a real design problem, not just incomplete docs, because:

- docs promise support
- implementation exposes a mutation API
- implementation does not define the lifecycle semantics required to make that mutation safe

### 3. There is no single live context owner, even though the public philosophy says live APIs are consolidated

The intended philosophy is clear:

- live API should consolidate around `Client`
  - `README.md:26-35`
- runtime adapter internals should not be public extension surface
  - `README.md:201-205`
  - `docs/migration-remove-legacy-compat.md:13-21`

But `Client::into_runtime()` creates a `LiveMarketAdapter`:

- `src/client/facade.rs:530-538`

And `LiveMarketAdapter` lazy-initializes its own quote websocket path:

- `src/runtime/market.rs:65-104`

It also creates a fresh `MarketDataState` for that websocket:

- `src/runtime/market.rs:96-100`

So "single live entry point" is true only at the naming layer, not at the live-context ownership layer.

This matters because it is the structural reason the retained issues keep showing up:

- no single owner means no single teardown contract
- no single owner means no single re-auth contract
- no single owner means the runtime path can drift from the direct `Client` live path

## Findings Withdrawn After Re-Check

### 1. `DataManager` plus `MarketDataState` is not, by itself, a confirmed design bug

Earlier review overreached here.

The current codebase and docs consistently describe:

- `DataManager` as the DIFF merge / epoch / query state core
- `MarketDataState` as the typed market snapshot surface used by `QuoteRef` / `KlineRef` / `TickRef`

Evidence:

- `docs/migration-marketdata-state.md`
- `docs/architecture.md`
- `README.md:419-484`

This split may still deserve simplification later, but current evidence supports treating it as an intentional layered design, not as a proven defect by itself.

### 2. Auto-initializing tracing in `ClientBuilder::build()` is not the main design-philosophy problem

It is still a debatable library design choice:

- `src/client/builder.rs:233`
- `src/logger.rs:121-126`

But re-checking the repository as a whole suggests this is a convenience tradeoff, not the core inconsistency driving the retained issues.

The lifecycle/context problem is much more central.

## Higher-Level Diagnosis

The real philosophy issue is this:

**the crate currently optimizes for a single object name (`Client`) instead of a single live session owner**

Those are not the same thing.

The repo's stated philosophy is already close to correct:

- market data: state-driven, not push fan-out
- trade: state vs reliable events
- replay: explicit separate mode

What is missing is an explicit public object representing:

- one authenticated live market context
- one lifecycle
- one teardown boundary
- one re-auth boundary
- one shared owner reused by direct market APIs and live runtime

Without that object, lifecycle leaks into:

- docs
- booleans like `market_active`
- facade-specific guard logic
- implicit assumptions about when references remain meaningful

## Recommended One-Shot Redesign

### Target philosophy

Replace:

- "single entry object"

With:

- "single live session owner"

### Proposed public model

#### 1. Session object: `Client`

After checking the official Python implementation, the better public model is to align `Client` with Python's `TqApi`:

- `Client` itself is one live session owner
- `ClientBuilder`, `ClientConfig`, and `TqAuth` are construction-side concepts
- no public `LiveClient` / `LiveSession` second object is introduced

`Client` should own one private `LiveContext`, containing:

- auth
- `DataManager`
- `MarketDataState`
- quote websocket
- `SeriesAPI`
- `InsAPI`
- lifecycle state machine
- lifecycle signal for waiters and refs

`Client` should expose the canonical live operations:

- `quote()`
- `kline_ref()`
- `tick_ref()`
- `wait_update()`
- `wait_update_and_drain()`
- `subscribe_quote()`
- `get_kline_serial()`
- `get_tick_serial()`
- `query_*()`

#### 2. Close semantics

Closing the `Client` session should:

- tear down the unique live context
- wake any waiter bound to that context
- make all bound refs return a closed/unavailable error instead of hanging forever

#### 3. Re-auth semantics

Delete:

- `Client::set_auth()`

Replace with explicit new session construction:

- close the old `Client`
- build a new `Client` with new auth
- do not reuse `DataManager` / `MarketDataState` across accounts

No field-level authenticator swapping should remain in the public API.

#### 4. Runtime integration

`Client::into_runtime()` may remain, but it must reuse the same private live context, not create an independent market wiring path.

That means:

- one live websocket owner
- one lifecycle
- one re-auth boundary
- one teardown boundary

### Why this fixes the retained problems

Problem 1:

- `close()` no longer leaves `wait_update()` and refs attached to a still-open internal watch channel without lifecycle knowledge.

Problem 2:

- re-auth becomes a context replacement operation, not a field mutation.

Problem 3:

- runtime and direct `Client` live APIs stop diverging structurally because both consume the same `Client`-owned live context.

## Compatibility Cost

This is intentionally a breaking redesign.

Main public changes:

- `set_auth()` is removed
- `Client::into_runtime()` is refactored to reuse the same live context instead of creating its own market wiring

This is a large break, but it aligns with the repository's own stated cleanup direction:

- explicit canonical surface
- fewer legacy escape hatches
- stronger lifecycle semantics

## Suggested Review Questions For A Second Reviewer

If another reviewer checks this note, the key questions should be:

1. Do they agree that the core issue is missing live-context ownership, rather than state-driven market design itself?
2. Do they agree that `close()` contract is incomplete for wait/read paths?
3. Do they agree that README currently over-promises runtime account switching?
4. Do they agree that runtime should reuse the same live context instead of creating an independent one?
5. Do they agree that public `Client + LiveClient` would be unnecessary double naming compared with Python's `TqApi` session model?

## Current Recommendation

Do not patch the two retained issues independently first.

They should be solved as consequences of a stronger public contract:

- `Client` as the live session owner
- one live context owner
- explicit teardown semantics
- explicit new-client re-auth semantics

If review agrees, the next artifact should be a design/spec document for making `Client` itself the single session owner, followed by implementation.
