//! update が返した Effect を実行する. スレッドの起動と infra の呼び出しはここに閉じる.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread;
use std::time::Duration;

use chrono::Utc;
use ratatui::crossterm::event::KeyEvent;

use super::event::{Action, Effect, Event, WatchUntil};
use super::state::AppState;
use super::update;
use crate::infra::cache::{self, Cache};
use crate::infra::gh::{self, fetch_pr, fetch_prs, linear_review_url, update_branch};
use crate::infra::git::read_local_state;
use crate::infra::process::open_url;

pub const AUTO_REFRESH: Duration = Duration::from_secs(300);
/// 操作の反映を待って PR を取り直す間隔と回数. update-branch は GitHub 側で非同期に merge される
const WATCH_INTERVAL: Duration = Duration::from_secs(2);
const WATCH_TRIES: usize = 10;

pub struct App {
    pub state: AppState,
    tx: Sender<Event>,
    rx: Receiver<Event>,
}

impl App {
    pub fn new(dir: PathBuf, repo: String) -> Self {
        let (tx, rx) = channel();
        let cached = cache::load(&repo);
        let local = read_local_state(&dir).unwrap_or_default();
        let mut state = AppState::new(dir, repo, local);
        state.fetched_at = cached.as_ref().map(|c| c.fetched_at);
        state.from_cache = cached.is_some();
        state.prs = cached.map(|c| c.prs).unwrap_or_default();
        let mut app = Self { state, tx, rx };
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

    /// スレッドから届いた結果をすべて反映する
    pub fn pump(&mut self) {
        while let Ok(ev) = self.rx.try_recv() {
            let effects = update::handle_event(&mut self.state, ev);
            self.perform(effects);
        }
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
