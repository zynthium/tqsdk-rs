use super::InsAPI;
use crate::errors::{Result, TqError};
use crate::types::{EdbIndexData, SymbolRanking, SymbolSettlement, TradingCalendarDay};
use crate::utils::{fetch_json_with_headers, split_symbol, value_to_f64};
use chrono::Duration as ChronoDuration;
use chrono::NaiveDate;
use chrono::Utc;
use reqwest::Client;
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};

impl InsAPI {
    /// 查询结算价数据
    ///
    /// `days` 控制返回的交易日数量，`start_dt` 可指定起始日期。
    pub async fn query_symbol_settlement(
        &self,
        symbols: &[&str],
        days: i32,
        start_dt: Option<NaiveDate>,
    ) -> Result<Vec<SymbolSettlement>> {
        if symbols.is_empty() {
            return Err(TqError::InvalidParameter("symbol 不能为空列表".to_string()));
        }
        if symbols.iter().any(|s| s.is_empty()) {
            return Err(TqError::InvalidParameter("symbol 参数中不能有空字符串".to_string()));
        }
        if days < 1 {
            return Err(TqError::InvalidParameter(format!("days 参数 {} 错误。", days)));
        }
        let mut query_days = days;
        let mut url = url::Url::parse("https://md-settlement-system-fc-api.shinnytech.com/mss")
            .map_err(|e| TqError::InternalError(format!("URL 解析失败: {}", e)))?;
        let symbols_str = symbols.join(",");
        let mut params = vec![("symbols", symbols_str), ("days", query_days.to_string())];
        if let Some(dt) = start_dt {
            params.push(("start_date", dt.format("%Y%m%d").to_string()));
        } else {
            query_days += 1;
            params[1].1 = query_days.to_string();
        }
        url.query_pairs_mut().extend_pairs(params);

        let headers = self.auth.read().await.base_header();
        let content = fetch_json_with_headers(url.as_str(), headers).await?;
        let obj = content
            .as_object()
            .ok_or_else(|| TqError::ParseError("结算价数据格式错误，应为对象".to_string()))?;

        let mut dates: Vec<String> = obj.keys().cloned().collect();
        dates.sort_by(|a, b| b.cmp(a));
        let mut rows: Vec<SymbolSettlement> = Vec::new();
        for date in dates.into_iter().take(days as usize) {
            if let Some(symbols_map) = obj.get(&date).and_then(|v| v.as_object()) {
                for (symbol, settlement_val) in symbols_map.iter() {
                    if settlement_val.is_null() {
                        continue;
                    }
                    let settlement = value_to_f64(settlement_val);
                    rows.push(SymbolSettlement {
                        datetime: date.clone(),
                        symbol: symbol.clone(),
                        settlement,
                    });
                }
            }
        }

        rows.sort_by(|a, b| {
            let dt_cmp = a.datetime.cmp(&b.datetime);
            if dt_cmp == std::cmp::Ordering::Equal {
                a.symbol.cmp(&b.symbol)
            } else {
                dt_cmp
            }
        });
        Ok(rows)
    }

