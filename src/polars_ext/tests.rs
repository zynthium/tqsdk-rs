use super::{KlineBuffer, TickBuffer};
use crate::types::{Kline, Tick};

#[test]
fn test_kline_buffer() {
    let mut buffer = KlineBuffer::new();

    let kline1 = Kline {
        id: 1,
        datetime: 1000000000,
        open: 100.0,
        high: 105.0,
        low: 99.0,
        close: 103.0,
        volume: 1000,
        open_oi: 500,
        close_oi: 520,
        epoch: None,
    };

    buffer.push(&kline1);
    assert_eq!(buffer.len(), 1);

    let kline2 = Kline {
        id: 1,
        datetime: 1000000000,
        open: 100.0,
        high: 106.0,
        low: 98.0,
        close: 104.0,
        volume: 1200,
        open_oi: 500,
        close_oi: 530,
        epoch: None,
    };

    buffer.update_last(&kline2);
    assert_eq!(buffer.len(), 1);
    assert_eq!(buffer.highs[0], 106.0);
    assert_eq!(buffer.closes[0], 104.0);

    let df = buffer.to_dataframe().unwrap();
    assert_eq!(df.height(), 1);
    assert_eq!(df.width(), 9);
}

#[test]
fn test_tick_buffer() {
    let mut buffer = TickBuffer::new();

    let tick = Tick {
        id: 1,
        datetime: 1000000000,
        last_price: 100.0,
        average: 99.5,
        highest: 101.0,
        lowest: 98.0,
        ask_price1: 100.5,
        ask_volume1: 10,
        ask_price2: f64::NAN,
        ask_volume2: 0,
        ask_price3: f64::NAN,
        ask_volume3: 0,
        ask_price4: f64::NAN,
        ask_volume4: 0,
        ask_price5: f64::NAN,
        ask_volume5: 0,
        bid_price1: 99.5,
        bid_volume1: 20,
        bid_price2: f64::NAN,
        bid_volume2: 0,
        bid_price3: f64::NAN,
        bid_volume3: 0,
        bid_price4: f64::NAN,
        bid_volume4: 0,
        bid_price5: f64::NAN,
        bid_volume5: 0,
        volume: 1000,
        amount: 100000.0,
        open_interest: 5000,
        epoch: None,
    };

    buffer.push(&tick);
    assert_eq!(buffer.len(), 1);

    let df = buffer.to_dataframe().unwrap();
    assert_eq!(df.height(), 1);
}
