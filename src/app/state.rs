use std::collections::HashSet;
use std::path::PathBuf;
use std::time::Instant;

use chrono::{DateTime, Utc};
use ratatui::widgets::{ListState, TableState};

use crate::domain::copy::CopyItem;
use crate::domain::devin::{DevinMessage, DevinSession, session_for};
use crate::domain::label::{Label, label_choices};
use crate::domain::local::LocalState;
use crate::domain::op::PrOp;
use crate::domain::pr::Pr;
use crate::domain::review::{ReviewerChoice, reviewer_choices};

pub struct Message {
    pub text: String,
    pub is_error: bool,
}

/// Devin session との会話ビュー. session 本体は `AppState::devin_sessions` から引く
#[derive(Debug, Default)]
pub struct ChatView {
    pub session_id: String,
    /// 開いたときの PR 番号. 一覧の選択は PR が消えるとずれるので持っておく
    pub number: u64,
    pub messages: Vec<DevinMessage>,
    /// 1 度でもメッセージを取得できた
    pub loaded: bool,
    /// メッセージを取得中. ポーリングを 1 本に絞る
    pub fetching: bool,
    /// 最後にメッセージの取得を出した時刻. None なら次の tick で取る
    pub last_poll: Option<Instant>,
    /// 先頭からの表示位置. None なら末尾に追従する
    pub scroll: Option<usize>,
    /// 描画が書く. 末尾を表示するときの scroll
    pub max_scroll: usize,
    /// 描画が書く. メッセージ欄の高さ
    pub viewport_height: u16,
    /// 1 行入力中
    pub typing: bool,
    /// 書きかけのメッセージ. 送信が成功するまで消さない. 書き換えるときは `set_draft` で cursor も合わせる
    pub draft: String,
    /// draft の中の入力位置. 先頭からの文字数 (byte ではない)
    pub cursor: usize,
    pub sending: bool,
    /// 会話ビューのフッターに出すエラー
    pub error: Option<String>,
}

impl ChatView {
    pub fn new(session_id: String, number: u64) -> Self {
        Self {
            session_id,
            number,
            ..Default::default()
        }
    }

    /// draft を差し替え, 入力位置を末尾に置く
    pub fn set_draft(&mut self, draft: String) {
        self.cursor = draft.chars().count();
        self.draft = draft;
    }
}

/// `a` で開くラベルの選択画面. 自分タブの PR だけが対象
#[derive(Debug, Default)]
pub struct LabelMenu {
    pub number: u64,
    /// 開いたときに PR に付いていたラベル. 並びと差分の基準
    pub before: Vec<String>,
    /// 付いた状態にするラベル. Tab で切り替える
    pub chosen: Vec<String>,
    /// 絞り込みの入力
    pub query: String,
    /// 絞り込んだ選択肢の中の位置. スクロール位置は描画が書く
    pub list: ListState,
    /// ラベル一覧の取得失敗
    pub error: Option<String>,
}

impl LabelMenu {
    pub fn new(number: u64, labels: Vec<String>) -> Self {
        Self {
            number,
            chosen: labels.clone(),
            before: labels,
            list: ListState::default().with_selected(Some(0)),
            ..Default::default()
        }
    }
}

/// `R` で開く review 依頼の選択画面. 自分タブの Draft でない PR だけが対象
#[derive(Debug, Default)]
pub struct ReviewerMenu {
    pub number: u64,
    /// 依頼する login. Tab で切り替え, 組を選ぶとメンバーをまとめて足す
    pub chosen: Vec<String>,
    /// 絞り込みの入力
    pub query: String,
    /// 絞り込んだ選択肢の中の位置. スクロール位置は描画が書く
    pub list: ListState,
}

impl ReviewerMenu {
    pub fn new(number: u64) -> Self {
        Self {
            number,
            list: ListState::default().with_selected(Some(0)),
            ..Default::default()
        }
    }
}

