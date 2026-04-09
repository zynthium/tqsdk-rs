use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{FixedOffset, NaiveDateTime, TimeZone};
use serde_json::Value;
use tokio::sync::watch;

use crate::runtime::{AccountHandle, RuntimeError, RuntimeResult, TargetPosConfig};
use crate::types::Trade;

use super::TargetPosHandle;

#[derive(Debug, Clone)]
pub struct TargetPosScheduleStep {
    pub interval: Duration,
    pub target_volume: i64,
    pub price_mode: Option<crate::runtime::PriceMode>,
}

#[derive(Debug, Clone, Default)]
pub struct TargetPosSchedulerConfig {
    pub offset_priority: crate::runtime::OffsetPriority,
    pub split_policy: Option<crate::runtime::VolumeSplitPolicy>,
}

#[derive(Debug, Clone, Default)]
pub struct TargetPosExecutionReport {
    pub trades: Vec<Trade>,
}

#[derive(Clone)]
pub struct TargetPosSchedulerBuilder {
    account: AccountHandle,
    symbol: String,
    steps: Vec<TargetPosScheduleStep>,
    config: TargetPosSchedulerConfig,
}

#[derive(Clone)]
pub struct TargetPosSchedulerHandle {
    inner: Arc<TargetPosSchedulerInner>,
}

struct TargetPosSchedulerInner {
    account: AccountHandle,
    symbol: String,
    steps: Vec<TargetPosScheduleStep>,
    config: TargetPosSchedulerConfig,
    task_id: crate::runtime::TaskId,
    cancel_tx: watch::Sender<bool>,
    finished_tx: watch::Sender<bool>,
    report: Mutex<TargetPosExecutionReport>,
    failure: Mutex<Option<String>>,
}

enum StepOutcome {
    DeadlineElapsed,
    Cancelled,
    LastStepReached(RuntimeResult<()>),
}

impl TargetPosSchedulerBuilder {
    pub(crate) fn new(account: AccountHandle, symbol: impl Into<String>) -> Self {
        Self {
            account,
            symbol: symbol.into(),
            steps: Vec::new(),
            config: TargetPosSchedulerConfig::default(),
        }
    }

    pub fn steps(mut self, steps: Vec<TargetPosScheduleStep>) -> Self {
        self.steps = steps;
        self
    }

    pub fn config(mut self, config: TargetPosSchedulerConfig) -> Self {
        self.config = config;
        self
    }

    pub fn build(self) -> RuntimeResult<TargetPosSchedulerHandle> {
        let handle = tokio::runtime::Handle::try_current().map_err(|_| RuntimeError::TokioRuntimeRequired {
            operation: "build_target_pos_scheduler",
        })?;
        let task_id = self.account.runtime().registry().register_manual_order_guard(
            self.account.runtime_id(),
            self.account.account_key(),
            &self.symbol,
        )?;
        let (cancel_tx, _) = watch::channel(false);
        let (finished_tx, _) = watch::channel(false);
        let inner = Arc::new(TargetPosSchedulerInner {
            account: self.account,
            symbol: self.symbol,
            steps: self.steps,
            config: self.config,
            task_id,
            cancel_tx,
            finished_tx,
            report: Mutex::new(TargetPosExecutionReport::default()),
            failure: Mutex::new(None),
        });
        let task = Arc::clone(&inner);
        handle.spawn(async move {
            task.run().await;
        });
        Ok(TargetPosSchedulerHandle { inner })
    }
}

impl TargetPosSchedulerHandle {
    pub fn account(&self) -> &AccountHandle {
        &self.inner.account
    }

    pub fn symbol(&self) -> &str {
        &self.inner.symbol
    }

    pub fn config(&self) -> &TargetPosSchedulerConfig {
        &self.inner.config
    }

    pub fn steps(&self) -> &[TargetPosScheduleStep] {
        &self.inner.steps
    }

    pub fn is_finished(&self) -> bool {
        *self.inner.finished_tx.borrow()
    }

