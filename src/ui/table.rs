use ratatui::Frame;
use ratatui::layout::Constraint;
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Cell, Row, Table};

use super::format::{ago, devin_label};
use super::{DIM, HEADER_FG, SELECTED_BG, devin_style};
use crate::app::{AppState, Tab};
use crate::domain::devin::DevinSession;
use crate::domain::local::LocalState;
use crate::domain::pr::{CheckState, MyReview, Pr, ReviewerState};

pub(super) fn draw_table(f: &mut Frame, state: &mut AppState, area: ratatui::layout::Rect) {
    let (header, widths, rows) = match state.tab {
        Tab::Mine => mine_rows(state),
        Tab::Review => review_rows(state),
        Tab::Custom(_) => custom_rows(state),
    };
    let title = table_title(state, rows.len());
    let table = Table::new(rows, widths)
        .header(
            Row::new(header).style(
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
    f.render_stateful_widget(table, area, &mut state.list_mut().table);
}

type Columns<'a> = (Vec<&'static str>, Vec<Constraint>, Vec<Row<'a>>);

fn mine_rows(state: &AppState) -> Columns<'static> {
    let rows = state
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
    let header = vec![
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
    ];
    let widths = vec![
        Constraint::Length(6),
        Constraint::Length(5),
        Constraint::Length(8),
        Constraint::Length(7),
        Constraint::Length(6),
        Constraint::Length(5),
        Constraint::Length(12),
        Constraint::Length(9),
        Constraint::Length(13),
        Constraint::Length(10),
        Constraint::Length(11),
        Constraint::Length(4),
        Constraint::Fill(1),
    ];
    (header, widths, rows)
}

/// レビュータブ. 他人の PR なので main 取り込みの要否と stack は出さず, author と自分の立場を出す
fn review_rows(state: &AppState) -> Columns<'static> {
    let rows = state
        .visible()
        .into_iter()
        .map(|p| {
            Row::new(vec![
                Cell::from(format!("#{}", p.number)),
                state_cell(p),
                ci_cell(p),
                threads_cell(p),
                size_cell(p),
                Cell::from(p.author.clone()),
                my_review_cell(p),
                requested_at_cell(p),
                reviewer_cell(p),
                local_cell(&state.local, p),
                Cell::from(devin_span(state.devin_for(p))),
                Cell::from(ago(p.updated_at)).fg(DIM),
                Cell::from(p.title.clone()),
            ])
        })
        .collect();
    let header = vec![
        "#",
        "状態",
        "CI",
        "未解決",
        "差分",
        "Author",
        "自分",
        "依頼",
        "Reviewer",
        "Local",
        "Devin",
        "更新",
        "タイトル",
    ];
    let widths = vec![
        Constraint::Length(6),
        Constraint::Length(8),
        Constraint::Length(7),
        Constraint::Length(6),
        Constraint::Length(12),
        Constraint::Length(14),
        Constraint::Length(10),
        Constraint::Length(4),
        Constraint::Length(13),
        Constraint::Length(10),
        Constraint::Length(11),
        Constraint::Length(4),
        Constraint::Fill(1),
    ];
    (header, widths, rows)
}

/// 追加のタブ. 自分と他人の PR が混ざるので Author と Assignee を出す. マージの判断に使うことが多いので
/// main を出す. stack はまとめないので Stack は出さない. Review (`reviewDecision`) は必須レビューのルールが
/// 無い repo では常に `-` なので出さず, 承認の状況は Reviewer で見る
fn custom_rows(state: &AppState) -> Columns<'static> {
    let rows = state
        .visible()
        .into_iter()
        .map(|p| {
            Row::new(vec![
                Cell::from(format!("#{}", p.number)),
                state_cell(p),
                ci_cell(p),
                threads_cell(p),
                behind_cell(p),
                size_cell(p),
                reviewer_cell(p),
                Cell::from(p.author.clone()),
                assignee_cell(p),
                local_cell(&state.local, p),
                Cell::from(devin_span(state.devin_for(p))),
                Cell::from(ago(p.updated_at)).fg(DIM),
                Cell::from(p.title.clone()),
            ])
        })
        .collect();
    let header = vec![
        "#",
        "状態",
        "CI",
        "未解決",
        "main",
        "差分",
        "Reviewer",
        "Author",
        "Assignee",
        "Local",
        "Devin",
        "更新",
        "タイトル",
    ];
    let widths = vec![
        Constraint::Length(6),
        Constraint::Length(8),
        Constraint::Length(7),
        Constraint::Length(6),
        Constraint::Length(5),
        Constraint::Length(12),
        Constraint::Length(13),
        Constraint::Length(12),
        Constraint::Length(12),
        Constraint::Length(10),
        Constraint::Length(11),
        Constraint::Length(4),
        Constraint::Fill(1),
    ];
    (header, widths, rows)
}

/// 先頭の assignee と残りの人数 `alice +1`. 全員は詳細欄に出す. 居なければ DIM の `-`
fn assignee_cell(p: &Pr) -> Cell<'static> {
    match p.assignees.as_slice() {
        [] => Cell::from("-").fg(DIM),
        [one] => Cell::from(one.clone()),
        [first, rest @ ..] => Cell::from(Line::from(vec![
            Span::raw(first.clone()),
            Span::raw(format!(" +{}", rest.len())).fg(DIM),
        ])),
    }
}

