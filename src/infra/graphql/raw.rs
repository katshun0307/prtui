//! GraphQL の応答を受ける serde の型. Pr への変換は親の convert で行う

use chrono::{DateTime, Utc};
use serde::Deserialize;

#[derive(Deserialize)]
pub(super) struct Nodes<T> {
    pub(super) nodes: Vec<T>,
}

#[derive(Deserialize)]
pub(super) struct Login {
    pub(super) login: String,
}

#[derive(Deserialize)]
pub(super) struct LabelNode {
    pub(super) name: String,
    #[serde(default)]
    pub(super) color: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ReviewRequest {
    pub(super) requested_reviewer: Option<RequestedReviewer>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct RequestedEvent {
    pub(super) created_at: DateTime<Utc>,
    pub(super) requested_reviewer: Option<RequestedReviewer>,
}

/// User なら login, Team なら slug が入る. Bot や Mannequin は両方 None
#[derive(Deserialize)]
pub(super) struct RequestedReviewer {
    pub(super) login: Option<String>,
    pub(super) slug: Option<String>,
}

#[derive(Deserialize)]
pub(super) struct Review {
    pub(super) state: String,
    pub(super) author: Option<ReviewAuthor>,
}

#[derive(Deserialize)]
pub(super) struct ReviewAuthor {
    /// User, Bot, Mannequin など. 古いキャッシュやテストの JSON に無ければ空
    #[serde(rename = "__typename", default)]
    pub(super) typename: String,
    pub(super) login: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Thread {
    pub(super) is_resolved: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct CommitNode {
    pub(super) commit: Commit,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Commit {
    pub(super) status_check_rollup: Option<Rollup>,
}

#[derive(Deserialize)]
pub(super) struct Rollup {
    pub(super) contexts: Nodes<Context_>,
}

#[derive(Deserialize)]
pub(super) struct Context_ {
    #[serde(rename = "__typename")]
    pub(super) typename: String,
    pub(super) name: Option<String>,
    pub(super) conclusion: Option<String>,
    pub(super) status: Option<String>,
    pub(super) context: Option<String>,
    pub(super) state: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct RawPr {
    // 検索結果に PullRequest 以外が混ざった場合は空オブジェクトになるので Option にする
    pub(super) number: Option<u64>,
    #[serde(default)]
    pub(super) title: String,
    #[serde(default)]
    pub(super) url: String,
    /// OPEN / CLOSED / MERGED
    #[serde(default)]
    pub(super) state: String,
    #[serde(default)]
    pub(super) is_draft: bool,
    #[serde(default)]
    pub(super) head_ref_name: String,
    #[serde(default)]
    pub(super) head_ref_oid: String,
    #[serde(default)]
    pub(super) base_ref_name: String,
    pub(super) updated_at: Option<DateTime<Utc>>,
    pub(super) review_decision: Option<String>,
    #[serde(default)]
    pub(super) mergeable: String,
    #[serde(default)]
    pub(super) additions: u64,
    #[serde(default)]
    pub(super) deletions: u64,
    #[serde(default)]
    pub(super) changed_files: u64,
    pub(super) author: Option<Login>,
    pub(super) labels: Option<Nodes<LabelNode>>,
    pub(super) assignees: Option<Nodes<Login>>,
    pub(super) review_requests: Option<Nodes<ReviewRequest>>,
    pub(super) latest_reviews: Option<Nodes<Review>>,
    pub(super) review_threads: Option<Nodes<Thread>>,
    pub(super) commits: Option<Nodes<CommitNode>>,
    pub(super) head_ref: Option<HeadRef>,
    /// レビュータブの依頼中側だけ取る
    pub(super) timeline_items: Option<Nodes<RequestedEvent>>,
}

#[derive(Deserialize)]
pub(super) struct HeadRef {
    pub(super) compare: Option<Compare>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Compare {
    pub(super) ahead_by: u64,
}

#[derive(Deserialize)]
pub(super) struct Response {
    pub(super) data: Data,
}

#[derive(Deserialize)]
pub(super) struct Data {
    pub(super) author: Nodes<RawPr>,
    pub(super) assignee: Nodes<RawPr>,
}

#[derive(Deserialize)]
pub(super) struct ReviewResponse {
    pub(super) data: ReviewData,
}

#[derive(Deserialize)]
pub(super) struct ReviewData {
    pub(super) viewer: Login,
    /// owner が User なら空オブジェクトになる
    pub(super) owner: Option<Owner>,
    pub(super) requested: Nodes<RawPr>,
    pub(super) reviewed: Nodes<RawPr>,
}

#[derive(Deserialize)]
pub(super) struct Owner {
    pub(super) member: Option<Nodes<Slug>>,
    pub(super) admin: Option<Nodes<Slug>>,
}

#[derive(Deserialize)]
pub(super) struct Slug {
    pub(super) slug: String,
}

#[derive(Deserialize)]
pub(super) struct CustomResponse {
    pub(super) data: CustomData,
}

#[derive(Deserialize)]
pub(super) struct CustomData {
    pub(super) search: Nodes<RawPr>,
}

#[derive(Deserialize)]
pub(super) struct OneResponse {
    pub(super) data: OneData,
}

#[derive(Deserialize)]
pub(super) struct OneData {
    pub(super) repository: Repository,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Repository {
    pub(super) pull_request: Option<RawPr>,
}

/// 件数の上限で切られうる connection. 切られた分を数えるため totalCount も取る
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Counted<T> {
    pub(super) total_count: usize,
    pub(super) nodes: Vec<T>,
}

/// PR ビューのコメント. 削除されたユーザーなら author は null
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct CommentNode {
    pub(super) author: Option<Login>,
    pub(super) created_at: DateTime<Utc>,
    #[serde(default)]
    pub(super) body: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ReviewNode {
    pub(super) state: String,
    /// 未提出 (PENDING) なら null
    pub(super) submitted_at: Option<DateTime<Utc>>,
    pub(super) created_at: DateTime<Utc>,
    #[serde(default)]
    pub(super) body: String,
    pub(super) author: Option<Login>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ThreadNode {
    pub(super) id: String,
    pub(super) is_resolved: bool,
    pub(super) is_outdated: bool,
    pub(super) path: String,
    pub(super) line: Option<u64>,
    pub(super) original_line: Option<u64>,
    pub(super) comments: Nodes<CommentNode>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct RawConversation {
    #[serde(default)]
    pub(super) body: String,
    pub(super) created_at: DateTime<Utc>,
    pub(super) author: Option<Login>,
    pub(super) comments: Counted<CommentNode>,
    pub(super) reviews: Counted<ReviewNode>,
    pub(super) review_threads: Counted<ThreadNode>,
}

#[derive(Deserialize)]
pub(super) struct ConversationResponse {
    pub(super) data: ConversationData,
}

#[derive(Deserialize)]
pub(super) struct ConversationData {
    pub(super) repository: ConversationRepository,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ConversationRepository {
    pub(super) pull_request: Option<RawConversation>,
}
