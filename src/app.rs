use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::Result;
use chrono::{DateTime, Utc};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::widgets::TableState;

use crate::domain::linear::linear_issue_key;
use crate::domain::local::LocalState;
use crate::domain::op::PrOp;
use crate::domain::pr::Pr;
use crate::domain::review::review_targets;
use crate::domain::stack::arrange;
use crate::infra::cache::{self, Cache};
use crate::infra::gh::{self, fetch_pr, fetch_prs, linear_review_url, update_branch};
use crate::infra::git::read_local_state;
use crate::infra::process::open_url;

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

pub struct Message {
    pub text: String,
    pub is_error: bool,
}

pub struct App {
    pub dir: PathBuf,
    pub repo: String,
    pub prs: Vec<Pr>,
    pub local: LocalState,
    pub draft_only: bool,
    pub table: TableState,
    pub message: Option<Message>,
    pub loading: bool,
    pub busy: Option<String>,
    /// base 取り込みの確認待ちの PR 番号と base ブランチ名
    pub confirm_update: Option<(u64, String)>,
    /// 2 回目のキー入力待ちの状態変更 (draft, close)
    pub confirm_op: Option<(u64, PrOp)>,
    pub last_fetch: Option<Instant>,
    /// `prs` の全件を取得した時刻. キャッシュから読んだときはキャッシュを書いた時刻
    pub fetched_at: Option<DateTime<Utc>>,
    /// `prs` がキャッシュのままで, 起動後の取得がまだ終わっていない
    pub from_cache: bool,
    /// 操作の反映を待って取り直している PR 番号
    pub syncing: HashSet<u64>,
    tx: Sender<Event>,
    pub rx: Receiver<Event>,
}

impl App {
    pub fn new(dir: PathBuf, repo: String) -> Self {
        let (tx, rx) = channel();
        let cached = cache::load(&repo);
        let mut app = Self {
            local: read_local_state(&dir).unwrap_or_default(),
            dir,
            repo,
            fetched_at: cached.as_ref().map(|c| c.fetched_at),
            from_cache: cached.is_some(),
            prs: cached.map(|c| c.prs).unwrap_or_default(),
            syncing: HashSet::new(),
            draft_only: false,
            table: TableState::default().with_selected(0),
            message: None,
            loading: false,
            busy: None,
            confirm_update: None,
            confirm_op: None,
            last_fetch: None,
            tx,
            rx,
        };
        app.refresh();
        app
    }

    pub fn visible(&self) -> Vec<&Pr> {
        self.prs
            .iter()
            .filter(|p| !self.draft_only || p.is_draft)
            .collect()
    }

    pub fn selected(&self) -> Option<&Pr> {
        let i = self.table.selected()?;
        self.visible().get(i).copied()
    }

    pub fn refresh(&mut self) {
        if self.loading {
            return;
        }
        self.loading = true;
        self.last_fetch = Some(Instant::now());
        let (tx, dir, repo) = (self.tx.clone(), self.dir.clone(), self.repo.clone());
        thread::spawn(move || {
            let _ = tx.send(Event::Fetched(fetch_prs(&dir, &repo)));
        });
        self.reload_local();
    }

    fn pr(&self, n: u64) -> Option<&Pr> {
        self.prs.iter().find(|p| p.number == n)
    }

    /// 一覧を差し替えてキャッシュに書く. 並びが変わっても同じ PR を選んだままにする
    fn set_prs(&mut self, prs: Vec<Pr>) {
        let sel = self.selected().map(|p| p.number);
        self.prs = prs;
        if let Some(i) = sel.and_then(|n| self.visible().iter().position(|p| p.number == n)) {
            self.table.select(Some(i));
        }
        self.clamp_selection();
        let cache = Cache {
            fetched_at: self.fetched_at.unwrap_or_else(Utc::now),
            prs: self.prs.clone(),
        };
        if let Err(e) = cache::save(&self.repo, &cache) {
            self.error(format!("キャッシュの保存失敗: {e}"));
        }
    }

    /// 操作が GitHub に反映されるまで PR を 1 件だけ取り直す. 全件の取得より軽く, 他の行は触らない.
    /// `done` が true を返すか取得に失敗した時点で止め, 上限回数に達したらその時点の結果を出す.
    fn watch(&mut self, n: u64, done: impl Fn(Option<&Pr>) -> bool + Send + 'static) {
        self.syncing.insert(n);
        let (tx, dir, repo) = (self.tx.clone(), self.dir.clone(), self.repo.clone());
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
        match read_local_state(&self.dir) {
            Ok(l) => self.local = l,
            Err(e) => self.error(e.to_string()),
        }
    }

