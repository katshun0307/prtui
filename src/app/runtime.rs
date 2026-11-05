//! update が返した Effect を実行する. スレッドの起動と外部の呼び出し (`Ports` 越し) はここに閉じる.

use std::fs;
use std::io;
use std::path::PathBuf;
use std::process::ExitStatus;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread;
use std::time::Instant;

use anyhow::{Result, anyhow};
use chrono::Utc;
use ratatui::crossterm::event::KeyEvent;

use super::adapters::real_ports;
use super::event::{Action, Effect, Event, WatchUntil};
use super::ports::{Ports, RuntimeConfig};
use super::state::AppState;
use super::update;
use crate::domain::cache::{Cache, CustomCache};
use crate::domain::devin::DevinConfig;

pub struct App {
    pub state: AppState,
    tx: Sender<Event>,
    rx: Receiver<Event>,
    ports: Ports,
    cfg: RuntimeConfig,
    /// `Effect::Compose` でエディタに渡した一時ファイル. 戻ったら読んで消す
    pending_compose: Option<PathBuf>,
}

impl App {
    /// 実際の gh, git, Devin API, キャッシュファイルに繋いで, 全件の取得を始める
    pub fn new(dir: PathBuf, repo: String) -> Self {
        let ports = real_ports(dir.clone(), repo.clone());
        let mut app = Self::with_ports(dir, repo, ports, RuntimeConfig::default());
        app.refresh();
        app
    }

