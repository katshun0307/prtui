use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, List, ListItem, Paragraph};

use super::{DIM, SELECTED_BG, popup_area};
use crate::app::AppState;
use crate::domain::pr::ReviewerState;
use crate::domain::review::{ReviewerChoice, same_login};

/// 一覧の上に出す review 依頼の選択画面. 1 行目が絞り込みの入力, その下に組と個人を並べる.
/// 印は 選んだ `[+]` (green), 組の一部だけ選んだ `[~]` (green), 依頼中 `[x]`, 選んでいない `[ ]` (DIM).
/// 組は名前の前に `◆` を付けてメンバーを DIM で並べる. 個人は PR での今の状態を
/// 詳細欄の reviewers と同じ配色で後ろに出す
pub(super) fn draw_reviewer_menu(f: &mut Frame, state: &mut AppState, area: Rect) {
    let choices = state.reviewer_choices();
    let Some(menu) = state.reviewer_menu() else {
        return;
    };
    let Some(pr) = state.pr(menu.number) else {
        return;
    };
    let name = |c: &ReviewerChoice| match c {
        ReviewerChoice::Set { name, .. } => name.clone(),
        ReviewerChoice::Person { login, .. } => login.clone(),
    };
    let name_width = choices
        .iter()
        .map(|c| Span::raw(name(c)).width())
        .max()
        .unwrap_or(0);
    let items: Vec<ListItem> = choices
        .iter()
        .map(|c| {
            let targets = c.targets(pr);
            let chosen = targets
                .iter()
                .filter(|t| menu.chosen.iter().any(|x| same_login(x, t)))
                .count();
            let mark = if targets.is_empty() {
                Span::raw(" [x] ")
            } else if chosen == targets.len() {
                Span::raw(" [+] ").green().bold()
            } else if chosen > 0 {
                Span::raw(" [~] ").green()
            } else {
                Span::raw(" [ ] ").fg(DIM)
            };
            let text = name(c);
            let pad = " ".repeat(name_width - Span::raw(text.as_str()).width() + 2);
            let (head, tail) = match c {
                ReviewerChoice::Set { members, .. } => (
                    Span::raw(format!("◆ {text}{pad}")).bold(),
                    Span::raw(members.join(", ")).fg(DIM),
                ),
                ReviewerChoice::Person { state, .. } => {
                    (Span::raw(format!("  {text}{pad}")), status(*state))
                }
            };
            ListItem::new(Line::from(vec![mark, head, tail]))
        })
        .collect();

    let status_lines: Vec<Line> = if choices.is_empty() {
        vec![Line::from(Span::raw(" 該当する相手が居ない").fg(DIM))]
    } else {
        vec![]
    };
    let popup = popup_area(area, (items.len() + status_lines.len() + 1) as u16);
    let block = Block::bordered().title(format!(" review 依頼 #{} ", menu.number));
    let inner = block.inner(popup);
    f.render_widget(Clear, popup);
    f.render_widget(block, popup);

    let [input, status_area, list] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(status_lines.len() as u16),
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
    f.render_widget(Paragraph::new(status_lines), status_area);
    let Some(menu) = state.reviewer_menu_mut() else {
        return;
    };
    f.render_stateful_widget(
        List::new(items).highlight_style(Style::new().bg(SELECTED_BG)),
        list,
        &mut menu.list,
    );
}

/// PR での今の状態. 詳細欄の reviewers と同じ配色
fn status(state: Option<ReviewerState>) -> Span<'static> {
    match state {
        Some(ReviewerState::Requested) => Span::raw("依頼中").yellow(),
        Some(ReviewerState::Approved) => Span::raw("✓ 承認").green(),
        Some(ReviewerState::ChangesRequested) => Span::raw("✗ 変更要求").red(),
        Some(ReviewerState::Commented) => Span::raw("コメント").fg(DIM),
        None => Span::raw(""),
    }
}
