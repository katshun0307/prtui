use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};

use super::format::ago;
use super::{DIM, SELECTED_BG};
use crate::app::{AppState, PrPage, PrView};
use crate::domain::conversation::{Comment, Conversation, Entry, Verdict, tidy_body};
use crate::domain::diff::{Diff, DiffLine, FileDiff, FileStatus, LineKind, Row};

/// PR ビュー (説明とコメントか diff) を全画面に描く. scroll の上限と欄の高さを `PrView` に書く
pub(super) fn draw_pr_view(f: &mut Frame, state: &mut AppState) {
    let Some(view) = state.pr_view_mut() else {
        return;
    };
    let [header, body, footer] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Min(1),
        Constraint::Length(1),
    ])
    .areas(f.area());

    // スクロールしてもどの PR かが分かるよう, 背景を付けて本文と分ける
    f.render_widget(
        Paragraph::new(header_lines(view)).style(Style::new().bg(SELECTED_BG)),
        header,
    );

    view.viewport_height = body.height;
    match view.page {
        PrPage::Conversation => draw_conversation(f, view, body),
        PrPage::Diff => draw_diff(f, view, body),
    }

    f.render_widget(footer_line(view), footer);
}

/// 折り返した後の行で, スレッドの見出しの位置を `thread_rows` に書く (`]`/`[` で飛ぶ先)
fn draw_conversation(f: &mut Frame, view: &mut PrView, body: Rect) {
    let (lines, thread_lines) = match &view.conversation {
        Some(c) => conversation_lines(c, body.width, view.thread.as_deref()),
        None if view.fetching => (vec![Line::from(Span::raw("取得中…").fg(DIM))], vec![]),
        None => (vec![], vec![]),
    };
    // 折り返しは行ごとなので, 1 行ずつの高さを足せば全体と各行の位置になる
    let mut starts = Vec::with_capacity(lines.len());
    let mut total = 0;
    for l in &lines {
        starts.push(total);
        total += Paragraph::new(l.clone())
            .wrap(Wrap { trim: false })
            .line_count(body.width);
    }
    view.thread_rows = thread_lines.iter().map(|&i| starts[i]).collect();
    let para = Paragraph::new(lines).wrap(Wrap { trim: false });
    view.max_scroll = total.saturating_sub(body.height as usize);
    let offset = view.scroll.min(view.max_scroll);
    f.render_widget(para.scroll((offset.min(u16::MAX as usize) as u16, 0)), body);
}

/// diff は折り返さない. 行の位置がずれずファイルの見出しへ飛べるようにするため.
/// 長い行は `h`/`l` で横に送る. ファイルの見出しは横に送らない. 大きな diff でも重くならないよう,
/// 見えている行だけを作る
fn draw_diff(f: &mut Frame, view: &mut PrView, body: Rect) {
    let page = &mut view.diff;
    let Some(d) = &page.diff else {
        let text = if page.fetching { "取得中…" } else { "" };
        f.render_widget(Line::from(Span::raw(text).fg(DIM)), body);
        return;
    };
    if d.files.is_empty() {
        f.render_widget(Line::from(Span::raw("差分なし").fg(DIM)), body);
        return;
    }
    page.max_scroll = d.row_count().saturating_sub(body.height as usize);
    let offset = page.scroll.min(page.max_scroll);
    let rows: Vec<Row> = d.rows().skip(offset).take(body.height as usize).collect();

    let lines: Vec<Line> = rows
        .iter()
        .map(|r| match r {
            Row::Line(l) => diff_line(l),
            Row::File(_) | Row::Blank => Line::default(),
        })
        .collect();
    let widest = lines.iter().map(Line::width).max().unwrap_or(0);
    page.max_x = widest.saturating_sub(body.width as usize);
    let x = page.x.min(page.max_x);
    f.render_widget(
        Paragraph::new(lines).scroll((0, x.min(u16::MAX as usize) as u16)),
        body,
    );
    for (i, r) in rows.iter().enumerate() {
        if let Row::File(file) = r {
            let area = Rect {
                y: body.y + i as u16,
                height: 1,
                ..body
            };
            f.render_widget(file_rule(file, body.width), area);
        }
    }
}

