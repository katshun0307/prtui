use ratatui::Frame;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Stylize};
use ratatui::text::{Line, Span};

use crate::app::AppState;

mod chat;
mod detail;
pub mod format;
mod table;

use chat::draw_chat;
use detail::detail_view;
use format::ago;
use table::draw_table;

/// 配色の意味
/// - green: 問題なし (Ready, CI 成功, APPROVED, 承認した reviewer)
/// - red: 壊れている (Conflict, CI 失敗, CHANGES_REQUESTED, 変更要求した reviewer)
/// - yellow: 自分の対応が要る (未解決スレッド, REVIEW_REQUIRED, reviewer 未設定, 確認プロンプト)
///   依頼中の reviewer も yellow にする. 返事待ちで, 催促するかを自分が判断するため
/// - cyan: ブランチ名, ローカルが最新, stack 内の位置
/// - magenta: ローカルとリモートに差分, 絞り込み中
/// - DIM: 補助情報, 0 件, Draft
///
/// `Color::DarkGray` は端末テーマによって背景色と同化するので使わない.
/// 256 色の固定値を使い, 選択行の背景とも区別が付くようにする.
pub(super) const DIM: Color = Color::Indexed(245);
pub(super) const SELECTED_BG: Color = Color::Indexed(237);
pub(super) const HEADER_FG: Color = Color::Indexed(250);

pub fn draw(f: &mut Frame, state: &mut AppState) {
    if state.chat.is_some() {
        draw_chat(f, state);
        return;
    }
    let [header, list, detail, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(5),
        Constraint::Length(10),
        Constraint::Length(1),
    ])
    .areas(f.area());

    f.render_widget(header_line(state), header);
    draw_table(f, state, list);
    f.render_widget(detail_view(state), detail);
    f.render_widget(footer_line(state), footer);
}

fn header_line(state: &AppState) -> Line<'static> {
    let mut spans = vec![
        Span::raw(format!(" {} ", state.repo)).bold(),
        Span::raw(format!("{}  ", state.dir.display())).fg(DIM),
        Span::raw(format!("branch: {}", state.local.current_branch)).cyan(),
    ];
    if let Some(t) = state.fetched_at {
        let label = if state.from_cache {
            format!("  取得 {} 前 (キャッシュ)", ago(t))
        } else {
            format!("  取得 {} 前", ago(t))
        };
        spans.push(Span::raw(label).fg(DIM));
    }
    if state.local.dirty {
        spans.push(Span::raw("  未コミットの変更あり").yellow());
    }
    if state.draft_only {
        spans.push(Span::raw("  [Draft のみ]").magenta());
    }
    Line::from(spans)
}

fn footer_line(state: &AppState) -> Line<'static> {
    if let Some(busy) = &state.busy {
        return Line::from(Span::raw(format!(" {busy}")).yellow());
    }
    if let Some((n, base)) = &state.confirm_update {
        return Line::from(
            Span::raw(format!(
                " #{n} に {base} を merge commit で取り込んで push する? (y/N)"
            ))
            .yellow()
            .bold(),
        );
    }
    if let Some((n, op)) = state.confirm_op {
        return Line::from(
            Span::raw(format!(
                " #{n} を {}する? もう一度 {} で実行, 他のキーで取りやめ",
                op.phrase(&state.local.reviewers),
                op.key()
            ))
            .yellow()
            .bold(),
        );
    }
    if let Some(m) = &state.message {
        let span = Span::raw(format!(" {}", m.text));
        return Line::from(if m.is_error { span.red() } else { span.green() });
    }
    const KEYS: [(&str, &str); 14] = [
        ("j/k", "移動"),
        ("Enter/c", "checkout"),
        ("u", "main 取込"),
        ("O", "open"),
        ("DD", "draft"),
        ("XX", "close"),
        ("RR", "review 依頼"),
        ("d", "diff"),
        ("o", "ブラウザ"),
        ("l", "Linear"),
        ("L", "Linear review"),
        ("t", "Draft のみ"),
        ("r", "更新"),
        ("q", "終了"),
    ];
    let mut spans = vec![Span::raw(" ")];
    for (key, label) in KEYS {
        spans.push(Span::raw(key).cyan());
        spans.push(Span::raw(format!(" {label}  ")).fg(DIM));
    }
    Line::from(spans)
}
