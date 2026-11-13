use std::collections::HashSet;
use std::path::PathBuf;
use std::time::Instant;

use chrono::{DateTime, Utc};
use ratatui::widgets::{ListState, TableState};

use crate::domain::conversation::{Conversation, ReviewAction};
use crate::domain::copy::CopyItem;
use crate::domain::devin::{DevinMessage, DevinSession, session_for};
use crate::domain::diff::Diff;
use crate::domain::label::{Label, label_choices};
use crate::domain::local::LocalState;
use crate::domain::merge::MergeSettings;
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

/// PR ビューでエディタに書かせているもの
#[derive(Debug, Clone, PartialEq)]
pub enum Composing {
    Review(ReviewAction),
    /// 説明の書き換え. original はエディタを開いたときの説明で, 変更と GitHub 側の更新の判定に使う
    Body {
        original: String,
    },
}

/// PR ビューのページ. `d` で切り替える
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PrPage {
    #[default]
    Conversation,
    Diff,
}

/// PR ビューの diff のページ. 初めて開いたときに取る
#[derive(Debug, Default)]
pub struct DiffPage {
    /// 取り直している間も前の内容を出す
    pub diff: Option<Diff>,
    pub fetching: bool,
    /// 先頭からの表示位置
    pub scroll: usize,
    /// 描画が書く. 末尾を表示するときの scroll
    pub max_scroll: usize,
    /// 行頭からの表示位置. 長い行は折り返さず横に送って読む
    pub x: usize,
    /// 描画が書く. 表示中の行の右端を出すときの x
    pub max_x: usize,
}

/// `p` で開く PR ビュー. PR の説明とコメントを全画面に出す. どのタブの PR でも開ける
#[derive(Debug, Default)]
pub struct PrView {
    pub number: u64,
    /// 以下はヘッダに出す, 開いたときの PR の値. 自分タブ以外の PR は `AppState::pr` で引けないので持っておく
    pub title: String,
    pub author: String,
    pub head: String,
    pub base: String,
    pub is_draft: bool,
    /// 取得した説明とコメント. 取り直している間も前の内容を出す
    pub conversation: Option<Conversation>,
    pub fetching: bool,
    /// PR ビューのフッターに出すエラー
    pub error: Option<String>,
    /// PR ビューのフッターに出す操作の結果. 次のキーで消す
    pub notice: Option<String>,
    /// `!` の後, 承認かコメントかのキー待ち
    pub choosing_review: bool,
    /// エディタで書いている. 戻った `Event::Composed` をレビューか説明の更新に使う
    pub composing: Option<Composing>,
    /// レビューを送信中
    pub submitting: bool,
    /// 説明を更新中
    pub saving_body: bool,
    /// `e` で書いた説明. 更新に失敗したら残し, 次の `e` でこれをエディタに渡す
    pub body_draft: Option<String>,
    /// 先頭からの表示位置
    pub scroll: usize,
    /// 描画が書く. 末尾を表示するときの scroll
    pub max_scroll: usize,
    /// 描画が書く. 本文の欄の高さ. 2 つのページで共通
    pub viewport_height: u16,
    /// `]`/`[` で選んだスレッドの ID. `x` で解決する
    pub thread: Option<String>,
    /// 描画が書く. 説明とコメントのページでの各スレッドの見出しの位置 (折り返した後の行).
    /// `Conversation::threads` と同じ順
    pub thread_rows: Vec<usize>,
    /// スレッドを解決中
    pub resolving: bool,
    /// 表示中のページ
    pub page: PrPage,
    pub diff: DiffPage,
}

impl PrView {
    /// レビューの送信, 説明の更新, スレッドの解決の最中. 閉じたり重ねたりしない
    pub fn is_sending(&self) -> bool {
        self.submitting || self.saving_body || self.resolving
    }

