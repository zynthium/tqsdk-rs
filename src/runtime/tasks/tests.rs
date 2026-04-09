use std::collections::HashMap;
use std::sync::Arc;

use async_channel::{Receiver, Sender, bounded};
use async_trait::async_trait;
use serde_json::json;
use tokio::sync::{Mutex, RwLock};
use tokio::time::{Duration, sleep, timeout};

use crate::datamanager::{DataManager, DataManagerConfig};
use crate::runtime::{
    ChildOrderRunner, ExecutionAdapter, MarketAdapter, OffsetAction, OrderDirection, PlannedOffset, PriceMode,
    RuntimeError, RuntimeResult, compute_plan, parse_offset_priority, validate_quote_constraints,
};
use crate::types::{
    DIRECTION_BUY, InsertOrderRequest, OFFSET_OPEN, ORDER_STATUS_ALIVE, ORDER_STATUS_FINISHED, Order, Position, Quote,
    Trade,
};

#[test]
fn offset_priority_parser_accepts_python_compatible_forms() {
    assert_eq!(
        parse_offset_priority("今昨,开").expect("default form should parse"),
        vec![
            vec![OffsetAction::Today, OffsetAction::Yesterday],
            vec![OffsetAction::Open],
        ]
    );
    assert_eq!(
        parse_offset_priority("今昨开").expect("simultaneous form should parse"),
        vec![vec![OffsetAction::Today, OffsetAction::Yesterday, OffsetAction::Open,]]
    );
    assert_eq!(
        parse_offset_priority("昨开").expect("yesterday-then-open should parse"),
        vec![vec![OffsetAction::Yesterday, OffsetAction::Open]]
    );
    assert_eq!(
        parse_offset_priority("开").expect("open only should parse"),
        vec![vec![OffsetAction::Open]]
    );
}

#[test]
fn planning_rejects_contracts_with_open_min_volume_gt_one() {
    let quote = Quote {
        instrument_id: "SHFE.rb2601".to_string(),
        open_min_market_order_volume: 2,
        ..Quote::default()
    };

    let err = validate_quote_constraints(&quote).expect_err("open min volume > 1 should be rejected");
    assert!(format!("{err}").contains("最小市价开仓手数"));
}

#[test]
fn planning_respects_shfe_close_today_vs_close_yesterday() {
    let priorities = parse_offset_priority("今昨开").expect("priority should parse");
    let position: Position = serde_json::from_value(json!({
        "exchange_id": "SHFE",
        "instrument_id": "rb2601",
        "pos_long_today": 2,
        "pos_long_his": 3,
        "volume_long_today": 2,
        "volume_long_his": 3,
        "volume_long": 5
    }))
    .expect("position should deserialize");

    let shfe_quote = Quote {
        instrument_id: "SHFE.rb2601".to_string(),
        exchange_id: "SHFE".to_string(),
        ..Quote::default()
    };
    let shfe_plan = compute_plan(&shfe_quote, &position, -1, &priorities).expect("shfe plan should succeed");
    assert_eq!(shfe_plan.len(), 1);
    assert_eq!(shfe_plan[0].orders.len(), 3);
    assert_eq!(shfe_plan[0].orders[0].offset, PlannedOffset::CloseToday);
    assert_eq!(shfe_plan[0].orders[0].volume, 2);
    assert_eq!(shfe_plan[0].orders[1].offset, PlannedOffset::Close);
    assert_eq!(shfe_plan[0].orders[1].volume, 3);
    assert_eq!(shfe_plan[0].orders[2].offset, PlannedOffset::Open);
    assert_eq!(shfe_plan[0].orders[2].volume, 1);

    let cffex_quote = Quote {
        instrument_id: "CFFEX.IF2601".to_string(),
        exchange_id: "CFFEX".to_string(),
        ..Quote::default()
    };
    let cffex_plan = compute_plan(&cffex_quote, &position, -1, &priorities).expect("cffex plan should succeed");
    assert_eq!(cffex_plan.len(), 1);
    assert_eq!(cffex_plan[0].orders.len(), 3);
    assert_eq!(cffex_plan[0].orders[0].offset, PlannedOffset::Close);
    assert_eq!(cffex_plan[0].orders[0].volume, 2);
    assert_eq!(cffex_plan[0].orders[1].offset, PlannedOffset::Close);
    assert_eq!(cffex_plan[0].orders[1].volume, 3);
    assert_eq!(cffex_plan[0].orders[2].offset, PlannedOffset::Open);
    assert_eq!(cffex_plan[0].orders[2].volume, 1);
}

