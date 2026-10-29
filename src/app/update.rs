//! キー入力とスレッドの結果を受けて AppState を更新し, 実行すべき副作用を Effect で返す.
//! スレッドの起動や外部コマンドの実行はしない (runtime の責務).

use std::time::Instant;

use chrono::Utc;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::event::{Effect, Event, WatchUntil};
use super::state::AppState;
use crate::domain::linear::linear_issue_key;
use crate::domain::op::PrOp;
use crate::domain::pr::Pr;
use crate::domain::review::review_targets;
use crate::domain::stack::arrange;

/// 全件を取り直す. 取得中なら何もしない
pub fn refresh(state: &mut AppState) -> Vec<Effect> {
    if state.loading {
        return vec![];
    }
    state.loading = true;
    state.last_fetch = Some(Instant::now());
    vec![Effect::Fetch]
}

pub fn handle_event(state: &mut AppState, ev: Event) -> Vec<Effect> {
    match ev {
        Event::Fetched(res) => {
            state.loading = false;
            match res {
                Ok(prs) => {
                    state.fetched_at = Some(Utc::now());
                    state.from_cache = false;
                    state.replace_prs(prs);
                    vec![Effect::SaveCache]
                }
                Err(e) => {
                    state.error(format!("取得失敗: {e}"));
                    vec![]
                }
            }
        }
        Event::Reloaded(n, res) => {
            state.syncing.remove(&n);
            match *res {
                Ok(pr) => {
                    // drain すると replace_prs が選択中の PR を引けなくなるので複製する
                    let mut prs: Vec<Pr> = state
                        .prs
                        .iter()
                        .filter(|p| p.number != n)
                        .cloned()
                        .collect();
                    prs.extend(pr);
                    state.replace_prs(arrange(prs));
                    vec![Effect::SaveCache]
                }
                Err(e) => {
                    state.error(format!("#{n} の再取得失敗: {e}"));
                    vec![]
                }
            }
        }
        Event::UpdatedBranch(n, res) => {
            state.busy = None;
            match res {
                Ok(base) => {
                    state.info(format!("#{n} に {base} を取り込んだ"));
                    // merge commit が積まれて head が動くまで待つ
                    let old = state.pr(n).map(|p| p.head_ref_oid.clone());
                    vec![Effect::Watch {
                        number: n,
                        until: WatchUntil::HeadMovedFrom(old),
                    }]
                }
                Err(e) => {
                    state.error(format!("#{n}: {e:#}"));
                    // head が動いていて 422 になったときなどに最新を出す
                    vec![Effect::Watch {
                        number: n,
                        until: WatchUntil::Immediately,
                    }]
                }
            }
        }
        Event::LinearReview(n, res) => {
            state.busy = None;
            match res {
                Ok(Some(url)) => vec![Effect::OpenUrl(url)],
                Ok(None) => {
                    state.error(format!("#{n} に Linear review のリンクが無い"));
                    vec![]
                }
                Err(e) => {
                    state.error(format!("#{n} の Linear review 取得失敗: {e}"));
                    vec![]
                }
            }
        }
        Event::ChangedState(n, op, res) => {
            state.busy = None;
            match res {
                Ok(_) => {
                    state.info(format!("#{n} を {}した", op.phrase(&state.local.reviewers)));
                    let until = match op {
                        PrOp::Open => WatchUntil::NotDraft,
                        PrOp::Draft => WatchUntil::IsDraft,
                        PrOp::Close => WatchUntil::Gone,
                        PrOp::Review => WatchUntil::Requested(state.local.reviewers.clone()),
                    };
                    vec![Effect::Watch { number: n, until }]
                }
                Err(e) => {
                    state.error(format!("#{n} の {} 失敗: {e}", op.label()));
                    vec![Effect::Watch {
                        number: n,
                        until: WatchUntil::Immediately,
                    }]
                }
            }
        }
        Event::CheckedOut(n, res) => {
            state.busy = None;
            match res {
                Ok(_) => state.info(format!("#{n} をチェックアウトした")),
                Err(e) => state.error(format!("#{n} のチェックアウト失敗: {e}")),
            }
            vec![Effect::ReloadLocal]
        }
    }
}

