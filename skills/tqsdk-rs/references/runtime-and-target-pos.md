# Runtime 与 TargetPos

当用户问下面这些问题时，优先读本文件：

- Rust 版有没有 Python `TargetPosTask` / `TargetPosScheduler`
- `TqRuntime`、`AccountHandle`、`build_runtime`、`into_runtime` 怎么用
- 目标持仓任务和 `TradeSession` 的边界是什么
- 回测下怎么跑 `TargetPosTask` / `TargetPosScheduler`
- 为什么手工下单会和 target-pos 任务冲突

## 核心心智

`runtime/` 是目标持仓任务的统一执行层。

- `TradeSession`：低层手工交易接口
- `TqRuntime`：任务运行时
- `AccountHandle`：账户级任务入口
- `TargetPosTask`：持续追到目标净持仓
- `TargetPosScheduler`：时间分片调仓

不要把 `TargetPosTask` 挂到 `TradeSession` 上去解释。

## 创建 runtime

### live 场景推荐：先有 TradeSession，再转 runtime

```rust
use std::sync::Arc;
use tqsdk_rs::{Client, Result, TqRuntime};

async fn build_runtime(username: &str, password: &str) -> Result<Arc<TqRuntime>> {
    let client = Client::builder(username, password).build().await?;
    let _session = client
        .create_trade_session("simnow", "user_id", "password")
        .await?;
    Ok(client.into_runtime())
}
```

这里的关键点是：

- live runtime 的可用账户来自已注册的 `TradeSession`
- 所以要先 `create_trade_session*()` 或 `register_trade_session()`
- 然后再 `into_runtime()`
- live 下 `runtime.account(account_key)` 的 `account_key` 通常是 `broker:user_id`
- backtest 下的 `account_key` 来自 `BacktestExecutionAdapter::new(vec![...])` 里预置的账户名

### `build_runtime()` 的当前语义

```rust
let runtime = Client::builder(username, password).build_runtime().await?;
```

它确实会返回 `Arc<TqRuntime>`，但当前不会自动帮你创建 live 账户句柄。
如果用户的下一步是 `runtime.account(...)`，优先把答案改回上面的 `into_runtime()` 路径。

## Rust 原生 API

```rust
use std::sync::Arc;
use tqsdk_rs::prelude::*;

async fn run_target(runtime: Arc<TqRuntime>) -> RuntimeResult<()> {
    let account = runtime.account("SIM")?;
    let task = account
        .target_pos("SHFE.rb2601")
        .config(TargetPosConfig::default())
        .build()?;

    task.set_target_volume(1)?;
    task.wait_target_reached().await?;
    task.cancel().await?;
    task.wait_finished().await?;
    Ok(())
}
```

scheduler:

```rust
use std::sync::Arc;
use std::time::Duration;
use tqsdk_rs::prelude::*;

async fn run_scheduler(runtime: Arc<TqRuntime>) -> RuntimeResult<()> {
    let account = runtime.account("SIM")?;
    let scheduler = account
        .target_pos_scheduler("SHFE.rb2601")
        .steps(vec![
            TargetPosScheduleStep {
                interval: Duration::from_secs(10),
                target_volume: 1,
                price_mode: Some(PriceMode::Passive),
            },
            TargetPosScheduleStep {
                interval: Duration::from_secs(5),
                target_volume: 1,
                price_mode: Some(PriceMode::Active),
            },
        ])
        .build()?;

    scheduler.wait_finished().await?;
    println!("trades={}", scheduler.execution_report().trades.len());
    Ok(())
}
```

## Python 兼容 facade

```rust
use std::sync::Arc;
use tqsdk_rs::prelude::*;

async fn run_compat(runtime: Arc<TqRuntime>) -> RuntimeResult<()> {
    let task = TargetPosTask::new(
        runtime.clone(),
        "SIM",
        "SHFE.rb2601",
        TargetPosTaskOptions::default(),
    )
    .await?;

    task.set_target_volume(1)?;
    task.wait_target_reached().await?;
    Ok(())
}
```

如果用户明确在做 Python 策略迁移，优先给 compat facade。

## 回测下运行 TargetPos

`BacktestHandle` 只负责市场时间推进。

target-pos 任务执行需要额外挂一个 runtime：

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

重点要说明：

- 市场时间仍由 `BacktestHandle` 推进
- `LiveMarketAdapter::new(backtest.dm())` 负责从回测 `DataManager` 读取 quote/trading_time
- `BacktestExecutionAdapter` 负责 runtime 任务的成交与持仓更新
- 它是内存内立即成交模型，不是完整撮合器

## 冲突与所有权规则

- 同一 `runtime + account + symbol` 上只允许一个 target-pos 任务所有者
- `TargetPosTask` 运行中，手工 `insert_order` 会被拒绝
- `TargetPosScheduler` 运行中，内部 step task 也会占用同一 symbol
- 即使 scheduler 某一步 `price_mode: None` 只是等待，它仍继续持有 symbol

用户遇到“为什么手工单被挡住”，先检查是不是同 symbol 已有 target-pos 任务。

## 什么时候别用 runtime

以下情况优先还是讲 `TradeSession`：

- 用户只是想手工下单/撤单/查账户
- 用户要注册 `on_order` / `on_trade` / `order_channel`
- 用户不需要“目标净持仓”这个抽象

## 常见坑

### “我已经有 `TradeSession` 了，能不能直接 new TargetPosTask”

不能。`TargetPosTask` 依赖的是 `TqRuntime`，不是 `TradeSession`。

### “回测里 scheduler 时间为什么会跨午休”

当前 scheduler 会优先用 `quote.datetime + trading_time` 计算交易时段内的 deadline；
如果 quote 没有有效时间字符串，才会退回 wall-clock 等待。

### “TargetPosTask 会不会和手工单混用”

默认不要混用。同 symbol 上会有所有权冲突保护。
