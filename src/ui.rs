use chrono::{DateTime, Utc};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table, Wrap};

use crate::app::App;
use crate::git::LocalState;
use crate::github::{CheckState, Pr, ReviewerState};

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
const DIM: Color = Color::Indexed(245);
const SELECTED_BG: Color = Color::Indexed(237);
const HEADER_FG: Color = Color::Indexed(250);

pub fn draw(f: &mut Frame, app: &mut App) {
    let [header, list, detail, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(5),
        Constraint::Length(10),
        Constraint::Length(1),
    ])
    .areas(f.area());

    f.render_widget(header_line(app), header);
    draw_table(f, app, list);
    f.render_widget(detail_view(app), detail);
    f.render_widget(footer_line(app), footer);
}

fn header_line(app: &App) -> Line<'static> {
    let mut spans = vec![
        Span::raw(format!(" {} ", app.repo)).bold(),
        Span::raw(format!("{}  ", app.dir.display())).fg(DIM),
        Span::raw(format!("branch: {}", app.local.current_branch)).cyan(),
    ];
    if let Some(t) = app.fetched_at {
        let label = if app.from_cache {
            format!("  取得 {} 前 (キャッシュ)", ago(t))
        } else {
            format!("  取得 {} 前", ago(t))
        };
        spans.push(Span::raw(label).fg(DIM));
    }
    if app.local.dirty {
        spans.push(Span::raw("  未コミットの変更あり").yellow());
    }
    if app.draft_only {
        spans.push(Span::raw("  [Draft のみ]").magenta());
    }
    Line::from(spans)
}

fn draw_table(f: &mut Frame, app: &mut App, area: ratatui::layout::Rect) {
    let rows: Vec<Row> = app
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
                local_cell(&app.local, p),
                Cell::from(ago(p.updated_at)).fg(DIM),
                title_cell(p),
            ])
        })
        .collect();
    let count = rows.len();
    let mut syncing: Vec<_> = app.syncing.iter().map(|n| format!("#{n}")).collect();
    syncing.sort();
    let title = if app.loading {
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
    f.render_stateful_widget(table, area, &mut app.table);
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
    if p.mergeable == "CONFLICTING" {
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

fn detail_view(app: &App) -> Paragraph<'static> {
    let block = Block::new()
        .borders(Borders::TOP)
        .border_style(Style::new().fg(DIM))
        .title(" 詳細 ");
    let Some(p) = app.selected() else {
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

fn footer_line(app: &App) -> Line<'static> {
    if let Some(busy) = &app.busy {
        return Line::from(Span::raw(format!(" {busy}")).yellow());
    }
    if let Some((n, base)) = &app.confirm_update {
        return Line::from(
            Span::raw(format!(
                " #{n} に {base} を merge commit で取り込んで push する? (y/N)"
            ))
            .yellow()
            .bold(),
        );
    }
    if let Some((n, op)) = app.confirm_op {
        return Line::from(
            Span::raw(format!(
                " #{n} を {}する? もう一度 {} で実行, 他のキーで取りやめ",
                op.phrase(&app.local.reviewers),
                op.key()
            ))
            .yellow()
            .bold(),
        );
    }
    if let Some(m) = &app.message {
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

pub fn ago(t: DateTime<Utc>) -> String {
    let secs = (Utc::now() - t).num_seconds().max(0);
    match secs {
        s if s < 3600 => format!("{}m", s / 60),
        s if s < 86400 => format!("{}h", s / 3600),
        s => format!("{}d", s / 86400),
    }
}
