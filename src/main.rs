mod app;
mod cache;
mod git;
mod github;
mod ui;

use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{self, Event as TermEvent, KeyEventKind};

use app::{AUTO_REFRESH, Action, App};
use git::LocalState;
use github::CheckState;

const USAGE: &str = "usage: prtui [-C <dir>] [--dump]";

fn main() -> Result<()> {
    let mut dir = std::env::current_dir()?;
    let mut dump = false;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "-C" => dir = PathBuf::from(args.next().context(USAGE)?),
            "--dump" => dump = true,
            "-h" | "--help" => {
                println!("{USAGE}");
                return Ok(());
            }
            _ => bail!("{USAGE}"),
        }
    }

    let dir = git::main_worktree(&dir)?;
    let repo = github::repo_name(&dir)?;

    if dump {
        return dump_prs(dir, &repo);
    }

    let terminal = ratatui::init();
    let result = run(terminal, App::new(dir, repo));
    ratatui::restore();
    result
}

fn run(mut terminal: DefaultTerminal, mut app: App) -> Result<()> {
    loop {
        terminal.draw(|f| ui::draw(f, &mut app))?;

        if event::poll(Duration::from_millis(200))?
            && let TermEvent::Key(key) = event::read()?
            && key.kind == KeyEventKind::Press
        {
            match app.handle_key(key) {
                Action::Quit => return Ok(()),
                Action::Suspend(cmd) => {
                    ratatui::restore();
                    let status = Command::new(&cmd[0])
                        .args(&cmd[1..])
                        .current_dir(&app.dir)
                        .status();
                    terminal = ratatui::init();
                    if let Err(e) = status {
                        app.error(e.to_string());
                    }
                }
                Action::None => {}
            }
        }

        while let Ok(ev) = app.rx.try_recv() {
            app.handle_event(ev);
        }
        if app.last_fetch.is_some_and(|t| t.elapsed() >= AUTO_REFRESH) {
            app.refresh();
        }
    }
}

fn dump_prs(dir: PathBuf, repo: &str) -> Result<()> {
    let local = LocalState::read(&dir)?;
    println!(
        "{repo} @ {} (branch: {})",
        dir.display(),
        local.current_branch
    );
    for p in github::fetch_prs(&dir, repo)? {
        let state = if p.mergeable == "CONFLICTING" {
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
            ui::ago(p.updated_at),
            p.title,
        );
    }
    Ok(())
}
