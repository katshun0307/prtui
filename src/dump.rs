use std::path::PathBuf;

use anyhow::Result;

use crate::domain::pr::CheckState;
use crate::infra::{gh, git};
use crate::ui;

pub fn run(dir: PathBuf, repo: &str) -> Result<()> {
    let local = git::read_local_state(&dir)?;
    println!(
        "{repo} @ {} (branch: {})",
        dir.display(),
        local.current_branch
    );
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
        println!(
            "#{}\tstack={stack}\tbase={}\t{state}\t{ci}\tthreads={}/{}\tbehind_main={}\tsize=+{}/-{}\treview={}\treviewers={reviewers}\tlocal={mark}{}\t{}\t{}",
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
    Ok(())
}
