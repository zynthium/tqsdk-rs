# 场景索引

当用户说“给我一个最接近的用法场景”时，先读本文件。

## live 行情 / 序列场景

- `Client::init_market()`
- `subscribe_quote()`
- `quote()`
- `get_kline_serial()` / `get_tick_serial()`
- `kline_ref()` / `tick_ref()`
- `wait_update_and_drain()`

## 一次性历史快照场景

- `get_kline_data_series()`
- `examples/history.rs` 演示 bounded serial
- `examples/data_series.rs` 演示显式时间范围下载
- 时间区间 `[start_dt, end_dt)`
- 环境变量驱动的历史查询参数

## 交易场景

- `TradeSession`
- `subscribe_events()`
- `wait_update()`
- `connect()` / `is_ready()`
- 手工下单 / 撤单片段

## replay / backtest 场景

- `create_backtest_session()`
- `ReplaySession::quote()` / `kline()`
- `ReplaySession::runtime()`
- `TargetPosTask`
- `step()` / `finish()`

- 基于 `ReplaySession` 的完整策略例子
- 日线开盘事件
- `TargetPosTask`
- `examples/backtest.rs` 演示基础回放 / runtime 汇总
- `examples/pivot_point.rs` 演示日线枢轴点反转
- `examples/doublema.rs` 演示双均线交叉
- `examples/dualthrust.rs` 演示 Dual Thrust 日线突破
- `examples/rbreaker.rs` 演示 R-Breaker 日内突破 / 反转

## 合约与期权场景

- `query_atm_options()`
- `query_all_level_options()`
- `query_all_level_finance_options()`

## DIFF / DataManager 场景

- `watch()` / `unwatch()`
- `get_by_path()`
- `subscribe_epoch()`

## 日志场景

- 自定义 logger 初始化
