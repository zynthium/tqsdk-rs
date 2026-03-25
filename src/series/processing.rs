use crate::datamanager::DataManager;
use crate::errors::{Result, TqError};
use crate::types::{ChartInfo, SeriesData, SeriesOptions, UpdateInfo};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::trace;

/// 处理 Series 更新
#[expect(
    clippy::too_many_arguments,
    reason = "序列更新计算需要共享多份增量状态"
)]
pub(super) async fn process_series_update(
    dm: &DataManager,
    options: &SeriesOptions,
    last_ids: &Arc<RwLock<HashMap<String, i64>>>,
    last_left_id: &Arc<RwLock<i64>>,
    last_right_id: &Arc<RwLock<i64>>,
    chart_ready: &Arc<RwLock<bool>>,
    has_chart_sync: &Arc<RwLock<bool>>,
    last_epoch: i64,
) -> Result<(SeriesData, UpdateInfo)> {
    let is_multi = options.symbols.len() > 1;
    let is_tick = options.duration == 0;

    let series_data: SeriesData = if is_tick {
        get_tick_data(dm, options).await?
    } else if is_multi {
        get_multi_kline_data(dm, options).await?
    } else {
        get_single_kline_data(dm, options).await?
    };

    let mut update_info = UpdateInfo {
        has_new_bar: false,
        has_bar_update: false,
        chart_range_changed: false,
        has_chart_sync: false,
        chart_ready: false,
        new_bar_ids: HashMap::new(),
        old_left_id: 0,
        old_right_id: 0,
        new_left_id: 0,
        new_right_id: 0,
    };

    detect_new_bars(dm, &series_data, last_ids, &mut update_info, last_epoch).await;

    detect_chart_range_change(
        dm,
        &series_data,
        last_left_id,
        last_right_id,
        chart_ready,
        has_chart_sync,
        &mut update_info,
    )
    .await;

    Ok((series_data, update_info))
}

async fn get_single_kline_data(dm: &DataManager, options: &SeriesOptions) -> Result<SeriesData> {
    let symbol = &options.symbols[0];
    let chart_id = options
        .chart_id
        .as_deref()
        .ok_or_else(|| TqError::InternalError("SeriesOptions.chart_id 为空".to_string()))?;

    let mut right_id = -1i64;
    let chart_info = dm
        .get_by_path(&["charts", chart_id])
        .and_then(|chart_data| dm.convert_to_struct::<ChartInfo>(&chart_data).ok())
        .map(|mut chart| {
            right_id = chart.right_id;
            chart.chart_id = chart_id.to_string();
            chart.view_width = options.view_width;
            chart
        });
    let mut kline_data =
        dm.get_klines_data(symbol, options.duration, options.view_width, right_id)?;

    kline_data.chart_id = chart_id.to_string();
    kline_data.chart = chart_info;

    Ok(SeriesData {
        is_multi: false,
        is_tick: false,
        symbols: vec![symbol.clone()],
        single: Some(kline_data),
        multi: None,
        tick_data: None,
    })
}

async fn get_multi_kline_data(dm: &DataManager, options: &SeriesOptions) -> Result<SeriesData> {
    let chart_id = options
        .chart_id
        .as_deref()
        .ok_or_else(|| TqError::InternalError("SeriesOptions.chart_id 为空".to_string()))?;
    let multi_data = dm.get_multi_klines_data(
        &options.symbols,
        options.duration,
        chart_id,
        options.view_width,
    )?;

    Ok(SeriesData {
        is_multi: true,
        is_tick: false,
        symbols: options.symbols.clone(),
        single: None,
        multi: Some(multi_data),
        tick_data: None,
    })
}

async fn get_tick_data(dm: &DataManager, options: &SeriesOptions) -> Result<SeriesData> {
    let symbol = &options.symbols[0];
    let chart_id = options
        .chart_id
        .as_deref()
        .ok_or_else(|| TqError::InternalError("SeriesOptions.chart_id 为空".to_string()))?;

    let mut right_id = -1i64;
    let chart_info = dm
        .get_by_path(&["charts", chart_id])
        .and_then(|chart_data| dm.convert_to_struct::<ChartInfo>(&chart_data).ok())
        .map(|mut chart| {
            right_id = chart.right_id;
            chart.chart_id = chart_id.to_string();
            chart.view_width = options.view_width;
            chart
        });

    let mut tick_data = dm.get_ticks_data(symbol, options.view_width, right_id)?;

    tick_data.chart_id = chart_id.to_string();
    tick_data.chart = chart_info;

    Ok(SeriesData {
        is_multi: false,
        is_tick: true,
        symbols: vec![symbol.clone()],
        single: None,
        multi: None,
        tick_data: Some(tick_data),
    })
}

