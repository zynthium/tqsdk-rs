# tqsdk-rs API 地图

当用户只要一个当前版本的总览时，先读本文件。

## 当前四条主路径

1. `Client`
   live 行情、序列、合约查询的统一入口。
2. `TradeSession`
   手工交易链路，负责连接交易前置、读取交易快照、消费可靠事件流。
3. `ReplaySession`
   回测 / 回放入口，负责注册 replay 句柄、推进时间、产出最终结果。
4. `TqRuntime`
   目标持仓和 scheduler 运行时，只在用户明确问 target-pos / runtime 时展开。

## 当前公开 API 地图

| 任务 | 当前 canonical API | 备注 | 例子 |
|------|--------------------|------|------|
| live Quote | `init_market()` -> `subscribe_quote()` -> `quote()` -> `wait_update()` / `load()` | 订阅 auto-start | `examples/quote.rs` |
| live 最新 K 线 / Tick | `kline_ref()` / `tick_ref()` | 读取 latest state | `examples/quote.rs` |
| live 窗口序列 | `kline()` / `tick()` -> `SeriesSubscription::wait_update()` / `load()` | auto-start，coalesced snapshot | `examples/quote.rs` |
| 一次性历史快照 | `get_kline_data_series()` / `get_tick_data_series()` | 语义为 `[start_dt, end_dt)` | `examples/history.rs` |
| 长时间历史导出 | `spawn_data_downloader*()` | 后台下载任务，不是实时订阅 | `src/download/` |
| 合约 / 期权 / 参考数据 | `query_*()`、`get_trading_calendar()`、`get_trading_status()` | 走 `Client` facade，不是 `InsAPI` 主路径 | `examples/option_levels.rs` |
| 手工交易 | `create_trade_session*()` -> `wait_update()` / getter + `subscribe_*events()` -> `connect()` | 状态 vs 事件分层 | `examples/trade.rs` |
| live target-pos | `ClientBuilder::trade_session*().build_runtime()` 或 `client.into_runtime()` | `runtime.account("broker:user")` | `README.md` |
| replay / backtest | `create_backtest_session()` -> `ReplaySession::{quote,kline,tick,aligned_kline,step,finish}` | `step()` 是唯一时间推进 | `examples/backtest.rs` |
| replay target-pos | `ReplaySession::runtime([account])` -> `runtime.account(account)` | backtest runtime | `examples/backtest.rs`, `examples/pivot_point.rs` |
| DIFF 状态调试 | `DataManager`、`subscribe_epoch()`、`get_path_epoch()` | 高级 / 内部导向 | `examples/datamanager.rs` |

## 当前推荐初始化顺序

### live 行情 / 查询

```text
Client::builder -> build -> init_market
  -> subscribe_quote / quote
  -> kline / tick / kline_ref / tick_ref
  -> query_* / get_trading_calendar / get_trading_status
```

### 手工交易

```text
Client::builder -> build
  -> create_trade_session*
  -> 建立 wait_update / reliable event 消费路径
  -> connect
```

### live target-pos

```text
Client::builder
  -> trade_session* (预配置 live 账户)
  -> build_runtime
  -> runtime.account("broker:user")
  -> target_pos / target_pos_scheduler
```

或：

```text
Client::builder -> build
  -> create_trade_session*
  -> into_runtime
  -> runtime.account("broker:user")
```

### replay / backtest

```text
Client::builder -> build
  -> create_backtest_session(ReplayConfig)
  -> quote / kline / tick / aligned_kline
  -> 可选 runtime([account])
  -> step() 循环
  -> finish()
```

## 什么时候不要切错路径

- 用户只问 Quote / K 线 / Tick：不要默认切到 `TqRuntime` 或 `TradeSession`
- 用户只问合约查询 / 期权筛选：不要展开 `InsAPI` 内部结构，直接给 `Client::query_*()`
- 用户问回测：不要再讲 `BacktestHandle` 或 `init_market_backtest()`
- 用户问 target-pos：不要说这是 `TradeSession` 上的方法
- 用户问“仓库现在的公开入口是什么”：先回答 `Client` / `TradeSession` / `ReplaySession` / `TqRuntime`

## 明确不要再用的旧写法

- `Client::tqapi()`
- `Client::series()`
- `Client::ins()`
- `QuoteSubscription::start()`
- `SeriesSubscription::start()`
- `QuoteSubscription::on_quote()` / `quote_channel()`
- `SeriesSubscription::on_update()` / `on_new_bar()` / `data_stream()`
- `BacktestHandle`
- `TargetPosTask::new(...)` / `TargetPosScheduler::new(...)` 作为默认答案
- `TradeSession` 的 snapshot callback / best-effort channel 叙事

## 仓库模块定位

- live facade：`src/client/`
- Quote 生命周期：`src/quote/`
- live K 线 / Tick / 历史下载：`src/series/`
- 合约与参考数据：`src/ins/`
- 交易：`src/trade_session/`
- replay / backtest：`src/replay/`
- runtime / target-pos：`src/runtime/`
- DIFF 状态中心：`src/datamanager/`
