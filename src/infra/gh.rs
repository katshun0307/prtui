use std::collections::HashSet;
use std::path::Path;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::Deserialize;

use crate::domain::linear::{REVIEW_LINK_MARK, extract_review_url};
use crate::domain::op::PrOp;
use crate::domain::pr::{Check, CheckState, Pr, Reviewer, ReviewerState};
use crate::domain::stack::arrange;
use crate::infra::process::run;

const QUERY: &str = r#"query($author:String!, $assignee:String!) {
  author: search(query:$author, type:ISSUE, first:100) { nodes { ...pr } }
  assignee: search(query:$assignee, type:ISSUE, first:100) { nodes { ...pr } }
}"#;

/// 1 件だけ取り直すときのクエリ. close 済みかを見るため state も取る
const QUERY_ONE: &str = r#"query($owner:String!, $name:String!, $number:Int!) {
  repository(owner:$owner, name:$name) { pullRequest(number:$number) { ...pr } }
}"#;

const FRAGMENT: &str = r#"
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
  latestReviews(first:20) { nodes { state author { login } } }
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

#[derive(Deserialize)]
struct Nodes<T> {
    nodes: Vec<T>,
}

#[derive(Deserialize)]
struct Login {
    login: String,
}

#[derive(Deserialize)]
struct Name {
    name: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReviewRequest {
    requested_reviewer: Option<RequestedReviewer>,
}

/// User なら login, Team なら slug が入る. Bot や Mannequin は両方 None
#[derive(Deserialize)]
struct RequestedReviewer {
    login: Option<String>,
    slug: Option<String>,
}

#[derive(Deserialize)]
struct Review {
    state: String,
    author: Option<Login>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Thread {
    is_resolved: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CommitNode {
    commit: Commit,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Commit {
    status_check_rollup: Option<Rollup>,
}

#[derive(Deserialize)]
struct Rollup {
    contexts: Nodes<Context_>,
}

#[derive(Deserialize)]
struct Context_ {
    #[serde(rename = "__typename")]
    typename: String,
    name: Option<String>,
    conclusion: Option<String>,
    status: Option<String>,
    context: Option<String>,
    state: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawPr {
    // 検索結果に PullRequest 以外が混ざった場合は空オブジェクトになるので Option にする
    number: Option<u64>,
    #[serde(default)]
    title: String,
    #[serde(default)]
    url: String,
    /// OPEN / CLOSED / MERGED
    #[serde(default)]
    state: String,
    #[serde(default)]
    is_draft: bool,
    #[serde(default)]
    head_ref_name: String,
    #[serde(default)]
    head_ref_oid: String,
    #[serde(default)]
    base_ref_name: String,
    updated_at: Option<DateTime<Utc>>,
    review_decision: Option<String>,
    #[serde(default)]
    mergeable: String,
    #[serde(default)]
    additions: u64,
    #[serde(default)]
    deletions: u64,
    #[serde(default)]
    changed_files: u64,
    author: Option<Login>,
    labels: Option<Nodes<Name>>,
    assignees: Option<Nodes<Login>>,
    review_requests: Option<Nodes<ReviewRequest>>,
    latest_reviews: Option<Nodes<Review>>,
    review_threads: Option<Nodes<Thread>>,
    commits: Option<Nodes<CommitNode>>,
    head_ref: Option<HeadRef>,
}

#[derive(Deserialize)]
struct HeadRef {
    compare: Option<Compare>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Compare {
    ahead_by: u64,
}

#[derive(Deserialize)]
struct Response {
    data: Data,
}

#[derive(Deserialize)]
struct Data {
    author: Nodes<RawPr>,
    assignee: Nodes<RawPr>,
}

#[derive(Deserialize)]
struct OneResponse {
    data: OneData,
}

#[derive(Deserialize)]
struct OneData {
    repository: Repository,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Repository {
    pull_request: Option<RawPr>,
}

pub fn repo_name(dir: &Path) -> Result<String> {
    run(
        dir,
        "gh",
        &[
            "repo",
            "view",
            "--json",
            "nameWithOwner",
            "-q",
            ".nameWithOwner",
        ],
    )
}

/// repo の open PR のうち, 自分が author か assignee のものを更新日時の降順で返す.
/// stacked PR は 1 か所にまとめ, main に近い側から順に並べる.
pub fn fetch_prs(dir: &Path, repo: &str) -> Result<Vec<Pr>> {
    let base = format!("repo:{repo} is:pr is:open");
    let out = run(
        dir,
        "gh",
        &[
            "api",
            "graphql",
            "-f",
            &format!("query={QUERY}{FRAGMENT}"),
            "-f",
            &format!("author={base} author:@me"),
            "-f",
            &format!("assignee={base} assignee:@me"),
        ],
    )?;
    let resp: Response = serde_json::from_str(&out).context("GraphQL レスポンスの解析に失敗")?;

    let mut seen = HashSet::new();
    let prs: Vec<Pr> = resp
        .data
        .author
        .nodes
        .into_iter()
        .chain(resp.data.assignee.nodes)
        .filter_map(convert)
        .filter(|p| seen.insert(p.number))
        .collect();
    Ok(arrange(prs))
}

/// PR を 1 件だけ取り直す. open でなくなっていれば None.
pub fn fetch_pr(dir: &Path, repo: &str, number: u64) -> Result<Option<Pr>> {
    let (owner, name) = repo
        .split_once('/')
        .with_context(|| format!("repo 名が owner/name の形でない: {repo}"))?;
    let out = run(
        dir,
        "gh",
        &[
            "api",
            "graphql",
            "-f",
            &format!("query={QUERY_ONE}{FRAGMENT}"),
            "-f",
            &format!("owner={owner}"),
            "-f",
            &format!("name={name}"),
            "-F",
            &format!("number={number}"),
        ],
    )?;
    let resp: OneResponse = serde_json::from_str(&out).context("GraphQL レスポンスの解析に失敗")?;
    Ok(resp
        .data
        .repository
        .pull_request
        .filter(|r| r.state == "OPEN")
        .and_then(convert))
}

fn convert(r: RawPr) -> Option<Pr> {
    let number = r.number?;
    let checks = r
        .commits
        .and_then(|c| c.nodes.into_iter().next())
        .and_then(|n| n.commit.status_check_rollup)
        .map(|roll| roll.contexts.nodes.into_iter().map(convert_check).collect())
        .unwrap_or_default();
    let threads = r.review_threads.map(|t| t.nodes).unwrap_or_default();
    let author = r.author.map(|a| a.login).unwrap_or_default();
    let reviewers = convert_reviewers(
        &author,
        r.review_requests.map(|n| n.nodes).unwrap_or_default(),
        r.latest_reviews.map(|n| n.nodes).unwrap_or_default(),
    );
    Some(Pr {
        number,
        title: r.title,
        url: r.url,
        author,
        is_draft: r.is_draft,
        head_ref_name: r.head_ref_name,
        head_ref_oid: r.head_ref_oid,
        base_ref_name: r.base_ref_name,
        stack: None,
        updated_at: r.updated_at.unwrap_or_default(),
        review_decision: r.review_decision,
        mergeable: r.mergeable,
        labels: r
            .labels
            .map(|l| l.nodes.into_iter().map(|n| n.name).collect())
            .unwrap_or_default(),
        assignees: r
            .assignees
            .map(|a| a.nodes.into_iter().map(|n| n.login).collect())
            .unwrap_or_default(),
        reviewers,
        additions: r.additions,
        deletions: r.deletions,
        changed_files: r.changed_files,
        checks,
        threads_total: threads.len(),
        threads_open: threads.iter().filter(|t| !t.is_resolved).count(),
        behind_main: r.head_ref.and_then(|h| h.compare).map(|c| c.ahead_by),
    })
}

/// 依頼中の reviewer を先に並べ, その後ろにレビュー済みの reviewer を最新の状態で並べる.
/// 承認後に再依頼された人は reviewRequests にも latestReviews にも出るので, 依頼中を優先する.
/// author 自身のレビュー (自分のコメント) と DISMISSED, 未送信 (PENDING) は出さない.
fn convert_reviewers(
    author: &str,
    requests: Vec<ReviewRequest>,
    reviews: Vec<Review>,
) -> Vec<Reviewer> {
    let mut out: Vec<Reviewer> = requests
        .into_iter()
        .filter_map(|q| q.requested_reviewer)
        .filter_map(|r| r.login.or(r.slug))
        .map(|name| Reviewer {
            name,
            state: ReviewerState::Requested,
        })
        .collect();
    for rv in reviews {
        let Some(name) = rv.author.map(|a| a.login) else {
            continue;
        };
        if name == author || out.iter().any(|r| r.name == name) {
            continue;
        }
        let state = match rv.state.as_str() {
            "APPROVED" => ReviewerState::Approved,
            "CHANGES_REQUESTED" => ReviewerState::ChangesRequested,
            "COMMENTED" => ReviewerState::Commented,
            _ => continue,
        };
        out.push(Reviewer { name, state });
    }
    out
}

/// PR ブランチに base (main) を merge commit で取り込む. merge は GitHub 側で非同期に行われる.
/// 取得時から head が動いていれば GitHub が 422 を返すので, 確認していない変更は巻き込まない.
pub fn update_branch(dir: &Path, repo: &str, number: u64, head_oid: &str) -> Result<String> {
    run(
        dir,
        "gh",
        &[
            "api",
            "-X",
            "PUT",
            &format!("repos/{repo}/pulls/{number}/update-branch"),
            "-f",
            &format!("expected_head_sha={head_oid}"),
            "-q",
            ".message",
        ],
    )
}

/// Linear の bot (linear-code) が PR に付けるコメントから Linear review の URL を取る.
/// bot が未投稿 (Linear の GitHub 連携が PR をまだ拾っていない) なら None.
pub fn linear_review_url(dir: &Path, repo: &str, number: u64) -> Result<Option<String>> {
    let bodies = run(
        dir,
        "gh",
        &[
            "api",
            "--paginate",
            &format!("repos/{repo}/issues/{number}/comments"),
            "--jq",
            &format!(r#".[] | select(.body | contains("{REVIEW_LINK_MARK}")) | .body"#),
        ],
    )?;
    Ok(extract_review_url(&bodies))
}

fn convert_check(c: Context_) -> Check {
    if c.typename == "StatusContext" {
        let state = match c.state.as_deref() {
            Some("SUCCESS") => CheckState::Success,
            Some("PENDING" | "EXPECTED") => CheckState::Pending,
            _ => CheckState::Failure,
        };
        return Check {
            name: c.context.unwrap_or_default(),
            state,
        };
    }
    let state = if c.status.as_deref() != Some("COMPLETED") {
        CheckState::Pending
    } else {
        match c.conclusion.as_deref() {
            Some("SUCCESS" | "NEUTRAL") => CheckState::Success,
            Some("SKIPPED") => CheckState::Skipped,
            _ => CheckState::Failure,
        }
    };
    Check {
        name: c.name.unwrap_or_default(),
        state,
    }
}

pub fn checkout(dir: &Path, number: u64) -> Result<String> {
    run(dir, "gh", &["pr", "checkout", &number.to_string()])
}

/// PR の状態を変える. Review のときは targets を `--add-reviewer` に渡す
pub fn change_state(dir: &Path, number: u64, op: PrOp, targets: &[String]) -> Result<String> {
    let n = number.to_string();
    let targets = targets.join(",");
    let args = match op {
        PrOp::Open => vec!["pr", "ready", &n],
        PrOp::Draft => vec!["pr", "ready", &n, "--undo"],
        PrOp::Close => vec!["pr", "close", &n],
        PrOp::Review => vec!["pr", "edit", &n, "--add-reviewer", &targets],
    };
    run(dir, "gh", &args)
}

pub fn view_web(dir: &Path, number: u64) -> Result<String> {
    run(dir, "gh", &["pr", "view", &number.to_string(), "--web"])
}