    pub async fn cancel(&self) -> RuntimeResult<()> {
        self.inner.cancel_tx.send_replace(true);
        Ok(())
    }

    pub async fn wait_finished(&self) -> RuntimeResult<()> {
        let mut rx = self.inner.finished_tx.subscribe();
        while !*rx.borrow() {
            rx.changed().await.map_err(|_| RuntimeError::AdapterChannelClosed {
                resource: "target pos scheduler finished",
            })?;
        }
        self.inner.failure_result()
    }

    pub fn execution_report(&self) -> TargetPosExecutionReport {
        self.inner
            .report
            .lock()
            .expect("scheduler report lock poisoned")
            .clone()
    }

    pub fn trades(&self) -> Vec<Trade> {
        self.execution_report().trades
    }
}

impl TargetPosSchedulerInner {
    async fn run(self: Arc<Self>) {
        if let Err(err) = self.run_loop().await {
            *self.failure.lock().expect("scheduler failure lock poisoned") = Some(err.to_string());
        }
        self.account.runtime().registry().unregister_task(self.task_id);
        self.finished_tx.send_replace(true);
    }

    async fn run_loop(&self) -> RuntimeResult<()> {
        let mut cancel_rx = self.cancel_tx.subscribe();
        for (index, step) in self.steps.iter().cloned().enumerate() {
            if *cancel_rx.borrow() {
                return Ok(());
            }

            let is_last = index + 1 == self.steps.len();
            let Some(price_mode) = step.price_mode else {
                if is_last {
                    return Ok(());
                }
                match self.wait_step_deadline(step.interval, &mut cancel_rx).await? {
                    StepOutcome::DeadlineElapsed => continue,
                    StepOutcome::Cancelled => return Ok(()),
                    StepOutcome::LastStepReached(_) => unreachable!("pause step cannot reach target"),
                }
            };

            let task = self.build_step_task(step.target_volume, price_mode)?;
            let outcome = if is_last {
                self.wait_last_step(&task, &mut cancel_rx).await?
            } else {
                self.wait_step_deadline(step.interval, &mut cancel_rx).await?
            };

            let should_return = matches!(outcome, StepOutcome::Cancelled | StepOutcome::LastStepReached(_));
            let finish_result = self.finish_step_task(&task).await;
            if let StepOutcome::LastStepReached(result) = outcome {
                result?;
            }
            finish_result?;

            if should_return {
                return Ok(());
            }
        }

        Ok(())
    }

    fn build_step_task(
        &self,
        target_volume: i64,
        price_mode: crate::runtime::PriceMode,
    ) -> RuntimeResult<TargetPosHandle> {
        let task = self
            .account
            .target_pos(&self.symbol)
            .config(TargetPosConfig {
                price_mode,
                offset_priority: self.config.offset_priority,
                split_policy: self.config.split_policy,
            })
            .build_internal()?;
        task.set_target_volume(target_volume)?;
        Ok(task)
    }

    async fn wait_step_deadline(
        &self,
        interval: Duration,
        cancel_rx: &mut watch::Receiver<bool>,
    ) -> RuntimeResult<StepOutcome> {
        let market = self.account.runtime().market();
        let initial_quote = market.latest_quote(&self.symbol).await?;
        let trading_time = market.trading_time(&self.symbol).await?;
        let Some(deadline) = compute_deadline(&initial_quote.datetime, interval, trading_time.as_ref())? else {
            tokio::select! {
                _ = tokio::time::sleep(interval) => return Ok(StepOutcome::DeadlineElapsed),
                cancel = wait_cancel(cancel_rx) => {
                    cancel?;
                    return Ok(StepOutcome::Cancelled);
                }
            }
        };

        loop {
            let latest_quote = market.latest_quote(&self.symbol).await?;
            if quote_timestamp_nanos(&latest_quote.datetime)? >= deadline {
                return Ok(StepOutcome::DeadlineElapsed);
            }

            tokio::select! {
                update = market.wait_quote_update(&self.symbol) => {
                    update?;
                }
                cancel = wait_cancel(cancel_rx) => {
                    cancel?;
                    return Ok(StepOutcome::Cancelled);
                }
            }
        }
    }

