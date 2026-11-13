//! キー入力とスレッドの結果を受けて AppState を更新し, 実行すべき副作用を Effect で返す.
//! スレッドの起動や外部コマンドの実行はしない (runtime の責務).

use std::time::{Duration, Instant};

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::event::Effect;
use super::state::{AppState, Screen};
use crate::domain::copy::copy_items;
use crate::domain::devin::review_url;
use crate::domain::linear::{issue_url, linear_issue_key};
use crate::domain::op::PrOp;

mod chat;
mod event;
mod label;
mod merge;
mod picker;
mod pr_view;
mod reviewer;

pub use event::handle_event;

/// 全件を取り直す間隔
pub const AUTO_REFRESH: Duration = Duration::from_secs(300);
/// 会話ビューを開いている間にメッセージを取り直す間隔
pub const CHAT_POLL: Duration = Duration::from_secs(5);

/// 全タブの全件を取り直す. 取得中のタブは飛ばす.
/// Devin を設定していれば session 一覧も取り直す. それぞれ独立に走らせる
pub fn refresh(state: &mut AppState) -> Vec<Effect> {
    let mut effects = vec![];
    state.last_fetch = Some(Instant::now());
    if !state.mine.loading {
        state.mine.loading = true;
        effects.push(Effect::Fetch);
    }
    if !state.review.loading {
        state.review.loading = true;
        effects.push(Effect::FetchReviews);
    }
    for (i, c) in state.custom.iter_mut().enumerate() {
        if !c.list.loading {
            c.list.loading = true;
            effects.push(Effect::FetchCustom(i));
        }
    }
    if state.local.devin.is_some() && !state.sessions_loading {
        state.sessions_loading = true;
        effects.push(Effect::FetchSessions);
    }
    effects
}

/// 時間で起きる遷移. main のループから毎回呼ぶ.
/// 会話ビューのメッセージを `CHAT_POLL` ごとに, 全件を `AUTO_REFRESH` ごとに取り直す
pub fn tick(state: &mut AppState, now: Instant) -> Vec<Effect> {
    let mut effects = vec![];
    if let Some(chat) = state.chat_mut()
        && !chat.fetching
        && chat
            .last_poll
            .is_none_or(|t| now.saturating_duration_since(t) >= CHAT_POLL)
    {
        chat.fetching = true;
        chat.last_poll = Some(now);
        effects.push(Effect::FetchMessages(chat.session_id.clone()));
    }
    if state
        .last_fetch
        .is_some_and(|t| now.saturating_duration_since(t) >= AUTO_REFRESH)
    {
        effects.extend(refresh(state));
    }
    effects
}

