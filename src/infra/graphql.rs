use std::collections::HashSet;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::Deserialize;

use crate::domain::pr::{Check, CheckState, MyReview, Pr, Reviewer, ReviewerState};

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

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RequestedEvent {
    created_at: DateTime<Utc>,
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
    author: Option<ReviewAuthor>,
}

#[derive(Deserialize)]
struct ReviewAuthor {
    /// User, Bot, Mannequin など. 古いキャッシュやテストの JSON に無ければ空
    #[serde(rename = "__typename", default)]
    typename: String,
    login: String,
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
    /// レビュータブの依頼中側だけ取る
    timeline_items: Option<Nodes<RequestedEvent>>,
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
struct ReviewResponse {
    data: ReviewData,
}

#[derive(Deserialize)]
struct ReviewData {
    viewer: Login,
    /// owner が User なら空オブジェクトになる
    owner: Option<Owner>,
    requested: Nodes<RawPr>,
    reviewed: Nodes<RawPr>,
}

#[derive(Deserialize)]
struct Owner {
    member: Option<Nodes<Slug>>,
    admin: Option<Nodes<Slug>>,
}

#[derive(Deserialize)]
struct Slug {
    slug: String,
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

/// レビュータブの検索結果を解析し, `my_review` を入れて番号で重複を除く. 並べ替えはしない.
/// 依頼中側を先に見るので, 両方に当たる PR は依頼中になる
pub(crate) fn parse_review(json: &str) -> Result<Vec<Pr>> {
    let resp: ReviewResponse =
        serde_json::from_str(json).context("GraphQL レスポンスの解析に失敗")?;
    let viewer = resp.data.viewer.login;
    let my_teams: HashSet<String> = resp
        .data
        .owner
        .into_iter()
        .flat_map(|o| o.member.into_iter().chain(o.admin))
        .flat_map(|n| n.nodes)
        .map(|t| t.slug)
        .collect();

    let mut seen = HashSet::new();
    let requested = resp.data.requested.nodes.into_iter().map(|r| (true, r));
    let reviewed = resp.data.reviewed.nodes.into_iter().map(|r| (false, r));
    Ok(requested
        .chain(reviewed)
        .filter_map(|(requested, r)| {
            let mine = my_review(&r, &viewer, &my_teams, requested);
            let mut p = convert(r)?;
            p.my_review = Some(mine);
            Some(p)
        })
        .filter(|p| seen.insert(p.number))
        .collect())
}

/// `reviewRequests` に自分の login が無ければ, 依頼はチーム経由で来ている.
/// 依頼されているチームのうち自分の所属チームを `via_teams` にする. 1 つも当たらなければ
/// (`read:org` が無い, 子チーム経由で親チームに依頼された, など) 依頼されている全チームにする.
/// login は大文字小文字を区別せずに比べる. 依頼を受けた時刻は `requested_at` を見る
fn my_review(r: &RawPr, viewer: &str, my_teams: &HashSet<String>, requested: bool) -> MyReview {
    let requests = r.review_requests.as_ref().map_or(&[][..], |n| &n.nodes);
    let reviewers = || {
        requests
            .iter()
            .filter_map(|q| q.requested_reviewer.as_ref())
    };
    let direct = reviewers().any(|x| {
        x.login
            .as_deref()
            .is_some_and(|l| l.eq_ignore_ascii_case(viewer))
    });
    let via_teams = if requested && !direct {
        let teams: Vec<String> = reviewers().filter_map(|x| x.slug.clone()).collect();
        let mine: Vec<String> = teams
            .iter()
            .filter(|t| my_teams.contains(*t))
            .cloned()
            .collect();
        if mine.is_empty() { teams } else { mine }
    } else {
        vec![]
    };
    let last = r
        .latest_reviews
        .as_ref()
        .into_iter()
        .flat_map(|n| &n.nodes)
        .filter(|rv| {
            rv.author
                .as_ref()
                .is_some_and(|a| a.login.eq_ignore_ascii_case(viewer))
        })
        .find_map(|rv| review_state(&rv.state));
    let requested_at = if requested {
        requested_at(r, viewer, &via_teams)
    } else {
        None
    };
    MyReview {
        requested,
        last,
        via_teams,
        requested_at,
    }
}

/// 今の依頼を受けた時刻. `teams` が空なら直接の依頼とみなし, 自分宛ての最新の event を使う
/// (再依頼なら最後の依頼). チーム経由なら, チームごとの最新の event のうち最も古いものを使う.
/// 所属チームで絞れなかったときに, 後から関係ないチームが依頼されても待ち時間を短く見せないため
fn requested_at(r: &RawPr, viewer: &str, teams: &[String]) -> Option<DateTime<Utc>> {
    let events = r.timeline_items.as_ref().map_or(&[][..], |n| &n.nodes);
    let latest_for = |hit: &dyn Fn(&RequestedReviewer) -> bool| {
        events
            .iter()
            .filter(|e| e.requested_reviewer.as_ref().is_some_and(hit))
            .map(|e| e.created_at)
            .max()
    };
    if teams.is_empty() {
        return latest_for(&|x| {
            x.login
                .as_deref()
                .is_some_and(|l| l.eq_ignore_ascii_case(viewer))
        });
    }
    teams
        .iter()
        .filter_map(|t| latest_for(&|x| x.slug.as_ref() == Some(t)))
        .min()
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
        my_review: None,
    })
}

