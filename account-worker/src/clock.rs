//! 时间工具：数据库与日志统一用 RFC3339（UTC），本地日期另算北京时间。
//!
//! 签到 / 领取按北京时间结算（PRD §4.3、§5.1），所以"今天"必须用 UTC+8，
//! 不能用运行机器的本地时区。

use chrono::{DateTime, FixedOffset, SecondsFormat, Utc};

pub fn now_rfc3339() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)
}

/// 北京时间的当天日期（`YYYY-MM-DD`）。
pub fn beijing_date() -> String {
    beijing_now().format("%Y-%m-%d").to_string()
}

pub fn beijing_now() -> DateTime<FixedOffset> {
    let offset = FixedOffset::east_opt(8 * 3600).expect("UTC+8 固定偏移");
    Utc::now().with_timezone(&offset)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn beijing_date_shape() {
        let d = beijing_date();
        assert_eq!(d.len(), 10);
        assert_eq!(d.as_bytes()[4], b'-');
    }

    #[test]
    fn rfc3339_has_utc_marker() {
        assert!(now_rfc3339().ends_with('Z'));
    }
}
