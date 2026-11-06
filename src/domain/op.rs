/// PR の状態を変える操作. `gh pr ready`, `gh pr close` で行う
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrOp {
    /// Draft を外してレビュー可能にする
    Open,
    Draft,
    Close,
}

impl PrOp {
    /// 実行するキー. Draft, Close は同じキーをもう一度押して確定する
    pub fn key(self) -> char {
        match self {
            PrOp::Open => 'O',
            PrOp::Draft => 'D',
            PrOp::Close => 'X',
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            PrOp::Open => "open",
            PrOp::Draft => "draft",
            PrOp::Close => "close",
        }
    }

    /// `#n を {phrase}する` の形でメッセージに埋める
    pub fn phrase(self) -> String {
        format!("{} に", self.label())
    }
}
