use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::Result;
use chrono::Utc;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::domain::linear::linear_issue_key;
use crate::domain::op::PrOp;
use crate::domain::pr::Pr;
use crate::domain::review::review_targets;
use crate::domain::stack::arrange;
use crate::infra::cache::{self, Cache};
use crate::infra::gh::{self, fetch_pr, fetch_prs, linear_review_url, update_branch};
use crate::infra::git::read_local_state;
use crate::infra::process::open_url;

mod state;

pub use state::AppState;

pub const AUTO_REFRESH: Duration = Duration::from_secs(300);
/// 操作の反映を待って PR を取り直す間隔と回数. update-branch は GitHub 側で非同期に merge される
const WATCH_INTERVAL: Duration = Duration::from_secs(2);
const WATCH_TRIES: usize = 10;

pub enum Event {
    Fetched(Result<Vec<Pr>>),
    CheckedOut(u64, Result<String>),
    /// 成功時は取り込んだ base ブランチ名
    UpdatedBranch(u64, Result<String>),
    /// Linear review の URL. コメントに見つからなければ None
    LinearReview(u64, Result<Option<String>>),
    ChangedState(u64, PrOp, Result<String>),
    /// 1 件だけ取り直した結果. open でなくなっていれば None
    Reloaded(u64, Box<Result<Option<Pr>>>),
}

pub enum Action {
    None,
    Quit,
    /// TUI を一時停止して外部コマンドを端末に繋いで実行する
    Suspend(Vec<String>),
}

pub struct App {
    pub state: AppState,
    tx: Sender<Event>,
    pub rx: Receiver<Event>,
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

    pub fn refresh(&mut self) {
        if self.state.loading {
            return;
        }
        self.state.loading = true;
        self.state.last_fetch = Some(Instant::now());
        let (tx, dir, repo) = (
            self.tx.clone(),
            self.state.dir.clone(),
            self.state.repo.clone(),
        );
        thread::spawn(move || {
            let _ = tx.send(Event::Fetched(fetch_prs(&dir, &repo)));
        });
        self.reload_local();
    }

    /// 一覧を差し替えてキャッシュに書く. 並びが変わっても同じ PR を選んだままにする
    fn set_prs(&mut self, prs: Vec<Pr>) {
        self.state.replace_prs(prs);
        let cache = Cache {
            fetched_at: self.state.fetched_at.unwrap_or_else(Utc::now),
            prs: self.state.prs.clone(),
        };
        if let Err(e) = cache::save(&self.state.repo, &cache) {
            self.state.error(format!("キャッシュの保存失敗: {e}"));
        }
    }

    /// 操作が GitHub に反映されるまで PR を 1 件だけ取り直す. 全件の取得より軽く, 他の行は触らない.
    /// `done` が true を返すか取得に失敗した時点で止め, 上限回数に達したらその時点の結果を出す.
    fn watch(&mut self, n: u64, done: impl Fn(Option<&Pr>) -> bool + Send + 'static) {
        self.state.syncing.insert(n);
        let (tx, dir, repo) = (
            self.tx.clone(),
            self.state.dir.clone(),
            self.state.repo.clone(),
        );
        thread::spawn(move || {
            let mut res = fetch_pr(&dir, &repo, n);
            for _ in 1..WATCH_TRIES {
                if res.as_ref().map_or(true, |p| done(p.as_ref())) {
                    break;
                }
                thread::sleep(WATCH_INTERVAL);
                res = fetch_pr(&dir, &repo, n);
            }
            let _ = tx.send(Event::Reloaded(n, Box::new(res)));
        });
    }

    fn reload_local(&mut self) {
        match read_local_state(&self.state.dir) {
            Ok(l) => self.state.local = l,
            Err(e) => self.state.error(e.to_string()),
        }
    }

