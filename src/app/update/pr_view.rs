//! PR ビュー (PR の説明とコメント) を開く, キー操作.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::event::Effect;
use crate::app::state::{AppState, Composing, PrView};
use crate::domain::conversation::{ReviewAction, editable_body, same_body};

/// 選択中の PR の PR ビューを開いて説明とコメントを取る. どのタブでも開ける
pub(super) fn open_pr_view(state: &mut AppState) -> Vec<Effect> {
    let Some(p) = state.selected() else {
        return vec![];
    };
    let n = p.number;
    state.pr_view = Some(PrView::new(p));
    vec![Effect::FetchConversation(n)]
}

/// PR ビューのキー. スクロール, ブラウザ, 取り直し, レビュー (`!`), 説明の編集 (`e`), 閉じる
pub(super) fn pr_view_key(state: &mut AppState, key: KeyEvent) -> Vec<Effect> {
    let approve_body = state.local.approve_body.clone();
    let Some(view) = state.pr_view.as_mut() else {
        return vec![];
    };
    view.notice = None;
    if view.choosing_review {
        view.choosing_review = false;
        let action = [ReviewAction::Approve, ReviewAction::Comment]
            .into_iter()
            .find(|a| key.code == KeyCode::Char(a.key()));
        return match action {
            // 本文はエディタで書く. 戻ったら `Event::Composed` で送る.
            // 承認は `prtui.approve-body` を初期値にする. 消して保存すれば本文なしで承認する
            Some(a) => {
                view.composing = Some(Composing::Review(a));
                let draft = match a {
                    ReviewAction::Approve => approve_body.unwrap_or_default(),
                    ReviewAction::Comment => String::new(),
                };
                vec![Effect::Compose { draft }]
            }
            None => {
                view.notice = Some("レビューを取りやめた".into());
                vec![]
            }
        };
    }
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let half = (view.viewport_height / 2).max(1) as isize;
    match key.code {
        KeyCode::Char('d') if ctrl => scroll_by(view, half),
        KeyCode::Char('u') if ctrl => scroll_by(view, -half),
        KeyCode::PageDown | KeyCode::Char(' ') => scroll_by(view, half * 2),
        KeyCode::PageUp => scroll_by(view, -half * 2),
        KeyCode::Char('j') | KeyCode::Down => scroll_by(view, 1),
        KeyCode::Char('k') | KeyCode::Up => scroll_by(view, -1),
        KeyCode::Char('g') | KeyCode::Home => view.scroll = 0,
        KeyCode::Char('G') | KeyCode::End => view.scroll = view.max_scroll,
        KeyCode::Char('o') => return vec![Effect::ViewWeb(view.number)],
        KeyCode::Char('!') if !view.is_sending() => {
            view.choosing_review = true;
            view.error = None;
        }
        KeyCode::Char('e') if !view.is_sending() => return edit_body(view),
        KeyCode::Char('r') if !view.fetching => {
            view.fetching = true;
            view.error = None;
            return vec![Effect::FetchConversation(view.number)];
        }
        // 送信中に閉じると結果を出す場所が無くなる
        KeyCode::Esc | KeyCode::Char('q') if !view.is_sending() => state.pr_view = None,
        _ => {}
    }
    vec![]
}

/// 説明をエディタで開く. 前に更新に失敗した内容があればそれを開く.
/// 表示中の説明を基準にし, 取り直し中でも待たない (GitHub 側の変更は更新時に確かめる)
fn edit_body(view: &mut PrView) -> Vec<Effect> {
    let Some(c) = &view.conversation else {
        view.notice = Some("説明を取得中".into());
        return vec![];
    };
    let original = c.body.clone();
    let draft = view
        .body_draft
        .clone()
        .unwrap_or_else(|| editable_body(&original));
    view.error = None;
    view.composing = Some(Composing::Body { original });
    vec![Effect::Compose { draft }]
}

/// エディタで書いた説明で PR を更新する. 変えていなければ何もしない.
/// 空にするのは許す (説明を消したいこともある)
pub(super) fn save_body(view: &mut PrView, original: String, text: String) -> Vec<Effect> {
    if same_body(&original, &text) {
        view.body_draft = None;
        view.notice = Some("説明に変更が無い".into());
        return vec![];
    }
    let body = text.trim_end().to_string();
    view.body_draft = Some(body.clone());
    view.saving_body = true;
    view.error = None;
    vec![Effect::EditBody {
        number: view.number,
        original,
        body,
    }]
}

