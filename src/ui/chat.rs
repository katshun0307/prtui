use chrono::Local;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Stylize;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};

use super::format::status_label;
use super::{DIM, devin_style};
use crate::app::AppState;
use crate::domain::devin::{DevinMessage, DevinSession};
use crate::domain::pr::Pr;

/// Devin session との会話ビューを全画面に描く. scroll の上限と欄の高さを `ChatView` に書く
pub(super) fn draw_chat(f: &mut Frame, state: &mut AppState) {
    let Some(chat) = &state.chat else {
        return;
    };
    // v は選択中の PR から開くので, 開いている間の選択はその PR のまま
    let header = header_line(state.selected(), state.chat_session(), &chat.session_id);
    render_chat(f, f.area(), header, state);
}

/// ヘッダ以外を `state.chat` だけで描く. `chat_session` を呼ばずにテストできるように分けてある
fn render_chat(f: &mut Frame, area: Rect, header: Line<'static>, state: &mut AppState) {
    let Some(chat) = state.chat.as_mut() else {
        return;
    };
    let input = input_line(chat.typing, &chat.draft);
    let [header_area, body, input_area, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(u16::from(input.is_some())),
        Constraint::Length(1),
    ])
    .areas(area);

    f.render_widget(header, header_area);

    let para =
        Paragraph::new(message_lines(&chat.messages, chat.loaded)).wrap(Wrap { trim: false });
    let total = para.line_count(body.width);
    chat.max_scroll = total.saturating_sub(body.height as usize);
    chat.viewport_height = body.height;
    let offset = chat.scroll.unwrap_or(chat.max_scroll).min(chat.max_scroll);
    f.render_widget(para.scroll((offset as u16, 0)), body);

    if let Some(input) = input {
        f.render_widget(input, input_area);
    }
    f.render_widget(
        footer_line(chat.sending, chat.error.as_deref(), chat.typing),
        footer,
    );
}

/// ` Devin #<PR番号> <title> [status]`. session が一覧から消えていれば status の代わりに id を出す
fn header_line(pr: Option<&Pr>, session: Option<&DevinSession>, session_id: &str) -> Line<'static> {
    let mut spans = vec![Span::raw(" Devin ").magenta().bold()];
    if let Some(p) = pr {
        spans.push(Span::raw(format!("#{} ", p.number)).bold());
        spans.push(Span::raw(format!("{} ", p.title)));
    }
    match session {
        Some(s) => spans.push(Span::styled(
            format!("[{}]", status_label(s.status)),
            devin_style(s),
        )),
        None => spans.push(Span::raw(format!("[{session_id}]")).fg(DIM)),
    }
    Line::from(spans)
}

/// 各メッセージを `<発言者> <MM-DD HH:MM>` の行, 本文の行, 空行の順に並べる. 古い順
fn message_lines(messages: &[DevinMessage], loaded: bool) -> Vec<Line<'static>> {
    if !loaded {
        return vec![Line::from(Span::raw("取得中…").fg(DIM))];
    }
    if messages.is_empty() {
        return vec![Line::from(Span::raw("メッセージが無い").fg(DIM))];
    }
    let mut lines = Vec::new();
    for m in messages {
        lines.push(Line::from(vec![
            speaker_span(m),
            Span::raw(" "),
            Span::raw(
                m.created_at
                    .with_timezone(&Local)
                    .format("%m-%d %H:%M")
                    .to_string(),
            )
            .fg(DIM),
        ]));
        lines.extend(m.text.lines().map(|l| Line::from(l.to_string())));
        lines.push(Line::default());
    }
    lines
}

/// Devin は magenta, 自分側は cyan. 自分側は username が無ければ `user`
fn speaker_span(m: &DevinMessage) -> Span<'static> {
    if m.from_devin {
        Span::raw("Devin").magenta().bold()
    } else {
        Span::raw(m.username.clone().unwrap_or_else(|| "user".into()))
            .cyan()
            .bold()
    }
}

