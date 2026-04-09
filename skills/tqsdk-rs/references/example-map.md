# 示例索引

当用户要“仓库里有现成例子吗”“给我一个最接近的 example”时，优先读本文件。

## 行情订阅

- `examples/quote.rs`
  - Quote、K线、Tick、多合约序列的综合示例

## 历史数据

- `examples/history.rs`
  - `kline_history`
  - `kline_history_with_focus`
  - 大量历史 K 线分片接收

## 交易

- `examples/trade.rs`
  - `TradeSession`
  - 回调注册顺序
  - 下单、撤单、查询
  - `TQ_TD_URL` / `TradeSessionOptions` 风格

## TargetPos / Runtime

- `README.md`
  - `TargetPosTask` compat facade
  - `TargetPosScheduler` compat facade
  - backtest runtime wiring 片段
- `src/runtime/`
  - `TqRuntime`
  - `AccountHandle`
  - `TargetPosTask` / `TargetPosScheduler`
  - `BacktestExecutionAdapter`

## 回测

- `examples/backtest.rs`
  - 回测初始化
  - 回放与推进
  - build backtest runtime helper

## 合约与基础数据

- `examples/datamanager.rs`
  - `DataManager` 风格使用
- `examples/option_levels.rs`
  - 期权层级与合约筛选

## 日志

- `examples/custom_logger.rs`
  - 自定义日志初始化

## 当用户问“看哪个模块源码”

- 客户端与公开入口：`src/client/`
- 行情：`src/quote/`
- K线 / Tick：`src/series/`
- 合约与基础数据：`src/ins/`
- 交易：`src/trade_session/`
- 目标持仓运行时：`src/runtime/`
- 认证：`src/auth/`
- 回测：`src/backtest/`
