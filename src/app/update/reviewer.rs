//! `R` の review 依頼の選択画面. 名前つきの組 (git config `prtui.reviewer-set`) と個人を並べ,
//! 文字で絞り込み, Tab で依頼する相手を選び, Enter でまとめて依頼する.
//! 何も選ばずに Enter を押したらカーソルの項目だけを依頼する (ラベルの選択画面と同じ).
//! 組を選ぶとメンバーのうち依頼中でない人をまとめて選ぶ. レビュー済みの人 (コメントのみを含む) を
//! 選ぶと再依頼になる. 依頼中の人は選べず, 依頼を外す操作は無い

use ratatui::crossterm::event::{KeyCode, KeyEvent};

use super::picker::{advance, clamp, picker_key};
use crate::app::event::Effect;
use crate::app::state::{AppState, ReviewerMenu};
use crate::domain::review::{review_targets, same_login};

/// 選択中の PR の review 依頼の選択画面を開く. Draft と, 選べる相手が 1 人も居ない PR では開かない
pub(super) fn open_reviewers(state: &mut AppState) -> Vec<Effect> {
    if state.busy.is_some() {
        return vec![];
    }
    let Some(pr) = state.selected() else {
        return vec![];
    };
    let n = pr.number;
    if pr.is_draft {
        state.error(format!("#{n} は Draft なので review 依頼しない"));
        return vec![];
    }
    let menu = ReviewerMenu::new(n);
    if state.reviewer_choices_for(&menu).is_empty() {
        state.error(
            "review 依頼の相手が居ない. git config prtui.reviewers <login>,<login> か prtui.reviewer-set <名前>=<login>,<login> を設定する"
                .into(),
        );
        return vec![];
    }
    state.push(menu);
    vec![]
}

pub(super) fn reviewer_key(state: &mut AppState, key: KeyEvent) -> Vec<Effect> {
    let len = state.reviewer_choices().len();
    let Some(menu) = state.reviewer_menu_mut() else {
        return vec![];
    };
    match key.code {
        KeyCode::Esc => {
            state.pop();
        }
        KeyCode::Enter => return apply(state),
        KeyCode::Tab => {
            toggle_cursor(state);
            if let Some(menu) = state.reviewer_menu_mut() {
                advance(&mut menu.list, len);
            }
        }
        _ => {
            picker_key(&mut menu.query, &mut menu.list, len, key);
        }
    }
    vec![]
}

/// 選択肢の数が変わったとき (PR を取り直して依頼中の人が増えたなど) にカーソルを範囲に収める
pub(super) fn clamp_cursor(state: &mut AppState) {
    let len = state.reviewer_choices().len();
    if let Some(menu) = state.reviewer_menu_mut() {
        clamp(&mut menu.list, len);
    }
}

/// カーソルの項目の依頼する相手を切り替える. 組はメンバーが全員選ばれていれば全員外し,
/// そうでなければ残りを足す. 依頼中の人しか居ない項目は何もしない
fn toggle_cursor(state: &mut AppState) {
    let choices = state.reviewer_choices();
    let Some(menu) = state.reviewer_menu() else {
        return;
    };
    let Some(pr) = state.pr(menu.number) else {
        return;
    };
    let Some(choice) = menu.list.selected().and_then(|i| choices.get(i)) else {
        return;
    };
    let targets = choice.targets(pr);
    let Some(menu) = state.reviewer_menu_mut() else {
        return;
    };
    let chosen = |l: &String| menu.chosen.iter().any(|c| same_login(c, l));
    if targets.iter().all(chosen) {
        menu.chosen
            .retain(|c| !targets.iter().any(|t| same_login(c, t)));
    } else {
        let add: Vec<String> = targets.into_iter().filter(|t| !chosen(t)).collect();
        menu.chosen.extend(add);
    }
}

