use std::cmp::Ordering;

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

#[cfg(test)]
mod tests {
    use super::{arrange_reviews, review_targets};
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
}