/// `── <path>  <状態>  +n -m ───…`. rename は `old → new`
fn file_rule(file: &FileDiff, width: u16) -> Line<'static> {
    let mut spans = Vec::new();
    if let Some(old) = &file.old_path {
        spans.push(Span::raw(old.clone()).bold());
        spans.push(Span::raw(" → ").fg(DIM));
    }
    spans.push(Span::raw(file.path.clone()).bold());
    match file.status {
        FileStatus::Added => spans.push(Span::raw("  新規").green()),
        FileStatus::Deleted => spans.push(Span::raw("  削除").red()),
        FileStatus::Renamed | FileStatus::Modified => {}
    }
    if file.binary {
        spans.push(Span::raw("  バイナリ").fg(DIM));
    }
    // rename だけやバイナリでは行の増減が無いので出さない
    if file.additions + file.deletions > 0 {
        spans.push(Span::raw(format!("  +{}", file.additions)).green());
        spans.push(Span::raw(format!(" -{}", file.deletions)).red());
    }
    rule(spans, width)
}

/// 追加は green, 削除は red, hunk の見出しは cyan. タブは幅が定まらないので空白 4 つにする
fn diff_line(l: &DiffLine) -> Line<'static> {
    let text = l.text.replace('\t', "    ");
    let span = Span::raw(text);
    Line::from(match l.kind {
        LineKind::Add => span.green(),
        LineKind::Del => span.red(),
        LineKind::Hunk => span.cyan(),
        LineKind::Note => span.fg(DIM),
        LineKind::Context => span,
    })
}

/// 1 行目に ` #<番号>  <title>  [Draft]`, 2 行目に author, ブランチと, 取得後は書き込みの件数
fn header_lines(view: &PrView) -> Vec<Line<'static>> {
    let mut title = vec![
        Span::raw(format!(" #{}  ", view.number)).bold(),
        Span::raw(view.title.clone()).bold(),
    ];
    if view.is_draft {
        title.push(Span::raw("  Draft").fg(DIM));
    }
    let mut info = vec![
        Span::raw(format!(" {}", view.author)),
        Span::raw("  "),
        Span::raw(view.head.clone()).cyan(),
        Span::raw(" → ").fg(DIM),
        Span::raw(view.base.clone()).cyan(),
    ];
    match view.page {
        PrPage::Conversation => conversation_counts(view, &mut info),
        PrPage::Diff => diff_counts(view.diff.diff.as_ref(), &mut info),
    }
    vec![Line::from(title), Line::from(info)]
}

/// 説明とコメントのページ. 取得後に書き込みの件数
fn conversation_counts(view: &PrView, info: &mut Vec<Span<'static>>) {
    if let Some(c) = &view.conversation {
        let (comments, reviews, threads, open) = c.counts();
        info.push(
            Span::raw(format!(
                "  コメント {comments}  レビュー {reviews}  スレッド {threads}"
            ))
            .fg(DIM),
        );
        if open > 0 {
            info.push(Span::raw(format!(" (未解決 {open})")).yellow());
        }
    }
}

/// diff のページ. 取得後にファイル数と追加, 削除の行数
fn diff_counts(diff: Option<&Diff>, info: &mut Vec<Span<'static>>) {
    info.push(Span::raw("  diff").bold());
    if let Some(d) = diff {
        let (add, del) = d.totals();
        info.push(Span::raw(format!("  {} ファイル", d.files.len())).fg(DIM));
        info.push(Span::raw(format!("  +{add}")).green());
        info.push(Span::raw(format!(" -{del}")).red());
    }
}

/// 説明を先頭に, 書き込みを古い順に並べる. 各書き込みは見出しの罫線で区切る.
/// 行と, スレッドの見出しの行の位置 (`Conversation::threads` の順) を返す. selected のスレッドの見出しは背景を付ける
fn conversation_lines(
    c: &Conversation,
    width: u16,
    selected: Option<&str>,
) -> (Vec<Line<'static>>, Vec<usize>) {
    let mut lines = vec![rule(
        vec![
            Span::raw("説明").bold(),
            Span::raw("  "),
            Span::raw(c.author.clone()).bold(),
            Span::raw(format!("  {} 前", ago(c.created_at))).fg(DIM),
        ],
        width,
    )];
    let body = body_lines(&c.body, "");
    if body.is_empty() {
        lines.push(Line::from(Span::raw("説明なし").fg(DIM)));
    }
    lines.extend(body);
    if c.omitted > 0 {
        lines.push(Line::default());
        lines.push(Line::from(
            Span::raw(format!(
                "古い書き込み {} 件は省略. o でブラウザで見る",
                c.omitted
            ))
            .fg(DIM),
        ));
    }
    let mut thread_lines = vec![];
    for e in &c.entries {
        lines.push(Line::default());
        if matches!(e, Entry::Thread { .. }) {
            thread_lines.push(lines.len());
        }
        entry_lines(e, width, selected, &mut lines);
    }
    (lines, thread_lines)
}