/// 依頼中の reviewer を先に並べ, その後ろにレビュー済みの reviewer を最新の状態で並べる.
/// 承認後に再依頼された人は reviewRequests にも latestReviews にも出るので, 依頼中を優先する.
/// author 自身のレビュー (自分のコメント) と Bot (devin-ai-integration など) のレビュー,
/// DISMISSED, 未送信 (PENDING) は出さない. Bot は GitHub の Reviewers 欄に出ないため.
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
        let Some(name) = rv.author.filter(|a| a.typename != "Bot").map(|a| a.login) else {
            continue;
        };
        if name == author || out.iter().any(|r| r.name == name) {
            continue;
        }
        let Some(state) = review_state(&rv.state) else {
            continue;
        };
        out.push(Reviewer { name, state });
    }
    out
}

/// DISMISSED と未送信 (PENDING) は None
fn review_state(s: &str) -> Option<ReviewerState> {
    match s {
        "APPROVED" => Some(ReviewerState::Approved),
        "CHANGES_REQUESTED" => Some(ReviewerState::ChangesRequested),
        "COMMENTED" => Some(ReviewerState::Commented),
        _ => None,
    }
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
            author: Some(ReviewAuthor {
                typename: "User".into(),
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
    fn reviewers_skip_author_bot_dismissed_and_pending() {
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
                Review {
                    state: "COMMENTED".into(),
                    author: Some(ReviewAuthor {
                        typename: "Bot".into(),
                        login: "devin-ai-integration".into(),
                    }),
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

    /// 所属チームが取れれば依頼されているチームをそれで絞り, 依頼時刻もそのチームだけで見る
    #[test]
    fn parse_review_narrows_to_my_teams() {
        let json = |owner: &str| {
            format!(
                r#"{{"data":{{
                    "viewer":{{"login":"me"}},
                    "owner":{owner},
                    "requested":{{"nodes":[
                        {{"number":1,"state":"OPEN",
                         "reviewRequests":{{"nodes":[
                            {{"requestedReviewer":{{"slug":"backend"}}}},
                            {{"requestedReviewer":{{"slug":"frontend"}}}}
                         ]}},
                         "timelineItems":{{"nodes":[
                            {{"createdAt":"2026-01-01T00:00:00Z","requestedReviewer":{{"slug":"backend"}}}},
                            {{"createdAt":"2026-01-03T00:00:00Z","requestedReviewer":{{"slug":"frontend"}}}},
                            {{"createdAt":"2026-01-05T00:00:00Z","requestedReviewer":{{"slug":"frontend"}}}}
                         ]}}}}
                    ]}},
                    "reviewed":{{"nodes":[]}}
                }}}}"#
            )
        };
        let got = |owner: &str| {
            let m = parse_review(&json(owner)).unwrap()[0]
                .my_review
                .clone()
                .unwrap();
            (m.via_teams, m.requested_at.map(|t| t.to_rfc3339()))
        };
        let teams = |v: &[&str]| v.iter().map(|t| t.to_string()).collect::<Vec<_>>();
        // maintainer として所属するチームも拾う. 再依頼されたチームは最後の依頼
        assert_eq!(
            got(r#"{"member":{"nodes":[{"slug":"ops"}]},"admin":{"nodes":[{"slug":"frontend"}]}}"#),
            (
                teams(&["frontend"]),
                Some("2026-01-05T00:00:00+00:00".into())
            )
        );
        // 所属チームが当たらない, owner が User (空オブジェクト), 取れなかったときは全チームに戻す
        for owner in [
            r#"{"member":{"nodes":[{"slug":"ops"}]},"admin":{"nodes":[]}}"#,
            "{}",
            "null",
        ] {
            assert_eq!(
                got(owner),
                (
                    teams(&["backend", "frontend"]),
                    Some("2026-01-01T00:00:00+00:00".into())
                )
            );
        }
    }

    #[test]
    fn parse_review_sets_my_review() {
        let json = r#"{"data":{
            "viewer":{"login":"Me"},
            "requested":{"nodes":[
                {"number":1,"state":"OPEN",
                 "reviewRequests":{"nodes":[{"requestedReviewer":{"login":"me"}}]},
                 "timelineItems":{"nodes":[
                    {"createdAt":"2026-01-02T00:00:00Z","requestedReviewer":{"login":"ME"}},
                    {"createdAt":"2026-01-04T00:00:00Z","requestedReviewer":{"login":"bob"}},
                    {"createdAt":"2026-01-01T00:00:00Z","requestedReviewer":{"login":"me"}},
                    {"createdAt":"2026-01-01T00:00:00Z","requestedReviewer":null}
                 ]}},
                {"number":2,"state":"OPEN",
                 "reviewRequests":{"nodes":[
                    {"requestedReviewer":{"slug":"backend"}},
                    {"requestedReviewer":{"login":"bob"}},
                    {"requestedReviewer":{"slug":"frontend"}}
                 ]},
                 "timelineItems":{"nodes":[
                    {"createdAt":"2026-01-01T00:00:00Z","requestedReviewer":{"slug":"backend"}},
                    {"createdAt":"2026-01-03T00:00:00Z","requestedReviewer":{"slug":"backend"}},
                    {"createdAt":"2026-01-05T00:00:00Z","requestedReviewer":{"login":"bob"}},
                    {"createdAt":"2026-01-06T00:00:00Z","requestedReviewer":{"slug":"frontend"}}
                 ]},
                 "latestReviews":{"nodes":[{"state":"APPROVED","author":{"login":"me"}}]}},
                {}
            ]},
            "reviewed":{"nodes":[
                {"number":2,"state":"OPEN"},
                {"number":3,"state":"OPEN",
                 "timelineItems":{"nodes":[
                    {"createdAt":"2026-01-01T00:00:00Z","requestedReviewer":{"login":"me"}}
                 ]},
                 "latestReviews":{"nodes":[
                    {"state":"APPROVED","author":{"login":"bob"}},
                    {"state":"CHANGES_REQUESTED","author":{"login":"ME"}}
                 ]}},
                {"number":4,"state":"OPEN",
                 "latestReviews":{"nodes":[{"state":"DISMISSED","author":{"login":"me"}}]}}
            ]}
        }}"#;
        let prs = parse_review(json).unwrap();
        let got: Vec<(u64, MyReview)> = prs
            .into_iter()
            .map(|p| (p.number, p.my_review.unwrap()))
            .collect();
        let mine = |requested, last, teams: &[&str], at: Option<&str>| MyReview {
            requested,
            last,
            via_teams: teams.iter().map(|t| t.to_string()).collect(),
            requested_at: at.map(|s| s.parse().unwrap()),
        };
        assert_eq!(
            got,
            vec![
                // 自分宛ての最新の依頼. 他人宛ては見ない
                (1, mine(true, None, &[], Some("2026-01-02T00:00:00Z"))),
                // チーム経由ならチームごとの最新の依頼のうち最も古いもの.
                // 後から依頼された frontend (1/6) に押し出されない
                (
                    2,
                    mine(
                        true,
                        Some(ReviewerState::Approved),
                        &["backend", "frontend"],
                        Some("2026-01-03T00:00:00Z")
                    )
                ),
                // 依頼中でなければ timeline があっても None
                (
                    3,
                    mine(false, Some(ReviewerState::ChangesRequested), &[], None)
                ),
                (4, mine(false, None, &[], None)),
            ]
        );
    }
}
