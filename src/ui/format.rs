use chrono::{DateTime, Utc};

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
}