    pub fn handle_event(&mut self, ev: Event) {
        match ev {
            Event::Fetched(res) => {
                self.loading = false;
                match res {
                    Ok(prs) => {
                        self.fetched_at = Some(Utc::now());
                        self.from_cache = false;
                        self.set_prs(prs);
                    }
                    Err(e) => self.error(format!("取得失敗: {e}")),
                }
            }
            Event::Reloaded(n, res) => {
                self.syncing.remove(&n);
                match *res {
                    Ok(pr) => {
                        // drain すると set_prs が選択中の PR を引けなくなるので複製する
                        let mut prs: Vec<Pr> =
                            self.prs.iter().filter(|p| p.number != n).cloned().collect();
                        prs.extend(pr);
                        self.set_prs(arrange(prs));
                    }
                    Err(e) => self.error(format!("#{n} の再取得失敗: {e}")),
                }
            }
            Event::UpdatedBranch(n, res) => {
                self.busy = None;
                match res {
                    Ok(base) => {
                        self.info(format!("#{n} に {base} を取り込んだ"));
                        // merge commit が積まれて head が動くまで待つ
                        let old = self.pr(n).map(|p| p.head_ref_oid.clone());
                        self.watch(n, move |p| {
                            p.is_none_or(|p| Some(&p.head_ref_oid) != old.as_ref())
                        });
                    }
                    Err(e) => {
                        self.error(format!("#{n}: {e:#}"));
                        // head が動いていて 422 になったときなどに最新を出す
                        self.watch(n, |_| true);
                    }
                }
            }
            Event::LinearReview(n, res) => {
                self.busy = None;
                match res {
                    Ok(Some(url)) => {
                        if let Err(e) = open_url(&self.dir, &url) {
                            self.error(e.to_string());
                        }
                    }
                    Ok(None) => self.error(format!("#{n} に Linear review のリンクが無い")),
                    Err(e) => self.error(format!("#{n} の Linear review 取得失敗: {e}")),
                }
            }
            Event::ChangedState(n, op, res) => {
                self.busy = None;
                match res {
                    Ok(_) => {
                        self.info(format!("#{n} を {}した", op.phrase(&self.local.reviewers)));
                        let reviewers = self.local.reviewers.clone();
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
                        self.error(format!("#{n} の {} 失敗: {e}", op.label()));
                        self.watch(n, |_| true);
                    }
                }
            }
            Event::CheckedOut(n, res) => {
                self.busy = None;
                match res {
                    Ok(_) => self.info(format!("#{n} をチェックアウトした")),
                    Err(e) => self.error(format!("#{n} のチェックアウト失敗: {e}")),
                }
                self.reload_local();
            }
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> Action {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            return Action::Quit;
        }
        if let Some((n, base)) = self.confirm_update.take() {
            if key.code == KeyCode::Char('y') {
                self.update_branch(n, base);
            } else {
                self.info(format!("{base} の取り込みを取りやめた"));
            }
            return Action::None;
        }
        if let Some((n, op)) = self.confirm_op.take() {
            if key.code == KeyCode::Char(op.key()) {
                self.change_state(n, op);
            } else {
                self.info(format!("#{n} の {} を取りやめた", op.label()));
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
            KeyCode::Char('j') | KeyCode::Down => self.move_by(1),
            KeyCode::Char('k') | KeyCode::Up => self.move_by(-1),
            KeyCode::Char('g') | KeyCode::Home => self.table.select(Some(0)),
            KeyCode::Char('G') | KeyCode::End => {
                let len = self.visible().len();
                self.table.select(Some(len.saturating_sub(1)));
            }
            KeyCode::Char('r') => {
                self.message = None;
                self.refresh();
            }
            KeyCode::Char('t') => {
                self.draft_only = !self.draft_only;
                self.clamp_selection();
            }
            KeyCode::Enter | KeyCode::Char('c') => self.checkout(),
            KeyCode::Char('o') => {
                if let Some(n) = self.selected().map(|p| p.number)
                    && let Err(e) = gh::view_web(&self.dir, n)
                {
                    self.error(e.to_string());
                }
            }
            KeyCode::Char('l') => self.open_linear(),
            KeyCode::Char('L') => self.open_linear_review(),
            KeyCode::Char('d') => {
                if let Some(p) = self.selected() {
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
            .selected()
            .map(|p| (p.number, linear_issue_key(&p.head_ref_name, &p.title)))
        else {
            return;
        };
        let Some(workspace) = self.local.linear_workspace.clone() else {
            self.error(
                "Linear の workspace が未設定. git config prtui.linear-workspace <slug> を設定する"
                    .into(),
            );
            return;
        };
        let Some(key) = key else {
            self.error(format!("#{n} に Linear の issue キーが無い"));
            return;
        };
        let url = format!("https://linear.app/{workspace}/issue/{key}");
        if let Err(e) = open_url(&self.dir, &url) {
            self.error(e.to_string());
        }
    }

    /// 選択中の PR の Linear review 画面をブラウザで開く.
    /// review の URL は slug が Linear 側の ID なので GitHub の情報からは組み立てられない.
    /// Linear の bot が PR に付けるコメントから取るので, コメントの取得をスレッドで行う.
    fn open_linear_review(&mut self) {
        if self.busy.is_some() {
            return;
        }
        let Some(n) = self.selected().map(|p| p.number) else {
            return;
        };
        self.busy = Some(format!("#{n} の Linear review を探し中…"));
        let (tx, dir, repo) = (self.tx.clone(), self.dir.clone(), self.repo.clone());
        thread::spawn(move || {
            let _ = tx.send(Event::LinearReview(n, linear_review_url(&dir, &repo, n)));
        });
    }

    /// update-branch API は PR の base を取り込むので, stacked PR では親のブランチが入る.
    /// main 以外が base のときは取り込み済みかを判定できないので確認だけ出す.
    fn request_update(&mut self) {
        if self.busy.is_some() {
            return;
        }
        let Some(pr) = self.selected() else { return };
        let (n, base) = (pr.number, pr.base_ref_name.clone());
        if pr.is_conflicting() {
            self.error(format!(
                "#{n} はコンフリクトしているので {base} を取り込めない"
            ));
        } else if base == "main" && pr.behind_main == Some(0) {
            self.info(format!("#{n} は main を取り込み済み"));
        } else {
            self.confirm_update = Some((n, base));
        }
    }

    fn update_branch(&mut self, n: u64, base: String) {
        let Some(oid) = self.pr(n).map(|p| p.head_ref_oid.clone()) else {
            return;
        };
        self.busy = Some(format!("#{n} に {base} を取り込み中…"));
        let (tx, dir, repo) = (self.tx.clone(), self.dir.clone(), self.repo.clone());
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
        if self.busy.is_some() {
            return;
        }
        let Some(pr) = self.selected() else { return };
        let n = pr.number;
        match op {
            PrOp::Open if !pr.is_draft => self.info(format!("#{n} は Draft ではない")),
            PrOp::Draft if pr.is_draft => self.info(format!("#{n} は既に Draft")),
            PrOp::Review if pr.is_draft => {
                self.error(format!("#{n} は Draft なので review 依頼しない"))
            }
            PrOp::Review if self.local.reviewers.is_empty() => self.error(
                "review 依頼の相手が未設定. git config prtui.reviewers <login>,<login> を設定する"
                    .into(),
            ),
            PrOp::Review if review_targets(pr, &self.local.reviewers).is_empty() => self.info(
                format!("#{n} は {} に依頼済み", self.local.reviewers.join(", ")),
            ),
            PrOp::Open => self.change_state(n, op),
            PrOp::Draft | PrOp::Close | PrOp::Review => self.confirm_op = Some((n, op)),
        }
    }

    fn change_state(&mut self, n: u64, op: PrOp) {
        let targets = self
            .pr(n)
            .map(|p| review_targets(p, &self.local.reviewers))
            .unwrap_or_default()
            .into_iter()
            .map(String::from)
            .collect::<Vec<_>>();
        self.busy = Some(format!(
            "#{n} を {}している…",
            op.phrase(&self.local.reviewers)
        ));
        let (tx, dir) = (self.tx.clone(), self.dir.clone());
        thread::spawn(move || {
            let res = gh::change_state(&dir, n, op, &targets);
            let _ = tx.send(Event::ChangedState(n, op, res));
        });
    }

    fn checkout(&mut self) {
        if self.busy.is_some() {
            return;
        }
        let Some(pr) = self.selected() else { return };
        let n = pr.number;
        self.reload_local();
        if self.local.dirty {
            self.error(format!(
                "{} に未コミットの変更があるのでチェックアウトしない",
                self.dir.display()
            ));
            return;
        }
        self.busy = Some(format!("#{n} をチェックアウト中…"));
        let (tx, dir) = (self.tx.clone(), self.dir.clone());
        thread::spawn(move || {
            let res = gh::checkout(&dir, n);
            let _ = tx.send(Event::CheckedOut(n, res));
        });
    }

    fn move_by(&mut self, delta: isize) {
        let len = self.visible().len();
        if len == 0 {
            return;
        }
        let cur = self.table.selected().unwrap_or(0) as isize;
        self.table
            .select(Some((cur + delta).clamp(0, len as isize - 1) as usize));
    }

    fn clamp_selection(&mut self) {
        let len = self.visible().len();
        let cur = self.table.selected().unwrap_or(0);
        self.table.select(Some(cur.min(len.saturating_sub(1))));
    }

    fn info(&mut self, text: String) {
        self.message = Some(Message {
            text,
            is_error: false,
        });
    }

    pub fn error(&mut self, text: String) {
        self.message = Some(Message {
            text,
            is_error: true,
        });
    }
}
