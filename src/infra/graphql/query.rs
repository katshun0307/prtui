//! `gh api graphql` に渡すクエリ文字列. fragment はクエリの後ろに連結して送る

pub(crate) const QUERY: &str = r#"query($author:String!, $assignee:String!) {
  author: search(query:$author, type:ISSUE, first:100) { nodes { ...pr } }
  assignee: search(query:$assignee, type:ISSUE, first:100) { nodes { ...pr } }
}"#;

/// レビュータブ. 依頼中とレビュー済みを 2 つの alias で取り, 自分の立場を出すため viewer も取る.
/// 依頼中側は依頼を受けた時刻を出すため `FRAGMENT_REQUESTED` も取る.
/// チーム経由の依頼を自分の所属チームに絞るため, repo の owner が組織なら自分の所属チームも取る.
/// `role` は viewer の立場での絞り込みで, maintainer の扱いが不明なので MEMBER と ADMIN を両方取る
pub(crate) const QUERY_REVIEW: &str = r#"query($requested:String!, $reviewed:String!, $owner:String!) {
  viewer { login }
  owner: repositoryOwner(login:$owner) { ... on Organization {
    member: teams(first:100, role:MEMBER) { nodes { slug } }
    admin: teams(first:100, role:ADMIN) { nodes { slug } }
  } }
  requested: search(query:$requested, type:ISSUE, first:100) { nodes { ...pr ...requested } }
  reviewed: search(query:$reviewed, type:ISSUE, first:100) { nodes { ...pr } }
}"#;

/// 追加のタブ. 検索クエリはタブの設定から組み立てる
pub(crate) const QUERY_CUSTOM: &str = r#"query($q:String!) {
  search(query:$q, type:ISSUE, first:100) { nodes { ...pr } }
}"#;

/// 1 件だけ取り直すときのクエリ. close 済みかを見るため state も取る
pub(crate) const QUERY_ONE: &str = r#"query($owner:String!, $name:String!, $number:Int!) {
  repository(owner:$owner, name:$name) { pullRequest(number:$number) { ...pr } }
}"#;

pub(crate) const FRAGMENT: &str = r#"
fragment pr on PullRequest {
  number title url state isDraft headRefName headRefOid baseRefName updatedAt reviewDecision mergeable
  additions deletions changedFiles
  author { login }
  labels(first:20) { nodes { name } }
  assignees(first:10) { nodes { login } }
  reviewRequests(first:20) { nodes { requestedReviewer {
    ... on User { login }
    ... on Team { slug }
  } } }
  latestReviews(first:20) { nodes { state author { __typename login } } }
  reviewThreads(first:100) { nodes { isResolved } }
  headRef { compare(headRef:"main") { aheadBy } }
  commits(last:1) { nodes { commit { statusCheckRollup {
    contexts(first:100) { nodes {
      __typename
      ... on CheckRun { name conclusion status }
      ... on StatusContext { context state }
    } }
  } } } }
}"#;

/// レビュー依頼の履歴. reviewRequests には依頼の時刻が無いので timeline から取る
pub(crate) const FRAGMENT_REQUESTED: &str = r#"
fragment requested on PullRequest {
  timelineItems(last:50, itemTypes:[REVIEW_REQUESTED_EVENT]) { nodes {
    ... on ReviewRequestedEvent { createdAt requestedReviewer {
      ... on User { login }
      ... on Team { slug }
    } }
  } }
}"#;
