use ratatui::Frame;
use ratatui::layout::Constraint;
use ratatui::style::{Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Cell, Row, Table};

use super::format::{ago, devin_label};
use super::{DIM, HEADER_FG, SELECTED_BG, devin_style};
use crate::app::AppState;
use crate::domain::devin::DevinSession;
use crate::domain::local::LocalState;
use crate::domain::pr::{CheckState, Pr, ReviewerState};

pub(super) fn draw_table(f: &mut Frame, state: &mut AppState, area: ratatui::layout::Rect) {
    let rows: Vec<Row> = state
        .visible()
        .into_iter()
        .map(|p| {
            Row::new(vec![
                Cell::from(format!("#{}", p.number)),
                stack_cell(p),
                state_cell(p),
                ci_cell(p),
                threads_cell(p),
                behind_cell(p),
                size_cell(p),
                review_cell(p),
                reviewer_cell(p),
                local_cell(&state.local, p),
                Cell::from(devin_span(state.devin_for(p))),
                Cell::from(ago(p.updated_at)).fg(DIM),
                title_cell(p),
            ])
        })
        .collect();
    let count = rows.len();
    let mut syncing: Vec<_> = state.syncing.iter().map(|n| format!("#{n}")).collect();
    syncing.sort();
    let title = if state.loading {
        format!(" PR {count} 件 (更新中…) ")
    } else if !syncing.is_empty() {
        format!(" PR {count} 件 ({} を同期中…) ", syncing.join(" "))
    } else {
        format!(" PR {count} 件 ")
    };
    let table = Table::new(
        rows,
        [
            Constraint::Length(6),
            Constraint::Length(5),
            Constraint::Length(8),
            Constraint::Length(7),
            Constraint::Length(6),
            Constraint::Length(5),
            Constraint::Length(12),
            Constraint::Length(9),
            Constraint::Length(10),
            Constraint::Length(10),
            Constraint::Length(11),
            Constraint::Length(4),
            Constraint::Fill(1),
        ],
    )
    .header(
        Row::new([
            "#",
            "Stack",
            "状態",
            "CI",
            "未解決",
            "main",
            "差分",
            "Review",
            "Reviewer",
            "Local",
            "Devin",
            "更新",
            "タイトル",
        ])
        .style(
            Style::new()
                .fg(HEADER_FG)
                .add_modifier(Modifier::UNDERLINED),
        ),
    )
    .block(
        Block::new()
            .borders(Borders::TOP)
            .border_style(Style::new().fg(DIM))
            .title(title),
    )
    .row_highlight_style(Style::new().bg(SELECTED_BG))
    .highlight_symbol(Span::raw("▶ ").cyan().bold());
    f.render_stateful_widget(table, area, &mut state.table);
}

/// stack 内の位置 `段/段数`. main に近い側が 1. stack に入っていなければ空
fn stack_cell(p: &Pr) -> Cell<'static> {
    match p.stack {
        Some(s) => Cell::from(format!("{}/{}", s.pos, s.len)).cyan(),
        None => Cell::from(""),
    }
}

/// stack の 2 段目以降は段に応じて字下げし, 親の下に積んであることが分かるようにする
fn title_cell(p: &Pr) -> Cell<'static> {
    match p.stack {
        Some(s) if s.pos > 1 => Cell::from(Line::from(vec![
            Span::raw(format!("{}└ ", "  ".repeat(s.pos - 2))).fg(DIM),
            Span::raw(p.title.clone()),
        ])),
        _ => Cell::from(p.title.clone()),
    }
}

fn state_cell(p: &Pr) -> Cell<'static> {
    if p.is_conflicting() {
        Cell::from("Conflict").red().bold()
    } else if p.is_draft {
        Cell::from("Draft").fg(DIM)
    } else {
        Cell::from("Ready").green()
    }
}

fn ci_cell(p: &Pr) -> Cell<'static> {
    let total = p.effective_checks();
    let failed = p.count_checks(CheckState::Failure);
    let pending = p.count_checks(CheckState::Pending);
    if total == 0 {
        Cell::from("-").fg(DIM)
    } else if failed > 0 {
        Cell::from(format!("✗ {failed}/{total}")).red()
    } else if pending > 0 {
        Cell::from(format!("● {pending}/{total}")).yellow()
    } else {
        Cell::from(format!("✓ {total}")).green()
    }
}

