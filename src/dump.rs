use std::path::PathBuf;

use anyhow::Result;
use chrono::{Duration, Utc};

use crate::domain::devin::session_for;
use crate::domain::pr::CheckState;
use crate::infra::{devin, gh, git};
use crate::ui;

pub fn run(dir: PathBuf, repo: &str) -> Result<()> {
    let local = git::read_local_state(&dir)?;
    println!(
        "{repo} @ {} (branch: {})",
        dir.display(),
        local.current_branch
    );
    // TUI と同じく 30 日以内に更新された session を見る. 取れなくても PR は出す
    let sessions = match &local.devin {
        Some(cfg) => devin::fetch_sessions(cfg, repo, Utc::now() - Duration::days(30))
            .unwrap_or_else(|e| {
                eprintln!("Devin session の取得失敗: {e}");
                Vec::new()
            }),
        None => Vec::new(),
    };
    for p in gh::fetch_prs(&dir, repo)? {
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
        let mark = if local.is_current(&p) { "*" } else { "" };
        let stack = p
            .stack
            .map_or("-".into(), |s| format!("{}/{}", s.pos, s.len));
        let reviewers = p
            .reviewers
            .iter()
            .map(|r| format!("{}:{:?}", r.name, r.state))
            .collect::<Vec<_>>()
            .join(",");
        let devin = session_for(&sessions, &p.url)
            .map_or("-".into(), |(s, n)| ui::format::devin_label(s, n));
        println!(
            "#{}\tstack={stack}\tbase={}\t{state}\t{ci}\tthreads={}/{}\tbehind_main={}\tsize=+{}/-{}\treview={}\treviewers={reviewers}\tlocal={mark}{}\tdevin={devin}\t{}\t{}",
            p.number,
            p.base_ref_name,
            p.threads_open,
            p.threads_total,
            p.behind_main.map_or("-".into(), |b| b.to_string()),
            p.additions,
            p.deletions,
            p.review_decision.as_deref().unwrap_or("-"),
            local.sync_label(&p).unwrap_or("-"),
            ui::format::ago(p.updated_at),
            p.title,
        );
    }
    // レビュータブ. 失敗しても自分の PR は出したので続けない
    println!("# review");
    for p in gh::fetch_reviews(&dir, repo)? {
        let mine = p.my_review.as_ref();
        let teams = mine.map_or(String::new(), |m| m.via_teams.join(","));
        let devin = session_for(&sessions, &p.url)
            .map_or("-".into(), |(s, n)| ui::format::devin_label(s, n));
        println!(
            "#{}\tauthor={}\tme={}\trequested={}\tvia={}\tdraft={}\tthreads={}/{}\tlocal={}\tdevin={devin}\t{}\t{}",
            p.number,
            p.author,
            mine.map_or("-", |m| m.label()),
            mine.and_then(|m| m.requested_at)
                .map_or("-".into(), ui::format::ago),
            if teams.is_empty() { "-" } else { &teams },
            p.is_draft,
            p.threads_open,
            p.threads_total,
            local.sync_label(&p).unwrap_or("-"),
            ui::format::ago(p.updated_at),
            p.title,
        );
    }
    Ok(())
}
