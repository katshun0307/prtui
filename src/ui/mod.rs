use ratatui::Frame;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Style, Stylize};
use ratatui::text::{Line, Span};

use crate::app::AppState;
use crate::domain::devin::{DevinSession, DevinStatus};

mod chat;
mod copy;
mod detail;
pub mod format;
mod table;

use chat::draw_chat;
use copy::draw_copy_menu;
use detail::detail_view;
use format::ago;
use table::draw_table;

/// 配色の意味
/// - green: 問題なし (Ready, CI 成功, APPROVED, 承認した reviewer)
/// - red: 壊れている (Conflict, CI 失敗, CHANGES_REQUESTED, 変更要求した reviewer)
/// - yellow: 自分の対応が要る (未解決スレッド, REVIEW_REQUIRED, reviewer 未設定, 確認プロンプト)
///   依頼中の reviewer も yellow にする. 返事待ちで, 催促するかを自分が判断するため
///   レビュータブの「自分」列も同じ意味で, 依頼中と再依頼は yellow, 承認済は green, 変更要求は red,
///   コメント済とレビュー済は DIM
/// - cyan: ブランチ名, ローカルが最新, stack 内の位置
/// - magenta: ローカルとリモートに差分, 絞り込み中
/// - DIM: 補助情報, 0 件, Draft
///
/// Devin 列と会話ビュー
/// - status: 動いている (new / claimed / running / resuming) は yellow, 返事待ちの suspended は cyan,
///   error は red, 終わった exit と unknown は DIM. archived の session は status によらず DIM
/// - 発言者: Devin は magenta bold, 自分側 (user) は cyan bold. 時刻は DIM
///
/// `Color::DarkGray` は端末テーマによって背景色と同化するので使わない.
/// 256 色の固定値を使い, 選択行の背景とも区別が付くようにする.
pub(super) const DIM: Color = Color::Indexed(245);
pub(super) const SELECTED_BG: Color = Color::Indexed(237);
pub(super) const HEADER_FG: Color = Color::Indexed(250);

/// Devin session の status の色. 一覧, 詳細欄, 会話ビューのヘッダで共通
pub(super) fn devin_style(s: &DevinSession) -> Style {
    if s.is_archived {
        return Style::new().fg(DIM);
    }
    let fg = match s.status {
        DevinStatus::New | DevinStatus::Claimed | DevinStatus::Running | DevinStatus::Resuming => {
            Color::Yellow
        }
        DevinStatus::Suspended => Color::Cyan,
        DevinStatus::Error => Color::Red,
        DevinStatus::Exit | DevinStatus::Unknown => DIM,
    };
    Style::new().fg(fg)
}

pub fn draw(f: &mut Frame, state: &mut AppState) {
    if state.chat.is_some() {
        draw_chat(f, state);
        return;
    }
    // タブは設定で増えるので, ヘッダとは別の行に並べて切れないようにする
    let [header, tabs, list, detail, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(5),
        Constraint::Length(10),
        Constraint::Length(1),
    ])
    .areas(f.area());

    f.render_widget(header_line(state), header);
    f.render_widget(Line::from(tab_spans(state)), tabs);
    draw_table(f, state, list);
    f.render_widget(detail_view(state), detail);
    f.render_widget(footer_line(state), footer);
    if let Some(items) = &state.copy_menu {
        draw_copy_menu(f, items, f.area());
    }
}

fn header_line(state: &AppState) -> Line<'static> {
    let mut spans = vec![
        Span::raw(format!(" {} ", state.repo)).bold(),
        Span::raw(format!("{}  ", state.dir.display())).fg(DIM),
        Span::raw(format!("branch: {}  ", state.local.current_branch)).cyan(),
    ];
    let list = state.list();
    if let Some(t) = list.fetched_at {
        let label = if list.from_cache {
            format!("  取得 {} 前 (キャッシュ)", ago(t))
        } else {
            format!("  取得 {} 前", ago(t))
        };
        spans.push(Span::raw(label).fg(DIM));
    }
    if state.local.dirty {
        spans.push(Span::raw("  未コミットの変更あり").yellow());
    }
    if list.draft_only {
        spans.push(Span::raw("  [Draft のみ]").magenta());
    }
    Line::from(spans)
}

