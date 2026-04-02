# 客户端与端点

当用户问下面这些问题时，优先读本文件：

- `Client` 和 `ClientBuilder` 怎么选
- `ClientConfig`、`EndpointConfig` 怎么配
- `TQ_AUTH_URL`、`TQ_MD_URL`、`TQ_TD_URL`、`TQ_INS_URL`、`TQ_CHINESE_HOLIDAY_URL` 有什么用
- 如何覆盖服务地址
- 为什么现在推荐 builder + endpoints 风格

## 推荐入口

```rust
use tqsdk_rs::{Client, ClientConfig, EndpointConfig};

let client = Client::builder(username, password)
    .config(ClientConfig::default())
    .endpoints(EndpointConfig::from_env())
    .build()
    .await?;
```

## 什么时候用什么

### `Client::builder`

优先推荐。适合：

- 显式配置 `ClientConfig`
- 覆盖端点
- 逐步讲清初始化顺序
- 给用户可扩展模板

### `Client::new`

便捷入口。适合：

- 用户只要最短可运行代码
- 不需要展示端点或更细配置

但如果用户问题涉及端点、环境变量、迁移旧写法、交易地址覆盖，优先切回 `Client::builder`。

## `ClientConfig`

常见字段：

- `log_level`
- `view_width`
- `development`
- `stock`
- `message_queue_capacity`
- `message_backlog_warn_step`
- `message_batch_max`

如果用户只是做普通行情或策略 demo，通常从 `ClientConfig::default()` 开始，再按需改字段。

## `EndpointConfig`

字段：

- `auth_url`
- `md_url`
- `td_url`
- `ins_url`
- `holiday_url`

### 推荐来源

```rust
let endpoints = EndpointConfig::from_env();
```

### 公开推荐环境变量

- `TQ_AUTH_URL`
- `TQ_MD_URL`
- `TQ_TD_URL`
- `TQ_INS_URL`
- `TQ_CHINESE_HOLIDAY_URL`

### 不再推荐的旧 auth 内部变量

不要把下面这些讲成正式公开配置：

- `TQ_NS_URL`
- `TQ_CLIENT_ID`
- `TQ_CLIENT_SECRET`
- `TQ_AUTH_PROXY`
- `TQ_AUTH_NO_PROXY`
- `TQ_AUTH_VERIFY_JWT`

## 端点覆盖优先级

### 行情 / 合约 / 假期数据

```text
builder 显式设置
> EndpointConfig::from_env()
> 默认值
```

### 交易地址

```text
TradeSessionOptions.td_url_override
> ClientBuilder::td_url / EndpointConfig.td_url
> TQ_TD_URL
> 鉴权返回的默认交易地址
```

## 常见问法的回答方向

### “怎么切换测试环境 / 私有服务地址？”

先给 `ClientBuilder::{auth_url, md_url, td_url, ins_url, holiday_url}` 或 `EndpointConfig::from_env()`。

### “环境变量应该怎么设？”

优先讲公开端点变量，不要展开讲 auth 内部实现细节。

### “旧代码里都是 `Client::new`，现在怎么迁移？”

先给 builder 写法，再强调：

```rust
Client::builder(user, pass)
    .config(config)
    .endpoints(EndpointConfig::from_env())
    .build()
    .await?;
```
