# TradeSession Reliable Events Design

## Summary

Upgrade `TradeSession` from best-effort bounded `order` / `trade` notifications to a first-class, process-local reliable event API.

The new model keeps two separate layers:

- State layer: `TradeSession` and `DataManager` remain the source of latest account / position / order / trade snapshots.
- Event layer: `TradeSession` adds a reliable append-only event journal for `order` and `trade` events only.

This is a breaking change with no compatibility shim for the old event API.

## Approved Scope

- Reliability target: process-local only.
- Multi-consumer: yes. Each subscriber has an independent cursor and can consume the full event stream from its own subscribe point.
- Event types in scope: `order` and `trade` only.
- Subscription start point: new subscribers only see events emitted after they subscribe.
- Compatibility: none. Old `on_order` / `on_trade` / `order_channel` / `trade_channel` are removed.

## Goals

- Make `order` and `trade` events reliable under normal in-process operation, even with slow consumers.
- Give `TqRuntime` and user strategies a public event API they can safely depend on.
- Keep trading state reads and event delivery separate, instead of forcing both through a single `watch`-style mechanism.
- Preserve Rust-friendly explicit backpressure and failure semantics.

## Non-Goals

- No reconnect replay or recovery of events missed while disconnected.
- No persistence across process restart.
- No reliable event stream for `account`, `position`, or `notification` in this iteration.
- No retention of the current best-effort trade event surface.

## Why This Exists

`tqsdk-python` public trading semantics are still centered on `wait_update()` plus live refs for account / order / trade state. However, its internal trading helpers (`TargetPosTask`, `InsertOrderTask`, `Twap`) use non-`last_only` `TqChan` instances to deliver fills and explicitly rely on receiving all fills for a task.

That means the right Rust boundary is not "convert everything to watch/state". The correct split is:

- snapshots for latest trading state
- reliable events for order state transitions and new trades

The current Rust implementation does not satisfy that boundary. `TradeSession` uses bounded async channels and drops order/trade updates on overflow, while `runtime::LiveExecutionAdapter::wait_order_update()` depends on those channels for wakeups.

## Alternatives Considered

### Option A: Unified journal plus per-subscriber cursor

- One session-level append-only journal stores both `OrderUpdated` and `TradeCreated`.
- Each subscriber tracks its own cursor.
- Consumers pull by cursor and wait on a notify primitive when no new events are available.

Pros:

- Natural support for multiple independent consumers.
- Clear and explicit reliability contract.
- Single ordered sequence across order and trade events.
- Reusable for both public APIs and runtime internals.

Cons:

- Requires explicit retention and lag handling.
- Slightly more infrastructure than channel-based fan-out.

### Option B: Separate reliable order log and trade log

- `order` and `trade` each get an independent journal and subscription API.

Pros:

- Slightly cheaper filtering.
- Event types stay physically separated.

Cons:

- Harder for consumers that need a single ordered stream.
- More public API surface for little benefit.

### Option C: Only add reliable per-order waiters

- Add `wait_order_update_reliable(order_id)` and keep general public channels best-effort.

Pros:

- Smallest implementation delta.

Cons:

- Does not satisfy the requirement for a public reliable event API.
- Still forces advanced users to build their own event layer.

## Recommendation

Use Option A.

It gives one canonical event model for both user code and runtime internals, while keeping the public API small and coherent.

## Public API

### New Types

```rust
pub struct TradeSessionEvent {
    pub seq: u64,
    pub kind: TradeSessionEventKind,
}

#[non_exhaustive]
pub enum TradeSessionEventKind {
    OrderUpdated { order_id: String, order: Order },
    TradeCreated { trade_id: String, trade: Trade },
}

pub enum TradeEventRecvError {
    Closed,
    Lagged { missed_before_seq: u64 },
}
```

### New Subscription APIs

```rust
impl TradeSession {
    pub fn subscribe_events(&self) -> TradeEventStream;
    pub fn subscribe_order_events(&self) -> OrderEventStream;
    pub fn subscribe_trade_events(&self) -> TradeOnlyEventStream;
    pub async fn wait_order_update_reliable(&self, order_id: &str) -> crate::Result<()>;
}
```

