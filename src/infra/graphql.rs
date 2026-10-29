use std::collections::HashSet;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::Deserialize;

use crate::domain::pr::{Check, CheckState, Pr, Reviewer, ReviewerState};

pub(crate) const QUERY: &str = r#"query($author:String!, $assignee:String!) {
  author: search(query:$author, type:ISSUE, first:100) { nodes { ...pr } }
  assignee: search(query:$assignee, type:ISSUE, first:100) { nodes { ...pr } }
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

/// author / assignee の検索結果を解析し, Pr に変換して番号で重複を除く. 並べ替えはしない
pub(crate) fn parse_search(json: &str) -> Result<Vec<Pr>> {
    let resp: Response = serde_json::from_str(json).context("GraphQL レスポンスの解析に失敗")?;

    let mut seen = HashSet::new();
    Ok(resp
        .data
        .author
        .nodes
        .into_iter()
        .chain(resp.data.assignee.nodes)
        .filter_map(convert)
        .filter(|p| seen.insert(p.number))
        .collect())
}

/// 1 件取得の結果を解析する. open でなくなっていれば None
pub(crate) fn parse_one(json: &str) -> Result<Option<Pr>> {
    let resp: OneResponse = serde_json::from_str(json).context("GraphQL レスポンスの解析に失敗")?;
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

#[cfg(test)]
mod tests {
    use super::*;

    fn check_run(status: &str, conclusion: Option<&str>) -> Context_ {
        Context_ {
            typename: "CheckRun".into(),
            name: Some("build".into()),
            conclusion: conclusion.map(Into::into),
            status: Some(status.into()),
            context: None,
            state: None,
        }
    }

    fn status_context(state: &str) -> Context_ {
        Context_ {
            typename: "StatusContext".into(),
            name: None,
            conclusion: None,
            status: None,
            context: Some("ci/legacy".into()),
            state: Some(state.into()),
        }
    }

    #[test]
    fn check_run_states() {
        let state = |s, c| convert_check(check_run(s, c)).state;
        assert_eq!(state("COMPLETED", Some("SUCCESS")), CheckState::Success);
        assert_eq!(state("COMPLETED", Some("NEUTRAL")), CheckState::Success);
        assert_eq!(state("COMPLETED", Some("SKIPPED")), CheckState::Skipped);
        assert_eq!(state("COMPLETED", Some("FAILURE")), CheckState::Failure);
        assert_eq!(state("COMPLETED", None), CheckState::Failure);
        // 未完了なら conclusion に関わらず Pending
        assert_eq!(state("IN_PROGRESS", None), CheckState::Pending);
        assert_eq!(state("QUEUED", Some("SUCCESS")), CheckState::Pending);
        assert_eq!(convert_check(check_run("COMPLETED", None)).name, "build");
    }

    #[test]
    fn status_context_states() {
        let state = |s| convert_check(status_context(s)).state;
        assert_eq!(state("SUCCESS"), CheckState::Success);
        assert_eq!(state("PENDING"), CheckState::Pending);
        assert_eq!(state("EXPECTED"), CheckState::Pending);
        assert_eq!(state("ERROR"), CheckState::Failure);
        assert_eq!(state("FAILURE"), CheckState::Failure);
        assert_eq!(convert_check(status_context("SUCCESS")).name, "ci/legacy");
    }

    fn request(login: Option<&str>, slug: Option<&str>) -> ReviewRequest {
        ReviewRequest {
            requested_reviewer: Some(RequestedReviewer {
                login: login.map(Into::into),
                slug: slug.map(Into::into),
            }),
        }
    }

    fn review(login: &str, state: &str) -> Review {
        Review {
            state: state.into(),
            author: Some(Login {
                login: login.into(),
            }),
        }
    }

    fn names(rs: &[Reviewer]) -> Vec<(&str, ReviewerState)> {
        rs.iter().map(|r| (r.name.as_str(), r.state)).collect()
    }

    #[test]
    fn reviewers_requested_first_and_deduplicated() {
        let rs = convert_reviewers(
            "me",
            vec![
                request(Some("alice"), None),
                request(None, Some("backend")),
                request(None, None),
                ReviewRequest {
                    requested_reviewer: None,
                },
            ],
            vec![
                // 再依頼された人は依頼中を優先する
                review("alice", "APPROVED"),
                review("bob", "CHANGES_REQUESTED"),
                review("carol", "COMMENTED"),
                review("dave", "APPROVED"),
            ],
        );
        assert_eq!(
            names(&rs),
            vec![
                ("alice", ReviewerState::Requested),
                ("backend", ReviewerState::Requested),
                ("bob", ReviewerState::ChangesRequested),
                ("carol", ReviewerState::Commented),
                ("dave", ReviewerState::Approved),
            ]
        );
    }

    #[test]
    fn reviewers_skip_author_dismissed_and_pending() {
        let rs = convert_reviewers(
            "me",
            vec![],
            vec![
                review("me", "COMMENTED"),
                review("x", "DISMISSED"),
                review("y", "PENDING"),
                Review {
                    state: "APPROVED".into(),
                    author: None,
                },
                review("z", "APPROVED"),
            ],
        );
        assert_eq!(names(&rs), vec![("z", ReviewerState::Approved)]);
    }

    #[test]
    fn parse_search_dedups_and_skips_non_pr() {
        let json = r#"{"data":{
            "author":{"nodes":[
                {"number":1,"title":"a","state":"OPEN"},
                {},
                {"number":2,"title":"b","state":"OPEN"}
            ]},
            "assignee":{"nodes":[
                {"number":2,"title":"b (assignee)","state":"OPEN"},
                {"number":3,"title":"c","state":"OPEN"}
            ]}
        }}"#;
        let prs = parse_search(json).unwrap();
        let got: Vec<(u64, &str)> = prs.iter().map(|p| (p.number, p.title.as_str())).collect();
        // author 側が先に残り, 並べ替えはしない
        assert_eq!(got, vec![(1, "a"), (2, "b"), (3, "c")]);
    }

    #[test]
    fn parse_search_converts_fields() {
        let json = r#"{"data":{"author":{"nodes":[{
            "number":7,"title":"t","url":"u","state":"OPEN","isDraft":true,
            "headRefName":"feat","headRefOid":"abc","baseRefName":"main",
            "updatedAt":"2026-01-02T03:04:05Z","reviewDecision":null,"mergeable":"CONFLICTING",
            "additions":10,"deletions":2,"changedFiles":3,
            "author":{"login":"me"},
            "labels":{"nodes":[{"name":"bug"}]},
            "assignees":{"nodes":[{"login":"me"}]},
            "reviewRequests":{"nodes":[{"requestedReviewer":{"login":"alice"}}]},
            "latestReviews":{"nodes":[{"state":"COMMENTED","author":{"login":"me"}}]},
            "reviewThreads":{"nodes":[{"isResolved":true},{"isResolved":false}]},
            "headRef":{"compare":{"aheadBy":4}},
            "commits":{"nodes":[{"commit":{"statusCheckRollup":{"contexts":{"nodes":[
                {"__typename":"CheckRun","name":"build","conclusion":"SUCCESS","status":"COMPLETED"},
                {"__typename":"StatusContext","context":"ci","state":"PENDING"}
            ]}}}}]}
        }]},"assignee":{"nodes":[]}}}"#;
        let prs = parse_search(json).unwrap();
        assert_eq!(prs.len(), 1);
        let p = &prs[0];
        assert_eq!(p.number, 7);
        assert!(p.is_draft);
        assert!(p.is_conflicting());
        assert_eq!(p.head_ref_name, "feat");
        assert_eq!(p.base_ref_name, "main");
        assert_eq!(p.labels, vec!["bug"]);
        assert_eq!(p.assignees, vec!["me"]);
        assert_eq!(
            names(&p.reviewers),
            vec![("alice", ReviewerState::Requested)]
        );
        assert_eq!((p.threads_total, p.threads_open), (2, 1));
        assert_eq!(p.behind_main, Some(4));
        assert_eq!(p.count_checks(CheckState::Success), 1);
        assert_eq!(p.count_checks(CheckState::Pending), 1);
        assert_eq!((p.additions, p.deletions, p.changed_files), (10, 2, 3));
        assert!(p.stack.is_none());
    }

    #[test]
    fn parse_search_error_message() {
        let err = parse_search("not json").unwrap_err();
        assert_eq!(err.to_string(), "GraphQL レスポンスの解析に失敗");
    }

    #[test]
    fn parse_one_open_and_closed() {
        let one = |state: &str| {
            format!(
                r#"{{"data":{{"repository":{{"pullRequest":{{"number":5,"state":"{state}"}}}}}}}}"#
            )
        };
        assert_eq!(parse_one(&one("OPEN")).unwrap().map(|p| p.number), Some(5));
        assert!(parse_one(&one("MERGED")).unwrap().is_none());
        assert!(parse_one(&one("CLOSED")).unwrap().is_none());
        let missing = r#"{"data":{"repository":{"pullRequest":null}}}"#;
        assert!(parse_one(missing).unwrap().is_none());
    }
}