    pub fn new(pr: &Pr) -> Self {
        Self {
            number: pr.number,
            title: pr.title.clone(),
            author: pr.author.clone(),
            head: pr.head_ref_name.clone(),
            base: pr.base_ref_name.clone(),
            is_draft: pr.is_draft,
            fetching: true,
            ..Default::default()
        }
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

/// `M` で開くマージの確認画面. 自分タブの Draft でもコンフリクトしてもいない PR だけが対象
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeMenu {
    pub number: u64,
    /// 開いたときの head. 確認画面で見た後に push された変更をマージしないよう, マージ API の `sha` に渡す
    pub head_oid: String,
}

/// 一覧の上に重ねて開く画面. `AppState::screens` に開いた順に積む.
/// `Esc` は一番上を閉じて, その下の画面 (無ければ一覧) に戻る.
/// 開くのは高々数個なので, 大きさの違う variant を Box に入れずに持つ
#[derive(Debug)]
#[allow(clippy::large_enum_variant)]
pub enum Screen {
    /// `p` で開く PR ビュー. 全画面
    PrView(PrView),
    /// `v` で開く Devin session の会話ビュー. 全画面
    Chat(ChatView),
    /// `c` で開くコピー選択画面の項目. 開いたときの PR から作る. ポップアップ
    Copy(Vec<CopyItem>),
    /// `a` で開くラベルの選択画面. ポップアップ
    Labels(LabelMenu),
    /// `R` で開く review 依頼の選択画面. ポップアップ
    Reviewers(ReviewerMenu),
    /// `M` で開くマージの確認画面. ポップアップ
    Merge(MergeMenu),
}

impl Screen {
    /// 下の画面を隠す全画面. ポップアップは下の画面の上に重ねて描く
    pub fn is_full(&self) -> bool {
        matches!(self, Screen::PrView(_) | Screen::Chat(_))
    }
}

/// `Screen` の種類ごとに, 積みの中で一番上にあるものを引く `$get` / `$get_mut` と,
/// 一番上がその種類のときだけ閉じて中身を返す `$pop`, `push` に渡すための `From` を作る
macro_rules! screen_accessors {
    ($($variant:ident: $ty:ty => $get:ident, $get_mut:ident, $pop:ident;)*) => {
        $(
            impl From<$ty> for Screen {
                fn from(v: $ty) -> Self {
                    Screen::$variant(v)
                }
            }
        )*

        impl AppState {
            $(
                #[allow(dead_code)]
                pub fn $get(&self) -> Option<&$ty> {
                    self.screens.iter().rev().find_map(|s| match s {
                        Screen::$variant(v) => Some(v),
                        _ => None,
                    })
                }

                #[allow(dead_code)]
                pub fn $get_mut(&mut self) -> Option<&mut $ty> {
                    self.screens.iter_mut().rev().find_map(|s| match s {
                        Screen::$variant(v) => Some(v),
                        _ => None,
                    })
                }

                /// 一番上が別の画面なら何もしない. 積みの途中を抜くと, その上に開いた画面が浮くため
                #[allow(dead_code)]
                pub fn $pop(&mut self) -> Option<$ty> {
                    match self.screens.pop() {
                        Some(Screen::$variant(v)) => Some(v),
                        other => {
                            self.screens.extend(other);
                            None
                        }
                    }
                }
            )*
        }
    };
}

screen_accessors! {
    PrView: PrView => pr_view, pr_view_mut, pop_pr_view;
    Chat: ChatView => chat, chat_mut, pop_chat;
    Copy: Vec<CopyItem> => copy_menu, copy_menu_mut, pop_copy_menu;
    Labels: LabelMenu => label_menu, label_menu_mut, pop_label_menu;
    Reviewers: ReviewerMenu => reviewer_menu, reviewer_menu_mut, pop_reviewer_menu;
    Merge: MergeMenu => merge_menu, merge_menu_mut, pop_merge_menu;
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
    /// 他人の PR が混ざるタブ. PR を変える操作 (u, O, DD, XX, R, a, M) は自分タブでしか使わない
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
    /// 自分タブ. 操作 (u, O, DD, XX, R, a, M) と 1 件の取り直しはこちらだけが対象
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
    /// 一覧の上に開いた画面. 後ろほど上. 空なら一覧を出す.
    /// 種類ごとの `pr_view()` や `label_menu()` は一番上にあるものを引く
    pub screens: Vec<Screen>,
    /// repo のラベル. 選択画面を開くたびに取り直す
    pub repo_labels: Vec<Label>,
    pub labels_loading: bool,
    /// repo で許可されたマージ方法. 確認画面を初めて開いたときに取り, 以後は取り直さない
    pub merge_settings: Option<MergeSettings>,
    pub merge_settings_loading: bool,
    pub last_fetch: Option<Instant>,
    /// 操作の反映を待って取り直している PR 番号
    pub syncing: HashSet<u64>,
    /// repo の Devin session. Devin を設定していなければ空
    pub devin_sessions: Vec<DevinSession>,
    /// Devin session 一覧を取得中. PR の取得 (`loading`) とは独立
    pub sessions_loading: bool,
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
            screens: Vec::new(),
            repo_labels: Vec::new(),
            labels_loading: false,
            merge_settings: None,
            merge_settings_loading: false,
            last_fetch: None,
            syncing: HashSet::new(),
            devin_sessions: Vec::new(),
            sessions_loading: false,
        }
    }

    /// 画面を一番上に開く
    pub fn push(&mut self, screen: impl Into<Screen>) {
        self.screens.push(screen.into());
    }

    /// 一番上の画面を閉じて, その下の画面に戻る
    pub fn pop(&mut self) -> Option<Screen> {
        self.screens.pop()
    }

    /// 一番上の画面. 一覧なら None
    pub fn top(&self) -> Option<&Screen> {
        self.screens.last()
    }

    /// 積みの中の PR #n の PR ビュー. スレッドから届いた結果を, 一番上でなくても, 下に隠れていても反映する
    pub fn pr_view_for(&mut self, n: u64) -> Option<&mut PrView> {
        self.screens.iter_mut().rev().find_map(|s| match s {
            Screen::PrView(v) if v.number == n => Some(v),
            _ => None,
        })
    }

    /// 積みの中の session `id` の会話ビュー. `pr_view_for` と同じく下に隠れていても引く
    pub fn chat_for(&mut self, id: &str) -> Option<&mut ChatView> {
        self.screens.iter_mut().rev().find_map(|s| match s {
            Screen::Chat(c) if c.session_id == id => Some(c),
            _ => None,
        })
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
        self.label_menu()
            .map(|m| label_choices(&self.repo_labels, &m.before, &m.query))
            .unwrap_or_default()
    }

    /// review 依頼の選択画面に並べる選択肢. 開いていないか, PR が一覧から消えていれば空
    pub fn reviewer_choices(&self) -> Vec<ReviewerChoice> {
        self.reviewer_menu()
            .map(|m| self.reviewer_choices_for(m))
            .unwrap_or_default()
    }

    /// `menu` に並べる選択肢. 開く前に選択肢があるかを確かめるのにも使う
    pub fn reviewer_choices_for(&self, menu: &ReviewerMenu) -> Vec<ReviewerChoice> {
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
        let id = &self.chat()?.session_id;
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
