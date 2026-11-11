use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph};

use super::{DIM, popup_area};
use crate::app::AppState;
use crate::domain::merge::merge_warnings;

/// 一覧の上に出すマージの確認画面. PR のタイトルとブランチ, 注意点 (yellow), 方法のキーを並べる.
/// 既定の方法 (Enter) は bold にして `(Enter)` を付ける
pub(super) fn draw_merge_menu(f: &mut Frame, state: &AppState, area: Rect) {
    let Some(menu) = &state.merge_menu else {
        return;
    };
    let Some(pr) = state.pr(menu.number) else {
        return;
    };
    let mut lines = vec![
        Line::from(Span::raw(format!(" {}", pr.title)).bold()),
        Line::from(vec![
            Span::raw(format!(" {}", pr.head_ref_name)).cyan(),
            Span::raw(" → ").fg(DIM),
            Span::raw(pr.base_ref_name.clone()).cyan(),
        ]),
        Line::raw(""),
    ];
    let warnings = merge_warnings(pr, &state.mine.prs);
    if warnings.is_empty() {
        lines.push(Line::from(
            Span::raw(" ✓ CI, レビュー, スレッドに気になる点は無い").green(),
        ));
    }
    for w in warnings {
        lines.push(Line::from(Span::raw(format!(" ! {w}")).yellow()));
    }
    lines.push(Line::raw(""));
    match &state.merge_settings {
        None => lines.push(Line::from(Span::raw(" マージ方法を取得中…").fg(DIM))),
        Some(settings) => {
            for m in &settings.allowed {
                let key = Span::raw(format!(" {} ", m.key())).cyan().bold();
                if settings.default == Some(*m) {
                    lines.push(Line::from(vec![
                        key,
                        Span::raw(format!(" {}", m.label())).bold(),
                        Span::raw("  (Enter)").fg(DIM),
                    ]));
                } else {
                    lines.push(Line::from(vec![key, Span::raw(format!(" {}", m.label()))]));
                }
            }
        }
    }
    let popup = popup_area(area, lines.len() as u16);
    f.render_widget(Clear, popup);
    f.render_widget(
        Paragraph::new(lines).block(
            Block::bordered()
                .title(format!(" #{} をマージする ", menu.number))
                .border_style(Style::new().yellow()),
        ),
        popup,
    );
}
