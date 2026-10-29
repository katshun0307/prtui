use std::collections::{HashMap, HashSet};
use std::path::Path;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::git::run;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CheckState {
    Success,
    Failure,
    Pending,
    Skipped,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Check {
    pub name: String,
    pub state: CheckState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReviewerState {
    /// レビューを依頼中でまだ返事が無い. 再依頼も含む
    Requested,
    Approved,
    ChangesRequested,
    Commented,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Reviewer {
    /// User は login, Team は slug
    pub name: String,
    pub state: ReviewerState,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pr {
    pub number: u64,
    pub title: String,
    pub url: String,
    pub author: String,
    pub is_draft: bool,
    pub head_ref_name: String,
    pub head_ref_oid: String,
    pub base_ref_name: String,
    /// 一覧内の別 PR の head を base にしている, またはされている PR だけ Some
    pub stack: Option<Stack>,
    pub updated_at: DateTime<Utc>,
    pub review_decision: Option<String>,
    pub mergeable: String,
    pub labels: Vec<String>,
    pub assignees: Vec<String>,
    /// 依頼中の reviewer と, レビュー済みの reviewer の最新の状態. PR author 自身は含めない
    pub reviewers: Vec<Reviewer>,
    pub additions: u64,
    pub deletions: u64,
    pub changed_files: u64,
    pub checks: Vec<Check>,
    pub threads_total: usize,
    pub threads_open: usize,
    /// main にあって PR ブランチに無いコミット数. head ブランチが消えていれば None
    pub behind_main: Option<u64>,
}

/// stacked PR の中での位置. main に近い側から 1 始まりで数える.
/// 枝分かれしている stack では len は最も深い枝の段数になる
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stack {
    pub pos: usize,
    pub len: usize,
}

impl Pr {
    pub fn count_checks(&self, state: CheckState) -> usize {
        self.checks.iter().filter(|c| c.state == state).count()
    }

    /// SKIPPED を除いたチェック数.
    pub fn effective_checks(&self) -> usize {
        self.checks.len() - self.count_checks(CheckState::Skipped)
    }

    pub fn count_reviewers(&self, state: ReviewerState) -> usize {
        self.reviewers.iter().filter(|r| r.state == state).count()
    }
}

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

/// 更新日時の降順に並べ, stacked PR をまとめ直す. 1 件を差し替えた後にも使う.
pub fn arrange(mut prs: Vec<Pr>) -> Vec<Pr> {
    for p in &mut prs {
        p.stack = None;
    }
    prs.sort_by_key(|p| std::cmp::Reverse(p.updated_at));
    group_stacks(prs)
}

/// base が一覧内の別 PR の head になっている PR を親の直後に並べ, `stack` に位置を入れる.
/// stack 同士は stack 内で最も新しい更新日時の降順に並べる. 入力は更新日時の降順であること.
/// 親が一覧に無い PR (他人の PR の上に積んだものなど) は stack の根として扱う.
fn group_stacks(prs: Vec<Pr>) -> Vec<Pr> {
    let heads: HashMap<&str, usize> = prs
        .iter()
        .enumerate()
        .map(|(i, p)| (p.head_ref_name.as_str(), i))
        .collect();
    let parent: Vec<Option<usize>> = prs
        .iter()
        .enumerate()
        .map(|(i, p)| {
            heads
                .get(p.base_ref_name.as_str())
                .copied()
                .filter(|&j| j != i)
        })
        .collect();
    // 入力順のまま push するので, 子も更新日時の降順になる
    let mut children = vec![vec![]; prs.len()];
    for (i, p) in parent.iter().enumerate() {
        if let Some(p) = p {
            children[*p].push(i);
        }
    }

    // 各根から深さ優先でたどり (PR の添字, 深さ) を並べる. 循環していれば visited で止まる
    let mut visited = vec![false; prs.len()];
    let mut groups: Vec<Vec<(usize, usize)>> = vec![];
    let roots: Vec<usize> = (0..prs.len()).filter(|&i| parent[i].is_none()).collect();
    for root in roots.into_iter().chain(0..prs.len()) {
        if visited[root] {
            continue;
        }
        let mut group = vec![];
        let mut todo = vec![(root, 0)];
        while let Some((i, depth)) = todo.pop() {
            if std::mem::replace(&mut visited[i], true) {
                continue;
            }
            group.push((i, depth));
            todo.extend(children[i].iter().rev().map(|&c| (c, depth + 1)));
        }
        groups.push(group);
    }
    // 更新日時の降順に入力されているので, 各 stack の最小の添字が最新の更新になる
    groups.sort_by_key(|g| g.iter().map(|&(i, _)| i).min());

    let mut slots: Vec<Option<Pr>> = prs.into_iter().map(Some).collect();
    let mut out = Vec::with_capacity(slots.len());
    for group in groups {
        let len = group.iter().map(|&(_, d)| d).max().unwrap_or(0) + 1;
        let stacked = group.len() > 1;
        for (i, depth) in group {
            let mut p = slots[i].take().expect("各 PR は 1 つの stack にだけ入る");
            if stacked {
                p.stack = Some(Stack {
                    pos: depth + 1,
                    len,
                });
            }
            out.push(p);
        }
    }
    out
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

/// bot のコメントは issue 本文の引用を含み, そこに関連 PR の review URL が並ぶことがある.
/// この PR 自身の「Review in Linear」リンクは目印のコメントの直後にあるので, 目印より後だけを見る.
const REVIEW_LINK_MARK: &str = "<!-- linear-review-link -->";

fn extract_review_url(body: &str) -> Option<String> {
    let after = &body[body.find(REVIEW_LINK_MARK)? + REVIEW_LINK_MARK.len()..];
    let start = after.find("href=\"")? + "href=\"".len();
    let url = &after[start..];
    let url = &url[..url.find('"')?];
    // ?openGuide=true などの query は付けずに開く
    let url = url.split('?').next().unwrap_or(url);
    url.starts_with("https://linear.app/")
        .then(|| url.to_string())
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
pub(crate) mod tests {
    use super::{Pr, extract_review_url, group_stacks};

    pub(crate) fn pr(number: u64, head: &str, base: &str) -> Pr {
        Pr {
            number,
            title: String::new(),
            url: String::new(),
            author: String::new(),
            is_draft: false,
            head_ref_name: head.into(),
            head_ref_oid: String::new(),
            base_ref_name: base.into(),
            stack: None,
            updated_at: Default::default(),
            review_decision: None,
            mergeable: String::new(),
            labels: vec![],
            assignees: vec![],
            reviewers: vec![],
            additions: 0,
            deletions: 0,
            changed_files: 0,
            checks: vec![],
            threads_total: 0,
            threads_open: 0,
            behind_main: None,
        }
    }

    fn layout(prs: Vec<Pr>) -> Vec<(u64, Option<(usize, usize)>)> {
        group_stacks(prs)
            .into_iter()
            .map(|p| (p.number, p.stack.map(|s| (s.pos, s.len))))
            .collect()
    }

    #[test]
    fn stack_is_grouped_from_bottom() {
        // 更新日時の降順: 3 (最上段) が最新, 間に単独の 9 が挟まる
        let prs = vec![
            pr(3, "c", "b"),
            pr(9, "x", "main"),
            pr(1, "a", "main"),
            pr(2, "b", "a"),
        ];
        assert_eq!(
            layout(prs),
            vec![
                (1, Some((1, 3))),
                (2, Some((2, 3))),
                (3, Some((3, 3))),
                (9, None),
            ]
        );
    }

    #[test]
    fn branching_stack_and_missing_parent() {
        let prs = vec![
            pr(5, "e", "other"),
            pr(2, "b", "a"),
            pr(3, "c", "a"),
            pr(1, "a", "main"),
        ];
        assert_eq!(
            layout(prs),
            vec![
                (5, None),
                (1, Some((1, 2))),
                (2, Some((2, 2))),
                (3, Some((2, 2))),
            ]
        );
    }

    #[test]
    fn cycle_does_not_hang() {
        let prs = vec![pr(1, "a", "b"), pr(2, "b", "a")];
        assert_eq!(layout(prs).len(), 2);
    }

    #[test]
    fn review_link_after_mark() {
        let body = concat!(
            "<!-- linear-linkback -->\n<details>本文の関連 PR ",
            "[x](https://linear.app/ws/review/other-pr-c0405ec8924e)</details>\n",
            "<!-- linear-review-link -->\n",
            "<p><a href=\"https://linear.app/ws/review/abc-343-遅い-0123456789ab?openGuide=true\">Review in Linear</a></p>\n",
        );
        assert_eq!(
            extract_review_url(body).as_deref(),
            Some("https://linear.app/ws/review/abc-343-遅い-0123456789ab")
        );
    }

    #[test]
    fn none_without_mark() {
        assert_eq!(
            extract_review_url("<a href=\"https://linear.app/ws/review/x-1\">"),
            None
        );
    }
}
