/// PR の状態を変える操作. `gh pr ready`, `gh pr close`, `gh pr edit` で行う
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrOp {
    /// Draft を外してレビュー可能にする
    Open,
    Draft,
    Close,
    /// git config `prtui.reviewers` の相手に review を依頼する. Draft でない PR に限る
    Review,
}

impl PrOp {
    /// 実行するキー. Draft, Close, Review は同じキーをもう一度押して確定する
    pub fn key(self) -> char {
        match self {
            PrOp::Open => 'O',
            PrOp::Draft => 'D',
            PrOp::Close => 'X',
            PrOp::Review => 'R',
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            PrOp::Open => "open",
            PrOp::Draft => "draft",
            PrOp::Close => "close",
            PrOp::Review => "review 依頼",
        }
    }

    /// `#n を {phrase}する` の形でメッセージに埋める. reviewers は review 依頼の相手
    pub fn phrase(self, reviewers: &[String]) -> String {
        match self {
            PrOp::Review => format!("{} に review 依頼", reviewers.join(", ")),
            _ => format!("{} に", self.label()),
        }
    }
}
