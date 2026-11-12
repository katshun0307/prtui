//! `gh api graphql` に渡すクエリ. 本文は同じディレクトリの `.graphql` に置き, 使う fragment を後ろに連結した
//! 文書をここで組み立てる. `schema.graphql` (GitHub の公開 schema) に照らした検証はテストで行う

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

/// PR ビューの説明とコメント. 一覧の fragment は使わない.
/// 書き込みは新しい側の 100 件ずつを取り, 取らなかった分は `totalCount` から数える
pub(crate) const QUERY_CONVERSATION: &str = include_str!("conversation.graphql");

/// PR ビューの `x`. 行へのスレッドを解決する. 変数は `id` (スレッドの node ID)
pub(crate) const MUTATION_RESOLVE: &str = include_str!("resolve_thread.graphql");

/// 解決済みのスレッドを未解決に戻す
pub(crate) const MUTATION_UNRESOLVE: &str = include_str!("unresolve_thread.graphql");

#[cfg(test)]
mod tests {
    use apollo_compiler::{ExecutableDocument, Schema};

    use super::*;

    /// `task schema` で取り直した GitHub の公開 schema
    const SCHEMA: &str = include_str!("schema.graphql");

    /// 送る文書がすべて schema に照らして正しい (フィールド名, 引数, 変数の型, fragment の型条件)
    #[test]
    fn queries_are_valid_against_schema() {
        let schema = Schema::parse_and_validate(SCHEMA, "schema.graphql").unwrap();
        for (name, doc) in [
            ("QUERY", QUERY),
            ("QUERY_REVIEW", QUERY_REVIEW),
            ("QUERY_CUSTOM", QUERY_CUSTOM),
            ("QUERY_ONE", QUERY_ONE),
            ("QUERY_CONVERSATION", QUERY_CONVERSATION),
            ("MUTATION_RESOLVE", MUTATION_RESOLVE),
            ("MUTATION_UNRESOLVE", MUTATION_UNRESOLVE),
        ] {
            if let Err(e) = ExecutableDocument::parse_and_validate(&schema, doc, name) {
                panic!("{name}:\n{}", e.errors);
            }
        }
    }
}