/// エディタで書いたレビューの本文を送る. 空のコメントは取りやめる
pub(super) fn submit_review(view: &mut PrView, action: ReviewAction, text: &str) -> Vec<Effect> {
    let body = text.trim();
    if body.is_empty() && !action.allows_empty() {
        view.notice = Some("本文が空なのでコメントを取りやめた".into());
        return vec![];
    }
    view.submitting = true;
    view.error = None;
    vec![Effect::SubmitReview {
        number: view.number,
        action,
        body: body.to_string(),
    }]
}

fn scroll_by(view: &mut PrView, delta: isize) {
    let cur = view.scroll.min(view.max_scroll) as isize;
    view.scroll = (cur + delta).clamp(0, view.max_scroll as isize) as usize;
}

#[cfg(test)]
mod tests {
    use chrono::DateTime;
    use pretty_assertions::assert_eq;
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    use super::super::test_util::{code, key, pr, review_state, state};
    use super::super::{handle_event, handle_key};
    use crate::app::event::{Effect, Event};
    use crate::app::state::{AppState, Composing, PrView};
    use crate::domain::conversation::{Conversation, ReviewAction};

    fn view(s: &AppState) -> &PrView {
        s.pr_view.as_ref().expect("pr_view")
    }

    fn conversation(body: &str) -> Conversation {
        Conversation::new(
            "me".into(),
            DateTime::from_timestamp(0, 0).unwrap(),
            body.into(),
            vec![],
            0,
        )
    }

    #[test]
    fn open_fetches_in_any_tab() {
        let mut p = pr(10);
        p.title = "他人の PR".into();
        let mut s = review_state(vec![pr(1)], vec![p]);
        handle_key(&mut s, key('2'));
        assert_eq!(
            handle_key(&mut s, key('p')),
            vec![Effect::FetchConversation(10)]
        );
        assert_eq!(
            (view(&s).number, view(&s).title.as_str()),
            (10, "他人の PR")
        );
        assert!(view(&s).fetching);
        // 開いている間のキーは一覧に届かない
        assert!(handle_key(&mut s, key('j')).is_empty());
        assert_eq!(s.selected().map(|p| p.number), Some(10));
    }

    #[test]
    fn open_without_pr_does_nothing() {
        let mut s = state(vec![]);
        assert!(handle_key(&mut s, key('p')).is_empty());
        assert!(s.pr_view.is_none());
    }

    #[test]
    fn fetched_applies_only_to_open_pr() {
        let mut s = state(vec![pr(1)]);
        handle_key(&mut s, key('p'));
        // 別の PR の結果は捨てる
        handle_event(&mut s, Event::FetchedConversation(2, Ok(conversation("x"))));
        assert!(view(&s).conversation.is_none() && view(&s).fetching);

        handle_event(
            &mut s,
            Event::FetchedConversation(1, Ok(conversation("説明"))),
        );
        assert!(!view(&s).fetching);
        assert_eq!(view(&s).conversation.as_ref().unwrap().body, "説明");

        // 取り直しに失敗しても前の内容は残す
        assert_eq!(
            handle_key(&mut s, key('r')),
            vec![Effect::FetchConversation(1)]
        );
        // 取得中は重ねない
        assert!(handle_key(&mut s, key('r')).is_empty());
        handle_event(
            &mut s,
            Event::FetchedConversation(1, Err(anyhow::anyhow!("boom"))),
        );
        assert_eq!(view(&s).error.as_deref(), Some("取得失敗: boom"));
        assert!(view(&s).conversation.is_some());

        // 閉じた後に届いた結果は捨てる
        handle_key(&mut s, code(KeyCode::Esc));
        assert!(s.pr_view.is_none());
        handle_event(&mut s, Event::FetchedConversation(1, Ok(conversation("x"))));
        assert!(s.pr_view.is_none());
    }

