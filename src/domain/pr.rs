use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::domain::label::Label;

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
    /// 名前と色. description は取らないので空
    pub labels: Vec<Label>,
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
    /// レビュータブの PR だけ Some. 自分 (viewer) がどう関わっているか
    #[serde(default)]
    pub my_review: Option<MyReview>,
}

/// レビュータブの PR に対する自分の立場
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MyReview {
    /// `review-requested:@me` に当たる. チーム経由の依頼も含む
    pub requested: bool,
    /// 自分の最新のレビュー. まだレビューしていなければ None
    pub last: Option<ReviewerState>,
    /// 自分個人ではなくチーム経由で依頼されているとき, PR に依頼されているチームのうち自分の所属チームの slug.
    /// 所属チームが 1 つも当たらなければ (`read:org` が無い, など) 依頼されている全チーム
    pub via_teams: Vec<String>,
    /// 今の依頼 (再依頼なら最後の依頼) を受けた時刻. 依頼中でないか, timeline に見つからなければ None
    #[serde(default)]
    pub requested_at: Option<DateTime<Utc>>,
}

impl MyReview {
    /// 一覧の「自分」列の表記. 依頼中でもレビュー済みなら再依頼とみなす
    pub fn label(&self) -> &'static str {
        match (self.requested, self.last) {
            (true, Some(_)) => "再依頼",
            (true, None) => "依頼中",
            (false, Some(ReviewerState::Approved)) => "承認済",
            (false, Some(ReviewerState::ChangesRequested)) => "変更要求",
            (false, Some(ReviewerState::Commented)) => "コメント済",
            // reviewed-by には当たるが latestReviews に残っていない (DISMISSED など)
            (false, Some(ReviewerState::Requested) | None) => "レビュー済",
        }
    }
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

    /// 付いているラベルの名前. 付け外しの差分と反映待ちは名前で比べる
    pub fn label_names(&self) -> Vec<String> {
        self.labels.iter().map(|l| l.name.clone()).collect()
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
            my_review: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{MyReview, Pr, ReviewerState};

    #[test]
    fn my_review_label() {
        let label = |requested, last| {
            MyReview {
                requested,
                last,
                via_teams: vec![],
                requested_at: None,
            }
            .label()
        };
        assert_eq!(label(true, None), "依頼中");
        assert_eq!(label(true, Some(ReviewerState::Approved)), "再依頼");
        assert_eq!(label(true, Some(ReviewerState::Commented)), "再依頼");
        assert_eq!(label(false, Some(ReviewerState::Approved)), "承認済");
        assert_eq!(
            label(false, Some(ReviewerState::ChangesRequested)),
            "変更要求"
        );
        assert_eq!(label(false, Some(ReviewerState::Commented)), "コメント済");
        assert_eq!(label(false, None), "レビュー済");
    }

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