fn entry_lines(e: &Entry, width: u16, selected: Option<&str>, lines: &mut Vec<Line<'static>>) {
    match e {
        Entry::Comment(c) => {
            lines.push(rule(byline(c, Span::raw("コメント").fg(DIM)), width));
            lines.extend(body_lines(&c.body, ""));
        }
        Entry::Review { verdict, comment } => {
            let style = match verdict {
                Verdict::Approved => Style::new().green(),
                Verdict::ChangesRequested => Style::new().red(),
                Verdict::Commented | Verdict::Dismissed => Style::new().fg(DIM),
            };
            let kind = Span::styled(verdict.label(), style);
            lines.push(rule(byline(comment, kind), width));
            lines.extend(body_lines(&comment.body, ""));
        }
        Entry::Thread {
            id,
            path,
            line,
            resolved,
            outdated,
            comments,
        } => {
            let place = match line {
                Some(l) => format!("{path}:{l}"),
                None => path.clone(),
            };
            let mut spans = vec![Span::raw(place).cyan(), Span::raw("  ")];
            spans.push(if *resolved {
                Span::raw("解決済").fg(DIM)
            } else {
                Span::raw("未解決").yellow()
            });
            if *outdated {
                spans.push(Span::raw("  outdated").fg(DIM));
            }
            let head = rule(spans, width);
            lines.push(if selected == Some(id.as_str()) {
                head.style(Style::new().bg(SELECTED_BG))
            } else {
                head
            });
            // スレッド内の返信は罫線にせず, 字下げした名前の行で分ける
            for c in comments {
                let mut by = vec![Span::raw("  ")];
                by.extend(byline(c, Span::raw("")));
                lines.push(Line::from(by));
                lines.extend(body_lines(&c.body, "  "));
            }
        }
    }
}

/// `<author>  <経過> 前  <kind>`
fn byline(c: &Comment, kind: Span<'static>) -> Vec<Span<'static>> {
    let mut spans = vec![
        Span::raw(c.author.clone()).bold(),
        Span::raw(format!("  {} 前", ago(c.created_at))).fg(DIM),
    ];
    if !kind.content.is_empty() {
        spans.push(Span::raw("  "));
        spans.push(kind);
    }
    spans
}

/// `── <見出し> ────…` を欄の幅いっぱいに引く. 書き込みの境目を見失わないため
fn rule(title: Vec<Span<'static>>, width: u16) -> Line<'static> {
    let mut spans = vec![Span::raw("── ").fg(DIM)];
    spans.extend(title);
    spans.push(Span::raw(" "));
    let used = Line::from(spans.clone()).width();
    let fill = (width as usize).saturating_sub(used);
    if fill > 0 {
        spans.push(Span::raw("─".repeat(fill)).fg(DIM));
    }
    Line::from(spans)
}

/// markdown の本文を行にする. 見出しは bold, 引用とコードブロックの囲みは DIM.
/// コードブロックの中は見出しや引用として扱わない
fn body_lines(body: &str, indent: &str) -> Vec<Line<'static>> {
    let mut in_code = false;
    tidy_body(body)
        .lines()
        .map(|l| {
            let text = format!("{indent}{l}");
            let trimmed = l.trim_start();
            if trimmed.starts_with("```") {
                in_code = !in_code;
                return Line::from(Span::raw(text).fg(DIM));
            }
            if in_code {
                return Line::from(text);
            }
            if trimmed.starts_with('#') {
                Line::from(Span::raw(text).bold())
            } else if trimmed.starts_with('>') {
                Line::from(Span::raw(text).fg(DIM))
            } else {
                Line::from(text)
            }
        })
        .collect()
}

/// 送信中, レビューの選択, 取り直し中, エラー, 操作の結果, キー一覧の順に優先する
fn footer_line(view: &PrView) -> Line<'static> {
    if view.submitting {
        return Line::from(Span::raw(" レビューを送信中…").yellow());
    }
    if view.saving_body {
        return Line::from(Span::raw(" 説明を更新中…").yellow());
    }
    if view.resolving {
        return Line::from(Span::raw(" スレッドを更新中…").yellow());
    }
    if view.choosing_review {
        return Line::from(
            Span::raw(
                " a 承認 (Approve)  c コメント (Comment)  他のキーで取りやめ. 本文はエディタで書く",
            )
            .yellow()
            .bold(),
        );
    }
    // 初回の取得中は本文の欄に出すので, フッターに出すのは取り直しのときだけ
    let refetching = match view.page {
        PrPage::Conversation => view.fetching && view.conversation.is_some(),
        PrPage::Diff => view.diff.fetching && view.diff.diff.is_some(),
    };
    if refetching {
        return Line::from(Span::raw(" 取得中…").yellow());
    }
    if let Some(e) = &view.error {
        return Line::from(Span::raw(format!(" {e}")).red());
    }
    if let Some(n) = &view.notice {
        return Line::from(Span::raw(format!(" {n}")).green());
    }
    let keys: &[(&str, &str)] = match view.page {
        PrPage::Conversation => &[
            ("j/k", "スクロール"),
            ("C-d/C-u", "半画面"),
            ("Space", "1 画面"),
            ("g/G", "先頭/末尾"),
            ("]/[", "スレッド"),
            ("x", "解決"),
            ("d", "diff"),
            ("!", "レビュー"),
            ("e", "説明を編集"),
            ("o", "ブラウザ"),
            ("r", "更新"),
            ("q", "戻る"),
        ],
        PrPage::Diff => &[
            ("j/k", "スクロール"),
            ("C-d/C-u", "半画面"),
            ("g/G", "先頭/末尾"),
            ("]/[", "次/前のファイル"),
            ("h/l", "横"),
            ("d", "説明とコメント"),
            ("!", "レビュー"),
            ("o", "ブラウザ"),
            ("r", "更新"),
            ("q", "戻る"),
        ],
    };
    let mut spans = vec![Span::raw(" ")];
    for &(key, label) in keys {
        spans.push(Span::raw(key).cyan());
        spans.push(Span::raw(format!(" {label}  ")).fg(DIM));
    }
    Line::from(spans)
}

