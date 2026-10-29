use chrono::{DateTime, Utc};

use crate::domain::devin::{DevinSession, DevinStatus};

pub fn ago(t: DateTime<Utc>) -> String {
    ago_at(t, Utc::now())
}

fn ago_at(t: DateTime<Utc>, now: DateTime<Utc>) -> String {
    let secs = (now - t).num_seconds().max(0);
    match secs {
        s if s < 3600 => format!("{}m", s / 60),
        s if s < 86400 => format!("{}h", s / 3600),
        s => format!("{}d", s / 86400),
    }
}

/// Devin session の status を API の表記のまま出す
pub fn status_label(s: DevinStatus) -> &'static str {
    match s {
        DevinStatus::New => "new",
        DevinStatus::Claimed => "claimed",
        DevinStatus::Running => "running",
        DevinStatus::Exit => "exit",
        DevinStatus::Error => "error",
        DevinStatus::Suspended => "suspended",
        DevinStatus::Resuming => "resuming",
        DevinStatus::Unknown => "unknown",
    }
}

/// 一覧の Devin 列と dump の表示. PR に session が 2 件以上あれば `×n` を付ける
pub fn devin_label(s: &DevinSession, count: usize) -> String {
    let label = status_label(s.status);
    if count > 1 {
        format!("{label}×{count}")
    } else {
        label.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    fn at(secs: i64) -> String {
        let now = Utc::now();
        ago_at(now - Duration::seconds(secs), now)
    }

    #[test]
    fn ago_units() {
        assert_eq!(at(0), "0m");
        assert_eq!(at(59), "0m");
        assert_eq!(at(3599), "59m");
        assert_eq!(at(3600), "1h");
        assert_eq!(at(86399), "23h");
        assert_eq!(at(86400), "1d");
        assert_eq!(at(86400 * 10), "10d");
    }

    #[test]
    fn future_is_zero() {
        assert_eq!(at(-120), "0m");
    }

    #[test]
    fn devin_label_count() {
        let s = DevinSession::fixture("devin-1", "https://github.com/o/r/pull/1");
        assert_eq!(devin_label(&s, 1), "running");
        assert_eq!(devin_label(&s, 2), "running×2");
        let s = DevinSession {
            status: DevinStatus::Unknown,
            ..s
        };
        assert_eq!(devin_label(&s, 1), "unknown");
    }
}
