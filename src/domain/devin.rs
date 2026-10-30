use std::fmt;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// git config `prtui.devin-token` と `prtui.devin-org`. 両方そろったときだけ作る
#[derive(Clone, PartialEq)]
pub struct DevinConfig {
    pub token: String,
    /// `org-` で始まる organization ID
    pub org: String,
}

/// `{:?}` で token が出ないように伏せる
impl fmt::Debug for DevinConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DevinConfig")
            .field("token", &"***")
            .field("org", &self.org)
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DevinStatus {
    New,
    Claimed,
    Running,
    Exit,
    Error,
    Suspended,
    Resuming,
    #[serde(other)]
    Unknown,
}

impl DevinStatus {
    /// 一覧や詳細に出す文字列. API の値そのもの
    pub fn label(self) -> &'static str {
        match self {
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
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DevinSession {
    /// `devin-` で始まる session ID
    pub id: String,
    pub status: DevinStatus,
    pub title: Option<String>,
    /// Devin の web 画面の URL
    pub url: String,
    pub updated_at: DateTime<Utc>,
    /// session が作った, または触った PR の URL
    pub pr_urls: Vec<String>,
    pub is_archived: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DevinMessage {
    pub from_devin: bool,
    pub username: Option<String>,
    pub text: String,
    pub created_at: DateTime<Utc>,
}

/// PR の Devin Review のページ. repo は `owner/name`
pub fn review_url(repo: &str, number: u64) -> String {
    format!("https://app.devin.ai/review/{repo}/pull/{number}")
}

/// PR の URL に紐づく session のうち updated_at が最新のものと, 紐づく session の件数.
/// URL は末尾の `/` と大文字小文字を無視して比べる
pub fn session_for<'a>(
    sessions: &'a [DevinSession],
    pr_url: &str,
) -> Option<(&'a DevinSession, usize)> {
    let key = pr_url.trim_end_matches('/');
    let mut hits = sessions.iter().filter(|s| {
        s.pr_urls
            .iter()
            .any(|u| u.trim_end_matches('/').eq_ignore_ascii_case(key))
    });
    let first = hits.next()?;
    // 同時刻なら先に出た方を残す
    let (latest, n) = hits.fold((first, 1), |(best, n), s| {
        (
            if s.updated_at > best.updated_at {
                s
            } else {
                best
            },
            n + 1,
        )
    });
    Some((latest, n))
}

#[cfg(test)]
impl DevinSession {
    pub(crate) fn fixture(id: &str, pr_url: &str) -> DevinSession {
        DevinSession {
            id: id.into(),
            status: DevinStatus::Running,
            title: None,
            url: format!("https://app.devin.ai/sessions/{id}"),
            updated_at: Default::default(),
            pr_urls: vec![pr_url.into()],
            is_archived: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn review_url_from_repo_and_number() {
        assert_eq!(
            review_url("acme/app", 8255),
            "https://app.devin.ai/review/acme/app/pull/8255"
        );
    }

    fn at(secs: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(secs, 0).unwrap()
    }

    #[test]
    fn session_for_picks_latest_and_counts() {
        let url = "https://github.com/o/r/pull/1";
        let mut a = DevinSession::fixture("devin-a", url);
        a.updated_at = at(100);
        let mut b = DevinSession::fixture("devin-b", "https://GitHub.com/O/R/pull/1/");
        b.updated_at = at(300);
        let mut c = DevinSession::fixture("devin-c", url);
        c.updated_at = at(200);
        let other = DevinSession::fixture("devin-d", "https://github.com/o/r/pull/12");
        let sessions = vec![a, b, c, other];
        let (s, n) = session_for(&sessions, url).unwrap();
        assert_eq!((s.id.as_str(), n), ("devin-b", 3));
        // PR 側の URL に末尾の / があっても同じ
        let (s, n) = session_for(&sessions, "https://github.com/o/r/pull/12/").unwrap();
        assert_eq!((s.id.as_str(), n), ("devin-d", 1));
    }

    #[test]
    fn session_for_none_without_match() {
        let sessions = vec![DevinSession::fixture(
            "devin-a",
            "https://github.com/o/r/pull/1",
        )];
        // 番号の前方一致では拾わない
        assert!(session_for(&sessions, "https://github.com/o/r/pull/10").is_none());
        assert!(session_for(&[], "https://github.com/o/r/pull/1").is_none());
    }

    #[test]
    fn session_with_several_prs() {
        let mut s = DevinSession::fixture("devin-a", "https://github.com/o/r/pull/1");
        s.pr_urls.push("https://github.com/o/r/pull/2".into());
        let sessions = vec![s];
        let (_, n) = session_for(&sessions, "https://github.com/o/r/pull/2").unwrap();
        assert_eq!(n, 1);
    }

    #[test]
    fn status_from_json() {
        let st = |s: &str| serde_json::from_str::<DevinStatus>(s).unwrap();
        assert_eq!(st(r#""suspended""#), DevinStatus::Suspended);
        assert_eq!(st(r#""blocked""#), DevinStatus::Unknown);
        assert_eq!(DevinStatus::Resuming.label(), "resuming");
    }
}