fn apply(state: &mut AppState) -> Vec<Effect> {
    let Some(menu) = state.reviewer_menu() else {
        return vec![];
    };
    if menu.chosen.is_empty() {
        toggle_cursor(state);
    }
    let Some(menu) = state.reviewer_menu() else {
        return vec![];
    };
    let n = menu.number;
    // 開いている間に取り直して依頼中になった人は送らない
    let targets: Vec<String> = state
        .pr(n)
        .map(|p| review_targets(p, &menu.chosen))
        .unwrap_or_default()
        .into_iter()
        .map(String::from)
        .collect();
    if targets.is_empty() {
        // 依頼中の人の上や, 絞り込みで何も無いときは開いたままにする
        return vec![];
    }
    state.pop_reviewer_menu();
    state.busy = Some(format!(
        "#{n} に {} の review を依頼している…",
        targets.join(", ")
    ));
    vec![Effect::RequestReviews { number: n, targets }]
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;
    use ratatui::crossterm::event::KeyCode;

    use super::super::test_util::{code, custom_state, key, message, pr, state};
    use super::super::{handle_event, handle_key};
    use crate::app::event::{Effect, Event, WatchUntil};
    use crate::app::state::AppState;
    use crate::domain::local::ReviewerSet;
    use crate::domain::pr::{Reviewer, ReviewerState};
    use crate::domain::review::ReviewerChoice;

    fn strings(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    /// #1 で alice が依頼中, carol がコメントのみ. 設定は個人 bob と組 backend (alice, bob, dave)
    fn reviewing() -> AppState {
        let mut p = pr(1);
        p.reviewers = vec![
            Reviewer {
                name: "alice".into(),
                state: ReviewerState::Requested,
            },
            Reviewer {
                name: "carol".into(),
                state: ReviewerState::Commented,
            },
        ];
        let mut s = state(vec![p]);
        s.local.reviewers = strings(&["bob"]);
        s.local.reviewer_sets = vec![ReviewerSet {
            name: "backend".into(),
            members: strings(&["alice", "bob", "dave"]),
        }];
        s
    }

    fn labels(s: &AppState) -> Vec<String> {
        s.reviewer_choices()
            .into_iter()
            .map(|c| match c {
                ReviewerChoice::Set { name, .. } => format!("@{name}"),
                ReviewerChoice::Person { login, .. } => login,
            })
            .collect()
    }

    fn chosen(s: &AppState) -> Vec<String> {
        s.reviewer_menu().unwrap().chosen.clone()
    }

    #[test]
    fn open_lists_sets_then_people() {
        let mut s = reviewing();
        assert!(handle_key(&mut s, key('R')).is_empty());
        assert_eq!(s.reviewer_menu().unwrap().number, 1);
        assert_eq!(labels(&s), ["@backend", "carol", "bob", "dave", "alice"]);
    }

    #[test]
    fn tab_on_set_selects_members_not_requested() {
        let mut s = reviewing();
        handle_key(&mut s, key('R'));
        handle_key(&mut s, code(KeyCode::Tab));
        assert_eq!(chosen(&s), ["bob", "dave"]);
        // 次の carol に進んでいる. carol も選ぶ
        handle_key(&mut s, code(KeyCode::Tab));
        assert_eq!(chosen(&s), ["bob", "dave", "carol"]);
        // 組をもう一度選ぶとメンバーを外す
        handle_key(&mut s, code(KeyCode::Up));
        handle_key(&mut s, code(KeyCode::Up));
        handle_key(&mut s, code(KeyCode::Tab));
        assert_eq!(chosen(&s), ["carol"]);
        assert_eq!(
            handle_key(&mut s, code(KeyCode::Enter)),
            vec![Effect::RequestReviews {
                number: 1,
                targets: strings(&["carol"]),
            }]
        );
        assert!(s.reviewer_menu().is_none());
        assert_eq!(
            s.busy.as_deref(),
            Some("#1 に carol の review を依頼している…")
        );
    }

    #[test]
    fn set_partially_chosen_adds_rest() {
        let mut s = reviewing();
        handle_key(&mut s, key('R'));
        for c in "dave".chars() {
            handle_key(&mut s, key(c));
        }
        // 絞り込みで @backend (dave を含む) と dave が残る. dave だけ選んでから組を選ぶ
        assert_eq!(labels(&s), ["@backend", "dave"]);
        handle_key(&mut s, code(KeyCode::Down));
        handle_key(&mut s, code(KeyCode::Tab));
        assert_eq!(chosen(&s), ["dave"]);
        handle_key(&mut s, code(KeyCode::Up));
        handle_key(&mut s, code(KeyCode::Tab));
        assert_eq!(chosen(&s), ["dave", "bob"]);
    }

    /// コメントだけした人は設定に無くても選べ, 打って Enter の 1 手で再依頼できる
    #[test]
    fn enter_without_choice_requests_cursor_commenter() {
        let mut s = reviewing();
        handle_key(&mut s, key('R'));
        for c in "car".chars() {
            handle_key(&mut s, key(c));
        }
        assert_eq!(
            handle_key(&mut s, code(KeyCode::Enter)),
            vec![Effect::RequestReviews {
                number: 1,
                targets: strings(&["carol"]),
            }]
        );
    }

    #[test]
    fn requested_person_cannot_be_chosen() {
        let mut s = reviewing();
        handle_key(&mut s, key('R'));
        handle_key(&mut s, code(KeyCode::Up));
        assert_eq!(labels(&s)[4], "alice");
        handle_key(&mut s, code(KeyCode::Tab));
        assert!(chosen(&s).is_empty());
        assert!(handle_key(&mut s, code(KeyCode::Enter)).is_empty());
        assert!(s.reviewer_menu().is_some());
        assert!(s.busy.is_none());
    }

    #[test]
    fn esc_closes() {
        let mut s = reviewing();
        handle_key(&mut s, key('R'));
        handle_key(&mut s, code(KeyCode::Tab));
        assert!(handle_key(&mut s, code(KeyCode::Esc)).is_empty());
        assert!(s.reviewer_menu().is_none());
        assert!(s.busy.is_none());
    }

    #[test]
    fn draft_is_error() {
        let mut s = reviewing();
        s.mine.prs[0].is_draft = true;
        assert!(handle_key(&mut s, key('R')).is_empty());
        assert_eq!(message(&s), ("#1 は Draft なので review 依頼しない", true));
        assert!(s.reviewer_menu().is_none());
    }

    #[test]
    fn nobody_to_choose_is_error() {
        let mut s = state(vec![pr(1)]);
        assert!(handle_key(&mut s, key('R')).is_empty());
        assert!(message(&s).1);
        assert!(s.reviewer_menu().is_none());

        // 設定が無くてもレビュー済みの人が居れば開く
        s.mine.prs[0].reviewers = vec![Reviewer {
            name: "carol".into(),
            state: ReviewerState::Commented,
        }];
        handle_key(&mut s, key('R'));
        assert_eq!(labels(&s), ["carol"]);
    }

    #[test]
    fn readonly_tabs_reject() {
        let mut s = custom_state(vec![pr(1)], vec![pr(20)]);
        s.local.reviewers = strings(&["bob"]);
        handle_key(&mut s, key('3'));
        assert!(handle_key(&mut s, key('R')).is_empty());
        assert_eq!(message(&s), ("R はマージ予定タブでは使えない", true));
        assert!(s.reviewer_menu().is_none());
    }

    #[test]
    fn requested_watches_until_all_requested() {
        let mut s = reviewing();
        s.busy = Some("…".into());
        let effects = handle_event(
            &mut s,
            Event::RequestedReviews {
                number: 1,
                targets: strings(&["bob", "carol"]),
                res: Ok(String::new()),
            },
        );
        assert!(s.busy.is_none());
        assert_eq!(
            message(&s),
            ("#1 に bob, carol の review を依頼した", false)
        );
        assert_eq!(
            effects,
            vec![Effect::Watch {
                number: 1,
                until: WatchUntil::Requested(strings(&["bob", "carol"])),
            }]
        );
    }

    #[test]
    fn request_failure_is_error_and_reloads() {
        let mut s = reviewing();
        let effects = handle_event(
            &mut s,
            Event::RequestedReviews {
                number: 1,
                targets: strings(&["nobody"]),
                res: Err(anyhow::anyhow!("could not find user")),
            },
        );
        assert_eq!(
            message(&s),
            ("#1 の review 依頼失敗: could not find user", true)
        );
        assert_eq!(
            effects,
            vec![Effect::Watch {
                number: 1,
                until: WatchUntil::Immediately,
            }]
        );
    }

    #[test]
    fn reload_while_open_clamps_cursor() {
        let mut s = reviewing();
        handle_key(&mut s, key('R'));
        handle_key(&mut s, code(KeyCode::Up));
        assert_eq!(s.reviewer_menu().unwrap().list.selected(), Some(4));
        // PR が消えたら選択肢は空になる
        handle_event(&mut s, Event::Reloaded(1, Box::new(Ok(None))));
        assert_eq!(s.reviewer_menu().unwrap().list.selected(), Some(0));
        assert!(handle_key(&mut s, code(KeyCode::Enter)).is_empty());
    }
}
