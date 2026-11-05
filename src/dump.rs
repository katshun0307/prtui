use std::path::{Path, PathBuf};

use anyhow::Result;
use chrono::{Duration, Utc};

use crate::domain::devin::{DevinSession, session_for};
use crate::domain::local::LocalState;
use crate::domain::pr::{CheckState, Pr};
use crate::infra::{devin, gh, git};
use crate::ui;

/// 取得したものから順に出す. 見出しは取得の前に出し, 失敗したところまでの出力を残す
pub fn run(dir: PathBuf, repo: &str) -> Result<()> {
    let local = git::read_local_state(&dir)?;
    print!("{}", header(repo, &dir, &local));
    // TUI と同じく 30 日以内に更新された session を見る. 取れなくても PR は出す
    let sessions = match &local.devin {
        Some(cfg) => devin::fetch_sessions(cfg, repo, Utc::now() - Duration::days(30))
            .unwrap_or_else(|e| {
                eprintln!("Devin session の取得失敗: {e}");
                Vec::new()
            }),
        None => Vec::new(),
    };
    print!("{}", mine(&gh::fetch_prs(&dir, repo)?, &local, &sessions));
    // レビュータブ. 失敗しても自分の PR は出したので続けない
    println!("# review");
    print!(
        "{}",
        review(&gh::fetch_reviews(&dir, repo)?, &local, &sessions)
    );
    // 追加のタブ. 失敗したタブはエラーを出して次のタブに進む
    for t in &local.tabs {
        println!("# {}: {}", t.name, t.query);
        match gh::fetch_custom(&dir, repo, &t.query) {
            Ok(prs) => print!("{}", custom(&prs, &local)),
            Err(e) => eprintln!("{} の取得失敗: {e}", t.name),
        }
    }
    Ok(())
}

fn header(repo: &str, dir: &Path, local: &LocalState) -> String {
    format!(
        "{repo} @ {} (branch: {})\n",
        dir.display(),
        local.current_branch
    )
}

/// 自分タブの 1 行 1 PR
fn mine(prs: &[Pr], local: &LocalState, sessions: &[DevinSession]) -> String {
    let mut out = String::new();
    for p in prs {
        let state = if p.is_conflicting() {
            "Conflict"
        } else if p.is_draft {
            "Draft"
        } else {
            "Ready"
        };
        let ci = format!(
            "ok={} fail={} pending={}",
            p.count_checks(CheckState::Success),
            p.count_checks(CheckState::Failure),
            p.count_checks(CheckState::Pending),
        );
        let mark = if local.is_current(p) { "*" } else { "" };
        let stack = p
            .stack
            .map_or("-".into(), |s| format!("{}/{}", s.pos, s.len));
        out.push_str(&format!(
            "#{}\tstack={stack}\tbase={}\t{state}\t{ci}\tthreads={}/{}\tbehind_main={}\tsize=+{}/-{}\treview={}\treviewers={}\tlocal={mark}{}\tdevin={}\t{}\t{}\n",
            p.number,
            p.base_ref_name,
            p.threads_open,
            p.threads_total,
            p.behind_main.map_or("-".into(), |b| b.to_string()),
            p.additions,
            p.deletions,
            p.review_decision.as_deref().unwrap_or("-"),
            reviewers(p),
            local.sync_label(p).unwrap_or("-"),
            devin_label(sessions, p),
            ui::format::ago(p.updated_at),
            p.title,
        ));
    }
    out
}

/// レビュータブの 1 行 1 PR. 見出しの `# review` は取得の前に run が出す
fn review(prs: &[Pr], local: &LocalState, sessions: &[DevinSession]) -> String {
    let mut out = String::new();
    for p in prs {
        let mine = p.my_review.as_ref();
        let teams = mine.map_or(String::new(), |m| m.via_teams.join(","));
        out.push_str(&format!(
            "#{}\tauthor={}\tme={}\trequested={}\tvia={}\tdraft={}\tthreads={}/{}\tlocal={}\tdevin={}\t{}\t{}\n",
            p.number,
            p.author,
            mine.map_or("-", |m| m.label()),
            mine.and_then(|m| m.requested_at)
                .map_or("-".into(), ui::format::ago),
            if teams.is_empty() { "-" } else { &teams },
            p.is_draft,
            p.threads_open,
            p.threads_total,
            local.sync_label(p).unwrap_or("-"),
            devin_label(sessions, p),
            ui::format::ago(p.updated_at),
            p.title,
        ));
    }
    out
}

/// 追加のタブ 1 つ分の 1 行 1 PR. 見出しの `# 名前: クエリ` は取得の前に run が出す
fn custom(prs: &[Pr], local: &LocalState) -> String {
    let mut out = String::new();
    for p in prs {
        out.push_str(&format!(
            "#{}\tauthor={}\tassignees={}\tdraft={}\tbehind_main={}\treview={}\treviewers={}\tlocal={}\t{}\t{}\n",
            p.number,
            p.author,
            p.assignees.join(","),
            p.is_draft,
            p.behind_main.map_or("-".into(), |b| b.to_string()),
            p.review_decision.as_deref().unwrap_or("-"),
            reviewers(p),
            local.sync_label(p).unwrap_or("-"),
            ui::format::ago(p.updated_at),
            p.title,
        ));
    }
    out
}