    /// 查询持仓排名数据
    ///
    /// `ranking_type` 支持 VOLUME/LONG/SHORT。
    pub async fn query_symbol_ranking(
        &self,
        symbol: &str,
        ranking_type: &str,
        days: i32,
        start_dt: Option<NaiveDate>,
        broker: Option<&str>,
    ) -> Result<Vec<SymbolRanking>> {
        if symbol.is_empty() {
            return Err(TqError::InvalidParameter(
                "symbol 参数应该填入有效的合约代码。".to_string(),
            ));
        }
        if !matches!(ranking_type, "VOLUME" | "LONG" | "SHORT") {
            return Err(TqError::InvalidParameter(
                "ranking_type 参数只支持以下值： 'VOLUME', 'LONG', 'SHORT'。".to_string(),
            ));
        }
        if days < 1 {
            return Err(TqError::InvalidParameter(format!("days 参数 {} 错误。", days)));
        }
        let mut url = url::Url::parse("https://symbol-ranking-system-fc-api.shinnytech.com/srs")
            .map_err(|e| TqError::InternalError(format!("URL 解析失败: {}", e)))?;
        let mut params = vec![("symbol", symbol.to_string()), ("days", days.to_string())];
        if let Some(dt) = start_dt {
            params.push(("start_date", dt.format("%Y%m%d").to_string()));
        }
        if let Some(broker) = broker {
            if broker.is_empty() {
                return Err(TqError::InvalidParameter("broker 不能为空字符串".to_string()));
            }
            params.push(("broker", broker.to_string()));
        }
        url.query_pairs_mut().extend_pairs(params);

        let headers = self.auth.read().await.base_header();
        let content = fetch_json_with_headers(url.as_str(), headers).await?;
        let obj = content
            .as_object()
            .ok_or_else(|| TqError::ParseError("持仓排名数据格式错误，应为对象".to_string()))?;

        let mut rows: HashMap<String, SymbolRanking> = HashMap::new();
        for (dt, symbols_val) in obj.iter() {
            let symbols_map = match symbols_val.as_object() {
                Some(map) => map,
                None => continue,
            };
            for (symbol_key, data_val) in symbols_map.iter() {
                if data_val.is_null() {
                    continue;
                }
                let data_map = match data_val.as_object() {
                    Some(map) => map,
                    None => continue,
                };
                for (data_type, broker_val) in data_map.iter() {
                    let broker_map = match broker_val.as_object() {
                        Some(map) => map,
                        None => continue,
                    };
                    for (broker_name, rank_val) in broker_map.iter() {
                        let rank_map = match rank_val.as_object() {
                            Some(map) => map,
                            None => continue,
                        };
                        let key = format!("{}|{}|{}", dt, symbol_key, broker_name);
                        let entry = rows.entry(key).or_insert_with(|| {
                            let (exchange_id, instrument_id) = split_symbol(symbol_key);
                            SymbolRanking {
                                datetime: dt.clone(),
                                symbol: symbol_key.clone(),
                                exchange_id: exchange_id.to_string(),
                                instrument_id: instrument_id.to_string(),
                                broker: broker_name.clone(),
                                volume: f64::NAN,
                                volume_change: f64::NAN,
                                volume_ranking: f64::NAN,
                                long_oi: f64::NAN,
                                long_change: f64::NAN,
                                long_ranking: f64::NAN,
                                short_oi: f64::NAN,
                                short_change: f64::NAN,
                                short_ranking: f64::NAN,
                            }
                        });

                        let volume = rank_map.get("volume").map(value_to_f64).unwrap_or(f64::NAN);
                        let varvolume = rank_map.get("varvolume").map(value_to_f64).unwrap_or(f64::NAN);
                        let ranking = rank_map.get("ranking").map(value_to_f64).unwrap_or(f64::NAN);

                        match data_type.as_str() {
                            "volume_ranking" => {
                                entry.volume = volume;
                                entry.volume_change = varvolume;
                                entry.volume_ranking = ranking;
                            }
                            "long_ranking" => {
                                entry.long_oi = volume;
                                entry.long_change = varvolume;
                                entry.long_ranking = ranking;
                            }
                            "short_ranking" => {
                                entry.short_oi = volume;
                                entry.short_change = varvolume;
                                entry.short_ranking = ranking;
                            }
                            _ => {}
                        }
                    }
                }
            }
        }

        let rank_field = match ranking_type {
            "VOLUME" => "volume_ranking",
            "LONG" => "long_ranking",
            _ => "short_ranking",
        };
        let mut list: Vec<SymbolRanking> = rows.into_values().collect();
        list.retain(|item| {
            let value = match rank_field {
                "volume_ranking" => item.volume_ranking,
                "long_ranking" => item.long_ranking,
                _ => item.short_ranking,
            };
            !value.is_nan()
        });
        list.sort_by(|a, b| {
            let dt_cmp = a.datetime.cmp(&b.datetime);
            if dt_cmp == std::cmp::Ordering::Equal {
                let a_rank = match rank_field {
                    "volume_ranking" => a.volume_ranking,
                    "long_ranking" => a.long_ranking,
                    _ => a.short_ranking,
                };
                let b_rank = match rank_field {
                    "volume_ranking" => b.volume_ranking,
                    "long_ranking" => b.long_ranking,
                    _ => b.short_ranking,
                };
                a_rank.partial_cmp(&b_rank).unwrap_or(std::cmp::Ordering::Equal)
            } else {
                dt_cmp
            }
        });
        Ok(list)
    }

