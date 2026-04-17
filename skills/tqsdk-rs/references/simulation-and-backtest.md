# 回放与回测

当用户问 `ReplaySession`、`ReplayConfig`、回放句柄、时间推进、回测结果或 replay runtime 时，读本文件。

## 当前唯一推荐入口

```rust
use tqsdk_rs::{Client, EndpointConfig, ReplayConfig};

let client = Client::builder(username, password)
    .endpoints(EndpointConfig::from_env())
    .build()
    .await?;

let mut session = client
    .create_backtest_session(ReplayConfig::new(start_dt, end_dt)?)
    .await?;
```

公开回测入口统一讲 `create_backtest_session()` 和 `ReplaySession`。

## 当前 replay 句柄

- `quote(symbol)` -> `ReplayQuoteHandle`
- `kline(symbol, duration, width)` -> `ReplayKlineHandle`
- `tick(symbol, width)` -> `ReplayTickHandle`
- `aligned_kline(symbols, duration, width)` -> `AlignedKlineHandle`

这些句柄都由 `ReplaySession::step()` 驱动更新。

## 当前推荐回测循环

```rust
let quote = session.quote("SHFE.rb2605").await?;
let bars = session.kline("SHFE.rb2605", std::time::Duration::from_secs(60), 128).await?;

while let Some(step) = session.step().await? {
    if step.updated_handles.iter().any(|id| id == bars.id()) {
        let rows = bars.rows().await;
        println!("rows={}", rows.len());
    }

    if step.updated_quotes.iter().any(|symbol| symbol == "SHFE.rb2605") {
        if let Some(snapshot) = quote.snapshot().await {
            println!("last={}", snapshot.last_price);
        }
    }
}

let result = session.finish().await?;
println!("trades={}", result.trades.len());
```

## `step()` 的语义

`ReplaySession::step()` 是唯一时间推进入口。

每次返回 `ReplayStep`，常用字段：

- `current_dt`
- `updated_handles`
- `updated_quotes`
- `settled_trading_day`

如果用户说“回测不动”“target-pos 不执行”“quote 不刷新”，先检查是不是没有持续调用 `step()`。

## `quote()` 的隐式驱动

`session.quote(symbol)` 在没有显式 tick / kline feed 时，会自动补一个隐式 1 分钟 quote 驱动。

因此：

- 只看 replay quote 时，通常先 `quote(symbol)` 就够
- runtime 下单也会为未显式订阅的 symbol 建立必要的 quote 驱动

## 当前能力边界

- 当前 replay / backtest 主路径聚焦期货 / 商品期权。
- replay 内核已支持日切结算，以及按交易日应用辅助元数据 patch；这层能力现在可用于主连 `underlying_symbol` 日切。
- 默认 `Client::create_backtest_session()` 历史源还不会自动抓取历史主连映射；如果要依赖 continuous mapping 时间线，当前应使用可注入的 historical source 或测试源。
- 当前还不覆盖：股票回放 / T+1、分红送股时间线、时间驱动 `TqReplay` 等价物、内建 report metrics。

## replay runtime

回测下的 target-pos 入口：

```rust
let runtime = session.runtime(["TQSIM"]).await?;
let account = runtime.account("TQSIM")?;
let task = account.target_pos("SHFE.rb2605").build()?;
```

## `finish()` 的结果

`finish()` 返回 `BacktestResult`，可读：

- `settlements`
- `final_accounts`
- `final_positions`
- `trades`

这是回测结束后的汇总结果，不是实时更新对象。

## 避免的非 canonical 写法

- 把回测入口讲成别的 facade 名字
- 把 replay 讲成 live 状态机上的模式切换
- 把 replay 说成 `Client` live 状态机的一部分