/// ` PR n 件 (更新中…) `. 取得に失敗していれば赤でエラーを足す
fn table_title(state: &AppState, count: usize) -> Line<'static> {
    let list = state.list();
    let mut syncing: Vec<_> = state.syncing.iter().map(|n| format!("#{n}")).collect();
    syncing.sort();
    let text = if list.loading {
        format!(" PR {count} 件 (更新中…) ")
    } else if state.tab == Tab::Mine && !syncing.is_empty() {
        format!(" PR {count} 件 ({} を同期中…) ", syncing.join(" "))
    } else {
        format!(" PR {count} 件 ")
    };
    let mut spans = vec![Span::raw(text)];
    if let Some(e) = &list.error {
        spans.push(Span::raw(format!("{e} ")).red());
    }
    Line::from(spans)
}

fn my_review_cell(p: &Pr) -> Cell<'static> {
    match &p.my_review {
        Some(m) => Cell::from(m.label()).fg(my_review_color(m)),
        None => Cell::from(""),
    }
}

/// 依頼を受けてからの経過時間. 対応待ちなので yellow. レビュー済みと時刻が分からないものは空
fn requested_at_cell(p: &Pr) -> Cell<'static> {
    match p.my_review.as_ref().and_then(|m| m.requested_at) {
        Some(t) => Cell::from(ago(t)).yellow(),
        None => Cell::from(""),
    }
}

/// 依頼中と再依頼は自分の対応が要るので yellow. レビュー済みは最新のレビューの色
fn my_review_color(m: &MyReview) -> Color {
    if m.requested {
        return Color::Yellow;
    }
    match m.last {
        Some(ReviewerState::Approved) => Color::Green,
        Some(ReviewerState::ChangesRequested) => Color::Red,
        _ => DIM,
    }
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

/// 承認 ✓n, 変更要求 ✗n, 依頼中 ●n, コメントのみ ✎n を数で出す. Bot は reviewers に入らないので数えない.
/// 承認, 変更要求, 依頼中が居なければ「未設定」を前に出す (コメントのみでは reviewer を設定したことにならない).
/// Draft なら設定前で当然なので DIM, それ以外は対応が要るので yellow
fn reviewer_cell(p: &Pr) -> Cell<'static> {
    Cell::from(Line::from(reviewer_counts(p)))
}

fn reviewer_counts(p: &Pr) -> Vec<Span<'static>> {
    let approved = p.count_reviewers(ReviewerState::Approved);
    let changes = p.count_reviewers(ReviewerState::ChangesRequested);
    let requested = p.count_reviewers(ReviewerState::Requested);
    let commented = p.count_reviewers(ReviewerState::Commented);
    let mut spans = Vec::new();
    if approved + changes + requested == 0 {
        let span = Span::raw("未設定 ");
        spans.push(if p.is_draft {
            span.fg(DIM)
        } else {
            span.yellow()
        });
    }
    if approved > 0 {
        spans.push(Span::raw(format!("✓{approved} ")).green());
    }
    if changes > 0 {
        spans.push(Span::raw(format!("✗{changes} ")).red());
    }
    if requested > 0 {
        spans.push(Span::raw(format!("●{requested} ")).yellow());
    }
    if commented > 0 {
        spans.push(Span::raw(format!("✎{commented}")).fg(DIM));
    }
    spans
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
    use crate::domain::pr::Reviewer;
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
    fn reviewer_counts_include_commenters() {
        let reviewer = |name: &str, state| Reviewer {
            name: name.into(),
            state,
        };
        let text = |p: &Pr| {
            reviewer_counts(p)
                .iter()
                .map(|s| s.content.to_string())
                .collect::<String>()
        };
        let mut p = Pr::fixture(1, "a", "main");
        p.reviewers = vec![
            reviewer("alice", ReviewerState::Requested),
            reviewer("bob", ReviewerState::Approved),
            reviewer("carol", ReviewerState::Commented),
            reviewer("dave", ReviewerState::Commented),
        ];
        assert_eq!(text(&p), "✓1 ●1 ✎2");
        p.reviewers.retain(|r| r.state == ReviewerState::Commented);
        assert_eq!(text(&p), "未設定 ✎2");
        assert_eq!(reviewer_counts(&p)[0].style.fg, Some(Color::Yellow));
    }

    #[test]
    fn my_review_cell_color() {
        let fg = |requested, last| {
            my_review_color(&MyReview {
                requested,
                last,
                via_teams: vec![],
                requested_at: None,
            })
        };
        assert_eq!(fg(true, None), Color::Yellow);
        assert_eq!(fg(true, Some(ReviewerState::Approved)), Color::Yellow);
        assert_eq!(fg(false, Some(ReviewerState::Approved)), Color::Green);
        assert_eq!(fg(false, Some(ReviewerState::ChangesRequested)), Color::Red);
        assert_eq!(fg(false, Some(ReviewerState::Commented)), DIM);
        assert_eq!(fg(false, None), DIM);
    }

    #[test]
    fn devin_span_empty_without_session() {
        assert_eq!(devin_span(None).content, "");
    }
}
