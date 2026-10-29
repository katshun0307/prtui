//! update が返した Effect を実行する. スレッドの起動と infra の呼び出しはここに閉じる.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::ExitStatus;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow};
use chrono::Utc;
use ratatui::crossterm::event::KeyEvent;

use super::event::{Action, Effect, Event, WatchUntil};
use super::state::AppState;
use super::update;
use crate::domain::devin::DevinConfig;
use crate::infra::cache::{self, Cache};
use crate::infra::devin::{fetch_messages, fetch_sessions, send_message};
use crate::infra::gh::{self, fetch_pr, fetch_prs, linear_review_url, update_branch};
use crate::infra::git::read_local_state;
use crate::infra::process::open_url;

/// Devin session 一覧を取る期間
const SESSIONS_SINCE_DAYS: i64 = 30;
/// 操作の反映を待って PR を取り直す間隔と回数. update-branch は GitHub 側で非同期に merge される
const WATCH_INTERVAL: Duration = Duration::from_secs(2);
const WATCH_TRIES: usize = 10;

pub struct App {
    pub state: AppState,
    tx: Sender<Event>,
    rx: Receiver<Event>,
    /// `Effect::Compose` でエディタに渡した一時ファイル. 戻ったら読んで消す
    pending_compose: Option<PathBuf>,
}

impl App {
    pub fn new(dir: PathBuf, repo: String) -> Self {
        let (tx, rx) = channel();
        let cached = cache::load(&repo);
        let local = read_local_state(&dir).unwrap_or_default();
        let mut state = AppState::new(dir, repo, local);
        if let Some(c) = cached {
            state.fetched_at = Some(c.fetched_at);
            state.from_cache = true;
            state.prs = c.prs;
            state.devin_sessions = c.devin_sessions;
        }
        let mut app = Self {
            state,
            tx,
            rx,
            pending_compose: None,
        };
        app.refresh();
        app
    }