#[tokio::test]
async fn child_order_runner_reprices_when_market_turns_worse() {
    let market = Arc::new(FakeMarketAdapter::new(Quote {
        instrument_id: "SHFE.rb2601".to_string(),
        exchange_id: "SHFE".to_string(),
        ask_price1: 101.0,
        bid_price1: 100.0,
        last_price: 100.0,
        ..Quote::default()
    }));
    let execution = Arc::new(FakeExecutionAdapter::default());
    let runner = ChildOrderRunner::new(
        market.clone(),
        execution.clone(),
        "SIM",
        "SHFE.rb2601",
        OrderDirection::Buy,
        PlannedOffset::Open,
        2,
        None,
        PriceMode::Active,
    );

    let task = tokio::spawn(async move { runner.run_until_all_traded().await });

    execution.wait_for_insert_count(1).await;
    let first_order = execution.latest_order_id().await.expect("first order should exist");

    market
        .update_quote(Quote {
            instrument_id: "SHFE.rb2601".to_string(),
            exchange_id: "SHFE".to_string(),
            ask_price1: 102.0,
            bid_price1: 101.0,
            last_price: 101.0,
            ..Quote::default()
        })
        .await;

    execution.wait_for_cancel_count(1).await;
    execution.wait_for_insert_count(2).await;
    let second_order = execution.latest_order_id().await.expect("second order should exist");
    assert_ne!(first_order, second_order);

    execution
        .finish_order(&second_order, 0, vec![make_trade(&second_order, "trade-1", 2, 102.0)])
        .await;

    task.await.expect("join should succeed").expect("runner should succeed");

    assert_eq!(execution.inserted_prices().await, vec![101.0, 102.0]);
    assert_eq!(execution.cancelled_order_ids().await, vec![first_order]);
}

#[tokio::test]
async fn child_order_runner_waits_for_trade_aggregation_after_finished_order() {
    let market = Arc::new(FakeMarketAdapter::new(Quote {
        instrument_id: "SHFE.rb2601".to_string(),
        exchange_id: "SHFE".to_string(),
        ask_price1: 101.0,
        bid_price1: 100.0,
        last_price: 100.0,
        ..Quote::default()
    }));
    let execution = Arc::new(FakeExecutionAdapter::default());
    let runner = ChildOrderRunner::new(
        market,
        execution.clone(),
        "SIM",
        "SHFE.rb2601",
        OrderDirection::Buy,
        PlannedOffset::Open,
        1,
        None,
        PriceMode::Active,
    );

    let task = tokio::spawn(async move { runner.run_until_all_traded().await });

    execution.wait_for_insert_count(1).await;
    let order_id = execution.latest_order_id().await.expect("order should exist");
    execution.finish_order(&order_id, 0, vec![]).await;

    sleep(Duration::from_millis(50)).await;
    assert!(
        !task.is_finished(),
        "runner should wait for trade aggregation after FINISHED"
    );

    execution
        .append_trade(&order_id, make_trade(&order_id, "trade-1", 1, 101.0))
        .await;

    timeout(Duration::from_secs(1), task)
        .await
        .expect("runner should finish once trades are visible")
        .expect("join should succeed")
        .expect("runner should succeed");
}

struct FakeMarketAdapter {
    dm: Arc<DataManager>,
    quote: RwLock<Quote>,
    updates_rx: Mutex<Receiver<()>>,
    updates_tx: Sender<()>,
}

impl FakeMarketAdapter {
    fn new(initial_quote: Quote) -> Self {
        let dm = Arc::new(DataManager::new(HashMap::new(), DataManagerConfig::default()));
        let (updates_tx, updates_rx) = bounded(32);
        Self {
            dm,
            quote: RwLock::new(initial_quote),
            updates_rx: Mutex::new(updates_rx),
            updates_tx,
        }
    }

    async fn update_quote(&self, quote: Quote) {
        *self.quote.write().await = quote;
        let _ = self.updates_tx.send(()).await;
    }
}

#[async_trait]
impl MarketAdapter for FakeMarketAdapter {
    fn dm(&self) -> Arc<DataManager> {
        Arc::clone(&self.dm)
    }

    async fn latest_quote(&self, _symbol: &str) -> RuntimeResult<Quote> {
        Ok(self.quote.read().await.clone())
    }

    async fn wait_quote_update(&self, _symbol: &str) -> RuntimeResult<()> {
        self.updates_rx
            .lock()
            .await
            .recv()
            .await
            .map_err(|_| RuntimeError::AdapterChannelClosed {
                resource: "fake market updates",
            })
    }
}

struct FakeExecutionAdapter {
    state: Mutex<FakeExecutionState>,
    updates_rx: Mutex<Receiver<String>>,
    updates_tx: Sender<String>,
}

#[derive(Default)]
struct FakeExecutionState {
    next_order_seq: usize,
    orders: HashMap<String, Order>,
    trades_by_order: HashMap<String, Vec<Trade>>,
    inserted_prices: Vec<f64>,
    cancelled_order_ids: Vec<String>,
}

impl Default for FakeExecutionAdapter {
    fn default() -> Self {
        let (updates_tx, updates_rx) = bounded(64);
        Self {
            state: Mutex::new(FakeExecutionState::default()),
            updates_rx: Mutex::new(updates_rx),
            updates_tx,
        }
    }
}

impl FakeExecutionAdapter {
    async fn wait_for_insert_count(&self, expected: usize) {
        loop {
            if self.state.lock().await.inserted_prices.len() >= expected {
                return;
            }
            sleep(Duration::from_millis(10)).await;
        }
    }