    pub fn handle_event(&mut self, ev: Event) {
        match ev {
            Event::Fetched(res) => {
                self.state.loading = false;
                match res {
                    Ok(prs) => {
                        self.state.fetched_at = Some(Utc::now());
                        self.state.from_cache = false;
                        self.set_prs(prs);
                    }
                    Err(e) => self.state.error(format!("取得失敗: {e}")),
                }
            }
            Event::Reloaded(n, res) => {
                self.state.syncing.remove(&n);
                match *res {
                    Ok(pr) => {
                        // drain すると set_prs が選択中の PR を引けなくなるので複製する
                        let mut prs: Vec<Pr> = self
                            .state
                            .prs
                            .iter()
                            .filter(|p| p.number != n)
                            .cloned()
                            .collect();
                        prs.extend(pr);
                        self.set_prs(arrange(prs));
                    }
                    Err(e) => self.state.error(format!("#{n} の再取得失敗: {e}")),
                }
            }
            Event::UpdatedBranch(n, res) => {
                self.state.busy = None;
                match res {
                    Ok(base) => {
                        self.state.info(format!("#{n} に {base} を取り込んだ"));
                        // merge commit が積まれて head が動くまで待つ
                        let old = self.state.pr(n).map(|p| p.head_ref_oid.clone());
                        self.watch(n, move |p| {
                            p.is_none_or(|p| Some(&p.head_ref_oid) != old.as_ref())
                        });
                    }
                    Err(e) => {
                        self.state.error(format!("#{n}: {e:#}"));
                        // head が動いていて 422 になったときなどに最新を出す
                        self.watch(n, |_| true);
                    }
                }
            }
            Event::LinearReview(n, res) => {
                self.state.busy = None;
                match res {
                    Ok(Some(url)) => {
                        if let Err(e) = open_url(&self.state.dir, &url) {
                            self.state.error(e.to_string());
                        }
                    }
                    Ok(None) => self
                        .state
                        .error(format!("#{n} に Linear review のリンクが無い")),
                    Err(e) => self
                        .state
                        .error(format!("#{n} の Linear review 取得失敗: {e}")),
                }
            }
            Event::ChangedState(n, op, res) => {
                self.state.busy = None;
                match res {
                    Ok(_) => {
                        self.state.info(format!(
                            "#{n} を {}した",
                            op.phrase(&self.state.local.reviewers)
                        ));
                        let reviewers = self.state.local.reviewers.clone();
                        self.watch(n, move |p| match op {
                            PrOp::Open => p.is_none_or(|p| !p.is_draft),
                            PrOp::Draft => p.is_none_or(|p| p.is_draft),
                            PrOp::Close => p.is_none(),
                            PrOp::Review => {
                                p.is_none_or(|p| review_targets(p, &reviewers).is_empty())
                            }
                        });
                    }
                    Err(e) => {
                        self.state
                            .error(format!("#{n} の {} 失敗: {e}", op.label()));
                        self.watch(n, |_| true);
                    }
                }
            }
            Event::CheckedOut(n, res) => {
                self.state.busy = None;
                match res {
                    Ok(_) => self.state.info(format!("#{n} をチェックアウトした")),
                    Err(e) => self.state.error(format!("#{n} のチェックアウト失敗: {e}")),
                }
                self.reload_local();
            }
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> Action {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            return Action::Quit;
        }
        if let Some((n, base)) = self.state.confirm_update.take() {
            if key.code == KeyCode::Char('y') {
                self.update_branch(n, base);
            } else {
                self.state.info(format!("{base} の取り込みを取りやめた"));
            }
            return Action::None;
        }
        if let Some((n, op)) = self.state.confirm_op.take() {
            if key.code == KeyCode::Char(op.key()) {
                self.change_state(n, op);
            } else {
                self.state
                    .info(format!("#{n} の {} を取りやめた", op.label()));
            }
            return Action::None;
        }
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => return Action::Quit,
            KeyCode::Char('u') => self.request_update(),
            KeyCode::Char('O') => self.request_state(PrOp::Open),
            KeyCode::Char('D') => self.request_state(PrOp::Draft),
            KeyCode::Char('X') => self.request_state(PrOp::Close),
            KeyCode::Char('R') => self.request_state(PrOp::Review),
            KeyCode::Char('j') | KeyCode::Down => self.state.move_by(1),
            KeyCode::Char('k') | KeyCode::Up => self.state.move_by(-1),
            KeyCode::Char('g') | KeyCode::Home => self.state.table.select(Some(0)),
            KeyCode::Char('G') | KeyCode::End => {
                let len = self.state.visible().len();
                self.state.table.select(Some(len.saturating_sub(1)));
            }
            KeyCode::Char('r') => {
                self.state.message = None;
                self.refresh();
            }
            KeyCode::Char('t') => {
                self.state.draft_only = !self.state.draft_only;
                self.state.clamp_selection();
            }
            KeyCode::Enter | KeyCode::Char('c') => self.checkout(),
            KeyCode::Char('o') => {
                if let Some(n) = self.state.selected().map(|p| p.number)
                    && let Err(e) = gh::view_web(&self.state.dir, n)
                {
                    self.state.error(e.to_string());
                }
            }
            KeyCode::Char('l') => self.open_linear(),
            KeyCode::Char('L') => self.open_linear_review(),
            KeyCode::Char('d') => {
                if let Some(p) = self.state.selected() {
                    return Action::Suspend(vec![
                        "gh".into(),
                        "pr".into(),
                        "diff".into(),
                        p.number.to_string(),
                    ]);
                }
            }
            _ => {}
        }
        Action::None
    }

    /// 選択中の PR に対応する Linear の issue をブラウザで開く.
    fn open_linear(&mut self) {
        let Some((n, key)) = self
            .state
            .selected()
            .map(|p| (p.number, linear_issue_key(&p.head_ref_name, &p.title)))
        else {
            return;
        };
        let Some(workspace) = self.state.local.linear_workspace.clone() else {
            self.state.error(
                "Linear の workspace が未設定. git config prtui.linear-workspace <slug> を設定する"
                    .into(),
            );
            return;
        };
        let Some(key) = key else {
            self.state
                .error(format!("#{n} に Linear の issue キーが無い"));
            return;
        };
        let url = format!("https://linear.app/{workspace}/issue/{key}");
        if let Err(e) = open_url(&self.state.dir, &url) {
            self.state.error(e.to_string());
        }
    }

