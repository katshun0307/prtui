mod app;
mod domain;
mod dump;
mod infra;
mod ui;

use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{self, Event as TermEvent, KeyEventKind};

use app::{Action, App};
use infra::{gh, git};

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
    let repo = gh::repo_name(&dir)?;

    if dump {
        return dump::run(dir, &repo);
    }

    let terminal = ratatui::init();
    let result = run(terminal, App::new(dir, repo));
    ratatui::restore();
    result
}

fn run(mut terminal: DefaultTerminal, mut app: App) -> Result<()> {
    loop {
        terminal.draw(|f| ui::draw(f, &mut app.state))?;

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
                        .current_dir(&app.state.dir)
                        .status();
                    terminal = ratatui::init();
                    app.resumed(status);
                }
                Action::None => {}
            }
        }

        app.pump();
        app.tick();
    }
}
