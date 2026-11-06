use std::cmp::Ordering;

use crate::domain::local::ReviewerSet;
use crate::domain::pr::{Pr, ReviewerState};

/// レビュータブの並び. 依頼中 (再依頼を含む) を先に, 依頼を受けた時刻の古い順に並べる.
/// 古い依頼から片付けるため. 依頼時刻が分からなければ更新日時で代える.
/// レビュー済みはその後ろに更新日時の降順. 他人の PR の stack はまとめない
pub fn arrange_reviews(mut prs: Vec<Pr>) -> Vec<Pr> {
    let requested = |p: &Pr| {
        p.my_review
            .as_ref()
            .filter(|m| m.requested)
            .map(|m| m.requested_at.unwrap_or(p.updated_at))
    };
    prs.sort_by(|a, b| {
        match (requested(a), requested(b)) {
            (Some(x), Some(y)) => x.cmp(&y),
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (None, None) => b.updated_at.cmp(&a.updated_at),
        }
        // 同じ時刻なら番号で決め, 取り直すたびに入れ替わらないようにする
        .then(a.number.cmp(&b.number))
    });
    prs
}

/// reviewers のうち, まだ依頼中でない人. author 自身には依頼できないので除く.
/// レビュー済みの人には再依頼する. GitHub の login は大文字小文字を区別しないので,
/// API が返す `bobBuilder` と git config の書き方が違っても同じ人とみなす
pub fn review_targets<'a>(pr: &Pr, reviewers: &'a [String]) -> Vec<&'a str> {
    reviewers
        .iter()
        .map(String::as_str)
        .filter(|r| !pr.author.eq_ignore_ascii_case(r))
        .filter(|r| {
            !pr.reviewers
                .iter()
                .any(|x| x.name.eq_ignore_ascii_case(r) && x.state == ReviewerState::Requested)
        })
        .collect()
}

/// GitHub の login は大文字小文字を区別しない
pub fn same_login(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}

/// PR での今の状態. PR に居なければ None
fn state_of(pr: &Pr, login: &str) -> Option<ReviewerState> {
    pr.reviewers
        .iter()
        .find(|r| same_login(&r.name, login))
        .map(|r| r.state)
}

/// `R` の選択画面の 1 行
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReviewerChoice {
    /// git config `prtui.reviewer-set` の組. members は PR author を除いた全員
    Set { name: String, members: Vec<String> },
    /// 個人. state は PR での今の状態で, PR に居なければ None
    Person {
        login: String,
        state: Option<ReviewerState>,
    },
}

impl ReviewerChoice {
    /// 選んだときに依頼する login. 既に依頼中の人は除く
    pub fn targets(&self, pr: &Pr) -> Vec<String> {
        let members = match self {
            ReviewerChoice::Set { members, .. } => members.as_slice(),
            ReviewerChoice::Person { login, .. } => std::slice::from_ref(login),
        };
        review_targets(pr, members)
            .into_iter()
            .map(String::from)
            .collect()
    }

    fn matches(&self, query: &str) -> bool {
        let hit = |s: &str| s.to_lowercase().contains(query);
        match self {
            ReviewerChoice::Set { name, members } => hit(name) || members.iter().any(|m| hit(m)),
            ReviewerChoice::Person { login, .. } => hit(login),
        }
    }
}

