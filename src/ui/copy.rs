use ratatui::Frame;
use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::style::Stylize;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph};

use super::DIM;
use crate::domain::copy::CopyItem;

/// 一覧の上に出すコピー選択画面. 1 行に キー, 項目名, コピーする値 を並べ,
/// 値が作れない項目は理由を DIM で出す
pub(super) fn draw_copy_menu(f: &mut Frame, items: &[CopyItem], area: Rect) {
    // format! の幅指定は文字数で数えて全角がずれるので, 表示幅で揃える
    let label_width = items
        .iter()
        .map(|i| Span::raw(i.label).width())
        .max()
        .unwrap_or(0);
    let lines: Vec<Line> = items
        .iter()
        .map(|item| {
            let key = Span::raw(format!(" {} ", item.key));
            let pad = label_width - Span::raw(item.label).width() + 2;
            let label = Span::raw(format!("{}{}", item.label, " ".repeat(pad)));
            match &item.value {
                Ok(v) => Line::from(vec![key.cyan().bold(), label, Span::raw(v.clone())]),
                Err(reason) => Line::from(vec![
                    key.fg(DIM),
                    label.fg(DIM),
                    Span::raw(reason.clone()).fg(DIM),
                ]),
            }
        })
        .collect();
    let width = area.width.saturating_sub(4).min(100);
    let height = (lines.len() as u16 + 2).min(area.height);
    let [popup] = Layout::horizontal([Constraint::Length(width)])
        .flex(Flex::Center)
        .areas(area);
    let [popup] = Layout::vertical([Constraint::Length(height)])
        .flex(Flex::Center)
        .areas(popup);
    f.render_widget(Clear, popup);
    f.render_widget(
        Paragraph::new(lines).block(Block::bordered().title(" コピー ")),
        popup,
    );
}