    async fn wait_for_cancel_count(&self, expected: usize) {
        loop {
            if self.state.lock().await.cancelled_order_ids.len() >= expected {
                return;
            }
            sleep(Duration::from_millis(10)).await;
        }
    }

    async fn latest_order_id(&self) -> Option<String> {
        let state = self.state.lock().await;
        state.orders.keys().cloned().max()
    }

    async fn finish_order(&self, order_id: &str, volume_left: i64, trades: Vec<Trade>) {
        let mut state = self.state.lock().await;
        if let Some(order) = state.orders.get_mut(order_id) {
            order.status = ORDER_STATUS_FINISHED.to_string();
            order.volume_left = volume_left;
        }
        state.trades_by_order.insert(order_id.to_string(), trades);
        drop(state);
        let _ = self.updates_tx.send(order_id.to_string()).await;
    }

    async fn append_trade(&self, order_id: &str, trade: Trade) {
        let mut state = self.state.lock().await;
        state
            .trades_by_order
            .entry(order_id.to_string())
            .or_default()
            .push(trade);
        drop(state);
        let _ = self.updates_tx.send(order_id.to_string()).await;
    }

    async fn inserted_prices(&self) -> Vec<f64> {
        self.state.lock().await.inserted_prices.clone()
    }

    async fn cancelled_order_ids(&self) -> Vec<String> {
        self.state.lock().await.cancelled_order_ids.clone()
    }
}

#[async_trait]
impl ExecutionAdapter for FakeExecutionAdapter {
    fn known_accounts(&self) -> Vec<String> {
        vec!["SIM".to_string()]
    }

    async fn insert_order(&self, _account_key: &str, req: &InsertOrderRequest) -> RuntimeResult<String> {
        let mut state = self.state.lock().await;
        state.next_order_seq += 1;
        let order_id = format!("order-{}", state.next_order_seq);
        state.inserted_prices.push(req.limit_price);
        state.orders.insert(
            order_id.clone(),
            make_order(&order_id, req.volume, req.limit_price, ORDER_STATUS_ALIVE, req.volume),
        );
        Ok(order_id)
    }

    async fn cancel_order(&self, _account_key: &str, order_id: &str) -> RuntimeResult<()> {
        let mut state = self.state.lock().await;
        state.cancelled_order_ids.push(order_id.to_string());
        if let Some(order) = state.orders.get_mut(order_id) {
            order.status = ORDER_STATUS_FINISHED.to_string();
        }
        drop(state);
        let _ = self.updates_tx.send(order_id.to_string()).await;
        Ok(())
    }

    async fn order(&self, _account_key: &str, order_id: &str) -> RuntimeResult<Order> {
        self.state
            .lock()
            .await
            .orders
            .get(order_id)
            .cloned()
            .ok_or_else(|| RuntimeError::OrderNotFound {
                account_key: "SIM".to_string(),
                order_id: order_id.to_string(),
            })
    }

    async fn trades_by_order(&self, _account_key: &str, order_id: &str) -> RuntimeResult<Vec<Trade>> {
        Ok(self
            .state
            .lock()
            .await
            .trades_by_order
            .get(order_id)
            .cloned()
            .unwrap_or_default())
    }

    async fn wait_order_update(&self, _account_key: &str, order_id: &str) -> RuntimeResult<()> {
        loop {
            let updated =
                self.updates_rx
                    .lock()
                    .await
                    .recv()
                    .await
                    .map_err(|_| RuntimeError::AdapterChannelClosed {
                        resource: "fake execution updates",
                    })?;
            if updated == order_id {
                return Ok(());
            }
        }
    }
}

fn make_order(order_id: &str, volume: i64, limit_price: f64, status: &str, volume_left: i64) -> Order {
    Order {
        seqno: 0,
        user_id: "SIM".to_string(),
        order_id: order_id.to_string(),
        exchange_id: "SHFE".to_string(),
        instrument_id: "rb2601".to_string(),
        direction: DIRECTION_BUY.to_string(),
        offset: OFFSET_OPEN.to_string(),
        volume_orign: volume,
        price_type: "LIMIT".to_string(),
        limit_price,
        time_condition: "GFD".to_string(),
        volume_condition: "ANY".to_string(),
        insert_date_time: 0,
        exchange_order_id: String::new(),
        status: status.to_string(),
        volume_left,
        frozen_margin: 0.0,
        last_msg: String::new(),
        epoch: None,
    }
}

fn make_trade(order_id: &str, trade_id: &str, volume: i64, price: f64) -> Trade {
    Trade {
        seqno: 0,
        user_id: "SIM".to_string(),
        trade_id: trade_id.to_string(),
        exchange_id: "SHFE".to_string(),
        instrument_id: "rb2601".to_string(),
        order_id: order_id.to_string(),
        exchange_trade_id: String::new(),
        direction: DIRECTION_BUY.to_string(),
        offset: OFFSET_OPEN.to_string(),
        volume,
        price,
        trade_date_time: 0,
        commission: 0.0,
        epoch: None,
    }
}
