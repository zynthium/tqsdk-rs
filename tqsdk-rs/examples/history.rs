//! 历史数据订阅示例
//!
//! 演示以下功能：
//! - 使用 left_kline_id 订阅历史 K线
//! - 使用 focus_datetime + focus_position 订阅历史 K线
//! - 大数据量分片传输监控

use chrono::NaiveDate;
use std::env;
use std::time::Duration;
use tqsdk_rs::*;
use tracing::info;

/// 使用 left_kline_id 订阅历史 K线
async fn history_kline_with_left_id_example() {
    info!("==================== 历史 K线订阅示例（使用 left_kline_id） ====================");

    let username = env::var("SHINNYTECH_ID").expect("请设置 SHINNYTECH_ID 环境变量");
    let password = env::var("SHINNYTECH_PW").expect("请设置 SHINNYTECH_PW 环境变量");

    let mut client = Client::builder(&username, &password)
        .log_level("info")
        .view_width(1000000)
        .build()
        .await
        .expect("创建客户端失败");

    client.init_market().await.expect("初始化行情功能失败");

    // 从指定的 K线 ID 开始订阅 8000 根历史 K线
    // 注意：数据会分片返回（每片最多 3000 根）
    let left_kline_id = 105761i64;
    let series_api = client.series().expect("获取 SeriesAPI 失败");
    let sub = series_api
        .kline_history("SHFE.au2512", Duration::from_secs(60), 8000, left_kline_id)
        .await
        .expect("订阅失败");

    // 监听数据更新
    sub.on_update(|data, info| {
        if let Some(sym_data) = data.get_symbol_klines("SHFE.au2512") {
            if info.has_chart_sync {
                info!("✅ Chart 初次同步完成");
                if let Some(single) = &data.single {
                    if let Some(chart) = &single.chart {
                        info!(
                            "   范围: [{}, {}]",
                            chart.left_id, chart.right_id
                        );
                    }
                    info!("   数据量: {} 根K线", sym_data.data.len());
                    if let Some(first) = sym_data.data.first() {
                        info!("   第一根 Bar: {:?}", first);
                    }
                }
            }

            if info.chart_range_changed {
                info!(
                    "📊 Chart 范围变化: [{},{}] -> [{},{}]",
                    info.old_left_id,
                    info.old_right_id,
                    info.new_left_id,
                    info.new_right_id
                );
                info!("   当前数据量: {} 根K线", sym_data.data.len());
            }

            // 检测分片数据传输完成
            if info.chart_ready {
                info!("\n🎉 所有历史数据传输完成！");
                if let Some(single) = &data.single {
                    if let Some(chart) = &single.chart {
                        info!(
                            "   最终范围: [{}, {}]",
                            chart.left_id, chart.right_id
                        );
                        info!("   总数据量: {} 根K线", sym_data.data.len());
                        info!(
                            "   Chart More Data: {}, Ready: {}",
                            chart.more_data, chart.ready
                        );

                        // 验证数据范围是否正确
                        if let (Some(first), Some(last)) = (sym_data.data.first(), sym_data.data.last())
                        {
                            info!("\n数据范围验证:");
                            info!(
                                "   首根 K线 ID: {} (应该 >= left_id: {})",
                                first.id, chart.left_id
                            );
                            info!(
                                "   末根 K线 ID: {} (应该 <= right_id: {})",
                                last.id, chart.right_id
                            );

                            if last.id <= chart.right_id && first.id >= chart.left_id {
                                info!("   ✓ 数据范围正确！");
                            } else {
                                info!("   ❌ 数据范围异常！");
                            }
                        }
                    }

                    // 显示前几根和后几根K线
                    if !sym_data.data.is_empty() {
                        info!("\n前3根K线:");
                        for k in sym_data.data.iter().take(3) {
                            let dt = chrono::DateTime::from_timestamp(
                                k.datetime / 1_000_000_000,
                                (k.datetime % 1_000_000_000) as u32,
                            )
                            .unwrap();
                            info!(
                                "  [{}] {} O:{:.2} H:{:.2} L:{:.2} C:{:.2} V:{}",
                                k.id,
                                dt.format("%Y-%m-%d %H:%M:%S"),
                                k.open,
                                k.high,
                                k.low,
                                k.close,
                                k.volume
                            );
                        }

                        info!("\n后3根K线:");
                        let start = if sym_data.data.len() > 3 {
                            sym_data.data.len() - 3
                        } else {
                            0
                        };
                        for k in &sym_data.data[start..] {
                            let dt = chrono::DateTime::from_timestamp(
                                k.datetime / 1_000_000_000,
                                (k.datetime % 1_000_000_000) as u32,
                            )
                            .unwrap();
                            info!(
                                "  [{}] {} O:{:.2} H:{:.2} L:{:.2} C:{:.2} V:{}",
                                k.id,
                                dt.format("%Y-%m-%d %H:%M:%S"),
                                k.open,
                                k.high,
                                k.low,
                                k.close,
                                k.volume
                            );
                        }
                    }
                }
            }
        }
    })
    .await;

    // 等待数据传输完成
    tokio::time::sleep(Duration::from_secs(30)).await;
    info!("\n历史 K线订阅示例结束");
}

/// 使用 focus_datetime + focus_position 订阅历史 K线
async fn history_kline_with_focus_example() {
    info!("==================== 历史 K线订阅示例（使用 focus_datetime） ====================");

    let username = env::var("SHINNYTECH_ID").expect("请设置 SHINNYTECH_ID 环境变量");
    let password = env::var("SHINNYTECH_PW").expect("请设置 SHINNYTECH_PW 环境变量");

    let mut client = Client::builder(&username, &password)
        .log_level("info")
        .view_width(500)
        .build()
        .await
        .expect("创建客户端失败");

    client.init_market().await.expect("初始化行情功能失败");

    // 从指定时间点开始订阅（focus_position=1 表示从该时间向右扩展）
    let focus_time = NaiveDate::from_ymd_opt(2025, 9, 1)
        .unwrap()
        .and_hms_opt(0, 0, 0)
        .unwrap()
        .and_utc();

    let series_api = client.series().expect("获取 SeriesAPI 失败");
    let sub = series_api
        .kline_history_with_focus("SHFE.au2512", Duration::from_secs(60), 8000, focus_time, 1)
        .await
        .expect("订阅失败");

    sub.on_update(move |data, info| {
        if info.chart_ready {
            if let Some(sym_data) = data.get_symbol_klines("SHFE.au2512") {
                info!("\n🎉 历史数据传输完成！");
                info!("   焦点时间: {}", focus_time.format("%Y-%m-%d %H:%M:%S"));

                if let Some(single) = &data.single {
                    if let Some(chart) = &single.chart {
                        info!(
                            "   范围: [{}, {}]",
                            chart.left_id, chart.right_id
                        );
                    }
                    info!("   数据量: {} 根K线", sym_data.data.len());
                }
            }
        }
    })
    .await;

    tokio::time::sleep(Duration::from_secs(30)).await;
    info!("\n历史 K线订阅示例结束");
}

#[tokio::main]
async fn main() {
    // 运行历史数据订阅示例
    history_kline_with_left_id_example().await;
    // history_kline_with_focus_example().await;

    info!("\n所有示例运行完成!");
}
