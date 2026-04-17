use super::{KlineKey, MarketDataState, QuoteRef, SymbolId, TqApi};
use crate::errors::TqError;
use crate::types::{Kline, Quote, Tick};
use std::sync::Arc;
use std::time::Duration;

fn quote_with_price(symbol: &str, last_price: f64) -> Quote {
    Quote {
        instrument_id: symbol.to_string(),
        datetime: "2024-01-01 09:00:00.000000".to_string(),
        last_price,
        ..Quote::default()
    }
}

fn kline_with_id(id: i64, dt: i64, close: f64) -> Kline {
    Kline {
        id,
        datetime: dt,
        open: close - 1.0,
        close,
        high: close + 1.0,
        low: close - 2.0,
        open_oi: id,
        close_oi: id + 1,
        volume: id * 10,
        epoch: None,
    }
}

fn tick_with_id(id: i64, dt: i64, last_price: f64) -> Tick {
    Tick {
        id,
        datetime: dt,
        last_price,
        average: last_price,
        highest: last_price,
        lowest: last_price,
        ask_price1: last_price,
        ask_volume1: 1,
        ask_price2: f64::NAN,
        ask_volume2: 0,
        ask_price3: f64::NAN,
        ask_volume3: 0,
        ask_price4: f64::NAN,
        ask_volume4: 0,
        ask_price5: f64::NAN,
        ask_volume5: 0,
        bid_price1: last_price,
        bid_volume1: 1,
        bid_price2: f64::NAN,
        bid_volume2: 0,
        bid_price3: f64::NAN,
        bid_volume3: 0,
        bid_price4: f64::NAN,
        bid_volume4: 0,
        bid_price5: f64::NAN,
        bid_volume5: 0,
        volume: 0,
        amount: 0.0,
        open_interest: 0,
        epoch: None,
    }
}

#[tokio::test]
async fn quote_epoch_receiver_is_monotonic() {
    let state = Arc::new(MarketDataState::default());
    let symbol = SymbolId::from("SHFE.au2602");
    let mut rx = state.subscribe_quote_epoch(&symbol).await;
    let mut prev = *rx.borrow();

    for i in 1..=50u64 {
        state
            .update_quote(symbol.clone(), quote_with_price(symbol.as_str(), i as f64))
            .await;
        rx.changed().await.unwrap();
        let current = *rx.borrow();
        assert!(current > prev);
        prev = current;
    }

    assert_eq!(state.quote_epoch(&symbol).await, prev);
}

#[tokio::test]
async fn global_epoch_is_monotonic_under_concurrent_updates() {
    let state = Arc::new(MarketDataState::default());
    let mut global_rx = state.subscribe_global_epoch();
    let updates = 200usize;

    let mut tasks = Vec::with_capacity(updates);
    for i in 0..updates {
        let state = Arc::clone(&state);
        tasks.push(tokio::spawn(async move {
            let symbol = SymbolId::from(format!("SYM.{i}"));
            state.update_quote(symbol, quote_with_price("SYM", i as f64)).await;
        }));
    }

    for task in tasks {
        task.await.unwrap();
    }

    while *global_rx.borrow() < updates as u64 {
        global_rx.changed().await.unwrap();
    }
}

#[tokio::test]
async fn is_changing_tracks_updates_per_reference() {
    let state = Arc::new(MarketDataState::default());
    let api = TqApi::new(Arc::clone(&state));
    let symbol = "SHFE.au2602";
    let symbol_id = SymbolId::from(symbol);

    let q1 = api.quote(symbol);
    let q2 = api.quote(symbol);
    assert!(!q1.is_changing().await);
    assert!(!q2.is_changing().await);

    state
        .update_quote(symbol_id.clone(), quote_with_price(symbol, 500.0))
        .await;
    assert!(q1.is_changing().await);
    assert!(q2.is_changing().await);
    assert!(!q1.is_changing().await);
    assert!(!q2.is_changing().await);

    state.update_quote(symbol_id, quote_with_price(symbol, 501.0)).await;
    assert!(q1.is_changing().await);
    assert!(q2.is_changing().await);
}

