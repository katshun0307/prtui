use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Stylize;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, List, ListItem, Paragraph};

use super::{DIM, SELECTED_BG, label_dot, popup_area};
use crate::app::AppState;
use crate::domain::label::has_label;

/// 一覧の上に出すラベルの選択画面. 1 行目が絞り込みの入力, その下にラベルを並べる.
/// 印は 付いている `[x]`, 足す `[+]` (green), 外す `[-]` (red), 付いていない `[ ]` (DIM).
/// 名前の前の ● は GitHub のラベルの色 (`label_dot`)
pub(super) fn draw_label_menu(f: &mut Frame, state: &mut AppState, area: Rect) {
    let choices = state.label_choices();
    let loading = state.labels_loading;
    let Some(menu) = state.label_menu_mut() else {
        return;
    };
    let name_width = choices
        .iter()
        .map(|l| Span::raw(l.name.as_str()).width())
        .max()
        .unwrap_or(0);
    let items: Vec<ListItem> = choices
        .iter()
        .map(|l| {
            let mark = match (
                has_label(&menu.before, &l.name),
                has_label(&menu.chosen, &l.name),
            ) {
                (true, true) => Span::raw(" [x] "),
                (false, true) => Span::raw(" [+] ").green().bold(),
                (true, false) => Span::raw(" [-] ").red().bold(),
                (false, false) => Span::raw(" [ ] ").fg(DIM),
            };
            let pad = name_width - Span::raw(l.name.as_str()).width() + 2;
            ListItem::new(Line::from(vec![
                mark,
                label_dot(l),
                Span::raw(format!("{}{}", l.name, " ".repeat(pad))),
                Span::raw(l.description.clone()).fg(DIM),
            ]))
        })
        .collect();

    let mut status = vec![];
    if let Some(e) = &menu.error {
        status.push(Line::from(Span::raw(format!(" {e}")).red()));
    }
    if choices.is_empty() {
        let text = if loading {
            " ラベルを取得中…"
        } else {
            " 該当するラベルが無い"
        };
        status.push(Line::from(Span::raw(text).fg(DIM)));
    }

    // 入力 1 行と選択肢
    let popup = popup_area(area, (items.len() + status.len() + 1) as u16);
    // 選択肢が無いときは本文に取得中と出す
    let title = if loading && !choices.is_empty() {
        format!(" ラベル #{} (取得中…) ", menu.number)
    } else {
        format!(" ラベル #{} ", menu.number)
    };
    let block = Block::bordered().title(title);
    let inner = block.inner(popup);
    f.render_widget(Clear, popup);
    f.render_widget(block, popup);

    let [input, status_area, list] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(status.len() as u16),
        Constraint::Min(0),
    ])
    .areas(inner);
    f.render_widget(
        Line::from(vec![
            Span::raw(" > ").cyan().bold(),
            Span::raw(menu.query.clone()),
            Span::raw("▏").cyan(),
        ]),
        input,
    );
    f.render_widget(Paragraph::new(status), status_area);
    f.render_stateful_widget(
        List::new(items).highlight_style(ratatui::style::Style::new().bg(SELECTED_BG)),
        list,
        &mut menu.list,
    );
}
