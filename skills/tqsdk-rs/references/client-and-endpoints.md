# 客户端与端点

当用户问 `Client`、`ClientBuilder`、端点覆盖、初始化顺序或 runtime 构造时，读本文件。

## 推荐的 live 客户端构造

`Client` 本身表示一次 live session owner，对齐 Python `TqApi` 的会话模型。`ClientBuilder` / `EndpointConfig` / auth 参数只负责构造 session；不要把 `Client` 当成可在运行时反复替换账号和复用行情状态的配置根。

```rust
use tqsdk_rs::{Client, ClientConfig, EndpointConfig};

let mut client = Client::builder(username, password)
    .config(ClientConfig {
        log_level: "info".to_string(),
        view_width: 10_000,
        ..Default::default()
    })
    .endpoints(EndpointConfig::from_env())
    .build()
    .await?;

client.init_market().await?;
```

如果用户没有特别要求，优先推荐 builder 风格，而不是直接把 `Client::new(...)` 当主写法。

## `ClientConfig` 里值得提的项

- `log_level`
- `view_width`
- `development`
- `stock`
- `message_queue_capacity`
- `message_backlog_warn_step`
- `message_batch_max`
- `series_disk_cache_enabled`
- `series_disk_cache_max_bytes`
- `series_disk_cache_retention_days`

如果用户只做普通 live 行情，通常只需要 `log_level` 与 `view_width`。

## `init_market()` 的规则

- `subscribe_quote()`、`quote()` 的有效更新
- `get_kline_serial()` / `get_tick_serial()`
- `kline_ref()` / `tick_ref()`
- `query_*()`、`get_trading_calendar()`、`get_trading_status()`

这些 live market / query 能力都应在 `init_market()` 之后使用。

## 切换账号

切换账号时关闭旧 `Client`，再用新账号创建新 `Client`。不要推荐 `set_auth() + init_market()` 作为运行时切账号方式。

如果用户追问 `set_auth()` 是否还能用：它只适合 live 尚未初始化的 `Client`；active 或已关闭的 session 会返回错误。
如果用户追问 `switch_to_live()`：把它讲成 market mode 切换接口，而不是 re-auth / 切账号接口；当前实现会替换整个 private live context。

```rust
client.close().await?;

let mut client = Client::builder("user2", "pass2")
    .build()
    .await?;
client.init_market().await?;
```

## 当前公开端点配置

优先讲 `EndpointConfig` 与 `ClientBuilder` 的端点方法，不要把 auth 内部细节当公开推荐路径。

- `TQ_AUTH_URL`
- `TQ_MD_URL`
- `TQ_TD_URL`
- `TQ_INS_URL`
- `TQ_CHINESE_HOLIDAY_URL`

常见写法：

```rust
let endpoints = EndpointConfig::from_env();

let client = Client::builder(username, password)
    .endpoints(endpoints)
    .build()
    .await?;
```

也可以用 builder 单独覆盖：

- `.auth_url(...)`
- `.md_url(...)`
- `.td_url(...)`
- `.ins_url(...)`
- `.holiday_url(...)`

## 交易地址优先级

对 `TradeSession` 来说，交易地址优先级是：

```text
TradeSessionOptions.td_url_override
> ClientBuilder::td_url / EndpointConfig.td_url
> TQ_TD_URL
> 鉴权返回的默认交易地址
```

## `build_runtime()` 的当前语义

`ClientBuilder::build_runtime()` 只是：

```text
build() -> into_runtime()
```

所以 live 模式下若想直接拿到可用账户句柄，推荐先在 builder 上预配置交易会话：

```rust
use std::sync::Arc;
use tqsdk_rs::{Client, TradeSessionOptions, TqRuntime};

let runtime: Arc<TqRuntime> = Client::builder(username, password)
    .trade_session_with_options(
        "simnow",
        user_id,
        trade_password,
        TradeSessionOptions::default(),
    )
    .build_runtime()
    .await?;

let account = runtime.account("simnow:user_id")?;
```

如果没有预配置任何 trade session，`build_runtime()` 得到的是一个没有 live 账户句柄的 runtime 外壳。

## `into_runtime()` 什么时候用

当用户已经先创建了 `Client`，并通过 `create_trade_session*()` 配好了交易账户时：

```rust
let client = Client::builder(username, password).build().await?;
let _session = client
    .create_trade_session("simnow", user_id, trade_password)
    .await?;

let runtime = client.into_runtime();
let account = runtime.account("simnow:user_id")?;
```

设计约束：live runtime 必须复用该 `Client` session 的同一私有 live context，不能自建第二套行情 websocket 或 `MarketDataState`。
这也意味着关闭该 `Client` session 后，runtime market wait 路径会收到同一关闭信号，而不是继续挂在旧状态池上。

## `Client::close()` 的语义

`Client::close()` 会：

- 关闭 live market 相关资源
- 关闭当前 `Client` 上已跟踪的 `TradeSession`
- 让该 session 绑定的 `wait_update()` / `wait_update_and_drain()` / `QuoteRef::wait_update()` / `KlineRef::wait_update()` / `TickRef::wait_update()` 返回关闭错误
- 让该 `Client` session 进入终止态；如需重新进入 live 模式，应创建新 `Client`，不要对已关闭 session 再调用 `init_market()`

普通文档 / 问答里可以把它讲成“退出前显式释放资源”的收尾动作。
