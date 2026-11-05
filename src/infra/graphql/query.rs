//! `gh api graphql` に渡すクエリ. 本文は同じディレクトリの `.graphql` に置き, 使う fragment を後ろに連結した
//! 文書をここで組み立てる

/// 自分タブ. author と assignee を 2 つの alias で同時に取る
pub(crate) const QUERY: &str = concat!(
    include_str!("prs.graphql"),
    include_str!("fragment_pr.graphql")
);

/// レビュータブ. 依頼中とレビュー済みを 2 つの alias で取り, 自分の立場を出すため viewer も取る.
/// 依頼中側は依頼を受けた時刻を出すため `fragment_requested.graphql` も取る.
/// チーム経由の依頼を自分の所属チームに絞るため, repo の owner が組織なら自分の所属チームも取る.
/// `role` は viewer の立場での絞り込みで, maintainer の扱いが不明なので MEMBER と ADMIN を両方取る
pub(crate) const QUERY_REVIEW: &str = concat!(
    include_str!("review.graphql"),
    include_str!("fragment_pr.graphql"),
    include_str!("fragment_requested.graphql")
);

/// 追加のタブ. 検索クエリはタブの設定から組み立てる
pub(crate) const QUERY_CUSTOM: &str = concat!(
    include_str!("custom.graphql"),
    include_str!("fragment_pr.graphql")
);

/// 1 件だけ取り直すときのクエリ. close 済みかを見るため state も取る
pub(crate) const QUERY_ONE: &str = concat!(
    include_str!("one.graphql"),
    include_str!("fragment_pr.graphql")
);