    async fn wait_last_step(
        &self,
        task: &TargetPosHandle,
        cancel_rx: &mut watch::Receiver<bool>,
    ) -> RuntimeResult<StepOutcome> {
        tokio::select! {
            result = task.wait_target_reached() => Ok(StepOutcome::LastStepReached(result)),
            cancel = wait_cancel(cancel_rx) => {
                cancel?;
                Ok(StepOutcome::Cancelled)
            }
        }
    }

    async fn finish_step_task(&self, task: &TargetPosHandle) -> RuntimeResult<()> {
        if !task.is_finished() {
            task.cancel().await?;
        }
        let wait_result = task.wait_finished().await;
        self.append_trades(task.trades());
        wait_result
    }

    fn append_trades(&self, mut trades: Vec<Trade>) {
        let mut report = self.report.lock().expect("scheduler report lock poisoned");
        report.trades.append(&mut trades);
        report.trades.sort_by(|left, right| {
            left.trade_date_time
                .cmp(&right.trade_date_time)
                .then_with(|| left.order_id.cmp(&right.order_id))
                .then_with(|| left.trade_id.cmp(&right.trade_id))
        });
    }

    fn failure_result(&self) -> RuntimeResult<()> {
        if let Some(reason) = self.failure.lock().expect("scheduler failure lock poisoned").clone() {
            return Err(RuntimeError::TargetTaskFailed {
                symbol: self.symbol.clone(),
                reason,
            });
        }
        Ok(())
    }
}

async fn wait_cancel(cancel_rx: &mut watch::Receiver<bool>) -> RuntimeResult<()> {
    if *cancel_rx.borrow() {
        return Ok(());
    }
    cancel_rx
        .changed()
        .await
        .map_err(|_| RuntimeError::AdapterChannelClosed {
            resource: "target pos scheduler cancel",
        })?;
    Ok(())
}

fn compute_deadline(
    current_datetime: &str,
    interval: Duration,
    trading_time: Option<&Value>,
) -> RuntimeResult<Option<i64>> {
    if current_datetime.trim().is_empty() {
        return Ok(None);
    }

    let current_timestamp = quote_timestamp_nanos(current_datetime)?;
    let interval_nanos = interval
        .as_nanos()
        .try_into()
        .map_err(|_| RuntimeError::Unsupported("scheduler interval exceeds supported nanoseconds"))?;

    match trading_time {
        Some(trading_time) => Ok(Some(compute_trading_deadline(
            current_timestamp,
            interval_nanos,
            trading_time,
        )?)),
        None => Ok(Some(current_timestamp + interval_nanos)),
    }
}

fn compute_trading_deadline(current_timestamp: i64, interval_nanos: i64, trading_time: &Value) -> RuntimeResult<i64> {
    let mut ranges = trading_ranges_for_timestamp(current_timestamp, trading_time)?;
    ranges.sort_by_key(|range| range.0);

    let mut remaining = interval_nanos;
    let mut cursor = current_timestamp;
    for (start, end) in ranges {
        if cursor < start {
            cursor = start;
        }
        if cursor >= end {
            continue;
        }

        let available = end - cursor;
        if available >= remaining {
            return Ok(cursor + remaining);
        }

        remaining -= available;
        cursor = end;
    }

    Err(RuntimeError::Unsupported(
        "scheduler interval exceeds available trading time in the current trading day",
    ))
}

fn trading_ranges_for_timestamp(current_timestamp: i64, trading_time: &Value) -> RuntimeResult<Vec<(i64, i64)>> {
    let current_trading_day = trading_day_from_timestamp(current_timestamp);
    let last_trading_day = trading_day_from_timestamp(trading_day_start_time(current_trading_day) - 1);

    let mut ranges = parse_period_ranges(last_trading_day, trading_time.get("night"))?;
    ranges.extend(parse_period_ranges(current_trading_day, trading_time.get("day"))?);
    Ok(ranges)
}

