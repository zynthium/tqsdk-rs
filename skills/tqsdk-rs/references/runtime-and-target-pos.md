# Runtime 与 TargetPos

当用户明确问 `TqRuntime`、`target_pos`、`target_pos_scheduler`、target-pos 回测或 live runtime 装配时，读本文件。

## 当前 canonical 入口

### live runtime

推荐两条路径：

1. builder 预配置交易账户，并且希望 runtime 直接负责 live 发单时，用 `build_connected_runtime()`
2. 已有 `Client` + `TradeSession`，先按 `TradeSession::connect()` 完成连接，再 `into_runtime()`

示例：

```rust
use std::sync::Arc;
use tqsdk_rs::prelude::*;

let runtime: Arc<TqRuntime> = Client::builder(username, password)
    .trade_session_with_options(
        "simnow",
        user_id,
        trade_password,
        TradeSessionOptions::default(),
    )
    .build_connected_runtime()
    .await?;

let account = runtime.account("simnow:user_id")?;
```

`build_runtime()` 仍然可用，但它只做 runtime 装配，不会隐式 `connect()` builder
预配置的 `TradeSession`。

### replay runtime

```rust
let mut session = client
    .create_backtest_session(ReplayConfig::new(start_dt, end_dt)?)
    .await?;

let runtime = session.runtime(["TQSIM"]).await?;
let account = runtime.account("TQSIM")?;
```

## `account_key` 规则

- live：通常是 `broker:user_id`
- replay：就是你传给 `session.runtime([...])` 的名字

如果用户问“为什么 account not found”，先检查 key 是否写错。

## `TargetPosTask`

当前推荐入口：

```rust
let task = account
    .target_pos("SHFE.rb2605")
    .config(TargetPosConfig::default())
    .build()?;

task.set_target_volume(1)?;
task.wait_target_reached().await?;
task.cancel().await?;
task.wait_finished().await?;
```

常见配置项：

- `price_mode`
- `offset_priority`
- `split_policy`

## `TargetPosScheduler`

当前推荐入口：

```rust
use std::time::Duration;

let scheduler = account
    .target_pos_scheduler("SHFE.rb2605")
    .steps(vec![
        TargetPosScheduleStep {
            interval: Duration::from_secs(10),
            target_volume: 1,
            price_mode: Some(PriceMode::Active),
        },
        TargetPosScheduleStep {
            interval: Duration::from_secs(10),
            target_volume: 0,
            price_mode: Some(PriceMode::Passive),
        },
    ])
    .build()?;

scheduler.wait_finished().await?;
let trades = scheduler.trades();
```

`TargetPosScheduleStep.price_mode = None` 表示纯等待步，不会提交调仓，但仍占用该 symbol。

## runtime 里的关键边界

- `TqRuntime` 是任务运行时，不是普通行情 / 手工交易默认入口
- live runtime 来自 `Client` 时，会复用该 `Client` session 的同一 private live context；不会自建第二套行情 websocket / `MarketDataState`
- 关闭该 `Client` session 后，runtime market wait 路径也会一起收到关闭信号
- target-pos 任务与 scheduler 会独占同一 `runtime + account + symbol`
- 任务占用 symbol 时，冲突的手工下单会报错
- live / replay 只是 adapter 不同，公开任务入口保持一致

## 避免的非 canonical surface

- 把 target-pos 默认讲成裸构造器入口
- 把 `TargetPosTask` 说成 `TradeSession` 的能力
- 把 runtime 的 adapter / registry / planning 内部件讲成普通用户扩展点