    /// 查询 EDB 指标数据
    ///
    /// `n` 表示返回的天数，`align` 与 `fill` 控制对齐和填充方式。
    pub async fn query_edb_data(
        &self,
        ids: &[i32],
        n: i32,
        align: Option<&str>,
        fill: Option<&str>,
    ) -> Result<Vec<EdbIndexData>> {
        if ids.is_empty() {
            return Err(TqError::InvalidParameter("ids 不能为空。".to_string()));
        }
        if n < 1 {
            return Err(TqError::InvalidParameter(format!(
                "n 参数 {} 错误，应为 >= 1 的整数。",
                n
            )));
        }
        if !matches!(align, None | Some("day")) {
            return Err(TqError::InvalidParameter(format!(
                "align 参数 {:?} 错误，仅支持 None 或 'day'",
                align
            )));
        }
        if !matches!(fill, None | Some("ffill") | Some("bfill")) {
            return Err(TqError::InvalidParameter(format!(
                "fill 参数 {:?} 错误，仅支持 None/'ffill'/'bfill'",
                fill
            )));
        }

        let mut seen = HashSet::new();
        let mut norm_ids: Vec<i32> = Vec::new();
        for &id in ids {
            if seen.insert(id) {
                norm_ids.push(id);
            }
        }
        if norm_ids.len() > 100 {
            return Err(TqError::InvalidParameter("ids 数量超过限制(<=100)".to_string()));
        }

        let end_date = Utc::now().date_naive();
        let start_date = end_date - ChronoDuration::days((n - 1) as i64);
        let start_s = start_date.format("%Y-%m-%d").to_string();
        let end_s = end_date.format("%Y-%m-%d").to_string();

        let headers = self.auth.read().await.base_header();
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .default_headers(headers)
            .build()
            .map_err(|e| TqError::Reqwest {
                context: "创建 HTTP 客户端失败".to_string(),
                source: e,
            })?;

        let payload = json!({
            "ids": norm_ids,
            "start": start_s,
            "end": end_s,
        });

        let url = "https://edb.shinnytech.com/data/index_data";
        let resp = client
            .post(url)
            .json(&payload)
            .send()
            .await
            .map_err(|e| TqError::Reqwest {
                context: format!("请求失败: POST {}", url),
                source: e,
            })?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.map_err(|e| TqError::Reqwest {
                context: format!("读取响应失败: POST {}", url),
                source: e,
            })?;
            return Err(TqError::HttpStatus {
                method: "POST".to_string(),
                url: url.to_string(),
                status,
                body_snippet: TqError::truncate_body(body),
            });
        }

        let body = resp.text().await.map_err(|e| TqError::Reqwest {
            context: format!("读取响应失败: POST {}", url),
            source: e,
        })?;
        let content: Value = serde_json::from_str(&body).map_err(|e| TqError::Json {
            context: format!("JSON 解析失败: POST {}", url),
            source: e,
        })?;

        if content.get("error_code").and_then(|v| v.as_i64()).unwrap_or(0) != 0 {
            return Err(TqError::Other(format!(
                "edb 指标取值查询失败: {}",
                content.get("error_msg").and_then(|v| v.as_str()).unwrap_or("")
            )));
        }

