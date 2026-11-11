//! `M` のマージ. 方法の選択肢と, 確認画面に出す注意点

use crate::domain::pr::{CheckState, Pr, ReviewerState};

/// `gh pr merge` の方法
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MergeMethod {
    Squash,
    Merge,
    Rebase,
}

impl MergeMethod {
    /// 確認画面に並べる順
    pub const ALL: [MergeMethod; 3] =
        [MergeMethod::Squash, MergeMethod::Merge, MergeMethod::Rebase];

    /// 確認画面でこの方法を選ぶキー
    pub fn key(self) -> char {
        match self {
            MergeMethod::Squash => 's',
            MergeMethod::Merge => 'm',
            MergeMethod::Rebase => 'r',
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            MergeMethod::Squash => "squash",
            MergeMethod::Merge => "merge commit",
            MergeMethod::Rebase => "rebase",
        }
    }

    /// GitHub の `MERGE` / `SQUASH` / `REBASE`. それ以外は None
    pub fn from_github(s: &str) -> Option<Self> {
        match s {
            "SQUASH" => Some(MergeMethod::Squash),
            "MERGE" => Some(MergeMethod::Merge),
            "REBASE" => Some(MergeMethod::Rebase),
            _ => None,
        }
    }
}

/// マージを頼んだ結果
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MergeOutcome {
    Merged,
    /// merge queue に入った. マージされるのは queue が進んでから
    Enqueued,
}

/// repo のマージの設定
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeSettings {
    /// repo で許可されている方法. `MergeMethod::ALL` の順
    pub allowed: Vec<MergeMethod>,
    /// 自分が前回使った方法 (`viewerDefaultMergeMethod`). Enter で選ぶ. 許可されていなければ None
    pub default: Option<MergeMethod>,
}

impl MergeSettings {
    pub fn new(allowed: Vec<MergeMethod>, default: Option<MergeMethod>) -> Self {
        let allowed: Vec<MergeMethod> = MergeMethod::ALL
            .into_iter()
            .filter(|m| allowed.contains(m))
            .collect();
        let default = default.filter(|d| allowed.contains(d));
        Self { allowed, default }
    }

    /// 許可されている方法のうちキーが c のもの
    pub fn by_key(&self, c: char) -> Option<MergeMethod> {
        self.allowed.iter().copied().find(|m| m.key() == c)
    }
}

/// マージする前に目に入れておきたい PR の状態. 止めはせず確認画面に出すだけで,
/// マージできるかは GitHub (branch protection) に任せる. prs は自分タブの一覧で, 子 PR を探すのに使う
pub fn merge_warnings(pr: &Pr, prs: &[Pr]) -> Vec<String> {
    let mut out = vec![];
    let failed = pr.count_checks(CheckState::Failure);
    if failed > 0 {
        out.push(format!("CI が {failed} 件失敗している"));
    }
    let pending = pr.count_checks(CheckState::Pending);
    if pending > 0 {
        out.push(format!("CI が {pending} 件実行中"));
    }
    let changes: Vec<&str> = pr
        .reviewers
        .iter()
        .filter(|r| r.state == ReviewerState::ChangesRequested)
        .map(|r| r.name.as_str())
        .collect();
    if !changes.is_empty() {
        out.push(format!("変更要求がある ({})", changes.join(", ")));
    }
    if pr.count_reviewers(ReviewerState::Approved) == 0 {
        out.push("承認が無い".into());
    }
    if pr.threads_open > 0 {
        out.push(format!("未解決のスレッドが {} 件ある", pr.threads_open));
    }
    if pr.base_ref_name != "main" {
        out.push(format!("base は main ではなく {}", pr.base_ref_name));
    }
    let children: Vec<String> = prs
        .iter()
        .filter(|p| p.number != pr.number && p.base_ref_name == pr.head_ref_name)
        .map(|p| format!("#{}", p.number))
        .collect();
    if !children.is_empty() {
        out.push(format!(
            "{} がこの PR のブランチを base にしている",
            children.join(", ")
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;
    use crate::domain::pr::{Check, Reviewer};

    #[test]
    fn settings_keep_order_and_drop_disallowed_default() {
        let s = MergeSettings::new(
            vec![MergeMethod::Rebase, MergeMethod::Squash],
            Some(MergeMethod::Merge),
        );
        assert_eq!(s.allowed, [MergeMethod::Squash, MergeMethod::Rebase]);
        assert_eq!(s.default, None);
        assert_eq!(s.by_key('r'), Some(MergeMethod::Rebase));
        assert_eq!(s.by_key('m'), None);
        assert_eq!(s.by_key('x'), None);
    }

    #[test]
    fn from_github() {
        assert_eq!(
            MergeMethod::from_github("SQUASH"),
            Some(MergeMethod::Squash)
        );
        assert_eq!(MergeMethod::from_github("MERGE"), Some(MergeMethod::Merge));
        assert_eq!(
            MergeMethod::from_github("REBASE"),
            Some(MergeMethod::Rebase)
        );
        assert_eq!(MergeMethod::from_github(""), None);
    }

    #[test]
    fn no_warnings_when_ready() {
        let mut p = Pr::fixture(1, "a", "main");
        p.checks = vec![Check {
            name: "build".into(),
            state: CheckState::Success,
        }];
        p.reviewers = vec![Reviewer {
            name: "alice".into(),
            state: ReviewerState::Approved,
        }];
        assert!(merge_warnings(&p, &[p.clone()]).is_empty());
    }

    #[test]
    fn warnings() {
        let mut p = Pr::fixture(1, "a", "parent");
        p.checks = ["build", "lint", "test"]
            .into_iter()
            .zip([
                CheckState::Failure,
                CheckState::Pending,
                CheckState::Failure,
            ])
            .map(|(name, state)| Check {
                name: name.into(),
                state,
            })
            .collect();
        p.reviewers = vec![
            Reviewer {
                name: "alice".into(),
                state: ReviewerState::ChangesRequested,
            },
            Reviewer {
                name: "bob".into(),
                state: ReviewerState::Requested,
            },
        ];
        p.threads_open = 2;
        let child = Pr::fixture(2, "b", "a");
        let other = Pr::fixture(3, "c", "main");
        assert_eq!(
            merge_warnings(&p, &[p.clone(), child, other]),
            [
                "CI が 2 件失敗している",
                "CI が 1 件実行中",
                "変更要求がある (alice)",
                "承認が無い",
                "未解決のスレッドが 2 件ある",
                "base は main ではなく parent",
                "#2 がこの PR のブランチを base にしている",
            ]
        );
    }
}
