#![allow(dead_code)]

use std::error::Error;
use std::result::Result as StdResult;

use chrono::{
    DateTime, Datelike, Duration as ChronoDuration, FixedOffset, NaiveDate, TimeZone, Timelike, Utc, Weekday,
};

fn cst() -> StdResult<FixedOffset, Box<dyn Error>> {
    Ok(FixedOffset::east_opt(8 * 3600).ok_or("unable to build Asia/Shanghai offset")?)
}

pub fn trading_day_start_dt(trading_day: NaiveDate) -> StdResult<DateTime<Utc>, Box<dyn Error>> {
    let tz = cst()?;
    let midnight = tz
        .from_local_datetime(&trading_day.and_hms_opt(0, 0, 0).ok_or("invalid trading day midnight")?)
        .single()
        .ok_or("ambiguous trading day midnight")?;
    let mut start = midnight - ChronoDuration::hours(6);
    while matches!(start.weekday(), Weekday::Sat | Weekday::Sun) {
        start -= ChronoDuration::days(1);
    }
    Ok(start.with_timezone(&Utc))
}

pub fn trading_day_end_dt(trading_day: NaiveDate) -> StdResult<DateTime<Utc>, Box<dyn Error>> {
    let tz = cst()?;
    Ok(tz
        .from_local_datetime(
            &trading_day
                .and_hms_nano_opt(17, 59, 59, 999_999_999)
                .ok_or("invalid trading day end time")?,
        )
        .single()
        .ok_or("ambiguous trading day end time")?
        .with_timezone(&Utc))
}

pub fn previous_trading_day(mut date: NaiveDate) -> NaiveDate {
    loop {
        date = date.pred_opt().expect("date underflow");
        if !matches!(date.weekday(), Weekday::Sat | Weekday::Sun) {
            return date;
        }
    }
}

pub fn rewind_trading_days(mut date: NaiveDate, days: usize) -> NaiveDate {
    for _ in 0..days {
        date = previous_trading_day(date);
    }
    date
}

pub fn trading_day_of(dt: DateTime<Utc>) -> NaiveDate {
    let tz = FixedOffset::east_opt(8 * 3600).expect("Asia/Shanghai offset should exist");
    let cst = dt.with_timezone(&tz);
    let mut day = cst.date_naive();
    if cst.time().hour() >= 18 {
        day = day.succ_opt().expect("date overflow");
    }
    while matches!(day.weekday(), Weekday::Sat | Weekday::Sun) {
        day = day.succ_opt().expect("date overflow");
    }
    day
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Timelike};

    use super::{rewind_trading_days, trading_day_end_dt, trading_day_of, trading_day_start_dt};

    #[test]
    fn trading_day_start_uses_previous_evening_for_midweek_day() {
        let cst = chrono::FixedOffset::east_opt(8 * 3600).unwrap();
        let start = trading_day_start_dt(chrono::NaiveDate::from_ymd_opt(2026, 4, 1).unwrap()).unwrap();
        let expected = cst
            .with_ymd_and_hms(2026, 3, 31, 18, 0, 0)
            .single()
            .unwrap()
            .with_timezone(&chrono::Utc);
        assert_eq!(start, expected);
    }

    #[test]
    fn trading_day_start_skips_weekend_night_session() {
        let cst = chrono::FixedOffset::east_opt(8 * 3600).unwrap();
        let start = trading_day_start_dt(chrono::NaiveDate::from_ymd_opt(2026, 4, 13).unwrap()).unwrap();
        let expected = cst
            .with_ymd_and_hms(2026, 4, 10, 18, 0, 0)
            .single()
            .unwrap()
            .with_timezone(&chrono::Utc);
        assert_eq!(start, expected);
    }

    #[test]
    fn trading_day_end_stops_before_night_session() {
        let cst = chrono::FixedOffset::east_opt(8 * 3600).unwrap();
        let end = trading_day_end_dt(chrono::NaiveDate::from_ymd_opt(2026, 4, 1).unwrap()).unwrap();
        let expected = cst
            .with_ymd_and_hms(2026, 4, 1, 17, 59, 59)
            .single()
            .unwrap()
            .with_nanosecond(999_999_999)
            .unwrap()
            .with_timezone(&chrono::Utc);
        assert_eq!(end, expected);
    }

    #[test]
    fn night_session_maps_to_next_trading_day() {
        let cst = chrono::FixedOffset::east_opt(8 * 3600).unwrap();
        let night = cst
            .with_ymd_and_hms(2026, 3, 31, 21, 0, 0)
            .single()
            .unwrap()
            .with_timezone(&chrono::Utc);
        assert_eq!(
            trading_day_of(night),
            chrono::NaiveDate::from_ymd_opt(2026, 4, 1).unwrap()
        );
    }

    #[test]
    fn rewind_trading_days_skips_weekends() {
        let start = chrono::NaiveDate::from_ymd_opt(2026, 4, 13).unwrap();
        assert_eq!(
            rewind_trading_days(start, 1),
            chrono::NaiveDate::from_ymd_opt(2026, 4, 10).unwrap()
        );
    }
}