/// ` [1 自分 (n)] 2 レビュー (m) 3 <追加のタブ> (k)`. 今のタブは反転して強調し, 他は DIM
fn tab_spans(state: &AppState) -> Vec<Span<'static>> {
    let tabs = state.tabs().into_iter().enumerate().map(|(i, tab)| {
        let text = format!(
            " {} {} ({}) ",
            i + 1,
            state.tab_label(tab),
            state.list_of(tab).prs.len()
        );
        if tab == state.tab {
            Span::raw(text).bold().reversed()
        } else {
            Span::raw(text).fg(DIM)
        }
    });
    std::iter::once(Span::raw(" ")).chain(tabs).collect()
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
    if let Some((n, branch)) = &state.confirm_force_checkout {
        return Line::from(
            Span::raw(format!(
                " #{n}: ローカルの {branch} が PR と分岐している. ローカルだけのコミットを捨てて PR の最新に合わせる? (y/N)"
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
    if state.copy_menu.is_some() {
        return Line::from(
            Span::raw(" コピーする項目のキーを押す. 他のキーで閉じる")
                .yellow()
                .bold(),
        );
    }
    if let Some(m) = &state.message {
        let span = Span::raw(format!(" {}", m.text));
        return Line::from(if m.is_error { span.red() } else { span.green() });
    }
    const MINE: &[(&str, &str)] = &[
        ("Enter", "checkout"),
        ("c", "コピー"),
        ("u", "main 取込"),
        ("O", "open"),
        ("DD", "draft"),
        ("XX", "close"),
        ("RR", "review 依頼"),
        ("d", "diff"),
        ("o", "ブラウザ"),
        ("l", "Linear"),
        ("L", "Linear review"),
        ("v", "Devin"),
        ("t", "Draft のみ"),
        ("r", "更新"),
        ("q", "終了"),
    ];
    // 他人の PR を変える操作は出さない. 追加のタブも同じ
    const REVIEW: &[(&str, &str)] = &[
        ("Enter", "checkout"),
        ("c", "コピー"),
        ("d", "diff"),
        ("o", "ブラウザ"),
        ("l", "Linear"),
        ("L", "Linear review"),
        ("v", "Devin"),
        ("V", "Devin Review"),
        ("t", "Draft のみ"),
        ("r", "更新"),
        ("q", "終了"),
    ];
    let keys = if state.tab.is_readonly() {
        REVIEW
    } else {
        MINE
    };
    // 数字キーは 9 までのタブにしか届かない
    let tab_key = format!("Tab/1-{}", state.tabs().len().min(9));
    let head = [("j/k", "移動"), (tab_key.as_str(), "タブ")];
    let mut spans = vec![Span::raw(" ")];
    for &(key, label) in head.iter().chain(keys) {
        spans.push(Span::raw(key.to_string()).cyan());
        spans.push(Span::raw(format!(" {label}  ")).fg(DIM));
    }
    Line::from(spans)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(status: DevinStatus, is_archived: bool) -> DevinSession {
        DevinSession {
            status,
            is_archived,
            ..DevinSession::fixture("devin-1", "https://github.com/o/r/pull/1")
        }
    }

    #[test]
    fn devin_style_by_status() {
        let fg = |status| devin_style(&session(status, false)).fg;
        assert_eq!(fg(DevinStatus::New), Some(Color::Yellow));
        assert_eq!(fg(DevinStatus::Claimed), Some(Color::Yellow));
        assert_eq!(fg(DevinStatus::Running), Some(Color::Yellow));
        assert_eq!(fg(DevinStatus::Resuming), Some(Color::Yellow));
        assert_eq!(fg(DevinStatus::Suspended), Some(Color::Cyan));
        assert_eq!(fg(DevinStatus::Error), Some(Color::Red));
        assert_eq!(fg(DevinStatus::Exit), Some(DIM));
        assert_eq!(fg(DevinStatus::Unknown), Some(DIM));
    }

    #[test]
    fn archived_is_dim() {
        let s = session(DevinStatus::Error, true);
        assert_eq!(devin_style(&s).fg, Some(DIM));
    }
}