### Stream Behavior

Each stream exposes an explicit receive API instead of introducing a new `Stream` dependency:

```rust
impl TradeEventStream {
    pub async fn recv(&mut self) -> Result<TradeSessionEvent, TradeEventRecvError>;
}
```

Filtered stream variants return the same `TradeEventRecvError`.

### Removed APIs

Delete these public surfaces:

- `TradeSession::on_order`
- `TradeSession::on_trade`
- `TradeSession::order_channel`
- `TradeSession::trade_channel`

`on_account`, `on_position`, `account_channel`, `position_channel`, `notification_channel`, and current snapshot getters remain unchanged in this iteration.

## Event Semantics

### Order Events

Emit `OrderUpdated` when the latest observable `Order` snapshot for a given `order_id` changes after a merge completes.

The event payload is the full `Order` snapshot, not a partial diff.

### Trade Events

Emit `TradeCreated` when a new `trade_id` is first observed.

`Trade` is treated as append-only in this design. There is no `TradeUpdated` event in this iteration.

### Ordering

All order and trade events share a single monotonic `seq` within a `TradeSession`.

This gives consumers one stable in-session ordering even when order and trade events interleave.

### Not Promised

The API does not promise:

- wire-level diff fidelity
- reconnect gap replay
- persistence across restart

The contract is at the business-event level:

- order state changes are observable and reliable in-process
- new trades are observable and reliable in-process

## Internal Design

Add a new `src/trade_session/events.rs` module containing a `TradeEventHub`.

### Core Structures

```rust
struct TradeEventHub {
    next_seq: AtomicU64,
    next_subscriber_id: AtomicU64,
    events: Mutex<VecDeque<Arc<TradeSessionEvent>>>,
    subscribers: Mutex<HashMap<u64, SubscriberState>>,
    closed: AtomicBool,
    notify: Notify,
    max_retained_events: usize,
}

struct SubscriberState {
    last_seen_seq: u64,
    invalidated: bool,
    invalidated_before_seq: u64,
}
```

Each subscriber owns:

- a `subscriber_id`
- a reference to the shared hub
- its own cursor state through `subscribers`

Subscribers do not own message buffers.

### Subscription Start

When a subscriber is created, initialize `last_seen_seq` to the current tail sequence.

That means it only observes events appended after subscription, matching the approved requirement.

## Production Path

Reliable events are produced by `TradeSession` after merge completion and after the latest snapshot state is visible.

### Order Deduplication

Maintain `last_emitted_orders: HashMap<String, Order>` in the event production path.

Emit `OrderUpdated` only when the new full `Order` value differs from the previously emitted snapshot for that `order_id`.

This design therefore requires `Order` to derive `PartialEq`.

### Trade Deduplication

Maintain `seen_trade_ids: HashSet<String>`.

Emit `TradeCreated` only the first time each `trade_id` appears.

### Why Full Snapshot Comparison

Use full `Order` snapshot comparison instead of a hand-picked fingerprint:

- simpler and harder to get wrong
- no hidden field list that can drift from the public type
- matches the contract that payloads are full observable snapshots

If `Order` equality later needs to ignore metadata fields, that should be an explicit type-level decision, not an implicit event-side shortcut.

## Consumer Path

`recv()` works as a cursor-based read:

1. Try to read the next event after `last_seen_seq`.
2. If found, advance cursor and return it.
3. If not found, wait on `Notify`.
4. After wakeup, retry from the cursor.

Consumers do not compete for messages.

## Retention And Lag Contract

The journal is bounded, but loss is explicit rather than silent.

### Normal Retention

Trim any event older than the minimum cursor of all active subscribers.

### Hard Retention Ceiling

Also enforce `max_retained_events`.

If keeping all active subscribers would exceed that cap:

