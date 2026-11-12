use anyhow::Result;

use crate::domain::conversation::{Conversation, ReviewAction};
use crate::domain::devin::{DevinMessage, DevinSession};
use crate::domain::diff::Diff;
use crate::domain::label::{Label, same_labels};
use crate::domain::merge::{MergeMethod, MergeOutcome, MergeSettings};
use crate::domain::op::PrOp;
use crate::domain::pr::Pr;
use crate::domain::review::review_targets;

/// スレッドで実行した処理の結果
pub enum Event {
    Fetched(Result<Vec<Pr>>),
    /// レビュータブの全件
    FetchedReviews(Result<Vec<Pr>>),
    /// 追加のタブ (`AppState::custom` の位置) の全件
    FetchedCustom(usize, Result<Vec<Pr>>),
    /// PR 番号, `--force` を付けたか, 結果
    CheckedOut(u64, bool, Result<String>),
    /// 成功時は取り込んだ base ブランチ名
    UpdatedBranch(u64, Result<String>),
    /// Linear review の URL. コメントに見つからなければ None
    LinearReview(u64, Result<Option<String>>),
    ChangedState(u64, PrOp, Result<String>),
    /// repo で許可されたマージ方法
    FetchedMergeSettings(Result<MergeSettings>),
    Merged {
        number: u64,
        method: MergeMethod,
        res: Result<MergeOutcome>,
    },
    /// 1 件だけ取り直した結果. open でなくなっていれば None
    Reloaded(u64, Box<Result<Option<Pr>>>),
    /// repo の Devin session 一覧
    DevinSessions(Result<Vec<DevinSession>>),
    /// session ID とそのメッセージ
    DevinMessages(String, Result<Vec<DevinMessage>>),
    /// 送信した session ID と, 送信後の session
    DevinSent(String, Result<DevinSession>),
    /// エディタで書いた内容. エディタが非 0 で終わったら None.
    /// PR ビューで書いていればレビューか説明の更新に, そうでなければ会話ビューの送信に使う
    Composed(Result<Option<String>>),
    /// repo のラベル一覧
    FetchedLabels(Result<Vec<Label>>),
    /// PR ビューの説明とコメント
    FetchedConversation(u64, Result<Conversation>),
    /// PR ビューの diff
    FetchedDiff(u64, Result<Diff>),
    /// PR ビューからスレッドを解決 (resolve が false なら未解決に戻) した結果
    ResolvedThread {
        number: u64,
        id: String,
        resolve: bool,
        res: Result<()>,
    },
    /// PR ビューから送ったレビューの結果
    SubmittedReview {
        number: u64,
        action: ReviewAction,
        res: Result<String>,
    },
    /// PR ビューから説明を更新した結果
    EditedBody {
        number: u64,
        res: Result<String>,
    },
    /// review を依頼した結果. targets は依頼した相手
    RequestedReviews {
        number: u64,
        targets: Vec<String>,
        res: Result<String>,
    },
    /// PR のラベルを付け外した結果. add と remove は依頼した差分
    EditedLabels {
        number: u64,
        add: Vec<String>,
        remove: Vec<String>,
        res: Result<String>,
    },
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
    /// 追加のタブ (`AppState::custom` の位置) の全件取得をスレッドで行う. タブごとに独立
    FetchCustom(usize),
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
    },
    /// repo で許可されたマージ方法をスレッドで取る
    FetchMergeSettings,
    /// PR のマージを頼み, 終わるまで待つ. head が head_oid から動いていれば GitHub がマージしない
    Merge {
        number: u64,
        method: MergeMethod,
        head_oid: String,
    },
    /// targets に review を依頼する. レビュー済みの人には再依頼になる
    RequestReviews {
        number: u64,
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
    /// repo のラベル一覧をスレッドで取る
    FetchLabels,
    /// PR の説明とコメントをスレッドで取る
    FetchConversation(u64),
    /// PR の diff をスレッドで取る
    FetchDiff(u64),
    /// 行へのスレッドを解決する. resolve が false なら未解決に戻す. number は結果を返す PR ビューの PR
    ResolveThread {
        number: u64,
        id: String,
        resolve: bool,
    },
    /// PR にレビュー (承認かコメント) を送る
    SubmitReview {
        number: u64,
        action: ReviewAction,
        body: String,
    },
    /// PR の説明を body に書き換える. GitHub 側の説明が original から変わっていれば書き換えない
    EditBody {
        number: u64,
        original: String,
        body: String,
    },
    /// PR のラベルを付け外す
    EditLabels {
        number: u64,
        add: Vec<String>,
        remove: Vec<String>,
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
    /// ラベルが指定の集合になった (順番と大文字小文字は見ない)
    Labels(Vec<String>),
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
            WatchUntil::Labels(labels) => pr.is_none_or(|p| same_labels(&p.label_names(), labels)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::pr::{Reviewer, ReviewerState};

    fn pr() -> Pr {
        let mut p = Pr::fixture(1, "b1", "main");
        p.head_ref_oid = "oid1".into();
        p
    }

    #[test]
    fn watch_until_is_done() {
        let open = pr();
        let mut draft = pr();
        draft.is_draft = true;
        let mut requested = pr();
        requested.reviewers = vec![Reviewer {
            name: "Alice".into(),
            state: ReviewerState::Requested,
        }];

        // None (open でなくなった) はすべて止める
        for until in [
            WatchUntil::Immediately,
            WatchUntil::HeadMovedFrom(Some("oid1".into())),
            WatchUntil::NotDraft,
            WatchUntil::IsDraft,
            WatchUntil::Gone,
            WatchUntil::Requested(vec!["alice".into()]),
            WatchUntil::Labels(vec!["bug".into()]),
        ] {
            assert!(until.is_done(None), "{until:?}");
        }

        assert!(WatchUntil::Immediately.is_done(Some(&open)));

        assert!(!WatchUntil::HeadMovedFrom(Some("oid1".into())).is_done(Some(&open)));
        assert!(WatchUntil::HeadMovedFrom(Some("old".into())).is_done(Some(&open)));
        assert!(WatchUntil::HeadMovedFrom(None).is_done(Some(&open)));

        assert!(WatchUntil::NotDraft.is_done(Some(&open)));
        assert!(!WatchUntil::NotDraft.is_done(Some(&draft)));

        assert!(WatchUntil::IsDraft.is_done(Some(&draft)));
        assert!(!WatchUntil::IsDraft.is_done(Some(&open)));

        assert!(!WatchUntil::Gone.is_done(Some(&open)));

        let alice = WatchUntil::Requested(vec!["alice".into()]);
        assert!(alice.is_done(Some(&requested)));
        assert!(!alice.is_done(Some(&open)));

        let mut labeled = pr();
        labeled.labels = ["Bug", "docs"]
            .map(|name| crate::domain::label::Label {
                name: name.into(),
                ..Default::default()
            })
            .to_vec();
        let labels = WatchUntil::Labels(vec!["docs".into(), "bug".into()]);
        assert!(labels.is_done(Some(&labeled)));
        assert!(!labels.is_done(Some(&open)));
        assert!(WatchUntil::Labels(vec![]).is_done(Some(&open)));
    }
}