pub fn handle_key(state: &mut AppState, key: KeyEvent) -> Vec<Effect> {
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
        return vec![Effect::Quit];
    }
    if let Some((n, base)) = state.confirm_update.take() {
        if key.code == KeyCode::Char('y') {
            return update_branch(state, n, base);
        }
        state.info(format!("{base} の取り込みを取りやめた"));
        return vec![];
    }
    if let Some((n, op)) = state.confirm_op.take() {
        if key.code == KeyCode::Char(op.key()) {
            return change_state(state, n, op);
        }
        state.info(format!("#{n} の {} を取りやめた", op.label()));
        return vec![];
    }
    match key.code {
        KeyCode::Char('q') | KeyCode::Esc => vec![Effect::Quit],
        KeyCode::Char('u') => request_update(state),
        KeyCode::Char('O') => request_state(state, PrOp::Open),
        KeyCode::Char('D') => request_state(state, PrOp::Draft),
        KeyCode::Char('X') => request_state(state, PrOp::Close),
        KeyCode::Char('R') => request_state(state, PrOp::Review),
        KeyCode::Char('j') | KeyCode::Down => {
            state.move_by(1);
            vec![]
        }
        KeyCode::Char('k') | KeyCode::Up => {
            state.move_by(-1);
            vec![]
        }
        KeyCode::Char('g') | KeyCode::Home => {
            state.table.select(Some(0));
            vec![]
        }
        KeyCode::Char('G') | KeyCode::End => {
            let len = state.visible().len();
            state.table.select(Some(len.saturating_sub(1)));
            vec![]
        }
        KeyCode::Char('r') => {
            state.message = None;
            refresh(state)
        }
        KeyCode::Char('t') => {
            state.draft_only = !state.draft_only;
            state.clamp_selection();
            vec![]
        }
        KeyCode::Enter | KeyCode::Char('c') => checkout(state),
        KeyCode::Char('o') => state
            .selected()
            .map(|p| Effect::ViewWeb(p.number))
            .into_iter()
            .collect(),
        KeyCode::Char('l') => open_linear(state),
        KeyCode::Char('L') => open_linear_review(state),
        KeyCode::Char('d') => state
            .selected()
            .map(|p| {
                Effect::Suspend(vec![
                    "gh".into(),
                    "pr".into(),
                    "diff".into(),
                    p.number.to_string(),
                ])
            })
            .into_iter()
            .collect(),
        _ => vec![],
    }
}

/// 選択中の PR に対応する Linear の issue をブラウザで開く.
fn open_linear(state: &mut AppState) -> Vec<Effect> {
    let Some((n, key)) = state
        .selected()
        .map(|p| (p.number, linear_issue_key(&p.head_ref_name, &p.title)))
    else {
        return vec![];
    };
    let Some(workspace) = state.local.linear_workspace.clone() else {
        state.error(
            "Linear の workspace が未設定. git config prtui.linear-workspace <slug> を設定する"
                .into(),
        );
        return vec![];
    };
    let Some(key) = key else {
        state.error(format!("#{n} に Linear の issue キーが無い"));
        return vec![];
    };
    vec![Effect::OpenUrl(format!(
        "https://linear.app/{workspace}/issue/{key}"
    ))]
}

/// 選択中の PR の Linear review 画面をブラウザで開く.
/// review の URL は slug が Linear 側の ID なので GitHub の情報からは組み立てられない.
/// Linear の bot が PR に付けるコメントから取るので, コメントの取得をスレッドで行う.
fn open_linear_review(state: &mut AppState) -> Vec<Effect> {
    if state.busy.is_some() {
        return vec![];
    }
    let Some(n) = state.selected().map(|p| p.number) else {
        return vec![];
    };
    state.busy = Some(format!("#{n} の Linear review を探し中…"));
    vec![Effect::LinearReview(n)]
}

