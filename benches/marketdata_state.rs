use criterion::{BenchmarkId, Criterion, black_box, criterion_group, criterion_main};
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::runtime::Runtime;
use tqsdk_rs::datamanager::{DataManager, DataManagerConfig};
use tqsdk_rs::marketdata::{KlineKey, MarketDataState, SymbolId, TqApi};
use tqsdk_rs::types::{Kline, Quote};

fn sample_quote(symbol: &str, last_price: f64) -> Quote {
    Quote {
        instrument_id: symbol.to_string(),
        datetime: "2024-01-01 09:00:00.000000".to_string(),
        last_price,
        ..Quote::default()
    }
}

fn sample_kline(id: i64, datetime: i64, close: f64) -> Kline {
    Kline {
        id,
        datetime,
        open: close - 1.0,
        close,
        high: close + 1.0,
        low: close - 2.0,
        open_oi: id * 10,
        close_oi: id * 10 + 1,
        volume: id * 100,
        epoch: None,
    }
}

fn dm_load_latest_kline(dm: &DataManager, symbol: &str, duration_nanos: i64) -> Option<Kline> {
    let duration_str = duration_nanos.to_string();
    let last_id = dm
        .get_by_path(&["klines", symbol, &duration_str, "last_id"])
        .and_then(|v| v.as_i64())?;
    let id_key = last_id.to_string();
    let value = dm.get_by_path(&["klines", symbol, &duration_str, "data", &id_key])?;
    let mut kline = dm.convert_to_struct::<Kline>(&value).ok()?;
    kline.id = last_id;
    Some(kline)
}

fn bench_quote(c: &mut Criterion) {
    let symbol = "SHFE.au2602";
    let mut group = c.benchmark_group("quote");

    let dm = DataManager::new(HashMap::new(), DataManagerConfig::default());
    group.bench_function(BenchmarkId::new("datamanager_merge_get", symbol), |b| {
        let mut price = 500.0f64;
        b.iter(|| {
            price += 1.0;
            dm.merge_data(
                json!({
                    "quotes": {
                        (symbol): {
                            "instrument_id": symbol,
                            "datetime": "2024-01-01 09:00:00.000000",
                            "last_price": price
                        }
                    }
                }),
                true,
                true,
            );
            let quote = dm.get_quote_data(symbol).unwrap();
            black_box(quote.last_price);
        });
    });

    let rt = Runtime::new().unwrap();
    let state = Arc::new(MarketDataState::default());
    let api = TqApi::new(Arc::clone(&state));
    let quote_ref = Arc::new(api.quote(symbol));
    let symbol_id = SymbolId::from(symbol);
    group.bench_function(BenchmarkId::new("state_update_load", symbol), |b| {
        let quote_ref = Arc::clone(&quote_ref);
        let state = Arc::clone(&state);
        let symbol_id = symbol_id.clone();
        let mut price = 500.0f64;
        b.to_async(&rt).iter(|| {
            price += 1.0;
            let quote_ref = Arc::clone(&quote_ref);
            let state = Arc::clone(&state);
            let symbol_id = symbol_id.clone();
            async move {
                state.update_quote(symbol_id, sample_quote(symbol, price)).await;
                let quote = quote_ref.load().await;
                black_box(quote.last_price);
            }
        });
    });

    group.finish();
}

fn bench_kline(c: &mut Criterion) {
    let symbol = "SHFE.au2602";
    let duration_nanos = 60_000_000_000i64;
    let mut group = c.benchmark_group("kline");
    group.measurement_time(Duration::from_secs(5));

    let dm = DataManager::new(HashMap::new(), DataManagerConfig::default());
    group.bench_function(BenchmarkId::new("datamanager_merge_load_latest", symbol), |b| {
        let mut id = 0i64;
        let mut dt = 1_000_000_000i64;
        b.iter(|| {
            id += 1;
            dt += duration_nanos;
            dm.merge_data(
                json!({
                    "klines": {
                        (symbol): {
                            (duration_nanos.to_string()): {
                                "last_id": id,
                                "data": {
                                    (id.to_string()): {
                                        "datetime": dt,
                                        "open": 9.0,
                                        "close": 10.0,
                                        "high": 11.0,
                                        "low": 8.0,
                                        "open_oi": 100,
                                        "close_oi": 101,
                                        "volume": 10
                                    }
                                }
                            }
                        }
                    }
                }),
                true,
                true,
            );
            let kline = dm_load_latest_kline(&dm, symbol, duration_nanos).unwrap();
            black_box(kline.close);
        });
    });

    let rt = Runtime::new().unwrap();
    let state = Arc::new(MarketDataState::default());
    let api = TqApi::new(Arc::clone(&state));
    let kline_ref = Arc::new(api.kline(symbol, Duration::from_secs(60)));
    let key = KlineKey::new(symbol, Duration::from_secs(60));
    group.bench_function(BenchmarkId::new("state_update_load", symbol), |b| {
        let kline_ref = Arc::clone(&kline_ref);
        let state = Arc::clone(&state);
        let key = key.clone();
        let mut id = 0i64;
        let mut dt = 1_000_000_000i64;
        b.to_async(&rt).iter(|| {
            id += 1;
            dt += duration_nanos;
            let kline_ref = Arc::clone(&kline_ref);
            let state = Arc::clone(&state);
            let key = key.clone();
            async move {
                state.update_kline(key, sample_kline(id, dt, 10.0)).await;
                let kline = kline_ref.load().await;
                black_box(kline.close);
            }
        });
    });

    group.finish();
}

criterion_group!(benches, bench_quote, bench_kline);
criterion_main!(benches);