#[cfg(test)]
mod tests {
    use chrono::DateTime;
    use ratatui::style::{Color, Modifier};

    use super::*;

    #[test]
    fn body_lines_styles_markdown() {
        let body = "# 概要\n> 引用\n```\n# コード\n```\n本文<!-- x -->";
        let lines = body_lines(body, "  ");
        let texts: Vec<String> = lines.iter().map(|l| l.to_string()).collect();
        assert_eq!(
            texts,
            vec![
                "  # 概要",
                "  > 引用",
                "  ```",
                "  # コード",
                "  ```",
                "  本文"
            ]
        );
        let style = |i: usize| lines[i].spans[0].style;
        assert!(style(0).add_modifier.contains(Modifier::BOLD));
        assert_eq!(style(1).fg, Some(DIM));
        assert_eq!(style(2).fg, Some(DIM));
        // コードブロックの中は見出しにしない
        assert!(!style(3).add_modifier.contains(Modifier::BOLD));
        assert_eq!(style(3).fg, None);
    }

    #[test]
    fn footer_priority() {
        let mut v = PrView::new(&crate::domain::pr::Pr::fixture(1, "b1", "main"));
        v.fetching = false;
        assert!(footer_line(&v).to_string().contains("! レビュー"));
        v.notice = Some("#1 を承認した".into());
        let line = footer_line(&v);
        assert_eq!(line.to_string(), " #1 を承認した");
        assert_eq!(line.spans[0].style.fg, Some(Color::Green));
        v.error = Some("取得失敗: x".into());
        let line = footer_line(&v);
        assert_eq!(line.to_string(), " 取得失敗: x");
        assert_eq!(line.spans[0].style.fg, Some(Color::Red));
        // 初回の取得中は本文の欄に出すので, フッターに出すのは取り直しのときだけ
        v.fetching = true;
        assert_eq!(footer_line(&v).to_string(), " 取得失敗: x");
        v.conversation = Some(Conversation::new(
            String::new(),
            DateTime::from_timestamp(0, 0).unwrap(),
            String::new(),
            vec![],
            0,
        ));
        assert_eq!(footer_line(&v).to_string(), " 取得中…");
        v.choosing_review = true;
        assert!(footer_line(&v).to_string().starts_with(" a 承認"));
        v.saving_body = true;
        assert_eq!(footer_line(&v).to_string(), " 説明を更新中…");
        v.submitting = true;
        assert_eq!(footer_line(&v).to_string(), " レビューを送信中…");
    }
}
