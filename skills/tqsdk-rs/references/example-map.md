# 示例索引

当用户说“仓库里有现成例子吗”“给我一个最接近的 example”时，先读本文件。

## live 行情 / 序列

- `examples/quote.rs`
  - `Client::init_market()`
  - `subscribe_quote()`
  - `quote()`
  - `kline()` / `tick()`
  - `kline_ref()` / `tick_ref()`
  - `wait_update_and_drain()`

## 一次性历史快照

- `examples/history.rs`
  - `get_kline_data_series()`
  - 时间区间 `[start_dt, end_dt)`
  - 环境变量驱动的历史查询参数

## 交易

- `examples/trade.rs`
  - `TradeSession`
  - `subscribe_events()`
  - `wait_update()`
  - `connect()` / `is_ready()`
  - 手工下单 / 撤单片段

## replay / backtest

- `examples/backtest.rs`
  - `create_backtest_session()`
  - `ReplaySession::quote()` / `kline()`
  - `ReplaySession::runtime()`
  - `TargetPosTask`
  - `step()` / `finish()`

- `examples/pivot_point.rs`
  - 基于 `ReplaySession` 的完整策略例子
  - 日线开盘事件
  - `TargetPosTask`

## 合约与期权

- `examples/option_levels.rs`
  - `query_atm_options()`
  - `query_all_level_options()`
  - `query_all_level_finance_options()`

## DIFF / DataManager

- `examples/datamanager.rs`
  - `watch()` / `unwatch()`
  - `get_by_path()`
  - `subscribe_epoch()`

## 日志

- `examples/custom_logger.rs`
  - 自定义 logger 初始化

## 如果用户要“看哪个模块源码”

- live facade：`src/client/`
- Quote 生命周期：`src/quote/`
- K 线 / Tick / 历史：`src/series/`
- 合约与基础数据：`src/ins/`
- 交易：`src/trade_session/`
- 回放：`src/replay/`
- runtime / target-pos：`src/runtime/`
- DIFF 状态中心：`src/datamanager/`