pub fn handle_key(state: &mut AppState, key: KeyEvent) -> Vec<Effect> {
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
        return vec![Effect::Quit];
    }
    // 一覧のフッターの確認とメッセージは一覧に属する. 全画面の画面 (とその上のポップアップ) では触らない
    if !state.screens.iter().any(Screen::is_full) {
        // 前の操作の結果はキーを押したら消し, フッターをキー案内に戻す
        state.message = None;
        if let Some((n, base)) = state.confirm_update.take() {
            if key.code == KeyCode::Char('y') {
                return update_branch(state, n, base);
            }
            state.info(format!("{base} の取り込みを取りやめた"));
            return vec![];
        }
        if let Some((n, branch)) = state.confirm_force_checkout.take() {
            if key.code == KeyCode::Char('y') {
                return checkout(state, n, true);
            }
            state.info(format!("{branch} を PR に合わせるのを取りやめた"));
            return vec![];
        }
        if let Some((n, op)) = state.confirm_op.take() {
            if key.code == KeyCode::Char(op.key()) {
                return change_state(state, n, op);
            }
            state.info(format!("#{n} の {} を取りやめた", op.label()));
            return vec![];
        }
    }
    // キーは一番上の画面に渡す. 何も開いていなければ一覧
    match state.top() {
        Some(Screen::Chat(_)) => return chat::chat_key(state, key),
        Some(Screen::PrView(_)) => return pr_view::pr_view_key(state, key),
        Some(Screen::Copy(_)) => return copy(state, key),
        Some(Screen::Labels(_)) => return label::label_key(state, key),
        Some(Screen::Reviewers(_)) => return reviewer::reviewer_key(state, key),
        Some(Screen::Merge(_)) => return merge::merge_key(state, key),
        None => {}
    }
    if state.tab.is_readonly()
        && let KeyCode::Char(c @ ('u' | 'O' | 'D' | 'X' | 'R' | 'a' | 'M')) = key.code
    {
        state.error(format!(
            "{c} は{}タブでは使えない",
            state.tab_label(state.tab)
        ));
        return vec![];
    }
    match key.code {
        // Esc は画面を戻るキーなので, 戻りすぎて終わらないよう一覧では何もしない
        KeyCode::Char('q') => vec![Effect::Quit],
        KeyCode::Tab => cycle_tab(state, 1),
        KeyCode::BackTab => cycle_tab(state, -1),
        KeyCode::Char(c @ '1'..='9') => {
            if let Some(&tab) = state.tabs().get(c as usize - '1' as usize) {
                state.tab = tab;
            }
            vec![]
        }
        KeyCode::Char('u') => request_update(state),
        KeyCode::Char('O') => request_state(state, PrOp::Open),
        KeyCode::Char('D') => request_state(state, PrOp::Draft),
        KeyCode::Char('X') => request_state(state, PrOp::Close),
        KeyCode::Char('R') => reviewer::open_reviewers(state),
        KeyCode::Char('a') => label::open_labels(state),
        KeyCode::Char('M') => merge::open_merge(state),
        KeyCode::Char('j') | KeyCode::Down => {
            state.list_mut().move_by(1);
            vec![]
        }
        KeyCode::Char('k') | KeyCode::Up => {
            state.list_mut().move_by(-1);
            vec![]
        }
        KeyCode::Char('g') | KeyCode::Home => {
            state.list_mut().table.select(Some(0));
            vec![]
        }
        KeyCode::Char('G') | KeyCode::End => {
            let list = state.list_mut();
            let len = list.visible().len();
            list.table.select(Some(len.saturating_sub(1)));
            vec![]
        }
        KeyCode::Char('r') => refresh(state),
        KeyCode::Char('t') => {
            let list = state.list_mut();
            list.draft_only = !list.draft_only;
            list.clamp_selection();
            vec![]
        }
        KeyCode::Enter => match state.selected() {
            Some(p) => checkout(state, p.number, false),
            None => vec![],
        },
        KeyCode::Char('c') => {
            if let Some(items) = state
                .selected()
                .map(|p| copy_items(p, state.local.linear_workspace.as_deref()))
            {
                state.push(items);
            }
            vec![]
        }
        KeyCode::Char('o') => state
            .selected()
            .map(|p| Effect::ViewWeb(p.number))
            .into_iter()
            .collect(),
        KeyCode::Char('l') => open_linear(state),
        KeyCode::Char('L') => open_linear_review(state),
        KeyCode::Char('v') => chat::open_chat(state),
        KeyCode::Char('p') => pr_view::open_pr_view(state),
        KeyCode::Char('V') if state.tab.is_readonly() => state
            .selected()
            .map(|p| Effect::OpenUrl(review_url(&state.repo, p.number)))
            .into_iter()
            .collect(),
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

/// 端を越えたら反対の端に回る
fn cycle_tab(state: &mut AppState, delta: isize) -> Vec<Effect> {
    let tabs = state.tabs();
    let cur = tabs.iter().position(|&t| t == state.tab).unwrap_or(0) as isize;
    state.tab = tabs[(cur + delta).rem_euclid(tabs.len() as isize) as usize];
    vec![]
}

/// コピー選択画面のキー. 項目のキーならその値をコピーし, 他のキーは何もせず閉じる
fn copy(state: &mut AppState, key: KeyEvent) -> Vec<Effect> {
    let Some(items) = state.pop_copy_menu() else {
        return vec![];
    };
    let KeyCode::Char(c) = key.code else {
        return vec![];
    };
    let Some(item) = items.into_iter().find(|i| i.key == c) else {
        return vec![];
    };
    match item.value {
        Ok(text) => vec![Effect::Copy(text)],
        Err(reason) => {
            state.error(format!("{} をコピーできない: {reason}", item.label));
            vec![]
        }
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
    vec![Effect::OpenUrl(issue_url(&workspace, &key))]
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

/// open は 1 回で実行し, draft, close は同じキーの 2 回目で実行する.
/// 既にその状態の PR では何もしない.
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
        PrOp::Open => return change_state(state, n, op),
        PrOp::Draft | PrOp::Close => state.confirm_op = Some((n, op)),
    }
    vec![]
}

fn change_state(state: &mut AppState, n: u64, op: PrOp) -> Vec<Effect> {
    state.busy = Some(format!("#{n} を {}している…", op.phrase()));
    vec![Effect::ChangeState { number: n, op }]
}

/// 未コミットの変更の判定は git status の読み直しが要るので runtime で行う
fn checkout(state: &mut AppState, number: u64, force: bool) -> Vec<Effect> {
    if state.busy.is_some() {
        return vec![];
    }
    vec![Effect::Checkout { number, force }]
}

/// 各ファイルのテストで共有する AppState, PR, キー入力の組み立て
#[cfg(test)]
mod test_util {
    use std::path::PathBuf;

    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    use crate::app::state::{AppState, ChatView};
    use crate::domain::devin::{DevinConfig, DevinSession};
    use crate::domain::local::{LocalState, TabQuery};
    use crate::domain::pr::Pr;

    pub(super) fn state(prs: Vec<Pr>) -> AppState {
        let mut s = AppState::new(PathBuf::from("/tmp"), "o/r".into(), LocalState::default());
        s.mine.prs = prs;
        s
    }

    pub(super) fn pr(n: u64) -> Pr {
        let mut p = Pr::fixture(n, &format!("b{n}"), "main");
        p.head_ref_oid = format!("oid{n}");
        p
    }

    pub(super) fn key(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
    }

    pub(super) fn message(s: &AppState) -> (&str, bool) {
        let m = s.message.as_ref().expect("message");
        (&m.text, m.is_error)
    }

    pub(super) fn review_state(mine: Vec<Pr>, reviews: Vec<Pr>) -> AppState {
        let mut s = state(mine);
        s.review.prs = reviews;
        s
    }

    pub(super) fn custom_state(mine: Vec<Pr>, custom: Vec<Pr>) -> AppState {
        let local = LocalState {
            tabs: vec![TabQuery {
                name: "マージ予定".into(),
                query: "label:マージ予定".into(),
            }],
            ..Default::default()
        };
        let mut s = AppState::new(PathBuf::from("/tmp"), "o/r".into(), local);
        s.mine.prs = mine;
        s.custom[0].list.prs = custom;
        s
    }

    pub(super) fn devin_state(prs: Vec<Pr>) -> AppState {
        let mut s = state(prs);
        s.local.devin = Some(DevinConfig {
            token: "t".into(),
            org: "org-1".into(),
        });
        s
    }

    /// 会話ビューを直接開いた状態
    pub(super) fn chat_state() -> AppState {
        let mut s = devin_state(vec![pr(1)]);
        s.devin_sessions = vec![DevinSession::fixture(
            "devin-a",
            "https://github.com/o/r/pull/1",
        )];
        s.push(ChatView::new("devin-a".into(), 1));
        s
    }

    pub(super) fn chat(s: &AppState) -> &ChatView {
        s.chat().expect("chat")
    }

    pub(super) fn code(c: KeyCode) -> KeyEvent {
        KeyEvent::new(c, KeyModifiers::NONE)
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use crate::app::Screen;

    use pretty_assertions::assert_eq;
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    use super::test_util::{
        code, custom_state, devin_state, key, message, pr, review_state, state,
    };
    use super::{AUTO_REFRESH, handle_event, handle_key, refresh, tick};
    use crate::app::event::{Effect, Event};
    use crate::app::state::{AppState, Tab};
    use crate::domain::op::PrOp;
    use crate::domain::pr::Pr;

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
            }]
        );
        assert!(s.confirm_op.is_none());
        assert_eq!(s.busy.as_deref(), Some("#1 を draft にしている…"));
    }

    #[test]
    fn move_wraps_around() {
        let mut s = state(vec![pr(1), pr(2), pr(3)]);
        let selected = |s: &AppState| s.selected().map(|p| p.number);
        handle_key(&mut s, code(KeyCode::Up));
        assert_eq!(selected(&s), Some(3));
        handle_key(&mut s, code(KeyCode::Down));
        assert_eq!(selected(&s), Some(1));
        handle_key(&mut s, key('j'));
        handle_key(&mut s, key('j'));
        handle_key(&mut s, key('j'));
        assert_eq!(selected(&s), Some(1));
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
    fn esc_goes_back_one_screen() {
        let mut s = state(vec![pr(1)]);
        let esc = || KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
        handle_key(&mut s, key('p'));
        handle_key(&mut s, key('d'));
        // 別の画面の上に開いた画面は, 閉じると下の画面に戻る
        s.push(crate::app::LabelMenu::new(1, vec![]));
        handle_key(&mut s, esc());
        assert!(matches!(s.top(), Some(Screen::PrView(_))));
        // diff のページから説明とコメントのページ, 一覧の順に戻る
        handle_key(&mut s, esc());
        assert_eq!(s.pr_view().unwrap().page, crate::app::PrPage::Conversation);
        handle_key(&mut s, esc());
        assert!(s.screens.is_empty());
        assert!(handle_key(&mut s, esc()).is_empty());
    }

    #[test]
    fn popup_over_full_screen_leaves_list_confirm() {
        let mut s = state(vec![pr(1)]);
        handle_key(&mut s, key('p'));
        s.confirm_op = Some((1, PrOp::Draft));
        s.push(crate::app::LabelMenu::new(1, vec![]));
        // 一覧の確認は一覧に戻るまで残し, キーは一番上のポップアップが受け取る
        assert!(handle_key(&mut s, key('D')).is_empty());
        assert_eq!(s.confirm_op, Some((1, PrOp::Draft)));
        assert_eq!(s.label_menu().unwrap().query, "D");
    }

    #[test]
    fn pop_by_kind_only_closes_top() {
        let mut s = state(vec![pr(1)]);
        handle_key(&mut s, key('p'));
        s.push(crate::app::LabelMenu::new(1, vec![]));
        // 下にある PR ビューは抜かない. 上のラベル選択画面が浮くため
        assert!(s.pop_pr_view().is_none());
        assert_eq!(s.screens.len(), 2);
        assert!(s.pop_label_menu().is_some());
        assert!(s.pop_pr_view().is_some());
        assert!(s.screens.is_empty());
    }

    #[test]
    fn quit_keys() {
        let mut s = state(vec![]);
        assert_eq!(handle_key(&mut s, key('q')), vec![Effect::Quit]);
        // Esc は戻るキーなので一覧では終わらない
        let esc = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
        assert!(handle_key(&mut s, esc).is_empty());
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
    fn tab_switch_keeps_selection_per_tab() {
        let mut s = review_state(vec![pr(1), pr(2)], vec![pr(10), pr(11), pr(12)]);
        handle_key(&mut s, key('j'));
        assert_eq!(s.selected().map(|p| p.number), Some(2));

        handle_key(&mut s, KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(s.tab, Tab::Review);
        assert_eq!(s.selected().map(|p| p.number), Some(10));
        handle_key(&mut s, key('G'));
        assert_eq!(s.selected().map(|p| p.number), Some(12));

        handle_key(&mut s, key('1'));
        assert_eq!(s.tab, Tab::Mine);
        assert_eq!(s.selected().map(|p| p.number), Some(2));
        handle_key(&mut s, KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT));
        assert_eq!(s.selected().map(|p| p.number), Some(12));
        handle_key(&mut s, key('2'));
        assert_eq!(s.tab, Tab::Review);
    }

    #[test]
    fn draft_filter_is_per_tab() {
        let mut d = pr(11);
        d.is_draft = true;
        let mut s = review_state(vec![pr(1)], vec![pr(10), d]);
        handle_key(&mut s, key('2'));
        handle_key(&mut s, key('t'));
        assert!(s.review.draft_only && !s.mine.draft_only);
        assert_eq!(s.selected().map(|p| p.number), Some(11));
    }

    #[test]
    fn review_tab_rejects_mutating_ops() {
        let mut s = review_state(vec![], vec![pr(10)]);
        s.local.reviewers = vec!["alice".into()];
        handle_key(&mut s, key('2'));
        for c in ['u', 'O', 'D', 'X', 'R', 'a', 'M'] {
            assert!(handle_key(&mut s, key(c)).is_empty(), "{c}");
            assert_eq!(
                message(&s),
                (&*format!("{c} はレビュータブでは使えない"), true)
            );
            assert!(s.confirm_op.is_none() && s.confirm_update.is_none());
            assert!(s.label_menu().is_none() && s.reviewer_menu().is_none());
            assert!(s.merge_menu().is_none());
        }
    }

    #[test]
    fn copy_menu_copies_chosen_item() {
        let mut s = review_state(vec![pr(1)], vec![pr(10)]);
        handle_key(&mut s, key('2'));
        assert!(handle_key(&mut s, key('c')).is_empty());
        assert!(s.copy_menu().is_some());
        assert_eq!(
            handle_key(&mut s, key('b')),
            vec![Effect::Copy("b10".into())]
        );
        assert!(s.copy_menu().is_none());
        handle_key(&mut s, key('c'));
        assert_eq!(
            handle_key(&mut s, key('n')),
            vec![Effect::Copy("#10".into())]
        );
    }

    #[test]
    fn copy_menu_unavailable_item_is_error() {
        let mut s = state(vec![pr(1)]);
        handle_key(&mut s, key('c'));
        assert!(handle_key(&mut s, key('i')).is_empty());
        assert_eq!(
            message(&s),
            ("Linear issue キー をコピーできない: issue キーが無い", true)
        );
        assert!(s.copy_menu().is_none());
    }

    #[test]
    fn copy_menu_other_key_closes() {
        let mut s = state(vec![pr(1), pr(2)]);
        handle_key(&mut s, key('c'));
        assert!(handle_key(&mut s, key('j')).is_empty());
        assert!(s.copy_menu().is_none());
        // 閉じるだけで移動はしない
        assert_eq!(s.mine.table.selected(), Some(0));
        assert!(s.message.is_none());
    }

    #[test]
    fn review_tab_allows_browsing_ops() {
        let mut s = review_state(vec![pr(1)], vec![pr(10)]);
        handle_key(&mut s, key('2'));
        assert_eq!(
            handle_key(&mut s, code(KeyCode::Enter)),
            vec![Effect::Checkout {
                number: 10,
                force: false
            }]
        );
        assert_eq!(handle_key(&mut s, key('o')), vec![Effect::ViewWeb(10)]);
        assert_eq!(handle_key(&mut s, key('L')), vec![Effect::LinearReview(10)]);
        assert_eq!(
            handle_key(&mut s, key('V')),
            vec![Effect::OpenUrl(
                "https://app.devin.ai/review/o/r/pull/10".into()
            )]
        );
    }

    #[test]
    fn custom_tab_switch_and_cycle() {
        let mut s = custom_state(vec![pr(1)], vec![pr(20), pr(21)]);
        handle_key(&mut s, key('3'));
        assert_eq!(s.tab, Tab::Custom(0));
        assert_eq!(s.selected().map(|p| p.number), Some(20));
        // 無いタブの数字は何もしない
        handle_key(&mut s, key('4'));
        assert_eq!(s.tab, Tab::Custom(0));
        handle_key(&mut s, code(KeyCode::Tab));
        assert_eq!(s.tab, Tab::Mine);
        handle_key(&mut s, KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT));
        assert_eq!(s.tab, Tab::Custom(0));
        handle_key(&mut s, KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT));
        assert_eq!(s.tab, Tab::Review);
    }

    #[test]
    fn custom_tab_rejects_mutating_ops_and_allows_browsing() {
        let mut s = custom_state(vec![pr(1)], vec![pr(20)]);
        s.local.reviewers = vec!["alice".into()];
        handle_key(&mut s, key('3'));
        for c in ['u', 'O', 'D', 'X', 'R', 'M'] {
            assert!(handle_key(&mut s, key(c)).is_empty(), "{c}");
            assert_eq!(
                message(&s),
                (&*format!("{c} はマージ予定タブでは使えない"), true)
            );
        }
        assert_eq!(handle_key(&mut s, key('o')), vec![Effect::ViewWeb(20)]);
        assert_eq!(
            handle_key(&mut s, key('V')),
            vec![Effect::OpenUrl(
                "https://app.devin.ai/review/o/r/pull/20".into()
            )]
        );
    }

    #[test]
    fn devin_review_is_not_in_mine_tab() {
        let mut s = state(vec![pr(1)]);
        assert!(handle_key(&mut s, key('V')).is_empty());
    }

    #[test]
    fn key_clears_message() {
        let mut s = state(vec![pr(1), pr(2)]);
        handle_event(&mut s, Event::CheckedOut(1, false, Ok(String::new())));
        assert_eq!(message(&s), ("#1 をチェックアウトした", false));
        handle_key(&mut s, key('j'));
        assert!(s.message.is_none());
    }

    #[test]
    fn refresh_fetches_sessions_only_with_config() {
        let mut s = state(vec![]);
        assert_eq!(refresh(&mut s), vec![Effect::Fetch, Effect::FetchReviews]);
        assert!(!s.sessions_loading);

        let mut s = devin_state(vec![]);
        assert_eq!(
            refresh(&mut s),
            vec![Effect::Fetch, Effect::FetchReviews, Effect::FetchSessions]
        );
        assert!(s.sessions_loading);
        // 取得中はどちらも出さない
        assert!(refresh(&mut s).is_empty());

        // PR の取得中でも session は独立に取る
        s.sessions_loading = false;
        assert_eq!(refresh(&mut s), vec![Effect::FetchSessions]);
    }

    #[test]
    fn tick_auto_refresh() {
        let mut s = state(vec![]);
        let now = Instant::now();
        assert!(tick(&mut s, now).is_empty());
        s.last_fetch = Some(now);
        assert!(tick(&mut s, now + Duration::from_secs(1)).is_empty());
        assert_eq!(
            tick(&mut s, now + AUTO_REFRESH),
            vec![Effect::Fetch, Effect::FetchReviews]
        );
        assert!(s.mine.loading && s.review.loading);
    }
}