/// 一覧のタブ
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tab {
    /// 自分が author か assignee の PR
    #[default]
    Mine,
    /// 自分にレビューが依頼されているか, 自分がレビューした PR
    Review,
    /// git config `prtui.tab` で足したタブ. `AppState::custom` の位置
    Custom(usize),
}

impl Tab {
    /// 他人の PR が混ざるタブ. PR を変える操作 (u, O, DD, XX, R, a) は自分タブでしか使わない
    pub fn is_readonly(self) -> bool {
        self != Tab::Mine
    }
}

/// git config `prtui.tab` で足したタブ. 設定は起動時に読んだものを使う
pub struct CustomTab {
    pub name: String,
    pub query: String,
    pub list: PrList,
}

/// タブ 1 つ分の一覧. 取得, 選択位置, 絞り込みはタブごとに持つ
#[derive(Default)]
pub struct PrList {
    pub prs: Vec<Pr>,
    pub table: TableState,
    pub draft_only: bool,
    pub loading: bool,
    /// `prs` の全件を取得した時刻. キャッシュから読んだときはキャッシュを書いた時刻
    pub fetched_at: Option<DateTime<Utc>>,
    /// `prs` がキャッシュのままで, 起動後の取得がまだ終わっていない
    pub from_cache: bool,
    /// 直近の取得の失敗. このタブの一覧の上に出す
    pub error: Option<String>,
}

impl PrList {
    fn new() -> Self {
        Self {
            table: TableState::default().with_selected(0),
            ..Default::default()
        }
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

    /// 一覧を差し替える. 並びが変わっても同じ PR を選んだままにする
    pub fn replace(&mut self, prs: Vec<Pr>) {
        let sel = self.selected().map(|p| p.number);
        self.prs = prs;
        if let Some(i) = sel.and_then(|n| self.visible().iter().position(|p| p.number == n)) {
            self.table.select(Some(i));
        }
        self.clamp_selection();
    }

    /// 端を越えたら反対の端に回る
    pub fn move_by(&mut self, delta: isize) {
        let len = self.visible().len();
        if len == 0 {
            return;
        }
        let cur = self.table.selected().unwrap_or(0) as isize;
        self.table
            .select(Some((cur + delta).rem_euclid(len as isize) as usize));
    }

    pub fn clamp_selection(&mut self) {
        let len = self.visible().len();
        let cur = self.table.selected().unwrap_or(0);
        self.table.select(Some(cur.min(len.saturating_sub(1))));
    }
}

pub struct AppState {
    pub dir: PathBuf,
    pub repo: String,
    pub tab: Tab,
    /// 自分タブ. 操作 (u, O, DD, XX, R, a) と 1 件の取り直しはこちらだけが対象
    pub mine: PrList,
    pub review: PrList,
    /// 追加のタブ. 自分タブとレビュータブの後ろに並べる
    pub custom: Vec<CustomTab>,
    pub local: LocalState,
    pub message: Option<Message>,
    pub busy: Option<String>,
    /// base 取り込みの確認待ちの PR 番号と base ブランチ名
    pub confirm_update: Option<(u64, String)>,
    /// ローカルブランチが PR と分岐してチェックアウトできなかった PR 番号とブランチ名.
    /// y で PR の最新に reset する
    pub confirm_force_checkout: Option<(u64, String)>,
    /// 2 回目のキー入力待ちの状態変更 (draft, close)
    pub confirm_op: Option<(u64, PrOp)>,
    /// `c` で開いたコピー選択画面の項目. 開いたときの PR から作る
    pub copy_menu: Option<Vec<CopyItem>>,
    /// `a` で開いたラベルの選択画面
    pub label_menu: Option<LabelMenu>,
    /// `R` で開いた review 依頼の選択画面
    pub reviewer_menu: Option<ReviewerMenu>,
    /// repo のラベル. 選択画面を開くたびに取り直す
    pub repo_labels: Vec<Label>,
    pub labels_loading: bool,
    pub last_fetch: Option<Instant>,
    /// 操作の反映を待って取り直している PR 番号
    pub syncing: HashSet<u64>,
    /// repo の Devin session. Devin を設定していなければ空
    pub devin_sessions: Vec<DevinSession>,
    /// Devin session 一覧を取得中. PR の取得 (`loading`) とは独立
    pub sessions_loading: bool,
    /// 会話ビューを開いていれば Some
    pub chat: Option<ChatView>,
}

impl AppState {
    pub fn new(dir: PathBuf, repo: String, local: LocalState) -> Self {
        Self {
            dir,
            repo,
            tab: Tab::Mine,
            mine: PrList::new(),
            review: PrList::new(),
            custom: local
                .tabs
                .iter()
                .map(|t| CustomTab {
                    name: t.name.clone(),
                    query: t.query.clone(),
                    list: PrList::new(),
                })
                .collect(),
            local,
            message: None,
            busy: None,
            confirm_update: None,
            confirm_force_checkout: None,
            confirm_op: None,
            copy_menu: None,
            label_menu: None,
            reviewer_menu: None,
            repo_labels: Vec::new(),
            labels_loading: false,
            last_fetch: None,
            syncing: HashSet::new(),
            devin_sessions: Vec::new(),
            sessions_loading: false,
            chat: None,
        }
    }