        let data = content.get("data").cloned().unwrap_or(Value::Null);
        let ids_from_server: Vec<i32> = data
            .get("ids")
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().filter_map(|v| v.as_i64()).map(|v| v as i32).collect())
            .unwrap_or_else(|| norm_ids.clone());
        let values_map = data
            .get("values")
            .and_then(|v| v.as_object())
            .cloned()
            .unwrap_or_default();

        let mut rows: HashMap<String, HashMap<i32, f64>> = HashMap::new();
        for (date, row_val) in values_map.iter() {
            let row_arr = match row_val.as_array() {
                Some(arr) => arr,
                None => continue,
            };
            let mut row_map: HashMap<i32, f64> = HashMap::new();
            for (idx, id) in ids_from_server.iter().enumerate() {
                let value = row_arr.get(idx).map(value_to_f64).unwrap_or(f64::NAN);
                row_map.insert(*id, value);
            }
            rows.insert(date.clone(), row_map);
        }

        let mut dates: Vec<String> = rows.keys().cloned().collect();
        dates.sort();

        if align == Some("day") {
            let mut all_dates: Vec<String> = Vec::new();
            let mut current = start_date;
            while current <= end_date {
                all_dates.push(current.format("%Y-%m-%d").to_string());
                current += ChronoDuration::days(1);
            }
            let mut aligned: HashMap<String, HashMap<i32, f64>> = HashMap::new();
            for date in all_dates.iter() {
                let row = rows.get(date).cloned().unwrap_or_default();
                aligned.insert(date.clone(), row);
            }
            rows = aligned;
            dates = all_dates;

            if fill == Some("ffill") {
                let mut last_values: HashMap<i32, f64> = HashMap::new();
                for date in dates.iter() {
                    let row = rows.entry(date.clone()).or_default();
                    for id in norm_ids.iter() {
                        let value = row.get(id).copied().unwrap_or(f64::NAN);
                        if value.is_nan() {
                            if let Some(last) = last_values.get(id) {
                                row.insert(*id, *last);
                            }
                        } else {
                            last_values.insert(*id, value);
                        }
                    }
                }
            } else if fill == Some("bfill") {
                let mut next_values: HashMap<i32, f64> = HashMap::new();
                for date in dates.iter().rev() {
                    let row = rows.entry(date.clone()).or_default();
                    for id in norm_ids.iter() {
                        let value = row.get(id).copied().unwrap_or(f64::NAN);
                        if value.is_nan() {
                            if let Some(next) = next_values.get(id) {
                                row.insert(*id, *next);
                            }
                        } else {
                            next_values.insert(*id, value);
                        }
                    }
                }
            }
        }

        let mut result: Vec<EdbIndexData> = Vec::new();
        for date in dates {
            if let Some(row) = rows.get(&date) {
                let mut values: HashMap<i32, f64> = HashMap::new();
                for id in norm_ids.iter() {
                    let value = row.get(id).copied().unwrap_or(f64::NAN);
                    values.insert(*id, value);
                }
                result.push(EdbIndexData { date, values });
            }
        }

        Ok(result)
    }

    /// 获取交易日历
    ///
    /// 返回指定日期区间内的交易日标记列表。
    pub async fn get_trading_calendar(
        &self,
        start_dt: NaiveDate,
        end_dt: NaiveDate,
    ) -> Result<Vec<TradingCalendarDay>> {
        use chrono::Datelike;

        if start_dt > end_dt {
            return Err(TqError::InvalidParameter("start_dt 必须小于等于 end_dt".to_string()));
        }
        let headers = self.auth.read().await.base_header();
        let content = fetch_json_with_headers(&self.holiday_url, headers).await?;
        let holidays = content
            .as_array()
            .ok_or_else(|| TqError::ParseError("节假日数据格式错误".to_string()))?;
        let mut holiday_set: HashSet<NaiveDate> = HashSet::new();
        for item in holidays.iter() {
            if let Some(date_str) = item.as_str()
                && let Ok(date) = NaiveDate::parse_from_str(date_str, "%Y-%m-%d")
            {
                holiday_set.insert(date);
            }
        }

        let mut result: Vec<TradingCalendarDay> = Vec::new();
        let mut current = start_dt;
        while current <= end_dt {
            let weekday = current.weekday().number_from_monday();
            let is_weekday = weekday <= 5;
            let trading = is_weekday && !holiday_set.contains(&current);
            result.push(TradingCalendarDay {
                date: current.format("%Y-%m-%d").to_string(),
                trading,
            });
            current += ChronoDuration::days(1);
        }
        Ok(result)
    }
}
