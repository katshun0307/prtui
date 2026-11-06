use ratatui::Frame;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Style, Stylize};
use ratatui::text::{Line, Span};

use crate::app::AppState;
use crate::domain::devin::{DevinSession, DevinStatus};
use crate::domain::label::Label;

mod chat;
mod copy;
mod detail;
pub mod format;
mod label;
mod table;

use chat::draw_chat;
use copy::draw_copy_menu;
use detail::detail_view;
use format::ago;
use label::draw_label_menu;
use table::draw_table;

/// 配色の意味
/// - green: 問題なし (Ready, CI 成功, 承認した reviewer)
/// - red: 壊れている (Conflict, CI 失敗, 変更要求した reviewer)
/// - yellow: 自分の対応が要る (未解決スレッド, reviewer 未設定, 確認プロンプト)
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
/// ラベル (詳細欄と選択画面) の名前の前の ● は GitHub で設定したラベルの色をそのまま 24bit 色で出す.
/// 上の配色の意味とは別で, GitHub の画面と見比べるためのもの
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

/// ラベルの色の ●. 色が分からなければ DIM の ○
pub(super) fn label_dot(l: &Label) -> Span<'static> {
    match l.rgb() {
        Some((r, g, b)) => Span::raw("● ").fg(Color::Rgb(r, g, b)),
        None => Span::raw("○ ").fg(DIM),
    }
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
    if state.label_menu.is_some() {
        draw_label_menu(f, state, f.area());
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
    if state.label_menu.is_some() {
        return Line::from(
            Span::raw(
                " 文字で絞り込み  ↑↓ 移動  Tab 付け外しを選ぶ  Enter 反映 (選んでいなければカーソルの項目を切り替え)  Esc 閉じる",
            )
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
        ("a", "ラベル"),
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
    use std::path::PathBuf;

    use chrono::{DateTime, Duration, Utc};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Cell;

    use super::*;
    use crate::app::LabelMenu;
    use crate::app::Tab;
    use crate::domain::copy::copy_items;
    use crate::domain::label::Label;
    use crate::domain::local::{LocalState, TabQuery};
    use crate::domain::op::PrOp;
    use crate::domain::pr::{Check, CheckState, MyReview, Pr, Reviewer, ReviewerState, Stack};

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

    /// `ago` は今の時刻から測るので, 単位の区切りから半端にずらして表示が動かないようにする
    fn days_ago(d: i64) -> DateTime<Utc> {
        Utc::now() - Duration::days(d) - Duration::hours(1)
    }

    fn hours_ago(h: i64) -> DateTime<Utc> {
        Utc::now() - Duration::hours(h) - Duration::minutes(20)
    }

    fn check(name: &str, state: CheckState) -> Check {
        Check {
            name: name.into(),
            state,
        }
    }

    fn reviewer(name: &str, state: ReviewerState) -> Reviewer {
        Reviewer {
            name: name.into(),
            state,
        }
    }

    fn pr(number: u64, head: &str, base: &str, title: &str) -> Pr {
        Pr {
            title: title.into(),
            url: format!("https://github.com/o/r/pull/{number}"),
            author: "katshun".into(),
            head_ref_oid: format!("oid-{number}"),
            updated_at: days_ago(2),
            mergeable: "MERGEABLE".into(),
            additions: 10 * number,
            deletions: number,
            changed_files: 3,
            behind_main: Some(0),
            ..Pr::fixture(number, head, base)
        }
    }

    /// feat-a は PR と同じ oid (最新) で checkout 中, feat-b は違う oid (差分あり)
    fn local() -> LocalState {
        LocalState {
            current_branch: "feat-a".into(),
            dirty: true,
            branches: [
                ("feat-a".to_string(), "oid-1".to_string()),
                ("feat-b".to_string(), "old".to_string()),
            ]
            .into(),
            reviewers: vec!["alice".into(), "bob".into()],
            tabs: vec![TabQuery {
                name: "マージ予定".into(),
                query: "label:merge".into(),
            }],
            ..Default::default()
        }
    }

    /// 自分タブ. stack の 2 段, Draft, Conflict, reviewer 未設定を並べる
    fn mine_prs() -> Vec<Pr> {
        let mut a = pr(1, "feat-a", "main", "ABC-1 土台を足す");
        a.stack = Some(Stack { pos: 1, len: 2 });
        a.checks = vec![
            check("build", CheckState::Success),
            check("lint", CheckState::Success),
            check("deploy", CheckState::Skipped),
        ];
        a.reviewers = vec![
            reviewer("alice", ReviewerState::Approved),
            reviewer("bob", ReviewerState::Commented),
        ];
        a.labels = vec![
            label("feature", "a2eeef", ""),
            label("good first issue", "7057ff", ""),
        ];
        a.updated_at = hours_ago(3);

        let mut b = pr(2, "feat-b", "feat-a", "土台の上に画面を足す");
        b.stack = Some(Stack { pos: 2, len: 2 });
        b.is_draft = true;
        b.checks = vec![
            check("build", CheckState::Success),
            check("test", CheckState::Pending),
        ];
        b.threads_open = 2;
        b.threads_total = 3;
        b.behind_main = None;

        let mut c = pr(3, "fix-c", "main", "壊れたテストを直す");
        c.mergeable = "CONFLICTING".into();
        c.checks = vec![
            check("build", CheckState::Failure),
            check("lint", CheckState::Success),
            check("test", CheckState::Failure),
        ];
        c.reviewers = vec![
            reviewer("carol", ReviewerState::ChangesRequested),
            reviewer("dave", ReviewerState::Requested),
        ];
        c.behind_main = Some(5);
        c.threads_total = 1;

        let mut d = pr(4, "chore-d", "main", "依存を上げる");
        d.assignees = vec!["katshun".into(), "erin".into()];
        d.updated_at = days_ago(10);
        vec![a, b, c, d]
    }

    fn my_review(requested: bool, last: Option<ReviewerState>) -> Option<MyReview> {
        Some(MyReview {
            requested,
            last,
            via_teams: vec![],
            requested_at: None,
        })
    }

    /// レビュータブ. 依頼中, チーム経由の再依頼, レビュー済みの各状態を並べる
    fn review_prs() -> Vec<Pr> {
        let mut a = pr(11, "feat-x", "main", "依頼された PR");
        a.author = "alice".into();
        a.my_review = my_review(true, None);
        a.my_review.as_mut().unwrap().requested_at = Some(days_ago(3));
        a.reviewers = vec![reviewer("katshun", ReviewerState::Requested)];

        let mut b = pr(12, "feat-y", "main", "チーム経由で再依頼された PR");
        b.author = "bob".into();
        b.my_review = my_review(true, Some(ReviewerState::Commented));
        let m = b.my_review.as_mut().unwrap();
        m.requested_at = Some(hours_ago(5));
        m.via_teams = vec!["backend".into()];
        b.reviewers = vec![
            reviewer("backend", ReviewerState::Requested),
            reviewer("katshun", ReviewerState::Commented),
        ];

        let mut c = pr(13, "feat-a", "main", "承認した PR");
        c.author = "carol".into();
        c.my_review = my_review(false, Some(ReviewerState::Approved));
        c.reviewers = vec![reviewer("katshun", ReviewerState::Approved)];

        let mut d = pr(14, "feat-z", "main", "変更を求めた PR");
        d.author = "dave".into();
        d.my_review = my_review(false, Some(ReviewerState::ChangesRequested));

        let mut e = pr(15, "feat-w", "main", "コメントした PR");
        e.author = "erin".into();
        e.my_review = my_review(false, Some(ReviewerState::Commented));

        let mut f = pr(16, "feat-v", "main", "dismiss された PR");
        f.author = "frank".into();
        f.my_review = my_review(false, None);
        vec![a, b, c, d, e, f]
    }

    /// 追加のタブ. assignee が 0 人, 1 人, 3 人の PR を並べる
    fn custom_prs() -> Vec<Pr> {
        let mut a = pr(21, "feat-a", "main", "誰にも割り当てていない PR");
        a.author = "alice".into();
        a.reviewers = vec![reviewer("bob", ReviewerState::Approved)];
        let mut b = pr(22, "feat-q", "main", "1 人に割り当てた PR");
        b.assignees = vec!["bob".into()];
        b.behind_main = Some(12);
        let mut c = pr(23, "feat-r", "main", "3 人に割り当てた PR");
        c.author = "carol".into();
        c.assignees = vec!["carol".into(), "dave".into(), "erin".into()];
        vec![a, b, c]
    }

    /// #1 に running, #2 に exit と suspended の 2 件, #12 に archived の error
    fn sessions() -> Vec<DevinSession> {
        let running = DevinSession::fixture("devin-1", "https://github.com/o/r/pull/1");
        let mut old = DevinSession::fixture("devin-2", "https://github.com/o/r/pull/2");
        old.status = DevinStatus::Exit;
        let mut new = DevinSession::fixture("devin-3", "https://github.com/o/r/pull/2");
        new.status = DevinStatus::Suspended;
        new.updated_at = days_ago(1);
        let mut archived = DevinSession::fixture("devin-4", "https://github.com/o/r/pull/12");
        archived.status = DevinStatus::Error;
        archived.is_archived = true;
        vec![running, old, new, archived]
    }

    /// 全タブを取得済みにした状態. 自分タブはキャッシュから読んだまま
    fn state() -> AppState {
        let fetched_at = Utc::now() - Duration::minutes(5) - Duration::seconds(20);
        let mut s = AppState::new(PathBuf::from("/src/r"), "o/r".into(), local());
        s.mine.replace(mine_prs());
        s.mine.fetched_at = Some(fetched_at);
        s.mine.from_cache = true;
        s.review.replace(review_prs());
        s.review.fetched_at = Some(fetched_at);
        s.custom[0].list.replace(custom_prs());
        s.devin_sessions = sessions();
        s
    }

    fn terminal(state: &mut AppState) -> Terminal<TestBackend> {
        let mut term = Terminal::new(TestBackend::new(160, 30)).unwrap();
        term.draw(|f| draw(f, state)).unwrap();
        term
    }

    /// 1 行のセルを左から (x, セル) で返す. 全角文字が覆う右隣のセルは飛ばす
    fn cells(term: &Terminal<TestBackend>, y: u16) -> Vec<(u16, &Cell)> {
        let buf = term.backend().buffer();
        let mut out = Vec::new();
        let mut skip = 0;
        for x in 0..buf.area.width {
            let cell = &buf[(x, y)];
            if skip == 0 {
                out.push((x, cell));
            }
            skip = std::cmp::max(skip, Span::raw(cell.symbol()).width()).saturating_sub(1);
        }
        out
    }

    /// 画面を行ごとの文字列にする
    fn screen(term: &Terminal<TestBackend>) -> String {
        let mut out = String::new();
        for y in 0..term.backend().buffer().area.height {
            let line: String = cells(term, y).iter().map(|(_, c)| c.symbol()).collect();
            out.push_str(line.trim_end());
            out.push('\n');
        }
        out
    }

    /// 同じ style が続くセルを 1 つにまとめ, 既定でない style の範囲を `y x "文字" style` で並べる
    fn styles(term: &Terminal<TestBackend>) -> String {
        let mut out = String::new();
        for y in 0..term.backend().buffer().area.height {
            let mut runs: Vec<(u16, Style, String)> = Vec::new();
            for (x, c) in cells(term, y) {
                match runs.last_mut() {
                    Some((_, style, text)) if *style == c.style() => text.push_str(c.symbol()),
                    _ => runs.push((x, c.style(), c.symbol().to_string())),
                }
            }
            for (x, style, text) in runs {
                if style.fg == Some(Color::Reset)
                    && style.bg == Some(Color::Reset)
                    && style.add_modifier.is_empty()
                {
                    continue;
                }
                let mut attrs = Vec::new();
                if style.fg != Some(Color::Reset) {
                    attrs.push(format!("fg={:?}", style.fg.unwrap()));
                }
                if style.bg != Some(Color::Reset) {
                    attrs.push(format!("bg={:?}", style.bg.unwrap()));
                }
                if !style.add_modifier.is_empty() {
                    attrs.push(format!("{:?}", style.add_modifier));
                }
                out.push_str(&format!("{y:>2} {x:>3} {text:?} {}\n", attrs.join(" ")));
            }
        }
        out
    }

    fn footer(state: &mut AppState) -> String {
        screen(&terminal(state)).lines().last().unwrap().to_string()
    }

    #[test]
    fn mine_tab() {
        let mut s = state();
        s.syncing.insert(2);
        s.mine.table.select(Some(2));
        let term = terminal(&mut s);
        insta::assert_snapshot!(screen(&term));
        insta::assert_snapshot!("mine_tab_styles", styles(&term));
    }

    #[test]
    fn mine_tab_draft_only() {
        let mut s = state();
        s.mine.draft_only = true;
        s.mine.from_cache = false;
        s.local.dirty = false;
        insta::assert_snapshot!(screen(&terminal(&mut s)));
    }

    #[test]
    fn review_tab() {
        let mut s = state();
        s.tab = Tab::Review;
        s.review.table.select(Some(1));
        let term = terminal(&mut s);
        insta::assert_snapshot!(screen(&term));
        insta::assert_snapshot!("review_tab_styles", styles(&term));
    }

    #[test]
    fn custom_tab() {
        let mut s = state();
        s.tab = Tab::Custom(0);
        s.custom[0].list.table.select(Some(2));
        s.custom[0].list.error = Some("取得失敗: HTTP 502".into());
        let term = terminal(&mut s);
        insta::assert_snapshot!(screen(&term));
        insta::assert_snapshot!("custom_tab_styles", styles(&term));
    }

    #[test]
    fn empty_and_loading() {
        let mut s = state();
        s.tab = Tab::Custom(0);
        s.custom[0].list.replace(vec![]);
        s.custom[0].list.loading = true;
        insta::assert_snapshot!(screen(&terminal(&mut s)));
    }

    /// フッターは優先度の低いものから順に足して, 上書きされることも見る
    #[test]
    fn footer_messages() {
        let mut s = state();
        let mut lines = vec![footer(&mut s)];
        s.tab = Tab::Review;
        lines.push(footer(&mut s));
        s.tab = Tab::Mine;
        s.info("#1 をチェックアウトした".into());
        lines.push(footer(&mut s));
        s.error("チェックアウト失敗: 未コミットの変更がある".into());
        lines.push(footer(&mut s));
        s.label_menu = Some(LabelMenu::new(1, vec![]));
        lines.push(footer(&mut s));
        s.copy_menu = Some(vec![]);
        lines.push(footer(&mut s));
        s.confirm_op = Some((3, PrOp::Close));
        lines.push(footer(&mut s));
        s.confirm_op = Some((1, PrOp::Review));
        lines.push(footer(&mut s));
        s.confirm_force_checkout = Some((2, "feat-b".into()));
        lines.push(footer(&mut s));
        s.confirm_update = Some((1, "main".into()));
        lines.push(footer(&mut s));
        s.busy = Some("#1 に main を取り込み中…".into());
        lines.push(footer(&mut s));
        insta::assert_snapshot!(lines.join("\n"));
    }

    fn label(name: &str, color: &str, description: &str) -> Label {
        Label {
            name: name.into(),
            color: color.into(),
            description: description.into(),
        }
    }

    /// #1 は feature が付いている. feature を外して docs を足すところ
    #[test]
    fn label_menu() {
        let mut s = state();
        s.repo_labels = vec![
            label("feature", "a2eeef", "新機能"),
            label("bug", "d73a4a", "壊れている"),
            label("docs", "0075ca", ""),
            label("good first issue", "7057ff", "最初の 1 件に"),
        ];
        let mut menu = LabelMenu::new(1, vec!["feature".into()]);
        menu.chosen = vec!["docs".into()];
        menu.list.select(Some(2));
        s.label_menu = Some(menu);
        let term = terminal(&mut s);
        insta::assert_snapshot!(screen(&term));
        let styles = styles(&term);
        // ポップアップは 12 行目から下, 41 桁目から右. 一覧の行の style は他の snapshot で見ている
        let at = |l: &str, r: std::ops::Range<usize>| l[r].trim().parse::<u16>().unwrap_or(0);
        let popup: Vec<&str> = styles
            .lines()
            .filter(|l| at(l, 0..2) >= 12 && at(l, 3..6) >= 41)
            .collect();
        insta::assert_snapshot!("label_menu_styles", popup.join("\n"));
    }

    /// 詳細欄のラベルの前にラベルの色の ● を出す. 色の無いラベルは DIM の ○
    #[test]
    fn detail_label_dots() {
        let mut s = state();
        s.mine.prs[0].labels.push(label("only-on-pr", "", ""));
        let styles = styles(&terminal(&mut s));
        let dots: Vec<&str> = styles
            .lines()
            .filter(|l| l.starts_with("22 ") && (l.contains('●') || l.contains('○')))
            .collect();
        insta::assert_snapshot!(dots.join("\n"), @r#"
        22  48 "● " fg=Rgb(162, 238, 239)
        22  59 "● " fg=Rgb(112, 87, 255)
        22  79 "○ " fg=Indexed(245)
        "#);
    }

    #[test]
    fn label_menu_loading_and_error() {
        let mut s = state();
        s.labels_loading = true;
        let mut menu = LabelMenu::new(1, vec![]);
        menu.query = "zz".into();
        menu.error = Some("ラベル一覧の取得失敗: HTTP 502".into());
        s.label_menu = Some(menu);
        insta::assert_snapshot!(screen(&terminal(&mut s)));
    }

    #[test]
    fn copy_menu() {
        let mut s = state();
        s.copy_menu = Some(copy_items(s.selected().unwrap(), Some("acme")));
        insta::assert_snapshot!(screen(&terminal(&mut s)));
    }
}