async fn detect_new_bars(
    dm: &DataManager,
    data: &SeriesData,
    last_ids: &Arc<RwLock<HashMap<String, i64>>>,
    info: &mut UpdateInfo,
    last_epoch: i64,
) {
    let mut ids = last_ids.write().await;

    let duration_str = if data.is_tick {
        String::new()
    } else if data.is_multi {
        data.multi
            .as_ref()
            .map(|m| m.duration.to_string())
            .unwrap_or_default()
    } else {
        data.single
            .as_ref()
            .map(|s| s.duration.to_string())
            .unwrap_or_default()
    };

    for symbol in &data.symbols {
        let current_id = if data.is_tick {
            data.tick_data.as_ref().map(|t| t.last_id).unwrap_or(-1)
        } else if data.is_multi {
            data.multi
                .as_ref()
                .and_then(|m| m.metadata.get(symbol))
                .map(|meta| meta.last_id)
                .unwrap_or(-1)
        } else {
            data.single.as_ref().map(|s| s.last_id).unwrap_or(-1)
        };
        let last_id = ids.get(symbol).copied().unwrap_or(-1);
        trace!("current_id = {}, last_id = {}", current_id, last_id);
        if current_id > last_id {
            info.has_new_bar = true;
            info.has_bar_update = true;
            info.new_bar_ids.insert(symbol.clone(), current_id);
        }
        ids.insert(symbol.clone(), current_id);
    }
    if !info.has_new_bar && !data.is_tick {
        for symbol in &data.symbols {
            if dm.get_path_epoch(&["klines", symbol, &duration_str]) > last_epoch {
                info.has_bar_update = true;
                break;
            }
        }
    }
}

async fn detect_chart_range_change(
    dm: &DataManager,
    data: &SeriesData,
    last_left_id: &Arc<RwLock<i64>>,
    last_right_id: &Arc<RwLock<i64>>,
    chart_ready: &Arc<RwLock<bool>>,
    has_chart_sync: &Arc<RwLock<bool>>,
    info: &mut UpdateInfo,
) {
    let chart: Option<&ChartInfo> = if let Some(single) = &data.single {
        single.chart.as_ref()
    } else if let Some(tick) = &data.tick_data {
        tick.chart.as_ref()
    } else {
        None
    };

    let multi_chart = if let Some(multi) = &data.multi {
        if let Some(chart_data) = dm.get_by_path(&["charts", &multi.chart_id]) {
            dm.convert_to_struct::<ChartInfo>(&chart_data)
                .ok()
                .map(|mut c| {
                    c.chart_id = multi.chart_id.clone();
                    c.view_width = multi.view_width;
                    c
                })
        } else {
            None
        }
    } else {
        None
    };

    if let Some(chart) = chart.or(multi_chart.as_ref()) {
        let mut last_left = last_left_id.write().await;
        let mut last_right = last_right_id.write().await;
        let mut ready = chart_ready.write().await;
        let mut has_sync = has_chart_sync.write().await;

        trace!(
            "before compare -> last_left: {}, last_right: {}, chart.left_id: {}, chart.right_id: {}, ready: {}, chart.ready: {}, has_sync: {}",
            *last_left, *last_right, chart.left_id, chart.right_id, *ready, chart.ready, *has_sync,
        );
        if chart.left_id != *last_left || chart.right_id != *last_right {
            info.chart_range_changed = true;
            info.old_left_id = *last_left;
            info.old_right_id = *last_right;
            info.new_left_id = chart.left_id;
            info.new_right_id = chart.right_id;

            *last_left = chart.left_id;
            *last_right = chart.right_id;
        }

        if chart.ready && !*ready {
            *ready = true;
            *has_sync = true;

            info.has_chart_sync = true;
            info.has_bar_update = true;
            info.has_new_bar = true;
        }

        if chart.ready && !chart.more_data {
            info.chart_ready = true;
        }
        trace!(
            "after compare -> last_left: {}, last_right: {}, chart.left_id: {}, chart.right_id: {}, ready: {}, chart.ready: {}, has_sync: {}",
            *last_left, *last_right, chart.left_id, chart.right_id, *ready, chart.ready, *has_sync,
        );
        info.has_chart_sync = *has_sync
    }
}
