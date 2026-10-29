use crate::domain::pr::{Pr, ReviewerState};

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
    use super::review_targets;
    use crate::domain::pr::{Pr, Reviewer, ReviewerState};

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