    #[test]
    fn scroll_is_clamped() {
        let mut s = state(vec![pr(1)]);
        handle_key(&mut s, key('p'));
        {
            let v = s.pr_view.as_mut().unwrap();
            v.max_scroll = 10;
            v.viewport_height = 6;
        }
        let scroll = |s: &AppState| view(s).scroll;
        handle_key(&mut s, key('k'));
        assert_eq!(scroll(&s), 0);
        handle_key(&mut s, key('j'));
        assert_eq!(scroll(&s), 1);
        handle_key(
            &mut s,
            KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL),
        );
        assert_eq!(scroll(&s), 4);
        handle_key(&mut s, key(' '));
        assert_eq!(scroll(&s), 10);
        handle_key(&mut s, code(KeyCode::PageUp));
        assert_eq!(scroll(&s), 4);
        handle_key(&mut s, key('G'));
        assert_eq!(scroll(&s), 10);
        handle_key(&mut s, key('g'));
        assert_eq!(scroll(&s), 0);
    }

    fn opened(n: u64) -> AppState {
        let mut s = review_state(vec![], vec![pr(n)]);
        handle_key(&mut s, key('2'));
        handle_key(&mut s, key('p'));
        handle_event(&mut s, Event::FetchedConversation(n, Ok(conversation("x"))));
        s
    }

    #[test]
    fn approve_via_editor_then_refetch() {
        let mut s = opened(10);
        assert!(handle_key(&mut s, key('!')).is_empty());
        assert!(view(&s).choosing_review);
        assert_eq!(
            handle_key(&mut s, key('a')),
            vec![Effect::Compose {
                draft: String::new()
            }]
        );
        assert_eq!(
            view(&s).composing,
            Some(Composing::Review(ReviewAction::Approve))
        );
        // 承認は本文が空でも送る
        assert_eq!(
            handle_event(&mut s, Event::Composed(Ok(Some("\n".into())))),
            vec![Effect::SubmitReview {
                number: 10,
                action: ReviewAction::Approve,
                body: String::new(),
            }]
        );
        assert!(view(&s).submitting && view(&s).composing.is_none());
        // 送信中は閉じない
        handle_key(&mut s, key('q'));
        assert!(s.pr_view.is_some());

        let effects = handle_event(
            &mut s,
            Event::SubmittedReview {
                number: 10,
                action: ReviewAction::Approve,
                res: Ok(String::new()),
            },
        );
        assert_eq!(
            effects,
            vec![Effect::FetchReviews, Effect::FetchConversation(10)]
        );
        assert!(!view(&s).submitting);
        assert_eq!(view(&s).notice.as_deref(), Some("#10 を承認した"));
    }

    #[test]
    fn approve_draft_from_config() {
        let mut s = opened(10);
        s.local.approve_body = Some("LGTM 👍".into());
        handle_key(&mut s, key('!'));
        assert_eq!(
            handle_key(&mut s, key('a')),
            vec![Effect::Compose {
                draft: "LGTM 👍".into()
            }]
        );
        assert_eq!(
            handle_event(&mut s, Event::Composed(Ok(Some("LGTM 👍\n".into())))),
            vec![Effect::SubmitReview {
                number: 10,
                action: ReviewAction::Approve,
                body: "LGTM 👍".into(),
            }]
        );
        // コメントには使わない
        let mut s = opened(10);
        s.local.approve_body = Some("LGTM 👍".into());
        handle_key(&mut s, key('!'));
        assert_eq!(
            handle_key(&mut s, key('c')),
            vec![Effect::Compose {
                draft: String::new()
            }]
        );
    }

    #[test]
    fn comment_needs_body() {
        let mut s = opened(10);
        handle_key(&mut s, key('!'));
        handle_key(&mut s, key('c'));
        assert!(handle_event(&mut s, Event::Composed(Ok(Some(" \n".into())))).is_empty());
        assert_eq!(
            view(&s).notice.as_deref(),
            Some("本文が空なのでコメントを取りやめた")
        );
        handle_key(&mut s, key('!'));
        handle_key(&mut s, key('c'));
        assert_eq!(
            handle_event(&mut s, Event::Composed(Ok(Some("直して\n".into())))),
            vec![Effect::SubmitReview {
                number: 10,
                action: ReviewAction::Comment,
                body: "直して".into(),
            }]
        );
        let effects = handle_event(
            &mut s,
            Event::SubmittedReview {
                number: 10,
                action: ReviewAction::Comment,
                res: Err(anyhow::anyhow!("Can not approve your own pull request")),
            },
        );
        assert!(effects.is_empty());
        assert_eq!(
            view(&s).error.as_deref(),
            Some("#10 のレビュー送信失敗: Can not approve your own pull request")
        );
    }

    #[test]
    fn review_cancelled_by_other_key_or_editor_exit() {
        let mut s = opened(10);
        handle_key(&mut s, key('!'));
        // 選択待ちの j はスクロールせず取りやめる
        assert!(handle_key(&mut s, key('j')).is_empty());
        assert!(!view(&s).choosing_review && view(&s).composing.is_none());
        assert_eq!(view(&s).notice.as_deref(), Some("レビューを取りやめた"));
        // 次のキーで消す
        handle_key(&mut s, key('k'));
        assert!(view(&s).notice.is_none());

        handle_key(&mut s, key('!'));
        handle_key(&mut s, key('a'));
        assert!(handle_event(&mut s, Event::Composed(Ok(None))).is_empty());
        assert!(view(&s).composing.is_none() && !view(&s).submitting);
    }

    #[test]
    fn browser_and_close() {
        let mut s = state(vec![pr(3)]);
        handle_key(&mut s, key('p'));
        assert_eq!(handle_key(&mut s, key('o')), vec![Effect::ViewWeb(3)]);
        assert!(handle_key(&mut s, key('q')).is_empty());
        assert!(s.pr_view.is_none());
        // Ctrl-C は常に終了
        handle_key(&mut s, key('p'));
        let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert_eq!(handle_key(&mut s, ctrl_c), vec![Effect::Quit]);
    }

    #[test]
    fn edit_body_via_editor() {
        let mut s = state(vec![pr(1)]);
        handle_key(&mut s, key('p'));
        // 取得前は開かない
        assert!(handle_key(&mut s, key('e')).is_empty());
        assert_eq!(view(&s).notice.as_deref(), Some("説明を取得中"));

        handle_event(
            &mut s,
            Event::FetchedConversation(1, Ok(conversation("説明\r\n<!-- t -->\r\n"))),
        );
        // CRLF は LF にしてエディタに渡す. HTML コメントも残す
        assert_eq!(
            handle_key(&mut s, key('e')),
            vec![Effect::Compose {
                draft: "説明\n<!-- t -->\n".into()
            }]
        );
        let original = "説明\r\n<!-- t -->\r\n".to_string();
        assert_eq!(
            view(&s).composing,
            Some(Composing::Body {
                original: original.clone()
            })
        );
        assert_eq!(
            handle_event(&mut s, Event::Composed(Ok(Some("新しい説明\n\n".into())))),
            vec![Effect::EditBody {
                number: 1,
                original,
                body: "新しい説明".into(),
            }]
        );
        assert!(view(&s).saving_body && view(&s).composing.is_none());
        // 更新中は閉じず, レビューも編集も重ねない
        for c in ['q', '!', 'e'] {
            assert!(handle_key(&mut s, key(c)).is_empty());
        }
        assert!(s.pr_view.is_some() && !view(&s).choosing_review);

        let effects = handle_event(
            &mut s,
            Event::EditedBody {
                number: 1,
                res: Ok(String::new()),
            },
        );
        assert_eq!(effects, vec![Effect::FetchConversation(1)]);
        assert!(!view(&s).saving_body && view(&s).body_draft.is_none());
        assert_eq!(view(&s).notice.as_deref(), Some("#1 の説明を更新した"));
    }

    #[test]
    fn edit_body_unchanged_or_cancelled() {
        let mut s = opened(10);
        handle_key(&mut s, key('e'));
        // 末尾の改行の違いは変更とみなさない
        assert!(handle_event(&mut s, Event::Composed(Ok(Some("x\n".into())))).is_empty());
        assert_eq!(view(&s).notice.as_deref(), Some("説明に変更が無い"));

        handle_key(&mut s, key('e'));
        assert!(handle_event(&mut s, Event::Composed(Ok(None))).is_empty());
        assert_eq!(view(&s).notice.as_deref(), Some("説明の編集を取りやめた"));
        assert!(view(&s).composing.is_none() && !view(&s).saving_body);
    }

    #[test]
    fn edit_body_failure_keeps_draft() {
        let mut s = opened(10);
        handle_key(&mut s, key('e'));
        handle_event(&mut s, Event::Composed(Ok(Some("書いた".into()))));
        let effects = handle_event(
            &mut s,
            Event::EditedBody {
                number: 10,
                res: Err(anyhow::anyhow!("説明が GitHub 側で変わった")),
            },
        );
        assert!(effects.is_empty());
        assert!(!view(&s).saving_body);
        assert_eq!(
            view(&s).error.as_deref(),
            Some("#10 の説明の更新失敗: 説明が GitHub 側で変わった. 書いた内容は次の e で開く")
        );
        // 取り直した説明を基準に, 書いた内容を開き直す
        handle_event(
            &mut s,
            Event::FetchedConversation(10, Ok(conversation("y"))),
        );
        assert_eq!(
            handle_key(&mut s, key('e')),
            vec![Effect::Compose {
                draft: "書いた".into()
            }]
        );
        assert_eq!(
            view(&s).composing,
            Some(Composing::Body {
                original: "y".into()
            })
        );
    }
}