    /// 選択中の PR の Linear review 画面をブラウザで開く.
    /// review の URL は slug が Linear 側の ID なので GitHub の情報からは組み立てられない.
    /// Linear の bot が PR に付けるコメントから取るので, コメントの取得をスレッドで行う.
    fn open_linear_review(&mut self) {
        if self.state.busy.is_some() {
            return;
        }
        let Some(n) = self.state.selected().map(|p| p.number) else {
            return;
        };
        self.state.busy = Some(format!("#{n} の Linear review を探し中…"));
        let (tx, dir, repo) = (
            self.tx.clone(),
            self.state.dir.clone(),
            self.state.repo.clone(),
        );
        thread::spawn(move || {
            let _ = tx.send(Event::LinearReview(n, linear_review_url(&dir, &repo, n)));
        });
    }

    /// update-branch API は PR の base を取り込むので, stacked PR では親のブランチが入る.
    /// main 以外が base のときは取り込み済みかを判定できないので確認だけ出す.
    fn request_update(&mut self) {
        if self.state.busy.is_some() {
            return;
        }
        let Some(pr) = self.state.selected() else {
            return;
        };
        let (n, base) = (pr.number, pr.base_ref_name.clone());
        if pr.is_conflicting() {
            self.state.error(format!(
                "#{n} はコンフリクトしているので {base} を取り込めない"
            ));
        } else if base == "main" && pr.behind_main == Some(0) {
            self.state.info(format!("#{n} は main を取り込み済み"));
        } else {
            self.state.confirm_update = Some((n, base));
        }
    }

    fn update_branch(&mut self, n: u64, base: String) {
        let Some(oid) = self.state.pr(n).map(|p| p.head_ref_oid.clone()) else {
            return;
        };
        self.state.busy = Some(format!("#{n} に {base} を取り込み中…"));
        let (tx, dir, repo) = (
            self.tx.clone(),
            self.state.dir.clone(),
            self.state.repo.clone(),
        );
        thread::spawn(move || {
            let res = update_branch(&dir, &repo, n, &oid)
                .map_err(|e| e.context(format!("{base} の取り込み失敗")));
            let _ = tx.send(Event::UpdatedBranch(n, res.map(|_| base)));
        });
    }

    /// open は 1 回で実行し, draft, close, review 依頼は同じキーの 2 回目で実行する.
    /// 既にその状態の PR では何もしない. review 依頼は open (Draft でない) PR に限り,
    /// 相手が未設定か, 全員が既に依頼中なら何もしない.
    fn request_state(&mut self, op: PrOp) {
        if self.state.busy.is_some() {
            return;
        }
        let Some(pr) = self.state.selected() else {
            return;
        };
        let n = pr.number;
        match op {
            PrOp::Open if !pr.is_draft => self.state.info(format!("#{n} は Draft ではない")),
            PrOp::Draft if pr.is_draft => self.state.info(format!("#{n} は既に Draft")),
            PrOp::Review if pr.is_draft => self
                .state
                .error(format!("#{n} は Draft なので review 依頼しない")),
            PrOp::Review if self.state.local.reviewers.is_empty() => self.state.error(
                "review 依頼の相手が未設定. git config prtui.reviewers <login>,<login> を設定する"
                    .into(),
            ),
            PrOp::Review if review_targets(pr, &self.state.local.reviewers).is_empty() => {
                self.state.info(format!(
                    "#{n} は {} に依頼済み",
                    self.state.local.reviewers.join(", ")
                ))
            }
            PrOp::Open => self.change_state(n, op),
            PrOp::Draft | PrOp::Close | PrOp::Review => self.state.confirm_op = Some((n, op)),
        }
    }

    fn change_state(&mut self, n: u64, op: PrOp) {
        let targets = self
            .state
            .pr(n)
            .map(|p| review_targets(p, &self.state.local.reviewers))
            .unwrap_or_default()
            .into_iter()
            .map(String::from)
            .collect::<Vec<_>>();
        self.state.busy = Some(format!(
            "#{n} を {}している…",
            op.phrase(&self.state.local.reviewers)
        ));
        let (tx, dir) = (self.tx.clone(), self.state.dir.clone());
        thread::spawn(move || {
            let res = gh::change_state(&dir, n, op, &targets);
            let _ = tx.send(Event::ChangedState(n, op, res));
        });
    }

    fn checkout(&mut self) {
        if self.state.busy.is_some() {
            return;
        }
        let Some(pr) = self.state.selected() else {
            return;
        };
        let n = pr.number;
        self.reload_local();
        if self.state.local.dirty {
            self.state.error(format!(
                "{} に未コミットの変更があるのでチェックアウトしない",
                self.state.dir.display()
            ));
            return;
        }
        self.state.busy = Some(format!("#{n} をチェックアウト中…"));
        let (tx, dir) = (self.tx.clone(), self.state.dir.clone());
        thread::spawn(move || {
            let res = gh::checkout(&dir, n);
            let _ = tx.send(Event::CheckedOut(n, res));
        });
    }
}
