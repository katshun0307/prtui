//! キー入力とスレッドの結果を受けて AppState を更新し, 実行すべき副作用を Effect で返す.
//! スレッドの起動や外部コマンドの実行はしない (runtime の責務).

use std::time::{Duration, Instant};

use chrono::Utc;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::event::{Effect, Event, WatchUntil};
use super::state::{AppState, ChatView, Tab};
use crate::domain::devin::review_url;
use crate::domain::linear::linear_issue_key;
use crate::domain::op::PrOp;
use crate::domain::pr::Pr;
use crate::domain::review::review_targets;
use crate::domain::stack::arrange;

/// 全件を取り直す間隔
pub const AUTO_REFRESH: Duration = Duration::from_secs(300);
/// 会話ビューを開いている間にメッセージを取り直す間隔
pub const CHAT_POLL: Duration = Duration::from_secs(5);

/// 両タブの全件を取り直す. 取得中のタブは飛ばす.
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
    if let Some(chat) = state.chat.as_mut()
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

pub fn handle_event(state: &mut AppState, ev: Event) -> Vec<Effect> {
    match ev {
        Event::Fetched(res) => {
            state.mine.loading = false;
            match res {
                Ok(prs) => {
                    state.mine.fetched_at = Some(Utc::now());
                    state.mine.from_cache = false;
                    state.mine.replace(prs);
                    vec![Effect::SaveCache]
                }
                Err(e) => {
                    state.error(format!("取得失敗: {e}"));
                    vec![]
                }
            }
        }
        // 失敗はフッターではなくレビュータブの一覧に出し, 自分タブの表示を邪魔しない
        Event::FetchedReviews(res) => {
            let list = &mut state.review;
            list.loading = false;
            match res {
                Ok(prs) => {
                    list.fetched_at = Some(Utc::now());
                    list.from_cache = false;
                    list.error = None;
                    list.replace(prs);
                    vec![Effect::SaveCache]
                }
                Err(e) => {
                    list.error = Some(format!("取得失敗: {e}"));
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
                        .mine
                        .prs
                        .iter()
                        .filter(|p| p.number != n)
                        .cloned()
                        .collect();
                    prs.extend(pr);
                    state.mine.replace(arrange(prs));
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
        Event::DevinSessions(res) => {
            state.sessions_loading = false;
            match res {
                // 開いている session が一覧から消えても会話ビューはそのまま残す
                Ok(sessions) => {
                    state.devin_sessions = sessions;
                    vec![Effect::SaveCache]
                }
                Err(e) => {
                    state.error(format!("Devin session の取得失敗: {e}"));
                    vec![]
                }
            }
        }
        Event::DevinMessages(id, res) => {
            // 閉じたか別の session に移った後に届いた結果は捨てる
            let Some(chat) = state.chat.as_mut().filter(|c| c.session_id == id) else {
                return vec![];
            };
            chat.fetching = false;
            match res {
                Ok(messages) => {
                    // 送信失敗のエラーを次のポーリングで消さない. 消すのは送信と r, i, e
                    chat.messages = messages;
                    chat.loaded = true;
                }
                Err(e) => chat.error = Some(format!("メッセージの取得失敗: {e}")),
            }
            vec![]
        }
        Event::DevinSent(id, res) => {
            let Some(chat) = state.chat.as_mut().filter(|c| c.session_id == id) else {
                return vec![];
            };
            chat.sending = false;
            match res {
                Ok(session) => {
                    chat.draft.clear();
                    chat.typing = false;
                    // 送ったメッセージをすぐ出すため次の tick で取り直す
                    chat.last_poll = None;
                    match state.devin_sessions.iter_mut().find(|s| s.id == session.id) {
                        Some(s) => *s = session,
                        None => state.devin_sessions.push(session),
                    }
                    vec![Effect::SaveCache]
                }
                Err(e) => {
                    chat.error = Some(format!("送信失敗: {e}"));
                    vec![]
                }
            }
        }
        Event::Composed(res) => {
            let Some(chat) = state.chat.as_mut() else {
                return vec![];
            };
            match res {
                Ok(Some(text)) => {
                    chat.draft = text.trim_end_matches(['\n', '\r']).to_string();
                    send(chat)
                }
                // エディタを非 0 で終えたら取りやめ. draft は変えない
                Ok(None) => vec![],
                Err(e) => {
                    chat.error = Some(format!("エディタの実行失敗: {e}"));
                    vec![]
                }
            }
        }
    }
}

pub fn handle_key(state: &mut AppState, key: KeyEvent) -> Vec<Effect> {
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
        return vec![Effect::Quit];
    }
    if state.chat.is_some() {
        return chat_key(state, key);
    }
    // 前の操作の結果はキーを押したら消し, フッターをキー案内に戻す
    state.message = None;
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
    if state.tab == Tab::Review
        && let KeyCode::Char(c @ ('u' | 'O' | 'D' | 'X' | 'R')) = key.code
    {
        state.error(format!("{c} はレビュータブでは使えない"));
        return vec![];
    }
    match key.code {
        KeyCode::Char('q') | KeyCode::Esc => vec![Effect::Quit],
        KeyCode::Tab | KeyCode::BackTab => switch_tab(state, state.tab.next()),
        KeyCode::Char('1') => switch_tab(state, Tab::Mine),
        KeyCode::Char('2') => switch_tab(state, Tab::Review),
        KeyCode::Char('u') => request_update(state),
        KeyCode::Char('O') => request_state(state, PrOp::Open),
        KeyCode::Char('D') => request_state(state, PrOp::Draft),
        KeyCode::Char('X') => request_state(state, PrOp::Close),
        KeyCode::Char('R') => request_state(state, PrOp::Review),
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
        KeyCode::Enter | KeyCode::Char('c') => checkout(state),
        KeyCode::Char('o') => state
            .selected()
            .map(|p| Effect::ViewWeb(p.number))
            .into_iter()
            .collect(),
        KeyCode::Char('l') => open_linear(state),
        KeyCode::Char('L') => open_linear_review(state),
        KeyCode::Char('v') => open_chat(state),
        KeyCode::Char('V') if state.tab == Tab::Review => state
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

fn switch_tab(state: &mut AppState, tab: Tab) -> Vec<Effect> {
    state.tab = tab;
    vec![]
}

/// 選択中の PR に紐づく最新の Devin session の会話ビューを開く.
/// メッセージは次の tick で取る
fn open_chat(state: &mut AppState) -> Vec<Effect> {
    if state.local.devin.is_none() {
        state.error(NO_DEVIN_CONFIG.into());
        return vec![];
    }
    let Some(pr) = state.selected() else {
        return vec![];
    };
    let n = pr.number;
    let Some(id) = state.devin_for(pr).map(|(s, _)| s.id.clone()) else {
        state.error(format!("#{n} に Devin session が無い"));
        return vec![];
    };
    state.chat = Some(ChatView::new(id, n));
    vec![]
}

const NO_DEVIN_CONFIG: &str = "Devin の設定が無い. git config prtui.devin-token <token> と prtui.devin-org <org-id> を設定する";

/// 会話ビューのキー. 1 行入力中は Enter と Esc と Backspace 以外を文字として draft に足す
fn chat_key(state: &mut AppState, key: KeyEvent) -> Vec<Effect> {
    let url = state.chat_session().map(|s| s.url.clone());
    let Some(chat) = state.chat.as_mut() else {
        return vec![];
    };
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    if chat.typing {
        match key.code {
            KeyCode::Enter => return send(chat),
            KeyCode::Esc => chat.typing = false,
            KeyCode::Backspace => {
                chat.draft.pop();
            }
            KeyCode::Char(c) if !ctrl => chat.draft.push(c),
            _ => {}
        }
        return vec![];
    }
    let half = (chat.viewport_height / 2).max(1) as isize;
    match key.code {
        KeyCode::Char('d') if ctrl => scroll_by(chat, half),
        KeyCode::Char('u') if ctrl => scroll_by(chat, -half),
        KeyCode::PageDown => scroll_by(chat, half),
        KeyCode::PageUp => scroll_by(chat, -half),
        KeyCode::Char('j') | KeyCode::Down => scroll_by(chat, 1),
        KeyCode::Char('k') | KeyCode::Up => scroll_by(chat, -1),
        KeyCode::Char('g') => chat.scroll = Some(0),
        KeyCode::Char('G') => chat.scroll = None,
        KeyCode::Char('i') => {
            chat.typing = true;
            chat.error = None;
        }
        KeyCode::Char('e') if !chat.sending => {
            chat.error = None;
            return vec![Effect::Compose {
                draft: chat.draft.clone(),
            }];
        }
        KeyCode::Char('o') => match url {
            Some(url) => return vec![Effect::OpenUrl(url)],
            None => chat.error = Some(format!("{} が session 一覧に無い", chat.session_id)),
        },
        KeyCode::Char('r') => {
            chat.last_poll = None;
            chat.error = None;
        }
        KeyCode::Esc | KeyCode::Char('q') if !chat.sending => state.chat = None,
        _ => {}
    }
    vec![]
}

/// 表示位置を動かす. 末尾まで行ったら末尾追従に戻す
fn scroll_by(chat: &mut ChatView, delta: isize) {
    let cur = chat.scroll.unwrap_or(chat.max_scroll) as isize;
    let next = (cur + delta).clamp(0, chat.max_scroll as isize) as usize;
    chat.scroll = (next < chat.max_scroll).then_some(next);
}

/// draft を送る. 空白だけか送信中なら何もしない
fn send(chat: &mut ChatView) -> Vec<Effect> {
    let text = chat.draft.trim();
    if text.is_empty() || chat.sending {
        return vec![];
    }
    let text = text.to_string();
    // 送信中に打った文字が, 成功時の draft.clear() で送らずに消えないよう入力を抜ける
    chat.typing = false;
    chat.sending = true;
    chat.error = None;
    vec![Effect::SendMessage {
        session_id: chat.session_id.clone(),
        text,
    }]
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
    use std::time::{Duration, Instant};

    use anyhow::anyhow;
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    use super::{AUTO_REFRESH, CHAT_POLL, handle_event, handle_key, refresh, tick};
    use crate::app::event::{Effect, Event, WatchUntil};
    use crate::app::state::{AppState, ChatView, Tab};
    use crate::domain::devin::{DevinConfig, DevinSession};
    use crate::domain::local::LocalState;
    use crate::domain::op::PrOp;
    use crate::domain::pr::{Pr, Reviewer, ReviewerState};

    fn state(prs: Vec<Pr>) -> AppState {
        let mut s = AppState::new(PathBuf::from("/tmp"), "o/r".into(), LocalState::default());
        s.mine.prs = prs;
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

    fn review_state(mine: Vec<Pr>, reviews: Vec<Pr>) -> AppState {
        let mut s = state(mine);
        s.review.prs = reviews;
        s
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
        for c in ['u', 'O', 'D', 'X', 'R'] {
            assert!(handle_key(&mut s, key(c)).is_empty(), "{c}");
            assert_eq!(
                message(&s),
                (&*format!("{c} はレビュータブでは使えない"), true)
            );
            assert!(s.confirm_op.is_none() && s.confirm_update.is_none());
        }
    }

    #[test]
    fn review_tab_allows_browsing_ops() {
        let mut s = review_state(vec![pr(1)], vec![pr(10)]);
        handle_key(&mut s, key('2'));
        assert_eq!(handle_key(&mut s, key('c')), vec![Effect::Checkout(10)]);
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
    fn devin_review_is_review_tab_only() {
        let mut s = state(vec![pr(1)]);
        assert!(handle_key(&mut s, key('V')).is_empty());
    }

    #[test]
    fn fetched_reviews_error_stays_in_review_tab() {
        let mut s = review_state(vec![], vec![pr(10)]);
        s.review.loading = true;
        let effects = handle_event(&mut s, Event::FetchedReviews(Err(anyhow!("boom"))));
        assert!(effects.is_empty());
        assert!(!s.review.loading);
        assert_eq!(s.review.prs.len(), 1);
        assert_eq!(s.review.error.as_deref(), Some("取得失敗: boom"));
        assert!(s.message.is_none());

        let effects = handle_event(&mut s, Event::FetchedReviews(Ok(vec![pr(11)])));
        assert_eq!(effects, vec![Effect::SaveCache]);
        assert!(s.review.error.is_none());
        assert!(s.review.fetched_at.is_some());
        assert_eq!(s.review.prs[0].number, 11);
    }

    #[test]
    fn key_clears_message() {
        let mut s = state(vec![pr(1), pr(2)]);
        handle_event(&mut s, Event::CheckedOut(1, Ok(String::new())));
        assert_eq!(message(&s), ("#1 をチェックアウトした", false));
        handle_key(&mut s, key('j'));
        assert!(s.message.is_none());
    }

    #[test]
    fn fetched_replaces_and_saves() {
        let mut s = state(vec![pr(1)]);
        s.mine.loading = true;
        s.mine.from_cache = true;
        let effects = handle_event(&mut s, Event::Fetched(Ok(vec![pr(2), pr(3)])));
        assert_eq!(effects, vec![Effect::SaveCache]);
        assert!(!s.mine.loading);
        assert!(!s.mine.from_cache);
        assert!(s.mine.fetched_at.is_some());
        let numbers: Vec<u64> = s.mine.prs.iter().map(|p| p.number).collect();
        assert_eq!(numbers, vec![2, 3]);
    }

    #[test]
    fn fetched_err_keeps_prs() {
        let mut s = state(vec![pr(1)]);
        s.mine.loading = true;
        let effects = handle_event(&mut s, Event::Fetched(Err(anyhow!("boom"))));
        assert!(effects.is_empty());
        assert!(!s.mine.loading);
        assert_eq!(s.mine.prs.len(), 1);
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

    fn devin_state(prs: Vec<Pr>) -> AppState {
        let mut s = state(prs);
        s.local.devin = Some(DevinConfig {
            token: "t".into(),
            org: "org-1".into(),
        });
        s
    }

    /// 会話ビューを直接開いた状態
    fn chat_state() -> AppState {
        let mut s = devin_state(vec![pr(1)]);
        s.devin_sessions = vec![DevinSession::fixture(
            "devin-a",
            "https://github.com/o/r/pull/1",
        )];
        s.chat = Some(ChatView::new("devin-a".into(), 1));
        s
    }

    fn chat(s: &AppState) -> &ChatView {
        s.chat.as_ref().expect("chat")
    }

    fn code(c: KeyCode) -> KeyEvent {
        KeyEvent::new(c, KeyModifiers::NONE)
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
    fn view_without_config_is_error() {
        let mut s = state(vec![pr(1)]);
        assert!(handle_key(&mut s, key('v')).is_empty());
        assert!(s.chat.is_none());
        assert!(message(&s).0.starts_with("Devin の設定が無い"));
        assert!(message(&s).1);
    }

    #[test]

    fn view_without_session_is_error() {
        let mut s = devin_state(vec![pr(1)]);
        assert!(handle_key(&mut s, key('v')).is_empty());
        assert!(s.chat.is_none());
        assert_eq!(message(&s), ("#1 に Devin session が無い", true));
    }

    #[test]

    fn view_opens_latest_session() {
        let mut p = pr(1);
        p.url = "https://github.com/o/r/pull/1".into();
        let mut s = devin_state(vec![p]);
        let old = DevinSession::fixture("devin-old", "https://github.com/o/r/pull/1");
        let mut new = DevinSession::fixture("devin-new", "https://github.com/O/R/pull/1/");
        new.updated_at = old.updated_at + chrono::Duration::seconds(1);
        s.devin_sessions = vec![old, new];
        s.info("x".into());
        assert!(handle_key(&mut s, key('v')).is_empty());
        assert_eq!(chat(&s).session_id, "devin-new");
        assert!(chat(&s).last_poll.is_none());
        assert!(s.message.is_none());
    }

    #[test]
    fn tick_polls_messages() {
        let mut s = chat_state();
        let now = Instant::now();
        assert_eq!(
            tick(&mut s, now),
            vec![Effect::FetchMessages("devin-a".into())]
        );
        assert!(chat(&s).fetching);
        // 取得中は出さない
        assert!(tick(&mut s, now + CHAT_POLL * 2).is_empty());

        handle_event(&mut s, Event::DevinMessages("devin-a".into(), Ok(vec![])));
        assert!(!chat(&s).fetching);
        assert!(chat(&s).loaded);
        // 間隔が空いていなければ出さない
        assert!(tick(&mut s, now + Duration::from_secs(1)).is_empty());
        assert_eq!(
            tick(&mut s, now + CHAT_POLL),
            vec![Effect::FetchMessages("devin-a".into())]
        );
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

    #[test]
    fn chat_r_polls_now() {
        let mut s = chat_state();
        s.chat.as_mut().unwrap().last_poll = Some(Instant::now());
        handle_key(&mut s, key('r'));
        assert!(chat(&s).last_poll.is_none());
    }

    #[test]
    fn chat_typing_and_send() {
        let mut s = chat_state();
        assert!(handle_key(&mut s, key('i')).is_empty());
        assert!(chat(&s).typing);
        // 入力中の q は文字として入る
        for c in " qx".chars() {
            handle_key(&mut s, key(c));
        }
        handle_key(&mut s, code(KeyCode::Backspace));
        assert_eq!(chat(&s).draft, " q");
        assert!(s.chat.is_some());
        assert_eq!(
            handle_key(&mut s, code(KeyCode::Enter)),
            vec![Effect::SendMessage {
                session_id: "devin-a".into(),
                text: "q".into(),
            }]
        );
        assert!(chat(&s).sending);
        // 送信中は重ねて送らない
        assert!(handle_key(&mut s, code(KeyCode::Enter)).is_empty());
    }

    #[test]
    fn chat_blank_draft_is_not_sent() {
        let mut s = chat_state();
        handle_key(&mut s, key('i'));
        handle_key(&mut s, key(' '));
        assert!(handle_key(&mut s, code(KeyCode::Enter)).is_empty());
        assert!(!chat(&s).sending);
    }

    #[test]
    fn chat_esc_stops_typing_then_closes() {
        let mut s = chat_state();
        handle_key(&mut s, key('i'));
        handle_key(&mut s, key('a'));
        handle_key(&mut s, code(KeyCode::Esc));
        assert!(!chat(&s).typing);
        assert_eq!(chat(&s).draft, "a");
        assert!(handle_key(&mut s, code(KeyCode::Esc)).is_empty());
        assert!(s.chat.is_none());
    }

    #[test]
    fn chat_close_ignored_while_sending() {
        let mut s = chat_state();
        s.chat.as_mut().unwrap().sending = true;
        handle_key(&mut s, key('q'));
        handle_key(&mut s, code(KeyCode::Esc));
        assert!(s.chat.is_some());
        // e も送信中は無視する
        assert!(handle_key(&mut s, key('e')).is_empty());
        // Ctrl-C は常に終了
        let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert_eq!(handle_key(&mut s, ctrl_c), vec![Effect::Quit]);
    }

    #[test]
    fn chat_compose_and_open() {
        let mut s = chat_state();
        s.chat.as_mut().unwrap().draft = "hi".into();
        assert_eq!(
            handle_key(&mut s, key('e')),
            vec![Effect::Compose { draft: "hi".into() }]
        );
        assert_eq!(
            handle_key(&mut s, key('o')),
            vec![Effect::OpenUrl(
                "https://app.devin.ai/sessions/devin-a".into()
            )]
        );
    }

    #[test]
    fn chat_scroll() {
        let mut s = chat_state();
        {
            let c = s.chat.as_mut().unwrap();
            c.max_scroll = 10;
            c.viewport_height = 6;
        }
        handle_key(&mut s, key('k'));
        assert_eq!(chat(&s).scroll, Some(9));
        handle_key(&mut s, code(KeyCode::PageUp));
        assert_eq!(chat(&s).scroll, Some(6));
        handle_key(
            &mut s,
            KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL),
        );
        assert_eq!(chat(&s).scroll, Some(3));
        handle_key(&mut s, key('g'));
        assert_eq!(chat(&s).scroll, Some(0));
        handle_key(&mut s, key('k'));
        assert_eq!(chat(&s).scroll, Some(0));
        // 末尾まで戻ったら追従に戻す
        handle_key(&mut s, code(KeyCode::PageDown));
        handle_key(&mut s, code(KeyCode::PageDown));
        handle_key(&mut s, code(KeyCode::PageDown));
        handle_key(&mut s, code(KeyCode::PageDown));
        assert_eq!(chat(&s).scroll, None);
        handle_key(&mut s, key('g'));
        handle_key(&mut s, key('G'));
        assert_eq!(chat(&s).scroll, None);
    }

    #[test]
    fn messages_for_other_session_are_dropped() {
        let mut s = chat_state();
        s.chat.as_mut().unwrap().fetching = true;
        let effects = handle_event(&mut s, Event::DevinMessages("devin-b".into(), Ok(vec![])));
        assert!(effects.is_empty());
        assert!(chat(&s).fetching);
        assert!(!chat(&s).loaded);
    }

    #[test]
    fn messages_err_goes_to_chat() {
        let mut s = chat_state();
        s.chat.as_mut().unwrap().fetching = true;
        handle_event(
            &mut s,
            Event::DevinMessages("devin-a".into(), Err(anyhow!("boom"))),
        );
        assert!(!chat(&s).fetching);
        assert_eq!(
            chat(&s).error.as_deref(),
            Some("メッセージの取得失敗: boom")
        );
    }

    #[test]
    fn sent_ok_clears_draft_and_updates_session() {
        let mut s = chat_state();
        {
            let c = s.chat.as_mut().unwrap();
            c.draft = "hi".into();
            c.typing = true;
            c.sending = true;
            c.last_poll = Some(Instant::now());
        }
        let mut sent = DevinSession::fixture("devin-a", "https://github.com/o/r/pull/1");
        sent.status = crate::domain::devin::DevinStatus::Resuming;
        let effects = handle_event(&mut s, Event::DevinSent("devin-a".into(), Ok(sent)));
        assert_eq!(effects, vec![Effect::SaveCache]);
        let c = chat(&s);
        assert!(!c.sending && !c.typing && c.draft.is_empty() && c.last_poll.is_none());
        assert_eq!(s.devin_sessions.len(), 1);
        assert_eq!(
            s.chat_session().map(|x| x.status),
            Some(crate::domain::devin::DevinStatus::Resuming)
        );
    }

    #[test]
    fn sent_err_keeps_draft() {
        let mut s = chat_state();
        {
            let c = s.chat.as_mut().unwrap();
            c.draft = "hi".into();
            c.sending = true;
        }
        let effects = handle_event(
            &mut s,
            Event::DevinSent("devin-a".into(), Err(anyhow!("409 conflict"))),
        );
        assert!(effects.is_empty());
        let c = chat(&s);
        assert!(!c.sending);
        assert_eq!(c.draft, "hi");
        assert_eq!(c.error.as_deref(), Some("送信失敗: 409 conflict"));

        // 次のポーリングが成功しても送信失敗は消さない. i で消す
        handle_event(&mut s, Event::DevinMessages("devin-a".into(), Ok(vec![])));
        assert_eq!(chat(&s).error.as_deref(), Some("送信失敗: 409 conflict"));
        handle_key(&mut s, key('i'));
        assert!(chat(&s).error.is_none());
    }

    #[test]
    fn typing_after_enter_does_not_touch_sent_draft() {
        let mut s = chat_state();
        handle_key(&mut s, key('i'));
        handle_key(&mut s, key('h'));
        let effects = handle_key(&mut s, code(KeyCode::Enter));
        assert_eq!(
            effects,
            vec![Effect::SendMessage {
                session_id: "devin-a".into(),
                text: "h".into(),
            }]
        );
        assert!(!chat(&s).typing);
        // 送信中は入力を抜けているので文字は draft に入らない
        handle_key(&mut s, key('x'));
        assert_eq!(chat(&s).draft, "h");
    }

    #[test]
    fn composed_sends_or_cancels() {
        let mut s = chat_state();
        s.chat.as_mut().unwrap().draft = "old".into();
        assert!(handle_event(&mut s, Event::Composed(Ok(None))).is_empty());
        assert_eq!(chat(&s).draft, "old");

        let effects = handle_event(&mut s, Event::Composed(Ok(Some("a\nb\n\n".into()))));
        assert_eq!(chat(&s).draft, "a\nb");
        assert_eq!(
            effects,
            vec![Effect::SendMessage {
                session_id: "devin-a".into(),
                text: "a\nb".into(),
            }]
        );

        let mut s = chat_state();
        assert!(handle_event(&mut s, Event::Composed(Ok(Some(" \n".into())))).is_empty());
        assert!(!chat(&s).sending);
        handle_event(&mut s, Event::Composed(Err(anyhow!("boom"))));
        assert!(chat(&s).error.is_some());
    }

    #[test]
    fn sessions_event() {
        let mut s = chat_state();
        s.sessions_loading = true;
        let effects = handle_event(&mut s, Event::DevinSessions(Ok(vec![])));
        assert_eq!(effects, vec![Effect::SaveCache]);
        assert!(!s.sessions_loading);
        // 開いている session が一覧から消えても会話ビューは残す
        assert!(s.chat.is_some());
        assert!(s.chat_session().is_none());

        s.sessions_loading = true;
        assert!(handle_event(&mut s, Event::DevinSessions(Err(anyhow!("boom")))).is_empty());
        assert!(!s.sessions_loading);
        assert_eq!(message(&s), ("Devin session の取得失敗: boom", true));
    }
}