/// 入力中か書きかけがあるときだけ出す. 改行は `⏎` にして 1 行に収める
fn input_line(typing: bool, draft: &str) -> Option<Line<'static>> {
    if !typing && draft.is_empty() {
        return None;
    }
    let mut spans = vec![
        Span::raw("> ").cyan().bold(),
        Span::raw(draft.replace('\n', "⏎")),
    ];
    if typing {
        spans.push(Span::raw("█"));
    }
    Some(Line::from(spans))
}

/// 送信中, エラー, キー一覧の順に優先する
fn footer_line(sending: bool, error: Option<&str>, typing: bool) -> Line<'static> {
    if sending {
        return Line::from(Span::raw(" 送信中…").yellow());
    }
    if let Some(e) = error {
        return Line::from(Span::raw(format!(" {e}")).red());
    }
    let keys: &[(&str, &str)] = if typing {
        &[("Enter", "送信"), ("Esc", "入力をやめる")]
    } else {
        &[
            ("j/k", "スクロール"),
            ("C-d/C-u", "半画面"),
            ("g/G", "先頭/末尾"),
            ("i", "入力"),
            ("e", "エディタ"),
            ("o", "ブラウザ"),
            ("r", "更新"),
            ("q", "戻る"),
        ]
    };
    let mut spans = vec![Span::raw(" ")];
    for (key, label) in keys {
        spans.push(Span::raw(*key).cyan());
        spans.push(Span::raw(format!(" {label}  ")).fg(DIM));
    }
    Line::from(spans)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::domain::devin::DevinStatus;
    use crate::domain::local::LocalState;
    use chrono::{DateTime, Utc};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::style::{Color, Modifier};

    const AT: i64 = 1_700_000_000;

    fn message(from_devin: bool, username: Option<&str>, text: &str) -> DevinMessage {
        DevinMessage {
            from_devin,
            username: username.map(Into::into),
            text: text.into(),
            created_at: DateTime::<Utc>::from_timestamp(AT, 0).unwrap(),
        }
    }

    /// 会話ビューを開いて messages を取得済みにした状態. `ChatView` は app の外から名前で呼べないので Default から作る
    fn state(messages: Vec<DevinMessage>) -> AppState {
        let mut s = AppState::new(PathBuf::from("/tmp"), "o/r".into(), LocalState::default());
        s.chat = Some(Default::default());
        let chat = s.chat.as_mut().unwrap();
        chat.session_id = "devin-1".into();
        chat.messages = messages;
        chat.loaded = true;
        s
    }

    fn time() -> String {
        DateTime::<Utc>::from_timestamp(AT, 0)
            .unwrap()
            .with_timezone(&Local)
            .format("%m-%d %H:%M")
            .to_string()
    }

    fn texts(lines: &[Line]) -> Vec<String> {
        lines.iter().map(|l| l.to_string()).collect()
    }

    /// TestBackend に描いて行ごとの文字列にする
    fn render(state: &mut AppState, width: u16, height: u16) -> Vec<String> {
        let mut term = Terminal::new(TestBackend::new(width, height)).unwrap();
        term.draw(|f| render_chat(f, f.area(), Line::from(" header"), state))
            .unwrap();
        let buf = term.backend().buffer();
        (0..height)
            .map(|y| {
                (0..width)
                    .map(|x| buf[(x, y)].symbol())
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .collect()
    }

    #[test]
    fn messages_in_order_with_speaker() {
        let messages = vec![
            message(false, Some("katshun"), "直して"),
            message(true, None, "はい\n直した"),
            message(false, None, "ok"),
        ];
        let lines = message_lines(&messages, true);
        let t = time();
        assert_eq!(
            texts(&lines),
            vec![
                format!("katshun {t}"),
                "直して".into(),
                String::new(),
                format!("Devin {t}"),
                "はい".into(),
                "直した".into(),
                String::new(),
                format!("user {t}"),
                "ok".into(),
                String::new(),
            ]
        );
        let user = &lines[0].spans[0].style;
        assert_eq!(user.fg, Some(Color::Cyan));
        assert!(user.add_modifier.contains(Modifier::BOLD));
        let devin = &lines[3].spans[0].style;
        assert_eq!(devin.fg, Some(Color::Magenta));
        assert!(devin.add_modifier.contains(Modifier::BOLD));
        assert_eq!(lines[0].spans[2].style.fg, Some(DIM));
    }

    #[test]
    fn loading_and_empty() {
        assert_eq!(texts(&message_lines(&[], true)), vec!["メッセージが無い"]);
        let messages = vec![message(true, None, "x")];
        assert_eq!(texts(&message_lines(&messages, false)), vec!["取得中…"]);
    }

    #[test]
    fn input_only_when_typing_or_draft() {
        assert!(input_line(false, "").is_none());
        assert_eq!(input_line(false, "a\nb").unwrap().to_string(), "> a⏎b");
        assert_eq!(input_line(true, "a\nb").unwrap().to_string(), "> a⏎b█");
        assert_eq!(input_line(true, "").unwrap().to_string(), "> █");
    }

    #[test]
    fn footer_priority() {
        assert!(
            footer_line(false, None, false)
                .to_string()
                .contains("i 入力")
        );
        assert!(
            footer_line(false, None, true)
                .to_string()
                .contains("Enter 送信")
        );
        let line = footer_line(false, Some("送信失敗: 409"), true);
        assert_eq!(line.to_string(), " 送信失敗: 409");
        assert_eq!(line.spans[0].style.fg, Some(Color::Red));
        let line = footer_line(true, Some("送信失敗: 409"), true);
        assert_eq!(line.to_string(), " 送信中…");
        assert_eq!(line.spans[0].style.fg, Some(Color::Yellow));
    }

    #[test]
    fn header_with_and_without_session() {
        let mut p = Pr::fixture(12, "feat", "main");
        p.title = "直す".into();
        let s = DevinSession {
            status: DevinStatus::Error,
            ..DevinSession::fixture("devin-1", "")
        };
        let line = header_line(Some(&p), Some(&s), "devin-1");
        assert_eq!(line.to_string(), " Devin #12 直す [error]");
        assert_eq!(line.spans.last().unwrap().style.fg, Some(Color::Red));
        let line = header_line(Some(&p), None, "devin-1");
        assert_eq!(line.to_string(), " Devin #12 直す [devin-1]");
    }

    fn five_messages() -> Vec<DevinMessage> {
        (0..5)
            .map(|i| message(true, None, &format!("m{i}")))
            .collect()
    }

    #[test]
    fn render_follows_tail_and_writes_scroll() {
        let mut s = state(five_messages());
        // 5 件 × 3 行 = 15 行. 高さ 8 からヘッダとフッターを引いて欄は 6 行
        let rows = render(&mut s, 30, 8);
        let chat = s.chat.as_ref().unwrap();
        assert_eq!(chat.viewport_height, 6);
        assert_eq!(chat.max_scroll, 9);
        assert_eq!(rows[0], " header");
        assert!(rows[1].starts_with("Devin"), "{rows:?}");
        assert_eq!(rows[2], "m3");
        assert_eq!(rows[5], "m4");
        assert!(rows[7].contains("j/k"), "{rows:?}");
    }

    #[test]
    fn render_scroll_is_clamped() {
        let mut s = state(five_messages());
        s.chat.as_mut().unwrap().scroll = Some(0);
        let rows = render(&mut s, 30, 8);
        assert!(rows[1].starts_with("Devin"), "{rows:?}");
        assert_eq!(rows[2], "m0");
        s.chat.as_mut().unwrap().scroll = Some(100);
        let rows = render(&mut s, 30, 8);
        assert_eq!(rows[5], "m4");
    }

    #[test]
    fn render_counts_wrapped_lines_and_input_row() {
        let mut s = state(vec![message(true, None, &"x".repeat(25))]);
        let chat = s.chat.as_mut().unwrap();
        chat.typing = true;
        chat.draft = "hi".into();
        // 幅 20 で本文 25 文字は 2 行. 発言者と空行を足して 4 行. 欄は 6 - 3 = 3 行
        let rows = render(&mut s, 20, 6);
        let chat = s.chat.as_ref().unwrap();
        assert_eq!(chat.viewport_height, 3);
        assert_eq!(chat.max_scroll, 1);
        assert_eq!(rows[4], "> hi█");
        assert!(rows[5].contains("Enter"), "{rows:?}");
    }
}
