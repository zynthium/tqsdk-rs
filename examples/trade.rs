//! 交易操作示例
//!
//! 演示以下功能：
//! - 实盘交易（可靠事件流）
//! - 账户 / 持仓快照查询
//! - `wait_order_update_reliable()` 的推荐用法
//!
//! 说明：
//! - 本示例聚焦底层 `TradeSession` 交易接口。
//! - 若需要目标持仓任务，请使用 `TqRuntime` 的 account builder：
//!   `runtime.account(...).target_pos(...).build()` 或 scheduler builder。

use std::env;
use std::time::Duration;
use tqsdk_rs::prelude::*;
use tracing::info;

async fn build_client(username: &str, password: &str, config: ClientConfig) -> Result<Client> {
    Client::builder(username, password)
        .config(config)
        .endpoints(EndpointConfig::from_env())
        .build()
        .await
}

fn example_duration() -> Duration {
    let secs = env::var("TQ_TRADE_EXAMPLE_DURATION_SECS")
        .ok()
        .and_then(|raw| raw.parse::<u64>().ok())
        .unwrap_or(300);
    Duration::from_secs(secs.max(1))
}

/// 使用可靠事件流的交易示例（实盘交易）
async fn trade_reliable_event_example() {
    info!("==================== 交易可靠事件流示例（实盘）====================");

    let username = env::var("TQ_AUTH_USER").expect("请设置 TQ_AUTH_USER 环境变量");
    let password = env::var("TQ_AUTH_PASS").expect("请设置 TQ_AUTH_PASS 环境变量");

    let sim_user_id = env::var("SIMNOW_USER_0").expect("请设置 SIMNOW_USER_0 环境变量");
    let sim_password = env::var("SIMNOW_PASS_0").expect("请设置 SIMNOW_PASS_0 环境变量");

    // 创建客户端
    let config = ClientConfig {
        log_level: "info".to_string(),
        ..Default::default()
    };
    let run_duration = example_duration();

    let client = build_client(&username, &password, config)
        .await
        .expect("创建客户端失败");

    // 创建交易会话（不自动连接）
    info!("创建交易会话: simnow, {}", sim_user_id);
    let trader = if let Ok(td_url) = env::var("TQ_TD_URL") {
        let td_url = td_url.trim().to_string();
        client
            .create_trade_session_with_options(
                "simnow",
                &sim_user_id,
                &sim_password,
                TradeSessionOptions {
                    td_url_override: (!td_url.is_empty()).then_some(td_url),
                    reliable_events_max_retained: 8_192,
                    use_sm: false,
                },
            )
            .await
    } else {
        client.create_trade_session("simnow", &sim_user_id, &sim_password).await
    }
    // .create_trade_session("X兴证期货", &sim_user_id, &sim_password)
    .expect("创建交易会话失败");

    let mut events = trader.subscribe_events();
    tokio::spawn(async move {
        while let Ok(event) = events.recv().await {
            match event.kind {
                TradeSessionEventKind::OrderUpdated { order_id, order } => {
                    info!(
                        "📝 订单 {}: {}.{} {} {}@{:.2}, 状态={}, 剩余={}",
                        order_id,
                        order.exchange_id,
                        order.instrument_id,
                        order.direction,
                        order.offset,
                        order.price(),
                        order.status,
                        order.volume_left
                    );
                }
                TradeSessionEventKind::TradeCreated { trade_id, trade } => {
                    info!(
                        "✅ 成交 {}: {}.{} {} {}@{:.2} x{}, 手续费={:.2}",
                        trade_id,
                        trade.exchange_id,
                        trade.instrument_id,
                        trade.direction,
                        trade.offset,
                        trade.price,
                        trade.volume,
                        trade.commission
                    );
                }
                TradeSessionEventKind::NotificationReceived { notification } => {
                    info!("🔔 通知: {}", notification.content);
                }
                TradeSessionEventKind::TransportError { message } => {
                    info!("⚠️ 异步错误: {}", message);
                }
                _ => {}
            }
        }
    });

    // 先建立事件消费路径，再连接交易服务器
    info!("连接交易服务器...");
    trader.connect().await.expect("连接失败");

    // 等待登录就绪
    info!("等待登录就绪...");
    trader.wait_ready().await.expect("交易会话未就绪");
    info!("✅ 已登录，交易就绪!");

    // 就绪后再启动 snapshot 监听，避免未连接时 wait_update() 误报未就绪错误。
    let snapshot_trader = trader.clone();
    tokio::spawn(async move {
        loop {
            match snapshot_trader.wait_update().await {
                Ok(()) => {
                    if let Ok(account) = snapshot_trader.get_account().await {
                        info!(
                            "💰 账户快照: 权益={:.2}, 可用={:.2}, 风险度={:.2}%",
                            account.balance,
                            account.available,
                            account.risk_ratio * 100.0
                        );
                    }

                    if let Ok(positions) = snapshot_trader.get_positions().await {
                        for (symbol, pos) in positions {
                            let total_long = pos.volume_long_today + pos.volume_long_his;
                            let total_short = pos.volume_short_today + pos.volume_short_his;
                            if total_long > 0 || total_short > 0 {
                                info!(
                                    "📊 {} 持仓快照: 多头={}, 空头={}, 浮动盈亏={:.2}",
                                    symbol, total_long, total_short, pos.float_profit
                                );
                            }
                        }
                    }
                }
                Err(err) => {
                    info!("交易快照监听结束: {}", err);
                    break;
                }
            }
        }
    });

    // 查询账户（同步方式）
    if let Ok(account) = trader.get_account().await {
        info!("\n当前账户信息:");
        info!("  权益: {:.2}", account.balance);
        info!("  可用: {:.2}", account.available);
        info!("  保证金: {:.2}", account.margin);
        info!("  风险度: {:.2}%", account.risk_ratio * 100.0);
    }

    // 查询持仓
    if let Ok(positions) = trader.get_positions().await
        && !positions.is_empty()
    {
        info!("\n当前持仓:");
        for (symbol, pos) in positions {
            let total_long = pos.volume_long_today + pos.volume_long_his;
            let total_short = pos.volume_short_today + pos.volume_short_his;
            if total_long > 0 || total_short > 0 {
                info!(
                    "  {}: 多={} 空={} 浮盈={:.2}",
                    symbol, total_long, total_short, pos.float_profit
                );
            }
        }
    }

    // 下单示例（注释掉，避免实际下单）
    /*
    info!("\n准备下单...");
    let order_id = trader.insert_order(&InsertOrderRequest {
        symbol: "SHFE.au2512".to_string(),
        exchange_id: None,
        instrument_id: None,
        direction: "BUY".to_string(),
        offset: "OPEN".to_string(),
        price_type: "LIMIT".to_string(),
        limit_price: 500.0,
        volume: 1,
    }).await;

    if let Ok(order_id) = order_id {
        info!("下单成功: {}", order_id);

        // 等待一会儿
        tokio::time::sleep(Duration::from_secs(2)).await;

        // 撤单
        info!("准备撤单 {}...", order_id);
        if let Ok(_) = trader.cancel_order(&order_id).await {
            info!("撤单成功!");
        } else {
            info!("撤单失败");
        }
    }
    */

    /*
    // 如果策略只关心某个订单后续是否发生了任何状态变化或成交，
    // 可以直接等待可靠事件而不是自己拼接 channel/select。
    trader.wait_order_update_reliable(&order_id).await.unwrap();
    */

    // 运行一段可配置时长，便于联调与 CI 验证
    info!("\n监听交易数据更新...");
    info!("示例将持续运行 {} 秒", run_duration.as_secs());
    tokio::time::sleep(run_duration).await;
    info!("可靠事件流示例结束\n");

    info!("开始关闭交易会话...");
    trader.close().await.ok();
    info!("交易会话已关闭");

    // 等待一下让所有后台任务完成
    tokio::time::sleep(Duration::from_millis(500)).await;
    info!("清理完成，准备退出");
}

#[tokio::main]
async fn main() {
    // 初始化日志
    tqsdk_rs::init_logger("debug", false);

    // 运行交易示例
    trade_reliable_event_example().await;

    info!("\n交易示例运行完成!");
}
