# 回测与回放

当用户问下面这些问题时，优先读本文件：

- 如何初始化回测
- `init_market_backtest` 和 `init_market` 的区别
- `BacktestHandle` 怎么推进
- live 和 backtest 怎么切换
- 回测下怎么挂 `TqRuntime` 跑 `TargetPosTask` / `TargetPosScheduler`

## 基本思路

```text
Client::builder
  -> build
  -> init_market_backtest
  -> 使用 quote / series / ins
  -> 通过 BacktestHandle 推进
```

## 常见入口

- `init_market_backtest`
- `switch_to_backtest`
- `switch_to_live`
- `BacktestHandle::next`
- `BacktestHandle::peek`
- `BacktestHandle::current_dt`
- `BacktestExecutionAdapter`
- `TqRuntime::with_id(..., RuntimeMode::Backtest, ...)`

## 用户提问时的回答重点

### “回测为什么不往前走？”

优先解释 `BacktestHandle` 推进机制，而不是先怀疑行情 API。

### “回测下还能不能用 quote / series / ins？”

可以，但前提是已经通过回测初始化行情链路。

### “回测下能不能跑 TargetPosTask / TargetPosScheduler？”

可以，但要区分两层：

- `BacktestHandle` 负责推进市场时间
- `TqRuntime + BacktestExecutionAdapter` 负责任务执行

常用 wiring：

```rust
use std::sync::Arc;
use tqsdk_rs::prelude::*;
use tqsdk_rs::runtime::LiveMarketAdapter;

fn build_backtest_runtime(backtest: &BacktestHandle) -> Arc<TqRuntime> {
    Arc::new(TqRuntime::with_id(
        "backtest-runtime",
        RuntimeMode::Backtest,
        Arc::new(LiveMarketAdapter::new(backtest.dm())),
        Arc::new(BacktestExecutionAdapter::new(vec!["TQSIM".to_string()])),
    ))
}
```

### “怎么在 live 和 backtest 间切换？”

直接指向：

- `switch_to_backtest`
- `switch_to_live`

同时提醒用户，切换会影响后续数据来源和时间推进语义。

## 常见坑

### “回测里数据不更新”

先检查是否真的在推进 `BacktestHandle`。

### “拿不到 Series / Ins”

仍然先检查初始化顺序：是不是已经走了回测版初始化。

### “回测和实盘代码能不能共用？”

可以共用大量上层逻辑，但回答时要明确指出：

- 数据推进方式不同
- 时间语义不同
- 交易链路和权限语义也可能不同

对于 target-pos 任务，还要补一句：

- `BacktestExecutionAdapter` 当前是内存内立即成交模型，不是完整交易所撮合模拟器