fn parse_period_ranges(base_day_timestamp: i64, raw_periods: Option<&Value>) -> RuntimeResult<Vec<(i64, i64)>> {
    let Some(periods) = raw_periods.and_then(Value::as_array) else {
        return Ok(Vec::new());
    };

    let mut ranges = Vec::with_capacity(periods.len());
    for period in periods {
        let pair = period
            .as_array()
            .filter(|pair| pair.len() == 2)
            .ok_or(RuntimeError::Unsupported("invalid trading_time period"))?;
        let start = pair[0]
            .as_str()
            .ok_or(RuntimeError::Unsupported("invalid trading_time period start"))?;
        let end = pair[1]
            .as_str()
            .ok_or(RuntimeError::Unsupported("invalid trading_time period end"))?;
        ranges.push((
            base_day_timestamp + parse_hms_nanos(start)?,
            base_day_timestamp + parse_hms_nanos(end)?,
        ));
    }
    Ok(ranges)
}

fn parse_hms_nanos(raw: &str) -> RuntimeResult<i64> {
    let mut parts = raw.split(':');
    let hour = parts
        .next()
        .ok_or(RuntimeError::Unsupported("invalid trading_time hh:mm:ss"))?
        .parse::<i64>()
        .map_err(|_| RuntimeError::Unsupported("invalid trading_time hour"))?;
    let minute = parts
        .next()
        .ok_or(RuntimeError::Unsupported("invalid trading_time hh:mm:ss"))?
        .parse::<i64>()
        .map_err(|_| RuntimeError::Unsupported("invalid trading_time minute"))?;
    let second = parts
        .next()
        .ok_or(RuntimeError::Unsupported("invalid trading_time hh:mm:ss"))?
        .parse::<i64>()
        .map_err(|_| RuntimeError::Unsupported("invalid trading_time second"))?;
    if parts.next().is_some() {
        return Err(RuntimeError::Unsupported("invalid trading_time hh:mm:ss"));
    }
    Ok((hour * 3600 + minute * 60 + second) * 1_000_000_000)
}

fn quote_timestamp_nanos(raw: &str) -> RuntimeResult<i64> {
    let naive = NaiveDateTime::parse_from_str(raw, "%Y-%m-%d %H:%M:%S%.f")
        .map_err(|_| RuntimeError::Unsupported("invalid scheduler quote datetime"))?;
    let cst = FixedOffset::east_opt(8 * 3600).ok_or(RuntimeError::Unsupported("invalid CST offset"))?;
    let datetime = cst
        .from_local_datetime(&naive)
        .single()
        .ok_or(RuntimeError::Unsupported("invalid scheduler quote datetime"))?;
    datetime
        .timestamp_nanos_opt()
        .ok_or(RuntimeError::Unsupported("scheduler quote datetime out of range"))
}

fn trading_day_start_time(trading_day: i64) -> i64 {
    const SIX_HOURS_NS: i64 = 21_600_000_000_000;
    const DAY_NS: i64 = 86_400_000_000_000;
    const BEGIN_MARK: i64 = 631_123_200_000_000_000;

    let mut start_time = trading_day - SIX_HOURS_NS;
    let week_day = (start_time - BEGIN_MARK) / DAY_NS % 7;
    if week_day >= 5 {
        start_time -= DAY_NS * (week_day - 4);
    }
    start_time
}

fn trading_day_from_timestamp(timestamp: i64) -> i64 {
    const DAY_NS: i64 = 86_400_000_000_000;
    const EIGHTEEN_HOURS_NS: i64 = 64_800_000_000_000;
    const BEGIN_MARK: i64 = 631_123_200_000_000_000;

    let mut days = (timestamp - BEGIN_MARK) / DAY_NS;
    if (timestamp - BEGIN_MARK) % DAY_NS >= EIGHTEEN_HOURS_NS {
        days += 1;
    }
    let week_day = days % 7;
    if week_day >= 5 {
        days += 7 - week_day;
    }
    BEGIN_MARK + days * DAY_NS
}
