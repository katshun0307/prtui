use ratatui::style::{Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

use super::DIM;
use crate::app::AppState;
use crate::domain::pr::{CheckState, Pr, ReviewerState};

pub(super) fn detail_view(state: &AppState) -> Paragraph<'static> {
    let block = Block::new()
        .borders(Borders::TOP)
        .border_style(Style::new().fg(DIM))
        .title(" 詳細 ");
    let Some(p) = state.selected() else {
        return Paragraph::new(Span::raw("PR がない").fg(DIM)).block(block);
    };
    let mut lines = vec![
        Line::from(vec![
            Span::raw(format!("#{} ", p.number)).bold(),
            Span::raw(p.title.clone()),
        ]),
        Line::from(vec![
            Span::raw("branch: ").fg(DIM),
            Span::raw(p.head_ref_name.clone()).cyan(),
            Span::raw(" ← ").fg(DIM),
            Span::raw(p.base_ref_name.clone()).cyan(),
            Span::raw("  author: ").fg(DIM),
            Span::raw(p.author.clone()),
            Span::raw("  assignees: ").fg(DIM),
            Span::raw(p.assignees.join(", ")),
        ]),
        Line::from(vec![
            Span::raw(format!("+{}", p.additions)).green(),
            Span::raw(" "),
            Span::raw(format!("-{}", p.deletions)).red(),
            Span::raw(format!("  {} files", p.changed_files)),
            Span::raw("  threads: ").fg(DIM),
            Span::raw(format!("未解決 {} / {}", p.threads_open, p.threads_total)),
            Span::raw("  labels: ").fg(DIM),
            Span::raw(p.labels.join(", ")),
        ]),
        Line::from(reviewer_spans(p)),
    ];
    let failed = check_names(p, CheckState::Failure);
    if !failed.is_empty() {
        lines.push(Line::from(vec![
            Span::raw("失敗: ").red(),
            Span::raw(failed),
        ]));
    }
    let pending = check_names(p, CheckState::Pending);
    if !pending.is_empty() {
        lines.push(Line::from(vec![
            Span::raw("実行中: ").yellow(),
            Span::raw(pending),
        ]));
    }
    lines.push(Line::from(Span::raw(p.url.clone()).fg(DIM).underlined()));
    Paragraph::new(lines).block(block).wrap(Wrap { trim: true })
}

fn check_names(p: &Pr, state: CheckState) -> String {
    p.checks
        .iter()
        .filter(|c| c.state == state)
        .map(|c| c.name.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

fn reviewer_spans(p: &Pr) -> Vec<Span<'static>> {
    let mut spans = vec![Span::raw("reviewers: ").fg(DIM)];
    if p.reviewers.is_empty() {
        spans.push(Span::raw("未設定").fg(DIM));
        return spans;
    }
    for (i, r) in p.reviewers.iter().enumerate() {
        if i > 0 {
            spans.push(Span::raw(", "));
        }
        let text = r.name.clone();
        spans.push(match r.state {
            ReviewerState::Requested => Span::raw(format!("{text} (依頼中)")).yellow(),
            ReviewerState::Approved => Span::raw(format!("{text} ✓")).green(),
            ReviewerState::ChangesRequested => Span::raw(format!("{text} ✗")).red(),
            ReviewerState::Commented => Span::raw(format!("{text} (コメント)")).fg(DIM),
        });
    }
    spans
}
