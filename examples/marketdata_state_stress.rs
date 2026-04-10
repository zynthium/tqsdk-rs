use std::sync::Arc;
use std::time::Instant;
use tqsdk_rs::marketdata::{MarketDataState, SymbolId, TqApi};
use tqsdk_rs::types::Quote;

fn quote_with_price(symbol: &str, last_price: f64) -> Quote {
    Quote {
        instrument_id: symbol.to_string(),
        datetime: "2024-01-01 09:00:00.000000".to_string(),
        last_price,
        ..Quote::default()
    }
}

async fn run_case(symbol_count: usize) {
    let state = Arc::new(MarketDataState::default());
    let api = TqApi::new(Arc::clone(&state));

    let symbols: Vec<SymbolId> = (0..symbol_count)
        .map(|idx| SymbolId::from(format!("SYM.{idx:04}")))
        .collect();
    let refs = symbols
        .iter()
        .cloned()
        .map(|symbol| api.quote(symbol))
        .collect::<Vec<_>>();

    let update_start = Instant::now();
    for (idx, symbol) in symbols.iter().enumerate() {
        state
            .update_quote(symbol.clone(), quote_with_price(symbol.as_str(), idx as f64))
            .await;
    }
    let update_cost = update_start.elapsed();

    let load_start = Instant::now();
    for r in &refs {
        let q = r.load().await;
        std::hint::black_box(q.last_price);
    }
    let load_cost = load_start.elapsed();

    let drain_start = Instant::now();
    let updates = api.wait_update_and_drain().await.unwrap();
    let drain_cost = drain_start.elapsed();
    let changed = updates.quotes.len();

    println!(
        "symbols={} update={:?} load={:?} drain_updates={:?} changed={}",
        symbol_count, update_cost, load_cost, drain_cost, changed
    );
}

#[tokio::main]
async fn main() {
    for symbol_count in [200usize, 500, 1000] {
        run_case(symbol_count).await;
    }
}