    /// キャッシュと LocalState を ports から読んで状態を作る. 取得はまだ始めない
    pub fn with_ports(dir: PathBuf, repo: String, ports: Ports, cfg: RuntimeConfig) -> Self {
        let (tx, rx) = channel();
        let cached = ports.cache.load();
        let local = ports.git.read_local_state().unwrap_or_default();
        let mut state = AppState::new(dir, repo, local);
        if let Some(c) = cached {
            state.mine.fetched_at = Some(c.fetched_at);
            state.mine.from_cache = true;
            state.mine.prs = c.prs;
            if let Some((at, prs)) = c.reviews {
                state.review.fetched_at = Some(at);
                state.review.from_cache = true;
                state.review.prs = prs;
            }
            for t in &mut state.custom {
                if let Some(c) = c.custom.iter().find(|c| c.query == t.query) {
                    t.list.fetched_at = Some(c.fetched_at);
                    t.list.from_cache = true;
                    t.list.prs = c.prs.clone();
                }
            }
            state.devin_sessions = c.devin_sessions;
        }
        Self {
            state,
            tx,
            rx,
            ports,
            cfg,
            pending_compose: None,
        }
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
                    self.spawn(|p| Event::Fetched(p.gh.fetch_prs()));
                    self.reload_local();
                }
                Effect::FetchReviews => self.spawn(|p| Event::FetchedReviews(p.gh.fetch_reviews())),
                Effect::FetchCustom(i) => {
                    let query = self.state.custom[i].query.clone();
                    self.spawn(move |p| Event::FetchedCustom(i, p.gh.fetch_custom(&query)))
                }
                Effect::ReloadLocal => self.reload_local(),
                Effect::SaveCache => self.save_cache(),
                Effect::Checkout { number, force } => self.checkout(number, force),
                Effect::UpdateBranch {
                    number,
                    base,
                    head_oid,
                } => self.spawn(move |p| {
                    let res =
                        p.gh.update_branch(number, &head_oid)
                            .map_err(|e| e.context(format!("{base} の取り込み失敗")));
                    Event::UpdatedBranch(number, res.map(|_| base))
                }),
                Effect::ChangeState {
                    number,
                    op,
                    targets,
                } => self.spawn(move |p| {
                    Event::ChangedState(number, op, p.gh.change_state(number, op, &targets))
                }),
                Effect::LinearReview(n) => {
                    self.spawn(move |p| Event::LinearReview(n, p.gh.linear_review_url(n)))
                }
                Effect::Watch { number, until } => self.watch(number, until),
                Effect::OpenUrl(url) => {
                    if let Err(e) = self.ports.desktop.open_url(&url) {
                        self.state.error(e.to_string());
                    }
                }
                Effect::Copy(text) => match self.ports.desktop.copy(&text) {
                    Ok(()) => self.state.info(format!("コピーした: {text}")),
                    Err(e) => self.state.error(format!("コピー失敗: {e}")),
                },
                Effect::ViewWeb(n) => {
                    if let Err(e) = self.ports.gh.view_web(n) {
                        self.state.error(e.to_string());
                    }
                }
                Effect::FetchSessions => {
                    let cfg = self.state.local.devin.clone();
                    let days = self.cfg.sessions_since_days;
                    self.spawn(move |p| {
                        let since = Utc::now() - chrono::Duration::days(days);
                        let res = with_devin(cfg.as_ref(), |c| p.devin.fetch_sessions(c, since));
                        Event::DevinSessions(res)
                    });
                }
                Effect::FetchMessages(id) => {
                    let cfg = self.state.local.devin.clone();
                    self.spawn(move |p| {
                        let res = with_devin(cfg.as_ref(), |c| p.devin.fetch_messages(c, &id));
                        Event::DevinMessages(id, res)
                    });
                }
                Effect::SendMessage { session_id, text } => {
                    let cfg = self.state.local.devin.clone();
                    self.spawn(move |p| {
                        let res = with_devin(cfg.as_ref(), |c| {
                            p.devin.send_message(c, &session_id, &text)
                        });
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
    fn spawn(&self, f: impl FnOnce(&Ports) -> Event + Send + 'static) {
        let (tx, ports) = (self.tx.clone(), self.ports.clone());
        thread::spawn(move || {
            let _ = tx.send(f(&ports));
        });
    }

    /// 操作が GitHub に反映されるまで PR を 1 件だけ取り直す. 全件の取得より軽く, 他の行は触らない.
    /// `until` を満たすか取得に失敗した時点で止め, 上限回数に達したらその時点の結果を出す.
    fn watch(&mut self, n: u64, until: WatchUntil) {
        self.state.syncing.insert(n);
        let RuntimeConfig {
            watch_interval,
            watch_tries,
            ..
        } = self.cfg;
        self.spawn(move |p| {
            let mut res = p.gh.fetch_pr(n);
            for _ in 1..watch_tries {
                if res.as_ref().map_or(true, |pr| until.is_done(pr.as_ref())) {
                    break;
                }
                thread::sleep(watch_interval);
                res = p.gh.fetch_pr(n);
            }
            Event::Reloaded(n, Box::new(res))
        });
    }

    fn reload_local(&mut self) {
        match self.ports.git.read_local_state() {
            Ok(l) => self.state.local = l,
            Err(e) => self.state.error(e.to_string()),
        }
    }

    fn save_cache(&mut self) {
        let (mine, review) = (&self.state.mine, &self.state.review);
        let cache = Cache {
            fetched_at: mine.fetched_at.unwrap_or_else(Utc::now),
            prs: mine.prs.clone(),
            devin_sessions: self.state.devin_sessions.clone(),
            reviews: review.fetched_at.map(|at| (at, review.prs.clone())),
            custom: self
                .state
                .custom
                .iter()
                .filter_map(|t| {
                    Some(CustomCache {
                        query: t.query.clone(),
                        fetched_at: t.list.fetched_at?,
                        prs: t.list.prs.clone(),
                    })
                })
                .collect(),
        };
        if let Err(e) = self.ports.cache.save(&cache) {
            self.state.error(format!("キャッシュの保存失敗: {e}"));
        }
    }

    /// 未コミットの変更 (untracked は除く) があれば実行しない. force の reset で消さないため
    fn checkout(&mut self, n: u64, force: bool) {
        self.reload_local();
        if self.state.local.dirty {
            self.state.error(format!(
                "{} に未コミットの変更があるのでチェックアウトしない",
                self.state.dir.display()
            ));
            return;
        }
        self.state.busy = Some(format!("#{n} をチェックアウト中…"));
        self.spawn(move |p| Event::CheckedOut(n, force, p.gh.checkout(n, force)));
    }
}

/// Devin の config が無ければ API を呼ばずにエラーにする. 実行中に config を消されたときに備える
fn with_devin<T>(
    cfg: Option<&DevinConfig>,
    f: impl FnOnce(&DevinConfig) -> Result<T>,
) -> Result<T> {
    f(cfg.ok_or_else(|| anyhow!("Devin の設定が無い"))?)
}