#[tokio::test]
async fn kline_and_tick_is_changing_semantics_match_quote() {
    let state = Arc::new(MarketDataState::default());
    let api = TqApi::new(Arc::clone(&state));
    let symbol = "SHFE.au2602";

    let key = KlineKey::new(symbol, Duration::from_secs(60));
    let k = api.kline(symbol, Duration::from_secs(60));
    let t = api.tick(symbol);

    assert!(!k.is_changing().await);
    assert!(!t.is_changing().await);

    state
        .update_kline(key.clone(), kline_with_id(1, 1_000_000_000, 10.0))
        .await;
    state
        .update_tick(SymbolId::from(symbol), tick_with_id(1, 1_000_000_000, 10.0))
        .await;

    assert!(k.is_changing().await);
    assert!(t.is_changing().await);
    assert!(!k.is_changing().await);
    assert!(!t.is_changing().await);

    state.update_kline(key, kline_with_id(2, 2_000_000_000, 11.0)).await;
    state
        .update_tick(SymbolId::from(symbol), tick_with_id(2, 2_000_000_000, 11.0))
        .await;

    assert!(k.is_changing().await);
    assert!(t.is_changing().await);
}

#[tokio::test]
async fn quote_ref_try_load_reports_not_ready_before_first_snapshot() {
    let state = Arc::new(MarketDataState::default());
    let quote_ref = QuoteRef::new_for_test(Arc::clone(&state), "SHFE.au2602");

    assert!(!quote_ref.is_ready().await);
    assert!(quote_ref.snapshot().await.is_none());

    let err = quote_ref.try_load().await.unwrap_err();
    assert!(matches!(err, TqError::DataNotReady(_)));
}

#[tokio::test]
async fn quote_ref_try_load_returns_live_snapshot_after_update() {
    let state = Arc::new(MarketDataState::default());
    let quote_ref = QuoteRef::new_for_test(Arc::clone(&state), "SHFE.au2602");

    state
        .update_quote(
            "SHFE.au2602".into(),
            Quote {
                instrument_id: "au2602".to_string(),
                last_price: 520.5,
                ..Default::default()
            },
        )
        .await;

    assert!(quote_ref.is_ready().await);
    let quote = quote_ref.try_load().await.unwrap();
    assert_eq!(quote.last_price, 520.5);
}

#[tokio::test]
async fn wait_update_and_drain_reports_unique_changed_keys() {
    let state = Arc::new(MarketDataState::default());
    let api = TqApi::new(Arc::clone(&state));

    let au = SymbolId::from("SHFE.au2602");
    let ag = SymbolId::from("SHFE.ag2512");
    let kline_key = KlineKey::new(au.clone(), Duration::from_secs(60));

    state
        .update_quote(au.clone(), quote_with_price(au.as_str(), 500.0))
        .await;
    state
        .update_quote(au.clone(), quote_with_price(au.as_str(), 501.0))
        .await;
    state
        .update_quote(ag.clone(), quote_with_price(ag.as_str(), 600.0))
        .await;
    state
        .update_kline(kline_key.clone(), kline_with_id(1, 1_000_000_000, 10.0))
        .await;
    state
        .update_tick(au.clone(), tick_with_id(1, 1_000_000_000, 10.0))
        .await;

    let updates = api.wait_update_and_drain().await.unwrap();
    assert_eq!(updates.quotes, vec![ag.clone(), au.clone()]);
    assert_eq!(updates.klines, vec![kline_key]);
    assert_eq!(updates.ticks, vec![au.clone()]);

    let empty = api.drain_updates().await.unwrap();
    assert!(empty.quotes.is_empty());
    assert!(empty.klines.is_empty());
    assert!(empty.ticks.is_empty());
}

#[tokio::test]
async fn wait_paths_return_client_closed_after_market_close() {
    let state = Arc::new(MarketDataState::default());
    let api = TqApi::new(Arc::clone(&state));
    state.close();

    let wait = api.wait_update().await;
    assert!(matches!(wait, Err(TqError::ClientClosed { .. })));

    let drain = api.wait_update_and_drain().await;
    assert!(matches!(drain, Err(TqError::ClientClosed { .. })));
}
