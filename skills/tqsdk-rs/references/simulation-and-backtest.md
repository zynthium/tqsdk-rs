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

不要再把 `BacktestHandle` 或 `init_market_backtest()` 当公开回测入口。

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

## replay runtime

回测下的 target-pos 入口：

```rust
let runtime = session.runtime(["TQSIM"]).await?;
let account = runtime.account("TQSIM")?;
let task = account.target_pos("SHFE.rb2605").build()?;
```

最接近的仓库例子是：

- `examples/backtest.rs`
- `examples/pivot_point.rs`

## `finish()` 的结果

`finish()` 返回 `BacktestResult`，可读：

- `settlements`
- `final_accounts`
- `final_positions`
- `trades`

这是回测结束后的汇总结果，不是实时更新对象。

## 当前不要再推荐的旧写法

- `BacktestHandle`
- `switch_to_backtest()` / `switch_to_live()` 叙事
- 把 replay 说成 `Client` live 状态机的一部分