/// update-branch API は PR の base を取り込むので, stacked PR では親のブランチが入る.
/// main 以外が base のときは取り込み済みかを判定できないので確認だけ出す.
fn request_update(state: &mut AppState) -> Vec<Effect> {
    if state.busy.is_some() {
        return vec![];
    }
    let Some(pr) = state.selected() else {
        return vec![];
    };
    let (n, base) = (pr.number, pr.base_ref_name.clone());
    if pr.is_conflicting() {
        state.error(format!(
            "#{n} はコンフリクトしているので {base} を取り込めない"
        ));
    } else if base == "main" && pr.behind_main == Some(0) {
        state.info(format!("#{n} は main を取り込み済み"));
    } else {
        state.confirm_update = Some((n, base));
    }
    vec![]
}

fn update_branch(state: &mut AppState, n: u64, base: String) -> Vec<Effect> {
    let Some(head_oid) = state.pr(n).map(|p| p.head_ref_oid.clone()) else {
        return vec![];
    };
    state.busy = Some(format!("#{n} に {base} を取り込み中…"));
    vec![Effect::UpdateBranch {
        number: n,
        base,
        head_oid,
    }]
}

/// open は 1 回で実行し, draft, close, review 依頼は同じキーの 2 回目で実行する.
/// 既にその状態の PR では何もしない. review 依頼は open (Draft でない) PR に限り,
/// 相手が未設定か, 全員が既に依頼中なら何もしない.
fn request_state(state: &mut AppState, op: PrOp) -> Vec<Effect> {
    if state.busy.is_some() {
        return vec![];
    }
    let Some(pr) = state.selected() else {
        return vec![];
    };
    let n = pr.number;
    match op {
        PrOp::Open if !pr.is_draft => state.info(format!("#{n} は Draft ではない")),
        PrOp::Draft if pr.is_draft => state.info(format!("#{n} は既に Draft")),
        PrOp::Review if pr.is_draft => {
            state.error(format!("#{n} は Draft なので review 依頼しない"))
        }
        PrOp::Review if state.local.reviewers.is_empty() => state.error(
            "review 依頼の相手が未設定. git config prtui.reviewers <login>,<login> を設定する"
                .into(),
        ),
        PrOp::Review if review_targets(pr, &state.local.reviewers).is_empty() => state.info(
            format!("#{n} は {} に依頼済み", state.local.reviewers.join(", ")),
        ),
        PrOp::Open => return change_state(state, n, op),
        PrOp::Draft | PrOp::Close | PrOp::Review => state.confirm_op = Some((n, op)),
    }
    vec![]
}

fn change_state(state: &mut AppState, n: u64, op: PrOp) -> Vec<Effect> {
    // gh に依頼相手を渡すのは review 依頼のときだけ
    let targets = match op {
        PrOp::Review => state
            .pr(n)
            .map(|p| review_targets(p, &state.local.reviewers))
            .unwrap_or_default()
            .into_iter()
            .map(String::from)
            .collect(),
        PrOp::Open | PrOp::Draft | PrOp::Close => vec![],
    };
    state.busy = Some(format!(
        "#{n} を {}している…",
        op.phrase(&state.local.reviewers)
    ));
    vec![Effect::ChangeState {
        number: n,
        op,
        targets,
    }]
}