    /// 全件を取り直す. 自動更新用
    pub fn refresh(&mut self) {
        let effects = update::refresh(&mut self.state);
        self.perform(effects);
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> Action {
        let effects = update::handle_key(&mut self.state, key);
        self.perform(effects)
    }

    /// 時間で起きる遷移を進める. main のループから毎回呼ぶ
    pub fn tick(&mut self) {
        let effects = update::tick(&mut self.state, Instant::now());
        self.perform(effects);
    }

    /// `Action::Suspend` の外部コマンドが終わった後に main が呼ぶ.
    /// エディタから戻ったときは書いた内容を `Event::Composed` にする
    pub fn resumed(&mut self, status: io::Result<ExitStatus>) {
        let Some(path) = self.pending_compose.take() else {
            if let Err(e) = status {
                self.state.error(e.to_string());
            }
            return;
        };
        let res = match status {
            Ok(s) if s.success() => fs::read_to_string(&path)
                .map(Some)
                .map_err(|e| anyhow!("{}: {e}", path.display())),
            Ok(_) => Ok(None),
            Err(e) => Err(e.into()),
        };
        let _ = fs::remove_file(&path);
        self.handle_event(Event::Composed(res));
    }

    /// スレッドから届いた結果をすべて反映する
    pub fn pump(&mut self) {
        while let Ok(ev) = self.rx.try_recv() {
            self.handle_event(ev);
        }
    }

    fn handle_event(&mut self, ev: Event) {
        let effects = update::handle_event(&mut self.state, ev);
        self.perform(effects);
    }

    /// Effect を順に実行する. Quit と Suspend は main への Action にする
    fn perform(&mut self, effects: Vec<Effect>) -> Action {
        let mut action = Action::None;
        for effect in effects {
            match effect {
                Effect::Quit => action = Action::Quit,
                Effect::Suspend(cmd) => action = Action::Suspend(cmd),
                Effect::Fetch => {
                    self.spawn(|dir, repo| Event::Fetched(fetch_prs(dir, repo)));
                    self.reload_local();
                }
                Effect::ReloadLocal => self.reload_local(),
                Effect::SaveCache => self.save_cache(),
                Effect::Checkout(n) => self.checkout(n),
                Effect::UpdateBranch {
                    number,
                    base,
                    head_oid,
                } => self.spawn(move |dir, repo| {
                    let res = update_branch(dir, repo, number, &head_oid)
                        .map_err(|e| e.context(format!("{base} の取り込み失敗")));
                    Event::UpdatedBranch(number, res.map(|_| base))
                }),
                Effect::ChangeState {
                    number,
                    op,
                    targets,
                } => self.spawn(move |dir, _| {
                    Event::ChangedState(number, op, gh::change_state(dir, number, op, &targets))
                }),
                Effect::LinearReview(n) => self.spawn(move |dir, repo| {
                    Event::LinearReview(n, linear_review_url(dir, repo, n))
                }),
                Effect::Watch { number, until } => self.watch(number, until),
                Effect::OpenUrl(url) => {
                    if let Err(e) = open_url(&self.state.dir, &url) {
                        self.state.error(e.to_string());
                    }
                }
                Effect::ViewWeb(n) => {
                    if let Err(e) = gh::view_web(&self.state.dir, n) {
                        self.state.error(e.to_string());
                    }
                }
                Effect::FetchSessions => {
                    let cfg = self.state.local.devin.clone();
                    self.spawn(move |_, repo| {
                        let since = Utc::now() - chrono::Duration::days(SESSIONS_SINCE_DAYS);
                        let res = with_devin(cfg.as_ref(), |c| fetch_sessions(c, repo, since));
                        Event::DevinSessions(res)
                    });
                }
                Effect::FetchMessages(id) => {
                    let cfg = self.state.local.devin.clone();
                    self.spawn(move |_, _| {
                        let res = with_devin(cfg.as_ref(), |c| fetch_messages(c, &id));
                        Event::DevinMessages(id, res)
                    });
                }
                Effect::SendMessage { session_id, text } => {
                    let cfg = self.state.local.devin.clone();
                    self.spawn(move |_, _| {
                        let res = with_devin(cfg.as_ref(), |c| send_message(c, &session_id, &text));
                        Event::DevinSent(session_id, res)
                    });
                }
                Effect::Compose { draft } => {
                    let path =
                        std::env::temp_dir().join(format!("prtui-devin-{}.md", std::process::id()));
                    match fs::write(&path, draft) {
                        Ok(()) => {
                            action = Action::Suspend(vec![
                                "sh".into(),
                                "-c".into(),
                                "${VISUAL:-${EDITOR:-vi}} \"$1\"".into(),
                                "sh".into(),
                                path.display().to_string(),
                            ]);
                            self.pending_compose = Some(path);
                        }
                        Err(e) => self
                            .handle_event(Event::Composed(Err(anyhow!("{}: {e}", path.display())))),
                    }
                }
            }
        }
        action
    }

    /// `f` をスレッドで実行し, 結果を rx に送る
    fn spawn(&self, f: impl FnOnce(&Path, &str) -> Event + Send + 'static) {
        let (tx, dir, repo) = (
            self.tx.clone(),
            self.state.dir.clone(),
            self.state.repo.clone(),
        );
        thread::spawn(move || {
            let _ = tx.send(f(&dir, &repo));
        });
    }

    /// 操作が GitHub に反映されるまで PR を 1 件だけ取り直す. 全件の取得より軽く, 他の行は触らない.
    /// `until` を満たすか取得に失敗した時点で止め, 上限回数に達したらその時点の結果を出す.
    fn watch(&mut self, n: u64, until: WatchUntil) {
        self.state.syncing.insert(n);
        self.spawn(move |dir, repo| {
            let mut res = fetch_pr(dir, repo, n);
            for _ in 1..WATCH_TRIES {
                if res.as_ref().map_or(true, |p| until.is_done(p.as_ref())) {
                    break;
                }
                thread::sleep(WATCH_INTERVAL);
                res = fetch_pr(dir, repo, n);
            }
            Event::Reloaded(n, Box::new(res))
        });
    }

    fn reload_local(&mut self) {
        match read_local_state(&self.state.dir) {
            Ok(l) => self.state.local = l,
            Err(e) => self.state.error(e.to_string()),
        }
    }

    fn save_cache(&mut self) {
        let cache = Cache {
            fetched_at: self.state.fetched_at.unwrap_or_else(Utc::now),
            prs: self.state.prs.clone(),
            devin_sessions: self.state.devin_sessions.clone(),
        };
        if let Err(e) = cache::save(&self.state.repo, &cache) {
            self.state.error(format!("キャッシュの保存失敗: {e}"));
        }
    }

    /// 未コミットの変更 (untracked は除く) があれば実行しない
    fn checkout(&mut self, n: u64) {
        self.reload_local();
        if self.state.local.dirty {
            self.state.error(format!(
                "{} に未コミットの変更があるのでチェックアウトしない",
                self.state.dir.display()
            ));
            return;
        }
        self.state.busy = Some(format!("#{n} をチェックアウト中…"));
        self.spawn(move |dir, _| Event::CheckedOut(n, gh::checkout(dir, n)));
    }
}

/// Devin の config が無ければ API を呼ばずにエラーにする. 実行中に config を消されたときに備える
fn with_devin<T>(
    cfg: Option<&DevinConfig>,
    f: impl FnOnce(&DevinConfig) -> Result<T>,
) -> Result<T> {
    f(cfg.ok_or_else(|| anyhow!("Devin の設定が無い"))?)
}
