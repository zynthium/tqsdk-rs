//! DataManager 高级功能示例
//!
//! 演示以下功能：
//! - 路径监听 (`watch_handle`)
//! - 数据访问 (Get/GetByPath)
//! - 多路径同时监听
//! - merge 完成后的 epoch 订阅

use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tqsdk_rs::{DataManager, DataManagerConfig};
use tracing::info;

/// DataManager Watch 功能示例
async fn watch_example() {
    info!("==================== DataManager Watch 示例 ====================");

    // 创建 DataManager
    let mut initial_data = HashMap::new();
    initial_data.insert("quotes".to_string(), json!({}));

    let config = DataManagerConfig::default();
    let dm = Arc::new(DataManager::new(initial_data, config));

    // 监听特定路径
    let mut watch_handle = dm.watch_handle(vec!["quotes".to_string(), "SHFE.au2512".to_string()]);
    let watch_rx = watch_handle.receiver().clone();

    // 启动 tokio task 接收数据
    tokio::spawn(async move {
        let rx = watch_rx;
        while let Ok(data) = rx.recv().await {
            if let Value::Object(quote_map) = data {
                info!(
                    "📊 Quote 更新: 最新价={:.2}, 成交量={}",
                    quote_map.get("last_price").and_then(|v| v.as_f64()).unwrap_or(0.0),
                    quote_map.get("volume").and_then(|v| v.as_i64()).unwrap_or(0)
                );
            }
        }
    });

    // 模拟数据更新
    info!("模拟数据更新...");
    tokio::time::sleep(Duration::from_millis(500)).await;

    dm.merge_data(
        json!({
            "quotes": {
                "SHFE.au2512": {
                    "last_price": 500.0,
                    "volume": 1000
                }
            }
        }),
        true,
        false,
    );

    tokio::time::sleep(Duration::from_millis(100)).await;

    // 更新数据
    dm.merge_data(
        json!({
            "quotes": {
                "SHFE.au2512": {
                    "last_price": 501.5,
                    "volume": 1200
                }
            }
        }),
        true,
        false,
    );

    tokio::time::sleep(Duration::from_millis(100)).await;

    // 取消监听
    info!("取消监听...");
    watch_handle.cancel();

    info!("Watch 示例结束");
}

/// 数据访问示例
async fn data_access_example() {
    info!("==================== 数据访问示例 ====================");

    let mut initial_data = HashMap::new();
    initial_data.insert(
        "quotes".to_string(),
        json!({
            "SHFE.au2512": {
                "last_price": 500.0,
                "volume": 1000
            }
        }),
    );

    let config = DataManagerConfig::default();
    let dm = DataManager::new(initial_data, config);

    // 使用 get_by_path 方法
    if let Some(data) = dm.get_by_path(&["quotes", "SHFE.au2512"]) {
        info!("GetByPath 成功: {:?}", data);
    }

    // 访问不存在的路径
    if dm.get_by_path(&["quotes", "INVALID"]).is_none() {
        info!("预期的行为: 路径不存在返回 None");
    }

    // 检查数据是否在变化
    if dm.is_changing(&["quotes", "SHFE.au2512"]) {
        info!("数据在最近一次更新中发生了变化");
    }

    // 获取当前 epoch
    let epoch = dm.get_epoch();
    info!("当前 epoch: {}", epoch);

    info!("数据访问示例结束");
}

/// 多路径监听示例
async fn multi_watch_example() {
    info!("==================== 多路径监听示例 ====================");

    let mut initial_data = HashMap::new();
    initial_data.insert("quotes".to_string(), json!({}));

    let config = DataManagerConfig::default();
    let dm = Arc::new(DataManager::new(initial_data, config));

    // 监听多个路径
    let symbols = vec!["SHFE.au2512", "SHFE.ag2512", "DCE.m2505"];
    let mut watchers = Vec::new();

    for symbol in &symbols {
        let handle = dm.watch_handle(vec!["quotes".to_string(), symbol.to_string()]);
        let rx = handle.receiver().clone();
        watchers.push(handle);
        tokio::spawn({
            let symbol = symbol.to_string();
            async move {
                while let Ok(data) = rx.recv().await {
                    if let Value::Object(quote_map) = data {
                        info!(
                            "📊 {} 更新: {:.2}",
                            symbol,
                            quote_map.get("last_price").and_then(|v| v.as_f64()).unwrap_or(0.0)
                        );
                    }
                }
            }
        });
    }

    // 模拟批量更新
    tokio::time::sleep(Duration::from_millis(500)).await;
    dm.merge_data(
        json!({
            "quotes": {
                "SHFE.au2512": {"last_price": 500.0},
                "SHFE.ag2512": {"last_price": 50.0},
                "DCE.m2505": {"last_price": 3000.0}
            }
        }),
        true,
        false,
    );

    tokio::time::sleep(Duration::from_millis(500)).await;

    // 清理
    for watcher in &mut watchers {
        watcher.cancel();
    }

    info!("多路径监听示例结束");
}

/// merge 完成后的 epoch 订阅示例
async fn epoch_subscription_example() {
    info!("==================== DataManager Epoch 示例 ====================");

    let mut initial_data = HashMap::new();
    initial_data.insert("quotes".to_string(), json!({}));

    let config = DataManagerConfig::default();
    let dm = Arc::new(DataManager::new(initial_data, config));
    let mut epoch_rx = dm.subscribe_epoch();

    let dm_for_task = Arc::clone(&dm);
    tokio::spawn(async move {
        while epoch_rx.changed().await.is_ok() {
            let epoch = *epoch_rx.borrow_and_update();
            info!("🔔 merge 完成，当前 epoch: {}", epoch);
            if let Some(data) = dm_for_task.get_by_path(&["quotes", "SHFE.au2512"]) {
                info!("   SHFE.au2512 数据: {:?}", data);
            }
        }
    });

    // 模拟数据更新
    tokio::time::sleep(Duration::from_millis(500)).await;
    info!("触发第一次数据更新...");
    dm.merge_data(
        json!({
            "quotes": {
                "SHFE.au2512": {
                    "last_price": 500.0,
                    "volume": 1000
                }
            }
        }),
        true,
        false,
    );

    tokio::time::sleep(Duration::from_millis(500)).await;
    info!("触发第二次数据更新...");
    dm.merge_data(
        json!({
            "quotes": {
                "SHFE.au2512": {
                    "last_price": 501.5,
                    "volume": 1200
                }
            }
        }),
        true,
        false,
    );

    tokio::time::sleep(Duration::from_millis(500)).await;

    info!("epoch 示例结束");
}

#[tokio::main]
async fn main() {
    // 初始化日志
    tracing_subscriber::fmt().with_max_level(tracing::Level::INFO).init();

    // 运行各个示例
    watch_example().await;
    tokio::time::sleep(Duration::from_secs(1)).await;

    data_access_example().await;
    tokio::time::sleep(Duration::from_secs(1)).await;

    multi_watch_example().await;
    tokio::time::sleep(Duration::from_secs(1)).await;

    epoch_subscription_example().await;

    info!("\n所有示例运行完成!");
}
