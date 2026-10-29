use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CheckState {
    Success,
    Failure,
    Pending,
    Skipped,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Check {
    pub name: String,
    pub state: CheckState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReviewerState {
    /// レビューを依頼中でまだ返事が無い. 再依頼も含む
    Requested,
    Approved,
    ChangesRequested,
    Commented,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Reviewer {
    /// User は login, Team は slug
    pub name: String,
    pub state: ReviewerState,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pr {
    pub number: u64,
    pub title: String,
    pub url: String,
    pub author: String,
    pub is_draft: bool,
    pub head_ref_name: String,
    pub head_ref_oid: String,
    pub base_ref_name: String,
    /// 一覧内の別 PR の head を base にしている, またはされている PR だけ Some
    pub stack: Option<Stack>,
    pub updated_at: DateTime<Utc>,
    pub review_decision: Option<String>,
    pub mergeable: String,
    pub labels: Vec<String>,
    pub assignees: Vec<String>,
    /// 依頼中の reviewer と, レビュー済みの reviewer の最新の状態. PR author 自身は含めない
    pub reviewers: Vec<Reviewer>,
    pub additions: u64,
    pub deletions: u64,
    pub changed_files: u64,
    pub checks: Vec<Check>,
    pub threads_total: usize,
    pub threads_open: usize,
    /// main にあって PR ブランチに無いコミット数. head ブランチが消えていれば None
    pub behind_main: Option<u64>,
}

/// stacked PR の中での位置. main に近い側から 1 始まりで数える.
/// 枝分かれしている stack では len は最も深い枝の段数になる
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stack {
    pub pos: usize,
    pub len: usize,
}

impl Pr {
    pub fn count_checks(&self, state: CheckState) -> usize {
        self.checks.iter().filter(|c| c.state == state).count()
    }

    /// SKIPPED を除いたチェック数.
    pub fn effective_checks(&self) -> usize {
        self.checks.len() - self.count_checks(CheckState::Skipped)
    }

    pub fn count_reviewers(&self, state: ReviewerState) -> usize {
        self.reviewers.iter().filter(|r| r.state == state).count()
    }

    pub fn is_conflicting(&self) -> bool {
        self.mergeable == "CONFLICTING"
    }
}

#[cfg(test)]
impl Pr {
    pub(crate) fn fixture(number: u64, head: &str, base: &str) -> Pr {
        Pr {
            number,
            title: String::new(),
            url: String::new(),
            author: String::new(),
            is_draft: false,
            head_ref_name: head.into(),
            head_ref_oid: String::new(),
            base_ref_name: base.into(),
            stack: None,
            updated_at: Default::default(),
            review_decision: None,
            mergeable: String::new(),
            labels: vec![],
            assignees: vec![],
            reviewers: vec![],
            additions: 0,
            deletions: 0,
            changed_files: 0,
            checks: vec![],
            threads_total: 0,
            threads_open: 0,
            behind_main: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Pr;

    #[test]
    fn conflicting_only_when_mergeable_is_conflicting() {
        let mut p = Pr::fixture(1, "a", "main");
        for (m, want) in [
            ("CONFLICTING", true),
            ("MERGEABLE", false),
            ("UNKNOWN", false),
            ("", false),
        ] {
            p.mergeable = m.into();
            assert_eq!(p.is_conflicting(), want, "{m}");
        }
    }
}
