//! 会話ビュー (Devin session) を開く, キー操作, 送信.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::event::Effect;
use crate::app::state::{AppState, ChatView};

/// 選択中の PR に紐づく最新の Devin session の会話ビューを開く.
/// メッセージは次の tick で取る
pub(super) fn open_chat(state: &mut AppState) -> Vec<Effect> {
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
    state.push(ChatView::new(id, n));
    vec![]
}

const NO_DEVIN_CONFIG: &str = "Devin の設定が無い. git config prtui.devin-token <token> と prtui.devin-org <org-id> を設定する";

/// 会話ビューのキー. 1 行入力中は編集キー以外の文字を入力位置に挿す
pub(super) fn chat_key(state: &mut AppState, key: KeyEvent) -> Vec<Effect> {
    let url = state.chat_session().map(|s| s.url.clone());
    let Some(chat) = state.chat_mut() else {
        return vec![];
    };
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    if chat.typing {
        return typing_key(chat, key.code, ctrl);
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
        KeyCode::Esc | KeyCode::Char('q') if !chat.sending => {
            state.pop();
        }
        _ => {}
    }
    vec![]
}

/// 1 行入力中のキー. 矢印と Home/End (と Emacs 風の C-b/C-f/C-a/C-e) で入力位置を動かす
fn typing_key(chat: &mut ChatView, code: KeyCode, ctrl: bool) -> Vec<Effect> {
    let len = chat.draft.chars().count();
    let cur = chat.cursor.min(len);
    match code {
        KeyCode::Enter => return send(chat),
        KeyCode::Esc => chat.typing = false,
        KeyCode::Left => chat.cursor = cur.saturating_sub(1),
        KeyCode::Char('b') if ctrl => chat.cursor = cur.saturating_sub(1),
        KeyCode::Right => chat.cursor = (cur + 1).min(len),
        KeyCode::Char('f') if ctrl => chat.cursor = (cur + 1).min(len),
        KeyCode::Home => chat.cursor = 0,
        KeyCode::Char('a') if ctrl => chat.cursor = 0,
        KeyCode::End => chat.cursor = len,
        KeyCode::Char('e') if ctrl => chat.cursor = len,
        KeyCode::Backspace if cur > 0 => {
            chat.draft.remove(byte_at(&chat.draft, cur - 1));
            chat.cursor = cur - 1;
        }
        KeyCode::Delete if cur < len => {
            chat.draft.remove(byte_at(&chat.draft, cur));
            chat.cursor = cur;
        }
        KeyCode::Char(c) if !ctrl => {
            chat.draft.insert(byte_at(&chat.draft, cur), c);
            chat.cursor = cur + 1;
        }
        _ => {}
    }
    vec![]
}

/// 先頭から n 文字目の byte 位置. n が文字数以上なら末尾
fn byte_at(s: &str, n: usize) -> usize {
    s.char_indices().nth(n).map_or(s.len(), |(i, _)| i)
}

/// 表示位置を動かす. 末尾まで行ったら末尾追従に戻す
fn scroll_by(chat: &mut ChatView, delta: isize) {
    let cur = chat.scroll.unwrap_or(chat.max_scroll) as isize;
    let next = (cur + delta).clamp(0, chat.max_scroll as isize) as usize;
    chat.scroll = (next < chat.max_scroll).then_some(next);
}

/// draft を送る. 空白だけか送信中なら何もしない
pub(super) fn send(chat: &mut ChatView) -> Vec<Effect> {
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

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use pretty_assertions::assert_eq;
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    use super::super::test_util::{chat, chat_state, code, devin_state, key, message, pr, state};
    use super::super::{CHAT_POLL, handle_event, handle_key, tick};
    use crate::app::event::{Effect, Event};
    use crate::domain::devin::DevinSession;

    #[test]
    fn view_without_config_is_error() {
        let mut s = state(vec![pr(1)]);
        assert!(handle_key(&mut s, key('v')).is_empty());
        assert!(s.chat().is_none());
        assert!(message(&s).0.starts_with("Devin の設定が無い"));
        assert!(message(&s).1);
    }

    #[test]

    fn view_without_session_is_error() {
        let mut s = devin_state(vec![pr(1)]);
        assert!(handle_key(&mut s, key('v')).is_empty());
        assert!(s.chat().is_none());
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
    fn chat_r_polls_now() {
        let mut s = chat_state();
        s.chat_mut().unwrap().last_poll = Some(Instant::now());
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
        assert!(s.chat().is_some());
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
    fn chat_typing_moves_cursor() {
        let mut s = chat_state();
        handle_key(&mut s, key('i'));
        for c in "あいう".chars() {
            handle_key(&mut s, key(c));
        }
        assert_eq!(chat(&s).cursor, 3);
        // 途中に挿す
        handle_key(&mut s, code(KeyCode::Left));
        handle_key(&mut s, code(KeyCode::Left));
        handle_key(&mut s, key('x'));
        assert_eq!((chat(&s).draft.as_str(), chat(&s).cursor), ("あxいう", 2));
        // Backspace は入力位置の前, Delete は入力位置の文字を消す
        handle_key(&mut s, code(KeyCode::Backspace));
        handle_key(&mut s, code(KeyCode::Delete));
        assert_eq!((chat(&s).draft.as_str(), chat(&s).cursor), ("あう", 1));
        // 端で止まる
        handle_key(&mut s, code(KeyCode::Home));
        handle_key(&mut s, code(KeyCode::Left));
        handle_key(&mut s, code(KeyCode::Backspace));
        assert_eq!((chat(&s).draft.as_str(), chat(&s).cursor), ("あう", 0));
        handle_key(&mut s, code(KeyCode::End));
        handle_key(&mut s, code(KeyCode::Right));
        handle_key(&mut s, code(KeyCode::Delete));
        assert_eq!((chat(&s).draft.as_str(), chat(&s).cursor), ("あう", 2));
        // Emacs 風
        let ctrl = |c| KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL);
        handle_key(&mut s, ctrl('a'));
        assert_eq!(chat(&s).cursor, 0);
        handle_key(&mut s, ctrl('f'));
        assert_eq!(chat(&s).cursor, 1);
        handle_key(&mut s, ctrl('e'));
        handle_key(&mut s, ctrl('b'));
        assert_eq!(chat(&s).cursor, 1);
        assert_eq!(chat(&s).draft, "あう");
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
        assert!(s.chat().is_none());
    }

    #[test]
    fn chat_close_ignored_while_sending() {
        let mut s = chat_state();
        s.chat_mut().unwrap().sending = true;
        handle_key(&mut s, key('q'));
        handle_key(&mut s, code(KeyCode::Esc));
        assert!(s.chat().is_some());
        // e も送信中は無視する
        assert!(handle_key(&mut s, key('e')).is_empty());
        // Ctrl-C は常に終了
        let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert_eq!(handle_key(&mut s, ctrl_c), vec![Effect::Quit]);
    }

    #[test]
    fn chat_compose_and_open() {
        let mut s = chat_state();
        s.chat_mut().unwrap().draft = "hi".into();
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
            let c = s.chat_mut().unwrap();
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
}