/// `R` の選択画面に並べる行. 組を設定の順に先に並べ, 個人は
/// 再依頼できるレビュー済みの人 (コメントのみを含む), 設定にあって PR に居ない人
/// (`prtui.reviewers`, 組のメンバーの順), 依頼中の人の順に並べる. PR author は出さない.
/// レビュー済みの人は設定に無くても出すので, コメントだけした人にも再依頼できる.
/// `query` は組の名前, メンバー, login の部分一致 (大文字小文字を無視) で絞る
pub fn reviewer_choices(
    pr: &Pr,
    people: &[String],
    sets: &[ReviewerSet],
    query: &str,
) -> Vec<ReviewerChoice> {
    let not_author = |l: &&String| !same_login(l, &pr.author);
    let mut out: Vec<ReviewerChoice> = sets
        .iter()
        .map(|s| ReviewerChoice::Set {
            name: s.name.clone(),
            members: s.members.iter().filter(not_author).cloned().collect(),
        })
        .filter(|c| matches!(c, ReviewerChoice::Set { members, .. } if !members.is_empty()))
        .collect();

    let reviewed = pr
        .reviewers
        .iter()
        .filter(|r| r.state != ReviewerState::Requested)
        .map(|r| &r.name);
    let configured = people
        .iter()
        .chain(sets.iter().flat_map(|s| &s.members))
        .filter(|l| state_of(pr, l).is_none());
    let requested = pr
        .reviewers
        .iter()
        .filter(|r| r.state == ReviewerState::Requested)
        .map(|r| &r.name);
    let mut logins: Vec<&String> = vec![];
    for l in reviewed
        .chain(configured)
        .chain(requested)
        .filter(not_author)
    {
        if !logins.iter().any(|x| same_login(x, l)) {
            logins.push(l);
        }
    }
    out.extend(logins.into_iter().map(|l| ReviewerChoice::Person {
        login: l.clone(),
        state: state_of(pr, l),
    }));

    let q = query.to_lowercase();
    out.retain(|c| c.matches(&q));
    out
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::{ReviewerChoice, arrange_reviews, review_targets, reviewer_choices};
    use crate::domain::local::ReviewerSet;
    use crate::domain::pr::{MyReview, Pr, Reviewer, ReviewerState};

    #[test]
    fn arrange_reviews_puts_oldest_request_first() {
        use chrono::{TimeZone, Utc};
        let day = |d: u32| Utc.with_ymd_and_hms(2026, 1, d, 0, 0, 0).unwrap();
        let at = |n: u64, requested: bool, updated: u32, requested_at: Option<u32>| {
            let mut p = Pr::fixture(n, "h", "main");
            p.updated_at = day(updated);
            p.my_review = Some(MyReview {
                requested,
                last: None,
                via_teams: vec![],
                requested_at: requested_at.map(day),
            });
            p
        };
        let prs = vec![
            at(1, false, 9, None),
            at(2, true, 9, Some(4)),
            at(3, true, 1, Some(6)),
            at(4, false, 3, None),
            // 依頼時刻が分からなければ更新日時 (5 日) で並べる
            at(5, true, 5, None),
            at(7, false, 3, None),
            at(6, true, 2, Some(4)),
        ];
        let got: Vec<u64> = arrange_reviews(prs).iter().map(|p| p.number).collect();
        assert_eq!(got, vec![2, 6, 5, 3, 1, 4, 7]);
    }

    fn configured() -> Vec<String> {
        vec!["alice".into(), "bobBuilder".into()]
    }

    fn reviewer(name: &str, state: ReviewerState) -> Reviewer {
        Reviewer {
            name: name.into(),
            state,
        }
    }

    #[test]
    fn review_targets_ignores_login_case() {
        let mut p = Pr::fixture(1, "a", "main");
        p.reviewers = vec![
            reviewer("ALICE", ReviewerState::Requested),
            reviewer("bobbuilder", ReviewerState::Requested),
        ];
        assert!(review_targets(&p, &configured()).is_empty());
    }

    #[test]
    fn review_targets_rerequests_reviewed() {
        let mut p = Pr::fixture(1, "a", "main");
        p.reviewers = vec![
            reviewer("alice", ReviewerState::Approved),
            reviewer("bobBuilder", ReviewerState::Requested),
        ];
        assert_eq!(review_targets(&p, &configured()), vec!["alice"]);
    }

    #[test]
    fn review_targets_skips_author() {
        let mut p = Pr::fixture(1, "a", "main");
        p.author = "BobBuilder".into();
        assert_eq!(review_targets(&p, &configured()), vec!["alice"]);
        p.author = "someone".into();
        assert_eq!(
            review_targets(&p, &configured()),
            vec!["alice", "bobBuilder"]
        );
    }

    fn strings(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    fn set(name: &str, members: &[&str]) -> ReviewerSet {
        ReviewerSet {
            name: name.into(),
            members: strings(members),
        }
    }

    fn person(login: &str, state: Option<ReviewerState>) -> ReviewerChoice {
        ReviewerChoice::Person {
            login: login.into(),
            state,
        }
    }

    /// author は me. alice は依頼中, carol はコメントのみ, dave は承認済み
    fn reviewed_pr() -> Pr {
        let mut p = Pr::fixture(1, "a", "main");
        p.author = "me".into();
        p.reviewers = vec![
            reviewer("Alice", ReviewerState::Requested),
            reviewer("carol", ReviewerState::Commented),
            reviewer("dave", ReviewerState::Approved),
        ];
        p
    }

    #[test]
    fn choices_sets_then_reviewed_configured_requested() {
        let p = reviewed_pr();
        let sets = vec![
            set("backend", &["alice", "bob", "ME"]),
            set("only-me", &["me"]),
        ];
        let got = reviewer_choices(&p, &strings(&["bob", "erin", "me"]), &sets, "");
        assert_eq!(
            got,
            vec![
                // author は組からも除き, 誰も残らない組は出さない
                ReviewerChoice::Set {
                    name: "backend".into(),
                    members: strings(&["alice", "bob"]),
                },
                person("carol", Some(ReviewerState::Commented)),
                person("dave", Some(ReviewerState::Approved)),
                person("bob", None),
                person("erin", None),
                person("Alice", Some(ReviewerState::Requested)),
            ]
        );
    }

    #[test]
    fn choices_filter_by_name_member_or_login() {
        let p = reviewed_pr();
        let sets = vec![set("backend", &["bob"]), set("front", &["frank"])];
        let got = reviewer_choices(&p, &[], &sets, "BO");
        assert_eq!(
            got,
            vec![
                ReviewerChoice::Set {
                    name: "backend".into(),
                    members: strings(&["bob"]),
                },
                person("bob", None),
            ]
        );
        assert_eq!(reviewer_choices(&p, &[], &sets, "fro").len(), 1);
    }

    #[test]
    fn choice_targets_skip_requested() {
        let p = reviewed_pr();
        let backend = ReviewerChoice::Set {
            name: "backend".into(),
            members: strings(&["alice", "bob", "carol"]),
        };
        assert_eq!(backend.targets(&p), strings(&["bob", "carol"]));
        assert!(
            person("Alice", Some(ReviewerState::Requested))
                .targets(&p)
                .is_empty()
        );
        assert_eq!(
            person("carol", Some(ReviewerState::Commented)).targets(&p),
            strings(&["carol"])
        );
    }
}
