use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use tokio::sync::watch;

use crate::runtime::{AccountHandle, OrderDirection, RuntimeError, RuntimeResult, TargetPosConfig};
use crate::types::{ORDER_STATUS_ALIVE, Position, Quote};

use super::{
    ChildOrderControl, ChildOrderRunner, ChildOrderStatus, OffsetAction, PlannedBatch, PlannedOffset, PlannedOrder,
};

#[derive(Clone)]
pub struct TargetPosBuilder {
    account: AccountHandle,
    symbol: String,
    config: TargetPosConfig,
}

#[derive(Clone)]
pub struct TargetPosHandle {
    inner: Arc<TargetPosTaskInner>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TargetRequest {
    seq: u64,
    volume: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum TaskCommand {
    Idle,
    Target(TargetRequest),
    Cancel,
}

struct TargetPosTaskInner {
    account: AccountHandle,
    symbol: String,
    config: TargetPosConfig,
    task_id: crate::runtime::TaskId,
    offset_priority: Vec<Vec<OffsetAction>>,
    next_request_seq: AtomicU64,
    closed: AtomicBool,
    started: Mutex<Option<tokio::task::JoinHandle<()>>>,
    command_tx: watch::Sender<TaskCommand>,
    reached_seq_tx: watch::Sender<u64>,
    finished_tx: watch::Sender<bool>,
    failure: Mutex<Option<String>>,
}

impl TargetPosBuilder {
    pub(crate) fn new(account: AccountHandle, symbol: impl Into<String>) -> Self {
        Self {
            account,
            symbol: symbol.into(),
            config: TargetPosConfig::default(),
        }
    }

    pub fn config(mut self, config: TargetPosConfig) -> Self {
        self.config = config;
        self
    }

    pub fn build(self) -> RuntimeResult<TargetPosHandle> {
        let offset_priority = parse_offset_priority(self.config.offset_priority.as_str())?;
        let registry = self.account.runtime().registry();
        let registered = registry.register_target_task(
            self.account.runtime_id(),
            self.account.account_key(),
            &self.symbol,
            &self.config,
        )?;

        let (command_tx, _) = watch::channel(TaskCommand::Idle);
        let (reached_seq_tx, _) = watch::channel(0_u64);
        let (finished_tx, _) = watch::channel(false);

        Ok(TargetPosHandle {
            inner: Arc::new(TargetPosTaskInner {
                account: self.account,
                symbol: self.symbol,
                config: self.config,
                task_id: registered.task_id,
                offset_priority,
                next_request_seq: AtomicU64::new(0),
                closed: AtomicBool::new(false),
                started: Mutex::new(None),
                command_tx,
                reached_seq_tx,
                finished_tx,
                failure: Mutex::new(None),
            }),
        })
    }
}

impl TargetPosHandle {
    pub fn account(&self) -> &AccountHandle {
        &self.inner.account
    }

    pub fn symbol(&self) -> &str {
        &self.inner.symbol
    }

    pub fn config(&self) -> &TargetPosConfig {
        &self.inner.config
    }

    pub fn task_id(&self) -> crate::runtime::TaskId {
        self.inner.task_id
    }

    pub fn offset_priority(&self) -> &[Vec<OffsetAction>] {
        &self.inner.offset_priority
    }

    pub fn compute_plan(
        &self,
        quote: &Quote,
        position: &Position,
        target_volume: i64,
    ) -> RuntimeResult<Vec<PlannedBatch>> {
        compute_plan(quote, position, target_volume, &self.inner.offset_priority)
    }

    pub fn set_target_volume(&self, volume: i64) -> RuntimeResult<()> {
        if self.inner.closed.load(Ordering::SeqCst) {
            return Err(RuntimeError::TargetTaskFinished {
                symbol: self.inner.symbol.clone(),
            });
        }
        self.inner.ensure_started("set_target_volume")?;
        let seq = self.inner.next_request_seq.fetch_add(1, Ordering::SeqCst) + 1;
        self.inner
            .command_tx
            .send_replace(TaskCommand::Target(TargetRequest { seq, volume }));
        Ok(())
    }

    pub async fn cancel(&self) -> RuntimeResult<()> {
        self.inner.closed.store(true, Ordering::SeqCst);
        self.inner.ensure_started("cancel")?;
        self.inner.command_tx.send_replace(TaskCommand::Cancel);
        Ok(())
    }

    pub fn is_finished(&self) -> bool {
        *self.inner.finished_tx.borrow()
    }

    pub async fn wait_finished(&self) -> RuntimeResult<()> {
        self.inner.ensure_started("wait_finished")?;
        let mut rx = self.inner.finished_tx.subscribe();
        while !*rx.borrow() {
            rx.changed().await.map_err(|_| RuntimeError::AdapterChannelClosed {
                resource: "target task finished",
            })?;
        }
        self.inner.failure_result()
    }

    pub async fn wait_target_reached(&self) -> RuntimeResult<()> {
        self.inner.ensure_started("wait_target_reached")?;
        let target_seq = match self.inner.command_tx.borrow().clone() {
            TaskCommand::Target(request) => request.seq,
            TaskCommand::Cancel => {
                return Err(RuntimeError::TargetTaskFinished {
                    symbol: self.inner.symbol.clone(),
                });
            }
            TaskCommand::Idle => return Ok(()),
        };

        let mut rx = self.inner.reached_seq_tx.subscribe();
        loop {
            if *rx.borrow() >= target_seq {
                return self.inner.failure_result();
            }
            if *self.inner.finished_tx.borrow() {
                return self.inner.failure_result();
            }
            rx.changed().await.map_err(|_| RuntimeError::AdapterChannelClosed {
                resource: "target task reached",
            })?;
        }
    }
}

impl TargetPosTaskInner {
    fn ensure_started(self: &Arc<Self>, operation: &'static str) -> RuntimeResult<()> {
        let mut started = self.started.lock().expect("target task start lock poisoned");
        if started.is_some() {
            return Ok(());
        }

        let handle =
            tokio::runtime::Handle::try_current().map_err(|_| RuntimeError::TokioRuntimeRequired { operation })?;
        let task = Arc::clone(self);
        *started = Some(handle.spawn(async move {
            task.run().await;
        }));
        Ok(())
    }

    async fn run(self: Arc<Self>) {
        let result = self.run_loop().await;
        if let Err(err) = result {
            *self.failure.lock().expect("target task failure lock poisoned") = Some(err.to_string());
        }
        self.account.runtime().registry().unregister_task(self.task_id);
        self.finished_tx.send_replace(true);
    }

    async fn run_loop(self: &Arc<Self>) -> RuntimeResult<()> {
        let mut command_rx = self.command_tx.subscribe();

        loop {
            let command = command_rx.borrow().clone();
            match command {
                TaskCommand::Idle => {
                    command_rx
                        .changed()
                        .await
                        .map_err(|_| RuntimeError::AdapterChannelClosed {
                            resource: "target task command",
                        })?;
                }
                TaskCommand::Cancel => {
                    self.cancel_owned_orders().await?;
                    return Ok(());
                }
                TaskCommand::Target(request) => {
                    self.drive_target(request, &mut command_rx).await?;
                }
            }
        }
    }

    async fn drive_target(
        &self,
        request: TargetRequest,
        command_rx: &mut watch::Receiver<TaskCommand>,
    ) -> RuntimeResult<()> {
        loop {
            let command = command_rx.borrow().clone();
            match command {
                TaskCommand::Cancel => {
                    self.cancel_owned_orders().await?;
                    return Ok(());
                }
                TaskCommand::Target(current) if current.seq != request.seq => return Ok(()),
                TaskCommand::Target(_) => {}
                TaskCommand::Idle => return Ok(()),
            }

            let runtime = self.account.runtime();
            let quote = runtime.market().latest_quote(&self.symbol).await?;
            let position = runtime
                .execution()
                .position(self.account.account_key(), &self.symbol)
                .await?;
            let plan = compute_plan(&quote, &position, request.volume, &self.offset_priority)?;

            if plan.is_empty() {
                self.reached_seq_tx.send_replace(request.seq);
                command_rx
                    .changed()
                    .await
                    .map_err(|_| RuntimeError::AdapterChannelClosed {
                        resource: "target task command",
                    })?;
                return Ok(());
            }

            for batch in plan {
                for order in batch.orders {
                    if self.command_changed(command_rx, request.seq) {
                        return Ok(());
                    }

                    let (control_tx, control_rx) = watch::channel(ChildOrderControl::Run);
                    let mut control_commands = command_rx.clone();
                    let forward = tokio::spawn(async move {
                        loop {
                            if child_order_should_stop(&control_commands, request.seq) {
                                let _ = control_tx.send(ChildOrderControl::Stop);
                                break;
                            }
                            if control_commands.changed().await.is_err() {
                                let _ = control_tx.send(ChildOrderControl::Stop);
                                break;
                            }
                        }
                    });

                    let runner = ChildOrderRunner::new(
                        runtime.market(),
                        runtime.execution(),
                        self.account.account_key(),
                        &self.symbol,
                        order.direction,
                        order.offset,
                        order.volume,
                        self.config.split_policy,
                        self.config.price_mode.clone(),
                    )
                    .with_owner(runtime.registry(), self.task_id)
                    .with_control(control_rx);

                    let outcome = runner.run().await?;
                    forward.abort();

                    if matches!(outcome, ChildOrderStatus::Interrupted) {
                        return Ok(());
                    }
                }
            }
        }
    }

    fn command_changed(&self, command_rx: &watch::Receiver<TaskCommand>, request_seq: u64) -> bool {
        match command_rx.borrow().clone() {
            TaskCommand::Cancel => true,
            TaskCommand::Target(current) => current.seq != request_seq,
            TaskCommand::Idle => false,
        }
    }

    async fn cancel_owned_orders(&self) -> RuntimeResult<()> {
        let runtime = self.account.runtime();
        let registry = runtime.registry();
        let execution = runtime.execution();

        loop {
            let mut alive_orders = Vec::new();
            for order_id in registry.task_orders(self.task_id) {
                match execution.order(self.account.account_key(), &order_id).await {
                    Ok(order) if order.status == ORDER_STATUS_ALIVE => alive_orders.push(order_id),
                    Ok(_) | Err(RuntimeError::OrderNotFound { .. }) => {}
                    Err(err) => return Err(err),
                }
            }

            if alive_orders.is_empty() {
                return Ok(());
            }

            for order_id in &alive_orders {
                execution.cancel_order(self.account.account_key(), order_id).await?;
            }
            for order_id in &alive_orders {
                execution
                    .wait_order_update(self.account.account_key(), order_id)
                    .await?;
            }
        }
    }

    fn failure_result(&self) -> RuntimeResult<()> {
        if let Some(reason) = self.failure.lock().expect("target task failure lock poisoned").clone() {
            return Err(RuntimeError::TargetTaskFailed {
                symbol: self.symbol.clone(),
                reason,
            });
        }
        Ok(())
    }
}

impl Drop for TargetPosTaskInner {
    fn drop(&mut self) {
        self.account.runtime().registry().unregister_task(self.task_id);
    }
}

fn child_order_should_stop(command_rx: &watch::Receiver<TaskCommand>, request_seq: u64) -> bool {
    match command_rx.borrow().clone() {
        TaskCommand::Cancel => true,
        TaskCommand::Target(request) => request.seq != request_seq,
        TaskCommand::Idle => false,
    }
}

pub fn parse_offset_priority(raw: &str) -> RuntimeResult<Vec<Vec<OffsetAction>>> {
    let mut groups = Vec::new();
    let mut current = Vec::new();
    let mut seen = HashSet::new();

    for ch in raw.chars() {
        if ch == ',' {
            if current.is_empty() {
                return Err(RuntimeError::InvalidOffsetPriority { raw: raw.to_string() });
            }
            groups.push(std::mem::take(&mut current));
            continue;
        }

        let action = match ch {
            '今' => OffsetAction::Today,
            '昨' => OffsetAction::Yesterday,
            '开' => OffsetAction::Open,
            _ => {
                return Err(RuntimeError::InvalidOffsetPriority { raw: raw.to_string() });
            }
        };

        if !seen.insert(action) {
            return Err(RuntimeError::InvalidOffsetPriority { raw: raw.to_string() });
        }
        current.push(action);
    }

    if current.is_empty() {
        return Err(RuntimeError::InvalidOffsetPriority { raw: raw.to_string() });
    }
    groups.push(current);

    Ok(groups)
}

pub fn validate_quote_constraints(quote: &Quote) -> RuntimeResult<()> {
    if quote.open_min_market_order_volume > 1 || quote.open_min_limit_order_volume > 1 {
        return Err(RuntimeError::UnsupportedOpenOrderVolume {
            symbol: quote.instrument_id.clone(),
            open_min_market_order_volume: quote.open_min_market_order_volume,
            open_min_limit_order_volume: quote.open_min_limit_order_volume,
        });
    }

    Ok(())
}

pub fn compute_plan(
    quote: &Quote,
    position: &Position,
    target_volume: i64,
    offset_priority: &[Vec<OffsetAction>],
) -> RuntimeResult<Vec<PlannedBatch>> {
    validate_quote_constraints(quote)?;

    let mut remaining = target_volume - net_position(position);
    if remaining == 0 {
        return Ok(Vec::new());
    }

    let mut batches = Vec::new();
    for group in offset_priority {
        if remaining == 0 {
            break;
        }

        let mut pending_frozen = 0_i64;
        let mut orders = Vec::new();

        for action in group {
            if remaining == 0 {
                break;
            }

            let Some((order, frozen_delta)) =
                planned_order_for_action(*action, remaining, pending_frozen, quote, position)
            else {
                continue;
            };

            pending_frozen += frozen_delta;
            remaining -= match order.direction {
                OrderDirection::Buy => order.volume,
                OrderDirection::Sell => -order.volume,
            };
            orders.push(order);
        }

        if !orders.is_empty() {
            batches.push(PlannedBatch { orders });
        }
    }

    Ok(batches)
}

fn planned_order_for_action(
    action: OffsetAction,
    delta_volume: i64,
    pending_frozen: i64,
    quote: &Quote,
    position: &Position,
) -> Option<(PlannedOrder, i64)> {
    let direction = if delta_volume > 0 {
        OrderDirection::Buy
    } else {
        OrderDirection::Sell
    };
    let requested = delta_volume.abs();
    let exchange = exchange_id(quote);
    let is_shfe_like = matches!(exchange.as_str(), "SHFE" | "INE");

    let (offset, available) = match action {
        OffsetAction::Open => (PlannedOffset::Open, requested),
        OffsetAction::Today => {
            if is_shfe_like {
                let available = match direction {
                    OrderDirection::Buy => short_today(position) - short_frozen_today(position),
                    OrderDirection::Sell => long_today(position) - long_frozen_today(position),
                };
                (PlannedOffset::CloseToday, available)
            } else {
                let frozen = pending_frozen + total_close_frozen(position, direction);
                let today_available = match direction {
                    OrderDirection::Buy => short_today(position),
                    OrderDirection::Sell => long_today(position),
                };
                (PlannedOffset::Close, today_available - frozen)
            }
        }
        OffsetAction::Yesterday => {
            if is_shfe_like {
                let available = match direction {
                    OrderDirection::Buy => short_his(position) - short_frozen_his(position),
                    OrderDirection::Sell => long_his(position) - long_frozen_his(position),
                };
                (PlannedOffset::Close, available)
            } else {
                let frozen = pending_frozen + total_close_frozen(position, direction);
                let today_left = match direction {
                    OrderDirection::Buy => short_today(position) - frozen,
                    OrderDirection::Sell => long_today(position) - frozen,
                };
                if today_left > 0 {
                    (PlannedOffset::Close, 0)
                } else {
                    let total_available = match direction {
                        OrderDirection::Buy => short_total(position),
                        OrderDirection::Sell => long_total(position),
                    };
                    (PlannedOffset::Close, total_available - frozen)
                }
            }
        }
    };

    let volume = requested.min(available.max(0));
    if volume == 0 {
        return None;
    }

    let frozen_delta = if matches!(offset, PlannedOffset::Open) {
        0
    } else {
        volume
    };

    Some((
        PlannedOrder {
            direction,
            offset,
            volume,
        },
        frozen_delta,
    ))
}

fn exchange_id(quote: &Quote) -> String {
    if !quote.exchange_id.is_empty() {
        return quote.exchange_id.clone();
    }

    quote
        .instrument_id
        .split_once('.')
        .map(|(exchange, _)| exchange.to_string())
        .unwrap_or_default()
}

fn net_position(position: &Position) -> i64 {
    long_total(position) - short_total(position)
}

fn long_today(position: &Position) -> i64 {
    if position.pos_long_today != 0 || position.pos_long_his != 0 {
        position.pos_long_today
    } else {
        position.volume_long_today
    }
}

fn long_his(position: &Position) -> i64 {
    if position.pos_long_today != 0 || position.pos_long_his != 0 {
        position.pos_long_his
    } else {
        position.volume_long_his
    }
}

fn long_total(position: &Position) -> i64 {
    if position.pos_long_today != 0 || position.pos_long_his != 0 {
        position.pos_long_today + position.pos_long_his
    } else if position.volume_long != 0 {
        position.volume_long
    } else {
        position.volume_long_today + position.volume_long_his
    }
}

fn short_today(position: &Position) -> i64 {
    if position.pos_short_today != 0 || position.pos_short_his != 0 {
        position.pos_short_today
    } else {
        position.volume_short_today
    }
}

fn short_his(position: &Position) -> i64 {
    if position.pos_short_today != 0 || position.pos_short_his != 0 {
        position.pos_short_his
    } else {
        position.volume_short_his
    }
}

fn short_total(position: &Position) -> i64 {
    if position.pos_short_today != 0 || position.pos_short_his != 0 {
        position.pos_short_today + position.pos_short_his
    } else if position.volume_short != 0 {
        position.volume_short
    } else {
        position.volume_short_today + position.volume_short_his
    }
}

fn long_frozen_today(position: &Position) -> i64 {
    position.volume_long_frozen_today
}

fn long_frozen_his(position: &Position) -> i64 {
    position.volume_long_frozen_his
}

fn short_frozen_today(position: &Position) -> i64 {
    position.volume_short_frozen_today
}

fn short_frozen_his(position: &Position) -> i64 {
    position.volume_short_frozen_his
}

fn total_close_frozen(position: &Position, direction: OrderDirection) -> i64 {
    match direction {
        OrderDirection::Buy => {
            if position.volume_short_frozen != 0 {
                position.volume_short_frozen
            } else {
                position.volume_short_frozen_today + position.volume_short_frozen_his
            }
        }
        OrderDirection::Sell => {
            if position.volume_long_frozen != 0 {
                position.volume_long_frozen
            } else {
                position.volume_long_frozen_today + position.volume_long_frozen_his
            }
        }
    }
}
