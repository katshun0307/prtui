use crate::domain::pr::{Pr, ReviewerState};

/// レビュータブの並び. 依頼中 (再依頼を含む) を先に, その中とレビュー済みの中はそれぞれ更新日時の降順.
/// 他人の PR の stack はまとめない
pub fn arrange_reviews(mut prs: Vec<Pr>) -> Vec<Pr> {
    prs.sort_by_key(|p| {
        let requested = p.my_review.as_ref().is_some_and(|m| m.requested);
        (!requested, std::cmp::Reverse(p.updated_at))
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

#[cfg(test)]
mod tests {
    use super::{arrange_reviews, review_targets};
    use crate::domain::pr::{MyReview, Pr, Reviewer, ReviewerState};

    #[test]
    fn arrange_reviews_puts_requested_first() {
        use chrono::{TimeZone, Utc};
        let at = |n: u64, requested: bool, day: u32| {
            let mut p = Pr::fixture(n, "h", "main");
            p.updated_at = Utc.with_ymd_and_hms(2026, 1, day, 0, 0, 0).unwrap();
            p.my_review = Some(MyReview {
                requested,
                last: None,
                via_teams: vec![],
            });
            p
        };
        let prs = vec![
            at(1, false, 9),
            at(2, true, 1),
            at(3, true, 5),
            at(4, false, 3),
        ];
        let got: Vec<u64> = arrange_reviews(prs).iter().map(|p| p.number).collect();
        assert_eq!(got, vec![3, 2, 1, 4]);
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
}