- trim oldest events until the cap is satisfied
- mark any subscriber whose cursor now points before the retained window as invalidated
- that subscriber's next `recv()` returns:

```rust
TradeEventRecvError::Lagged { missed_before_seq }
```

After `Lagged`, the stream is considered unusable and the caller must resubscribe.

### Default Configuration

Do not reuse `ClientConfig.message_queue_capacity`.

Reliable event retention is an application-level contract, not a transport queue size.

Add a dedicated option:

```rust
pub struct TradeSessionOptions {
    pub td_url_override: Option<String>,
    pub reliable_events_max_retained: usize,
}
```

Recommended default: `8192`.

Rationale:

- large enough for realistic bursts and a few slow consumers
- small enough to keep process memory bounded
- separate from websocket backlog tuning

## Close Semantics

When `TradeSession` closes:

- mark the event hub as closed
- wake all waiters
- any future `recv()` returns `TradeEventRecvError::Closed`

Closing does not flush synthetic terminal events.

## Runtime Integration

`runtime::LiveExecutionAdapter::wait_order_update()` must stop depending on dropped bounded channels.

New behavior:

- call `session.wait_order_update_reliable(order_id)`
- or internally subscribe to the reliable event hub and filter for:
  - `OrderUpdated { order_id == target }`
  - `TradeCreated { trade.order_id == target }`

`ChildOrderRunner` and `TargetPosTask` still read final truth from snapshot APIs:

- `order()`
- `trades_by_order()`
- `position()`

Reliable events are only for wakeup and observation, not for replacing snapshot queries as the source of truth.

## File-Level Changes

### New

- `src/trade_session/events.rs`

### Modified

- `AGENTS.md`
- `src/trade_session/mod.rs`
- `src/trade_session/core.rs`
- `src/trade_session/watch.rs`
- `src/runtime/execution.rs`
- `src/client/endpoints.rs`
- `src/client/facade.rs`
- `src/prelude.rs`
- `src/lib.rs`
- `README.md`
- `examples/trade.rs`
- other touched `examples/*.rs` that demonstrate trade session behavior or canonical API usage
- `src/trade_session/tests.rs`
- `src/runtime/tasks/tests.rs`

### Deleted Public Surface

- `TradeSession::on_order`
- `TradeSession::on_trade`
- `TradeSession::order_channel`
- `TradeSession::trade_channel`

## Testing

Minimum required coverage:

1. Single subscriber receives ordered `OrderUpdated` then `TradeCreated`.
2. Two subscribers consume independently from the same subscribe point.
3. Late subscriber only sees post-subscribe events.
4. Repeated merge of the same `Order` does not emit duplicate `OrderUpdated`.
5. Replayed or repeated `trade_id` does not emit duplicate `TradeCreated`.
6. `wait_order_update_reliable(order_id)` wakes on either matching order update or matching trade creation.
7. Slow subscriber exceeding `reliable_events_max_retained` receives `Lagged`.
8. Session close causes `Closed`.
9. Runtime child-order workflow remains correct under dense event bursts.

## Documentation Changes

README and examples must treat reliable events as the canonical trading event API.

Document explicitly:

- snapshots are for latest state
- reliable event streams are for order/trade event consumption
- subscribers only see events after subscribe
- `Lagged` is explicit and recoverable only by resubscription
- `AGENTS.md` must be updated so future contributors follow the new canonical trade event API, no longer recommend removed `on_order` / `on_trade` / channel surfaces, and remember to keep examples in sync with public API changes
- all affected examples must be updated to demonstrate the new canonical trade event subscription and waiting model, not the removed best-effort event interfaces

## Rollout

This ships as a breaking change in the same release that removes legacy marketdata surfaces.

No deprecated bridge is kept for old trade event APIs.

Implementation is not complete until `AGENTS.md`, README, and every affected example are consistent with the shipped trade event API.

## Open Questions Resolved

- Multi-consumer support: yes.
- Retention model: bounded journal with explicit lag failure.
- Compatibility: none.
- Event scope: `order` and `trade` only.
- Subscription start point: tail-only.