/// 未コミットの変更の判定は git status の読み直しが要るので runtime で行う
fn checkout(state: &mut AppState) -> Vec<Effect> {
    if state.busy.is_some() {
        return vec![];
    }
    state
        .selected()
        .map(|p| Effect::Checkout(p.number))
        .into_iter()
        .collect()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use anyhow::anyhow;
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    use super::{handle_event, handle_key};
    use crate::app::event::{Effect, Event, WatchUntil};
    use crate::app::state::AppState;
    use crate::domain::local::LocalState;
    use crate::domain::op::PrOp;
    use crate::domain::pr::{Pr, Reviewer, ReviewerState};

    fn state(prs: Vec<Pr>) -> AppState {
        let mut s = AppState::new(PathBuf::from("/tmp"), "o/r".into(), LocalState::default());
        s.prs = prs;
        s
    }

    fn pr(n: u64) -> Pr {
        let mut p = Pr::fixture(n, &format!("b{n}"), "main");
        p.head_ref_oid = format!("oid{n}");
        p
    }

    fn key(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
    }

    fn message(s: &AppState) -> (&str, bool) {
        let m = s.message.as_ref().expect("message");
        (&m.text, m.is_error)
    }

    #[test]
    fn update_conflicting_is_error() {
        let mut p = pr(1);
        p.mergeable = "CONFLICTING".into();
        let mut s = state(vec![p]);
        assert!(handle_key(&mut s, key('u')).is_empty());
        assert_eq!(
            message(&s),
            ("#1 はコンフリクトしているので main を取り込めない", true)
        );
        assert!(s.confirm_update.is_none());
    }

    #[test]
    fn update_up_to_date_is_info() {
        let mut p = pr(1);
        p.behind_main = Some(0);
        let mut s = state(vec![p]);
        assert!(handle_key(&mut s, key('u')).is_empty());
        assert_eq!(message(&s), ("#1 は main を取り込み済み", false));
        assert!(s.confirm_update.is_none());
    }

    #[test]
    fn update_confirm_then_yes() {
        let mut p = pr(1);
        p.behind_main = Some(3);
        let mut s = state(vec![p]);
        assert!(handle_key(&mut s, key('u')).is_empty());
        assert_eq!(s.confirm_update, Some((1, "main".into())));
        assert_eq!(
            handle_key(&mut s, key('y')),
            vec![Effect::UpdateBranch {
                number: 1,
                base: "main".into(),
                head_oid: "oid1".into(),
            }]
        );
        assert!(s.confirm_update.is_none());
        assert_eq!(s.busy.as_deref(), Some("#1 に main を取り込み中…"));
    }

    #[test]
    fn update_confirm_then_other_key_cancels() {
        // stacked PR は behind_main に関わらず確認を出す
        let mut s = state(vec![Pr::fixture(1, "b1", "parent")]);
        handle_key(&mut s, key('u'));
        assert_eq!(s.confirm_update, Some((1, "parent".into())));
        assert!(handle_key(&mut s, key('n')).is_empty());
        assert_eq!(message(&s), ("parent の取り込みを取りやめた", false));
        assert!(s.confirm_update.is_none());
        assert!(s.busy.is_none());
    }

    #[test]
    fn draft_needs_second_press() {
        let mut s = state(vec![pr(1)]);
        assert!(handle_key(&mut s, key('D')).is_empty());
        assert_eq!(s.confirm_op, Some((1, PrOp::Draft)));
        assert!(s.busy.is_none());
        assert_eq!(
            handle_key(&mut s, key('D')),
            vec![Effect::ChangeState {
                number: 1,
                op: PrOp::Draft,
                targets: vec![],
            }]
        );
        assert!(s.confirm_op.is_none());
        assert_eq!(s.busy.as_deref(), Some("#1 を draft にしている…"));
    }

    #[test]
    fn draft_cancelled_by_other_key() {
        let mut s = state(vec![pr(1)]);
        handle_key(&mut s, key('D'));
        assert!(handle_key(&mut s, key('j')).is_empty());
        assert!(s.confirm_op.is_none());
        assert!(s.busy.is_none());
        assert_eq!(message(&s), ("#1 の draft を取りやめた", false));
    }

    #[test]
    fn review_without_reviewers_is_error() {
        let mut s = state(vec![pr(1)]);
        assert!(handle_key(&mut s, key('R')).is_empty());
        assert!(message(&s).1);
        assert!(s.confirm_op.is_none());
    }

    #[test]
    fn review_targets_are_passed() {
        let mut p = pr(1);
        p.reviewers = vec![Reviewer {
            name: "alice".into(),
            state: ReviewerState::Requested,
        }];
        let mut s = state(vec![p]);
        s.local.reviewers = vec!["alice".into(), "bob".into()];
        handle_key(&mut s, key('R'));
        assert_eq!(
            handle_key(&mut s, key('R')),
            vec![Effect::ChangeState {
                number: 1,
                op: PrOp::Review,
                targets: vec!["bob".into()],
            }]
        );
    }

    #[test]
    fn quit_keys() {
        let mut s = state(vec![]);
        assert_eq!(handle_key(&mut s, key('q')), vec![Effect::Quit]);
        let esc = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
        assert_eq!(handle_key(&mut s, esc), vec![Effect::Quit]);
        let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert_eq!(handle_key(&mut s, ctrl_c), vec![Effect::Quit]);
    }

    #[test]
    fn diff_suspends() {
        let mut s = state(vec![pr(7)]);
        assert_eq!(
            handle_key(&mut s, key('d')),
            vec![Effect::Suspend(vec![
                "gh".into(),
                "pr".into(),
                "diff".into(),
                "7".into()
            ])]
        );
    }

    #[test]
    fn fetched_replaces_and_saves() {
        let mut s = state(vec![pr(1)]);
        s.loading = true;
        s.from_cache = true;
        let effects = handle_event(&mut s, Event::Fetched(Ok(vec![pr(2), pr(3)])));
        assert_eq!(effects, vec![Effect::SaveCache]);
        assert!(!s.loading);
        assert!(!s.from_cache);
        assert!(s.fetched_at.is_some());
        let numbers: Vec<u64> = s.prs.iter().map(|p| p.number).collect();
        assert_eq!(numbers, vec![2, 3]);
    }

    #[test]
    fn fetched_err_keeps_prs() {
        let mut s = state(vec![pr(1)]);
        s.loading = true;
        let effects = handle_event(&mut s, Event::Fetched(Err(anyhow!("boom"))));
        assert!(effects.is_empty());
        assert!(!s.loading);
        assert_eq!(s.prs.len(), 1);
        assert_eq!(message(&s), ("取得失敗: boom", true));
    }

    #[test]
    fn reloaded_none_removes_pr() {
        let mut s = state(vec![pr(1), pr(2)]);
        s.syncing.insert(1);
        let effects = handle_event(&mut s, Event::Reloaded(1, Box::new(Ok(None))));
        assert_eq!(effects, vec![Effect::SaveCache]);
        assert!(s.pr(1).is_none());
        assert!(s.pr(2).is_some());
        assert!(!s.syncing.contains(&1));
    }

    #[test]
    fn changed_state_open_watches_not_draft() {
        let mut s = state(vec![pr(1)]);
        s.busy = Some("x".into());
        let effects = handle_event(
            &mut s,
            Event::ChangedState(1, PrOp::Open, Ok(String::new())),
        );
        assert_eq!(
            effects,
            vec![Effect::Watch {
                number: 1,
                until: WatchUntil::NotDraft,
            }]
        );
        assert!(s.busy.is_none());
        assert_eq!(message(&s), ("#1 を open にした", false));
    }

    #[test]
    fn watch_until_is_done() {
        let open = pr(1);
        let mut draft = pr(1);
        draft.is_draft = true;
        let mut requested = pr(1);
        requested.reviewers = vec![Reviewer {
            name: "Alice".into(),
            state: ReviewerState::Requested,
        }];

        // None (open でなくなった) はすべて止める
        for until in [
            WatchUntil::Immediately,
            WatchUntil::HeadMovedFrom(Some("oid1".into())),
            WatchUntil::NotDraft,
            WatchUntil::IsDraft,
            WatchUntil::Gone,
            WatchUntil::Requested(vec!["alice".into()]),
        ] {
            assert!(until.is_done(None), "{until:?}");
        }

        assert!(WatchUntil::Immediately.is_done(Some(&open)));

        assert!(!WatchUntil::HeadMovedFrom(Some("oid1".into())).is_done(Some(&open)));
        assert!(WatchUntil::HeadMovedFrom(Some("old".into())).is_done(Some(&open)));
        assert!(WatchUntil::HeadMovedFrom(None).is_done(Some(&open)));

        assert!(WatchUntil::NotDraft.is_done(Some(&open)));
        assert!(!WatchUntil::NotDraft.is_done(Some(&draft)));

        assert!(WatchUntil::IsDraft.is_done(Some(&draft)));
        assert!(!WatchUntil::IsDraft.is_done(Some(&open)));

        assert!(!WatchUntil::Gone.is_done(Some(&open)));

        let alice = WatchUntil::Requested(vec!["alice".into()]);
        assert!(alice.is_done(Some(&requested)));
        assert!(!alice.is_done(Some(&open)));
    }
}
