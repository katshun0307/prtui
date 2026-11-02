use anyhow::Result;

use crate::domain::devin::{DevinMessage, DevinSession};
use crate::domain::op::PrOp;
use crate::domain::pr::Pr;
use crate::domain::review::review_targets;

/// スレッドで実行した処理の結果
pub enum Event {
    Fetched(Result<Vec<Pr>>),
    /// レビュータブの全件
    FetchedReviews(Result<Vec<Pr>>),
    /// PR 番号, `--force` を付けたか, 結果
    CheckedOut(u64, bool, Result<String>),
    /// 成功時は取り込んだ base ブランチ名
    UpdatedBranch(u64, Result<String>),
    /// Linear review の URL. コメントに見つからなければ None
    LinearReview(u64, Result<Option<String>>),
    ChangedState(u64, PrOp, Result<String>),
    /// 1 件だけ取り直した結果. open でなくなっていれば None
    Reloaded(u64, Box<Result<Option<Pr>>>),
    /// repo の Devin session 一覧
    DevinSessions(Result<Vec<DevinSession>>),
    /// session ID とそのメッセージ
    DevinMessages(String, Result<Vec<DevinMessage>>),
    /// 送信した session ID と, 送信後の session
    DevinSent(String, Result<DevinSession>),
    /// エディタで書いた内容. エディタが非 0 で終わったら None
    Composed(Result<Option<String>>),
}

/// main への指示
pub enum Action {
    None,
    Quit,
    /// TUI を一時停止して外部コマンドを端末に繋いで実行する
    Suspend(Vec<String>),
}

/// update が runtime に実行を求める副作用
#[derive(Debug, PartialEq)]
pub enum Effect {
    Quit,
    Suspend(Vec<String>),
    /// 全件取得をスレッドで行い, LocalState を同期で読み直す
    Fetch,
    /// レビュータブの全件取得をスレッドで行う. 自分タブの取得とは独立
    FetchReviews,
    /// LocalState を同期で読み直す
    ReloadLocal,
    /// 現在の一覧をキャッシュに書く
    SaveCache,
    /// 未コミットの変更が無ければスレッドで `gh pr checkout` する.
    /// force ならローカルの同名ブランチを PR の最新に reset する
    Checkout {
        number: u64,
        force: bool,
    },
    UpdateBranch {
        number: u64,
        base: String,
        head_oid: String,
    },
    ChangeState {
        number: u64,
        op: PrOp,
        targets: Vec<String>,
    },
    LinearReview(u64),
    /// 操作が GitHub に反映されるまで PR を 1 件だけ取り直す
    Watch {
        number: u64,
        until: WatchUntil,
    },
    OpenUrl(String),
    /// クリップボードに入れる
    Copy(String),
    ViewWeb(u64),
    /// Devin session 一覧をスレッドで取る. 対象期間は runtime が決める
    FetchSessions,
    /// session ID のメッセージをスレッドで取る
    FetchMessages(String),
    SendMessage {
        session_id: String,
        text: String,
    },
    /// draft を入れた一時ファイルをエディタで開く. 結果は `Event::Composed` で返る
    Compose {
        draft: String,
    },
}

/// watch を止める条件. 取り直した PR (open でなくなっていれば None) で判定する
#[derive(Debug, Clone, PartialEq)]
pub enum WatchUntil {
    /// 1 回取り直すだけ
    Immediately,
    /// head が指定の oid から動いた
    HeadMovedFrom(Option<String>),
    NotDraft,
    IsDraft,
    /// open でなくなった
    Gone,
    /// 指定の相手が全員依頼中になった
    Requested(Vec<String>),
}

impl WatchUntil {
    pub fn is_done(&self, pr: Option<&Pr>) -> bool {
        match self {
            WatchUntil::Immediately => true,
            WatchUntil::HeadMovedFrom(old) => {
                pr.is_none_or(|p| Some(&p.head_ref_oid) != old.as_ref())
            }
            WatchUntil::NotDraft => pr.is_none_or(|p| !p.is_draft),
            WatchUntil::IsDraft => pr.is_none_or(|p| p.is_draft),
            WatchUntil::Gone => pr.is_none(),
            WatchUntil::Requested(reviewers) => {
                pr.is_none_or(|p| review_targets(p, reviewers).is_empty())
            }
        }
    }
}