    pub fn list(&self) -> &PrList {
        self.list_of(self.tab)
    }

    pub fn list_mut(&mut self) -> &mut PrList {
        match self.tab {
            Tab::Mine => &mut self.mine,
            Tab::Review => &mut self.review,
            Tab::Custom(i) => &mut self.custom[i].list,
        }
    }

    pub fn list_of(&self, tab: Tab) -> &PrList {
        match tab {
            Tab::Mine => &self.mine,
            Tab::Review => &self.review,
            Tab::Custom(i) => &self.custom[i].list,
        }
    }

    /// 並び順のタブ. 自分, レビュー, 追加のタブの順
    pub fn tabs(&self) -> Vec<Tab> {
        [Tab::Mine, Tab::Review]
            .into_iter()
            .chain((0..self.custom.len()).map(Tab::Custom))
            .collect()
    }

    pub fn tab_label(&self, tab: Tab) -> &str {
        match tab {
            Tab::Mine => "自分",
            Tab::Review => "レビュー",
            Tab::Custom(i) => &self.custom[i].name,
        }
    }

    pub fn visible(&self) -> Vec<&Pr> {
        self.list().visible()
    }

    /// 今のタブで選択中の PR
    pub fn selected(&self) -> Option<&Pr> {
        self.list().selected()
    }

    /// 自分タブの PR. 操作の対象を番号で引く
    pub fn pr(&self, n: u64) -> Option<&Pr> {
        self.mine.prs.iter().find(|p| p.number == n)
    }

    /// ラベルの選択画面に並べる選択肢. 開いていなければ空
    pub fn label_choices(&self) -> Vec<Label> {
        self.label_menu
            .as_ref()
            .map(|m| label_choices(&self.repo_labels, &m.before, &m.query))
            .unwrap_or_default()
    }

    /// review 依頼の選択画面に並べる選択肢. 開いていないか, PR が一覧から消えていれば空
    pub fn reviewer_choices(&self) -> Vec<ReviewerChoice> {
        let Some(menu) = &self.reviewer_menu else {
            return vec![];
        };
        self.pr(menu.number)
            .map(|p| {
                reviewer_choices(
                    p,
                    &self.local.reviewers,
                    &self.local.reviewer_sets,
                    &menu.query,
                )
            })
            .unwrap_or_default()
    }

    /// 会話ビューで開いている session
    pub fn chat_session(&self) -> Option<&DevinSession> {
        let id = &self.chat.as_ref()?.session_id;
        self.devin_sessions.iter().find(|s| &s.id == id)
    }

    /// PR に紐づく最新の Devin session と件数. Devin の設定が無ければ, キャッシュに
    /// session が残っていても None
    pub fn devin_for(&self, pr: &Pr) -> Option<(&DevinSession, usize)> {
        self.local.devin.as_ref()?;
        session_for(&self.devin_sessions, &pr.url)
    }

    pub fn info(&mut self, text: String) {
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