/// `login:状態` をカンマでつなぐ
fn reviewers(p: &Pr) -> String {
    p.reviewers
        .iter()
        .map(|r| format!("{}:{:?}", r.name, r.state))
        .collect::<Vec<_>>()
        .join(",")
}

/// PR に紐づく最新の session の status. 無ければ `-`
fn devin_label(sessions: &[DevinSession], p: &Pr) -> String {
    session_for(sessions, &p.url).map_or("-".into(), |(s, n)| ui::format::devin_label(s, n))
}

#[cfg(test)]
mod tests {
    use chrono::{DateTime, Duration, Utc};

    use super::*;
    use crate::domain::devin::DevinStatus;
    use crate::domain::pr::{Check, MyReview, Reviewer, ReviewerState, Stack};

    /// `ago` は今の時刻から測るので, 単位の区切りから半端にずらして表示が動かないようにする
    fn days_ago(d: i64) -> DateTime<Utc> {
        Utc::now() - Duration::days(d) - Duration::hours(1)
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
            behind_main: Some(0),
            ..Pr::fixture(number, head, base)
        }
    }

    /// feat-a は PR と同じ oid (最新) で checkout 中, feat-b は違う oid (差分あり)
    fn local() -> LocalState {
        LocalState {
            current_branch: "feat-a".into(),
            branches: [
                ("feat-a".to_string(), "oid-1".to_string()),
                ("feat-b".to_string(), "old".to_string()),
            ]
            .into(),
            ..Default::default()
        }
    }

    fn sessions() -> Vec<DevinSession> {
        let running = DevinSession::fixture("devin-1", "https://github.com/o/r/pull/1");
        let mut old = DevinSession::fixture("devin-2", "https://github.com/o/r/pull/2");
        old.status = DevinStatus::Exit;
        let mut new = DevinSession::fixture("devin-3", "https://github.com/o/r/pull/2");
        new.status = DevinStatus::Suspended;
        new.updated_at = days_ago(1);
        vec![running, old, new]
    }

    #[test]
    fn header_line() {
        assert_eq!(
            header("o/r", Path::new("/src/r"), &local()),
            "o/r @ /src/r (branch: feat-a)\n"
        );
    }

    #[test]
    fn mine_lines() {
        let check = |name: &str, state| Check {
            name: name.into(),
            state,
        };
        let mut a = pr(1, "feat-a", "main", "ABC-1 土台を足す");
        a.stack = Some(Stack { pos: 1, len: 2 });
        a.checks = vec![
            check("build", CheckState::Success),
            check("deploy", CheckState::Skipped),
        ];
        a.reviewers = vec![
            reviewer("alice", ReviewerState::Approved),
            reviewer("bob", ReviewerState::Commented),
        ];
        a.review_decision = Some("APPROVED".into());
        let mut b = pr(2, "feat-b", "feat-a", "土台の上に画面を足す");
        b.stack = Some(Stack { pos: 2, len: 2 });
        b.is_draft = true;
        b.checks = vec![check("test", CheckState::Pending)];
        b.threads_open = 2;
        b.threads_total = 3;
        b.behind_main = None;
        let mut c = pr(3, "fix-c", "main", "壊れたテストを直す");
        c.mergeable = "CONFLICTING".into();
        c.checks = vec![check("build", CheckState::Failure)];
        c.reviewers = vec![reviewer("carol", ReviewerState::ChangesRequested)];
        c.behind_main = Some(5);
        c.updated_at = days_ago(10);
        insta::assert_snapshot!(mine(&[a, b, c], &local(), &sessions()));
    }

    #[test]
    fn review_lines() {
        let my_review = |requested, last| {
            Some(MyReview {
                requested,
                last,
                via_teams: vec![],
                requested_at: None,
            })
        };
        let mut a = pr(11, "feat-x", "main", "依頼された PR");
        a.author = "alice".into();
        a.my_review = my_review(true, None);
        a.my_review.as_mut().unwrap().requested_at = Some(days_ago(3));
        let mut b = pr(12, "feat-b", "main", "チーム経由で再依頼された PR");
        b.author = "bob".into();
        b.is_draft = true;
        b.my_review = my_review(true, Some(ReviewerState::Commented));
        b.my_review.as_mut().unwrap().via_teams = vec!["backend".into(), "infra".into()];
        let mut c = pr(13, "feat-z", "main", "承認した PR");
        c.author = "carol".into();
        c.my_review = my_review(false, Some(ReviewerState::Approved));
        c.threads_total = 4;
        let mut d = pr(1, "feat-a", "main", "my_review の無い PR");
        d.author = "dave".into();
        insta::assert_snapshot!(review(&[a, b, c, d], &local(), &sessions()));
    }

    #[test]
    fn custom_lines() {
        let mut a = pr(21, "feat-a", "main", "誰にも割り当てていない PR");
        a.author = "alice".into();
        a.reviewers = vec![reviewer("bob", ReviewerState::Requested)];
        let mut b = pr(22, "feat-q", "main", "2 人に割り当てた PR");
        b.assignees = vec!["bob".into(), "carol".into()];
        b.behind_main = Some(12);
        b.is_draft = true;
        b.review_decision = Some("REVIEW_REQUIRED".into());
        insta::assert_snapshot!(custom(&[a, b], &local()));
        assert_eq!(custom(&[], &local()), "");
    }
}