fn threads_cell(p: &Pr) -> Cell<'static> {
    if p.threads_open > 0 {
        Cell::from(p.threads_open.to_string()).yellow().bold()
    } else {
        Cell::from("0").fg(DIM)
    }
}

fn behind_cell(p: &Pr) -> Cell<'static> {
    match p.behind_main {
        None => Cell::from("-").fg(DIM),
        Some(0) => Cell::from("0").fg(DIM),
        Some(n) => Cell::from(format!("↓{n}")),
    }
}

/// 追加行 +x (green) と削除行 -y (red). 詳細欄の表示と同じ色にそろえる
fn size_cell(p: &Pr) -> Cell<'static> {
    Cell::from(Line::from(vec![
        Span::raw(format!("+{}", p.additions)).green(),
        Span::raw(" "),
        Span::raw(format!("-{}", p.deletions)).red(),
    ]))
}

fn review_cell(p: &Pr) -> Cell<'static> {
    match p.review_decision.as_deref() {
        Some("APPROVED") => Cell::from("APPROVED").green(),
        Some("CHANGES_REQUESTED") => Cell::from("CHANGES").red(),
        Some("REVIEW_REQUIRED") => Cell::from("REQUIRED").yellow(),
        _ => Cell::from("-").fg(DIM),
    }
}

/// 承認 ✓n, 変更要求 ✗n, 依頼中 ●n を数で出す. コメントのみの reviewer (bot を含む) は数えない.
/// 数える reviewer が居なければ「未設定」. Draft なら設定前で当然なので DIM, それ以外は対応が要るので yellow
fn reviewer_cell(p: &Pr) -> Cell<'static> {
    let approved = p.count_reviewers(ReviewerState::Approved);
    let changes = p.count_reviewers(ReviewerState::ChangesRequested);
    let requested = p.count_reviewers(ReviewerState::Requested);
    if approved + changes + requested == 0 {
        let cell = Cell::from("未設定");
        return if p.is_draft {
            cell.fg(DIM)
        } else {
            cell.yellow()
        };
    }
    let mut spans = Vec::new();
    if approved > 0 {
        spans.push(Span::raw(format!("✓{approved} ")).green());
    }
    if changes > 0 {
        spans.push(Span::raw(format!("✗{changes} ")).red());
    }
    if requested > 0 {
        spans.push(Span::raw(format!("●{requested}")).yellow());
    }
    Cell::from(Line::from(spans))
}

/// PR に紐づく最新の session の status. 2 件以上なら `×n` を付ける. session が無ければ空
fn devin_span(found: Option<(&DevinSession, usize)>) -> Span<'static> {
    match found {
        Some((s, count)) => Span::styled(devin_label(s, count), devin_style(s)),
        None => Span::raw(""),
    }
}

fn local_cell(local: &LocalState, p: &Pr) -> Cell<'static> {
    let mark = if local.is_current(p) { "● " } else { "" };
    match local.sync_label(p) {
        None => Cell::from(""),
        Some(label) => {
            let cell = Cell::from(format!("{mark}{label}"));
            if label == "最新" {
                cell.cyan()
            } else {
                cell.magenta()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::devin::DevinStatus;
    use ratatui::style::Color;

    #[test]
    fn devin_span_label_and_color() {
        let s = DevinSession {
            status: DevinStatus::Suspended,
            ..DevinSession::fixture("devin-1", "https://github.com/o/r/pull/1")
        };
        let span = devin_span(Some((&s, 1)));
        assert_eq!(span.content, "suspended");
        assert_eq!(span.style.fg, Some(Color::Cyan));
        let span = devin_span(Some((&s, 3)));
        assert_eq!(span.content, "suspended×3");
    }

    #[test]
    fn devin_span_empty_without_session() {
        assert_eq!(devin_span(None).content, "");
    }
}
