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
            apply_cache(&mut state, c);
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
                Effect::ChangeState { number, op } => self
                    .spawn(move |p| Event::ChangedState(number, op, p.gh.change_state(number, op))),
                Effect::RequestReviews { number, targets } => self.spawn(move |p| {
                    let res = p.gh.request_reviews(number, &targets);
                    Event::RequestedReviews {
                        number,
                        targets,
                        res,
                    }
                }),
                Effect::LinearReview(n) => {
                    self.spawn(move |p| Event::LinearReview(n, p.gh.linear_review_url(n)))
                }
                Effect::Watch { number, until } => self.watch(number, until),
                Effect::FetchLabels => self.spawn(|p| Event::FetchedLabels(p.gh.fetch_labels())),
                Effect::FetchConversation(n) => {
                    self.spawn(move |p| Event::FetchedConversation(n, p.gh.fetch_conversation(n)))
                }
                Effect::SubmitReview {
                    number,
                    action,
                    body,
                } => self.spawn(move |p| {
                    let res = p.gh.submit_review(number, action, &body);
                    Event::SubmittedReview {
                        number,
                        action,
                        res,
                    }
                }),
                Effect::EditBody {
                    number,
                    original,
                    body,
                } => self.spawn(move |p| Event::EditedBody {
                    number,
                    res: p.gh.edit_body(number, &original, &body),
                }),
                Effect::EditLabels {
                    number,
                    add,
                    remove,
                } => self.spawn(move |p| {
                    let res = p.gh.edit_labels(number, &add, &remove);
                    Event::EditedLabels {
                        number,
                        add,
                        remove,
                        res,
                    }
                }),
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
                    let path = std::env::temp_dir()
                        .join(format!("prtui-compose-{}.md", std::process::id()));
                    match fs::write(&path, draft) {
                        Ok(()) => {
                            action = Action::Suspend(editor_command(
                                self.state.local.editor.as_deref(),
                                &path.display().to_string(),
                            ));
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
        if let Err(e) = self.ports.cache.save(&snapshot(&self.state)) {
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

    /// スレッドから Event が n 個届くまで待って反映する. 届かなければ 5 秒で落とす
    #[cfg(test)]
    fn pump_blocking(&mut self, n: usize) {
        for _ in 0..n {
            let ev = self
                .rx
                .recv_timeout(std::time::Duration::from_secs(5))
                .expect("Event が届かない");
            self.handle_event(ev);
        }
    }
}

/// キャッシュの一覧を状態に戻す. 追加のタブはクエリが今の設定と一致するものだけ戻す
fn apply_cache(state: &mut AppState, c: Cache) {
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

/// キャッシュに書く内容. 未取得のレビュータブと追加のタブは書かない
fn snapshot(state: &AppState) -> Cache {
    let (mine, review) = (&state.mine, &state.review);
    Cache {
        fetched_at: mine.fetched_at.unwrap_or_else(Utc::now),
        prs: mine.prs.clone(),
        devin_sessions: state.devin_sessions.clone(),
        reviews: review.fetched_at.map(|at| (at, review.prs.clone())),
        custom: state
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
    }
}

/// path をエディタで開くコマンド. editor (git config `prtui.editor`) が無ければ
/// `$VISUAL`, `$EDITOR`, vi の順に使う. `code --wait` のように引数を含められるよう, git の
/// `core.editor` と同じく sh に渡す. path は `$1` で渡して sh に解釈させない
fn editor_command(editor: Option<&str>, path: &str) -> Vec<String> {
    let editor = editor.unwrap_or("${VISUAL:-${EDITOR:-vi}}");
    vec![
        "sh".into(),
        "-c".into(),
        format!("{editor} \"$1\""),
        "sh".into(),
        path.into(),
    ]
}

/// Devin の config が無ければ API を呼ばずにエラーにする. 実行中に config を消されたときに備える
fn with_devin<T>(
    cfg: Option<&DevinConfig>,
    f: impl FnOnce(&DevinConfig) -> Result<T>,
) -> Result<T> {
    f(cfg.ok_or_else(|| anyhow!("Devin の設定が無い"))?)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::time::Duration;

    use chrono::{DateTime, Utc};
    use pretty_assertions::assert_eq;

    use super::{App, apply_cache, snapshot};
    use crate::app::event::{Effect, WatchUntil};
    use crate::app::ports::RuntimeConfig;
    use crate::app::ports::fake::Fakes;
    use crate::app::state::AppState;
    use crate::domain::cache::{Cache, CustomCache};
    use crate::domain::label::Label;
    use crate::domain::local::{LocalState, TabQuery};
    use crate::domain::pr::Pr;

    fn pr(n: u64) -> Pr {
        let mut p = Pr::fixture(n, &format!("b{n}"), "main");
        p.head_ref_oid = format!("oid{n}");
        p
    }

    fn at(secs: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(secs, 0).unwrap()
    }

    fn cfg(watch_tries: usize) -> RuntimeConfig {
        RuntimeConfig {
            watch_interval: Duration::ZERO,
            watch_tries,
            ..RuntimeConfig::default()
        }
    }

    fn app(f: &Fakes) -> App {
        App::with_ports(PathBuf::from("/tmp"), "o/r".into(), f.ports(), cfg(10))
    }

    fn tabs(queries: &[(&str, &str)]) -> LocalState {
        LocalState {
            tabs: queries
                .iter()
                .map(|(name, query)| TabQuery {
                    name: (*name).into(),
                    query: (*query).into(),
                })
                .collect(),
            ..LocalState::default()
        }
    }

    fn numbers(prs: &[Pr]) -> Vec<u64> {
        prs.iter().map(|p| p.number).collect()
    }

    fn message(s: &AppState) -> (&str, bool) {
        let m = s.message.as_ref().expect("message");
        (&m.text, m.is_error)
    }

    /// `Pr` は PartialEq を持たないので JSON にして比べる
    fn json(c: &Cache) -> serde_json::Value {
        serde_json::to_value(c).unwrap()
    }

    #[test]
    fn checkout_refused_when_dirty() {
        let f = Fakes::default();
        let mut a = app(&f);
        f.git.local.lock().unwrap().dirty = true;
        a.perform(vec![Effect::Checkout {
            number: 1,
            force: false,
        }]);
        assert!(f.gh.calls().is_empty());
        assert!(a.state.busy.is_none());
        assert_eq!(
            message(&a.state),
            (
                "/tmp に未コミットの変更があるのでチェックアウトしない",
                true
            )
        );
    }

    #[test]
    fn checkout_runs_gh_when_clean() {
        let f = Fakes::default();
        let mut a = app(&f);
        a.perform(vec![Effect::Checkout {
            number: 1,
            force: true,
        }]);
        assert_eq!(a.state.busy.as_deref(), Some("#1 をチェックアウト中…"));
        a.pump_blocking(1);
        assert_eq!(f.gh.calls(), vec!["checkout 1 true"]);
        assert!(a.state.busy.is_none());
        assert_eq!(
            message(&a.state),
            ("#1 を PR の最新に合わせてチェックアウトした", false)
        );
    }

    #[test]
    fn watch_immediately_fetches_once_and_replaces() {
        let f = Fakes::default();
        let mut a = app(&f);
        a.state.mine.prs = vec![pr(1), pr(2)];
        let mut new = pr(1);
        new.title = "new".into();
        f.gh.pr.lock().unwrap().push_back(Ok(Some(new)));
        a.perform(vec![Effect::Watch {
            number: 1,
            until: WatchUntil::Immediately,
        }]);
        assert!(a.state.syncing.contains(&1));
        a.pump_blocking(1);
        assert_eq!(f.gh.calls(), vec!["fetch_pr 1"]);
        assert!(a.state.syncing.is_empty());
        assert_eq!(a.state.pr(1).map(|p| p.title.as_str()), Some("new"));
        assert!(a.state.pr(2).is_some());
        // 差し替えた一覧はキャッシュにも書く
        let saved = f.cache.cache.lock().unwrap().clone().expect("saved");
        assert_eq!(saved.prs.len(), 2);
    }

    #[test]
    fn watch_retries_until_done() {
        let f = Fakes::default();
        let mut a = app(&f);
        let mut draft = pr(1);
        draft.is_draft = true;
        a.state.mine.prs = vec![draft.clone()];
        f.gh.pr
            .lock()
            .unwrap()
            .extend([Ok(Some(draft.clone())), Ok(Some(draft)), Ok(Some(pr(1)))]);
        a.perform(vec![Effect::Watch {
            number: 1,
            until: WatchUntil::NotDraft,
        }]);
        a.pump_blocking(1);
        assert_eq!(f.gh.calls(), vec!["fetch_pr 1"; 3]);
        assert_eq!(a.state.pr(1).map(|p| p.is_draft), Some(false));
    }

    /// ラベルを付け外したら, その結果になるまで 1 件を取り直す
    #[test]
    fn edit_labels_then_watch_until_applied() {
        let f = Fakes::default();
        let mut a = app(&f);
        let mut before = pr(1);
        before.labels = vec![Label {
            name: "bug".into(),
            ..Label::default()
        }];
        let mut after = pr(1);
        after.labels = vec![Label {
            name: "docs".into(),
            ..Label::default()
        }];
        a.state.mine.prs = vec![before.clone()];
        f.gh.pr
            .lock()
            .unwrap()
            .extend([Ok(Some(before)), Ok(Some(after))]);
        a.perform(vec![Effect::EditLabels {
            number: 1,
            add: vec!["docs".into()],
            remove: vec!["bug".into()],
        }]);
        // EditedLabels と, それが出した Watch の Reloaded
        a.pump_blocking(2);
        assert_eq!(
            f.gh.calls(),
            ["edit_labels 1 +docs -bug", "fetch_pr 1", "fetch_pr 1"]
        );
        assert_eq!(
            a.state.pr(1).map(|p| p.label_names()),
            Some(vec!["docs".to_string()])
        );
        assert_eq!(
            message(&a.state),
            ("#1 のラベルを変えた: +docs -bug", false)
        );
    }

    #[test]
    fn fetch_labels_fills_repo_labels() {
        let f = Fakes::default();
        let mut a = app(&f);
        *f.gh.labels.lock().unwrap() = Ok(vec![Label {
            name: "bug".into(),
            ..Label::default()
        }]);
        a.state.labels_loading = true;
        a.perform(vec![Effect::FetchLabels]);
        a.pump_blocking(1);
        assert!(!a.state.labels_loading);
        assert_eq!(a.state.repo_labels.len(), 1);
    }

    #[test]
    fn watch_stops_at_tries() {
        let f = Fakes::default();
        let mut a = App::with_ports(PathBuf::from("/tmp"), "o/r".into(), f.ports(), cfg(3));
        let mut draft = pr(1);
        draft.is_draft = true;
        a.state.mine.prs = vec![draft.clone()];
        f.gh.pr.lock().unwrap().push_back(Ok(Some(draft)));
        a.perform(vec![Effect::Watch {
            number: 1,
            until: WatchUntil::NotDraft,
        }]);
        a.pump_blocking(1);
        assert_eq!(f.gh.calls(), vec!["fetch_pr 1"; 3]);
        // 上限に達したらその時点の結果を出す
        assert_eq!(a.state.pr(1).map(|p| p.is_draft), Some(true));
        assert!(a.state.syncing.is_empty());
    }

    #[test]
    fn watch_stops_on_error() {
        let f = Fakes::default();
        let mut a = app(&f);
        a.state.mine.prs = vec![pr(1)];
        f.gh.pr.lock().unwrap().push_back(Err("boom".into()));
        a.perform(vec![Effect::Watch {
            number: 1,
            until: WatchUntil::Gone,
        }]);
        a.pump_blocking(1);
        assert_eq!(f.gh.calls(), vec!["fetch_pr 1"]);
        assert_eq!(message(&a.state), ("#1 の再取得失敗: boom", true));
        assert!(a.state.pr(1).is_some());
    }

    #[test]
    fn edit_body_then_refetches_conversation() {
        let f = Fakes::default();
        let mut a = app(&f);
        let mut view = crate::app::state::PrView::new(&pr(1));
        view.fetching = false;
        view.saving_body = true;
        a.state.pr_view = Some(view);
        a.perform(vec![Effect::EditBody {
            number: 1,
            original: "old".into(),
            body: "new".into(),
        }]);
        a.pump_blocking(2);
        assert_eq!(
            f.gh.calls(),
            vec![r#"edit_body 1 "old" "new""#, "fetch_conversation 1"]
        );
        let v = a.state.pr_view.as_ref().unwrap();
        assert!(!v.saving_body);
        assert_eq!(v.notice.as_deref(), Some("#1 の説明を更新した"));
    }

    #[test]
    fn copy_success_is_info() {
        let f = Fakes::default();
        let mut a = app(&f);
        a.perform(vec![Effect::Copy("b1".into())]);
        assert_eq!(*f.desktop.copied.lock().unwrap(), vec!["b1"]);
        assert_eq!(message(&a.state), ("コピーした: b1", false));
    }

    #[test]
    fn copy_failure_is_error() {
        let f = Fakes::default();
        let mut a = app(&f);
        *f.desktop.fail.lock().unwrap() = Some("pbcopy: boom".into());
        a.perform(vec![Effect::Copy("b1".into())]);
        assert!(f.desktop.copied.lock().unwrap().is_empty());
        assert_eq!(message(&a.state), ("コピー失敗: pbcopy: boom", true));
    }

    #[test]
    fn open_url_failure_is_error() {
        let f = Fakes::default();
        let mut a = app(&f);
        a.perform(vec![Effect::OpenUrl("https://example.com".into())]);
        assert_eq!(
            *f.desktop.opened.lock().unwrap(),
            vec!["https://example.com"]
        );
        assert!(a.state.message.is_none());
        *f.desktop.fail.lock().unwrap() = Some("open: boom".into());
        a.perform(vec![Effect::OpenUrl("https://example.com".into())]);
        assert_eq!(message(&a.state), ("open: boom", true));
    }

    #[test]
    fn save_cache_writes_snapshot() {
        let f = Fakes::default();
        *f.git.local.lock().unwrap() = tabs(&[("a", "q1"), ("b", "q2")]);
        let mut a = app(&f);
        a.state.mine.prs = vec![pr(1), pr(2)];
        a.state.mine.fetched_at = Some(at(100));
        a.state.custom[0].list.prs = vec![pr(7)];
        a.state.custom[0].list.fetched_at = Some(at(300));
        a.perform(vec![Effect::SaveCache]);
        let saved = f.cache.cache.lock().unwrap().clone().expect("saved");
        assert_eq!(json(&saved), json(&snapshot(&a.state)));
        assert_eq!(numbers(&saved.prs), vec![1, 2]);
        // 未取得のレビュータブと追加のタブは書かない
        assert!(saved.reviews.is_none());
        let queries: Vec<&str> = saved.custom.iter().map(|c| c.query.as_str()).collect();
        assert_eq!(queries, vec!["q1"]);
    }

    #[test]
    fn with_ports_hydrates_from_cache() {
        let f = Fakes::default();
        *f.git.local.lock().unwrap() = tabs(&[("a", "q1"), ("b", "q2")]);
        *f.cache.cache.lock().unwrap() = Some(Cache {
            fetched_at: at(100),
            prs: vec![pr(1)],
            devin_sessions: vec![],
            reviews: Some((at(200), vec![pr(3)])),
            custom: vec![
                CustomCache {
                    query: "q1".into(),
                    fetched_at: at(300),
                    prs: vec![pr(5)],
                },
                // 設定から消えたクエリは戻さない
                CustomCache {
                    query: "old".into(),
                    fetched_at: at(400),
                    prs: vec![pr(6)],
                },
            ],
        });
        let a = app(&f);
        let s = &a.state;
        assert_eq!(numbers(&s.mine.prs), vec![1]);
        assert_eq!(s.mine.fetched_at, Some(at(100)));
        assert!(s.mine.from_cache);
        assert_eq!(numbers(&s.review.prs), vec![3]);
        assert_eq!(s.review.fetched_at, Some(at(200)));
        assert!(s.review.from_cache);
        assert_eq!(numbers(&s.custom[0].list.prs), vec![5]);
        assert_eq!(s.custom[0].list.fetched_at, Some(at(300)));
        assert!(s.custom[0].list.from_cache);
        assert!(s.custom[1].list.prs.is_empty());
        assert!(s.custom[1].list.fetched_at.is_none());
        assert!(!s.custom[1].list.from_cache);
        // with_ports は取得を始めない
        assert!(f.gh.calls().is_empty());
        assert!(!s.mine.loading);
    }

    #[test]
    fn apply_cache_then_snapshot_round_trips() {
        let c = Cache {
            fetched_at: at(100),
            prs: vec![pr(1), pr(2)],
            devin_sessions: vec![],
            reviews: Some((at(200), vec![pr(3)])),
            custom: vec![CustomCache {
                query: "q1".into(),
                fetched_at: at(300),
                prs: vec![pr(5)],
            }],
        };
        let mut s = AppState::new(PathBuf::from("/tmp"), "o/r".into(), tabs(&[("a", "q1")]));
        apply_cache(&mut s, c.clone());
        assert_eq!(json(&snapshot(&s)), json(&c));
    }

    #[test]
    fn fetch_updates_mine() {
        let f = Fakes::default();
        let mut a = app(&f);
        a.state.mine.prs = vec![pr(1)];
        a.state.mine.loading = true;
        *f.gh.prs.lock().unwrap() = Ok(vec![pr(2), pr(3)]);
        f.git.local.lock().unwrap().current_branch = "b2".into();
        a.perform(vec![Effect::Fetch]);
        // LocalState は同期で読み直す
        assert_eq!(a.state.local.current_branch, "b2");
        a.pump_blocking(1);
        assert_eq!(f.gh.calls(), vec!["fetch_prs"]);
        assert!(!a.state.mine.loading);
        assert_eq!(numbers(&a.state.mine.prs), vec![2, 3]);
        let saved = f.cache.cache.lock().unwrap().clone().expect("saved");
        assert_eq!(numbers(&saved.prs), vec![2, 3]);
    }

    #[test]
    fn editor_command_uses_config_or_env() {
        assert_eq!(
            super::editor_command(None, "/tmp/a b.md"),
            [
                "sh",
                "-c",
                "${VISUAL:-${EDITOR:-vi}} \"$1\"",
                "sh",
                "/tmp/a b.md"
            ]
        );
        assert_eq!(
            super::editor_command(Some("code --wait"), "/tmp/a.md"),
            ["sh", "-c", "code --wait \"$1\"", "sh", "/tmp/a.md"]
        );
    }
}
