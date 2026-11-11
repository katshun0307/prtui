mod query;
mod raw;

use std::collections::HashSet;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};

use crate::domain::conversation::{Comment, Conversation, Entry, Verdict};
use crate::domain::label::Label;
use crate::domain::pr::{Check, CheckState, MyReview, Pr, Reviewer, ReviewerState};

pub(crate) use query::{QUERY, QUERY_CONVERSATION, QUERY_CUSTOM, QUERY_ONE, QUERY_REVIEW};
use raw::{
    CommentNode, Context_, ConversationResponse, CustomResponse, OneResponse, RawPr,
    RequestedReviewer, Response, Review, ReviewRequest, ReviewResponse,
};

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
        // search の索引は遅れるので, マージした直後の PR が is:open に当たることがある. 状態は node の今の値で見る
        .filter(|r| r.state == "OPEN")
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

/// 追加のタブの検索結果を解析する. 並べ替えはしない
pub(crate) fn parse_custom(json: &str) -> Result<Vec<Pr>> {
    let resp: CustomResponse =
        serde_json::from_str(json).context("GraphQL レスポンスの解析に失敗")?;
    Ok(resp
        .data
        .search
        .nodes
        .into_iter()
        .filter_map(convert)
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
            .map(|l| {
                l.nodes
                    .into_iter()
                    .map(|n| Label {
                        name: n.name,
                        color: n.color,
                        description: String::new(),
                    })
                    .collect()
            })
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

/// PR ビューの説明とコメントを解析する. 未提出のレビューと, 本文の無いコメントだけのレビュー
/// (行へのコメントの入れ物. 中身はスレッドに出す) は除く. PR が無ければエラー
pub(crate) fn parse_conversation(json: &str) -> Result<Conversation> {
    let resp: ConversationResponse =
        serde_json::from_str(json).context("GraphQL レスポンスの解析に失敗")?;
    let r = resp
        .data
        .repository
        .pull_request
        .context("PR が見つからない")?;
    let omitted = [
        r.comments
            .total_count
            .saturating_sub(r.comments.nodes.len()),
        r.reviews.total_count.saturating_sub(r.reviews.nodes.len()),
        r.review_threads
            .total_count
            .saturating_sub(r.review_threads.nodes.len()),
    ]
    .iter()
    .sum();
    let comments = r
        .comments
        .nodes
        .into_iter()
        .map(|c| Entry::Comment(comment(c)));
    let reviews = r.reviews.nodes.into_iter().filter_map(|v| {
        let verdict = Verdict::parse(&v.state)?;
        if verdict == Verdict::Commented && v.body.trim().is_empty() {
            return None;
        }
        Some(Entry::Review {
            verdict,
            comment: Comment {
                author: login(v.author),
                created_at: v.submitted_at.unwrap_or(v.created_at),
                body: v.body,
            },
        })
    });
    let threads = r.review_threads.nodes.into_iter().map(|t| Entry::Thread {
        line: if t.is_outdated {
            t.original_line
        } else {
            t.line.or(t.original_line)
        },
        path: t.path,
        resolved: t.is_resolved,
        outdated: t.is_outdated,
        comments: t.comments.nodes.into_iter().map(comment).collect(),
    });
    Ok(Conversation::new(
        login(r.author),
        r.created_at,
        r.body,
        comments.chain(reviews).chain(threads).collect(),
        omitted,
    ))
}

/// 削除されたユーザーは GitHub の表示に合わせて `ghost`
fn login(l: Option<raw::Login>) -> String {
    l.map_or_else(|| "ghost".into(), |l| l.login)
}

fn comment(c: CommentNode) -> Comment {
    Comment {
        author: login(c.author),
        created_at: c.created_at,
        body: c.body,
    }
}

#[cfg(test)]
mod tests {
    use super::raw::ReviewAuthor;
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
                {"number":3,"title":"c","state":"OPEN"},
                {"number":4,"title":"d","state":"MERGED"}
            ]}
        }}"#;
        let prs = parse_search(json).unwrap();
        let got: Vec<(u64, &str)> = prs.iter().map(|p| (p.number, p.title.as_str())).collect();
        // author 側が先に残り, 並べ替えはしない. 索引が遅れて当たった MERGED は捨てる
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
            "labels":{"nodes":[{"name":"bug","color":"d73a4a"}]},
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
        assert_eq!(p.label_names(), vec!["bug"]);
        assert_eq!(p.labels[0].color, "d73a4a");
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
    fn parse_custom_skips_non_pr() {
        let json = r#"{"data":{"search":{"nodes":[
            {"number":3,"title":"c","assignees":{"nodes":[{"login":"alice"}]}},
            {},
            {"number":1,"title":"a"}
        ]}}}"#;
        let prs = parse_custom(json).unwrap();
        let got: Vec<u64> = prs.iter().map(|p| p.number).collect();
        // 並べ替えはしない
        assert_eq!(got, vec![3, 1]);
        assert_eq!(prs[0].assignees, vec!["alice"]);
        assert!(prs[0].my_review.is_none());
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

    #[test]
    fn parse_conversation_merges_and_filters() {
        let json = r#"{"data":{"repository":{"pullRequest":{
          "body":"説明","createdAt":"2026-11-01T00:00:00Z","author":{"login":"me"},
          "comments":{"totalCount":102,"nodes":[
            {"author":null,"createdAt":"2026-11-01T03:00:00Z","body":"消えた人"},
            {"author":{"login":"bob"},"createdAt":"2026-11-01T01:00:00Z","body":"c1"}
          ]},
          "reviews":{"totalCount":4,"nodes":[
            {"state":"APPROVED","submittedAt":"2026-11-01T05:00:00Z","createdAt":"2026-11-01T04:00:00Z","body":"","author":{"login":"alice"}},
            {"state":"COMMENTED","submittedAt":"2026-11-01T02:00:00Z","createdAt":"2026-11-01T02:00:00Z","body":" ","author":{"login":"alice"}},
            {"state":"COMMENTED","submittedAt":"2026-11-01T02:30:00Z","createdAt":"2026-11-01T02:30:00Z","body":"総評","author":{"login":"carol"}},
            {"state":"PENDING","submittedAt":null,"createdAt":"2026-11-01T06:00:00Z","body":"下書き","author":{"login":"me"}}
          ]},
          "reviewThreads":{"totalCount":2,"nodes":[
            {"isResolved":false,"isOutdated":false,"path":"src/a.rs","line":12,"originalLine":10,
             "comments":{"nodes":[{"author":{"login":"alice"},"createdAt":"2026-11-01T02:00:00Z","body":"ここ"}]}},
            {"isResolved":true,"isOutdated":true,"path":"src/b.rs","line":null,"originalLine":7,
             "comments":{"nodes":[{"author":{"login":"carol"},"createdAt":"2026-11-01T00:30:00Z","body":"古い"}]}}
          ]}
        }}}}"#;
        let c = parse_conversation(json).unwrap();
        assert_eq!((c.author.as_str(), c.body.as_str()), ("me", "説明"));
        assert_eq!(c.omitted, 100);
        let summary: Vec<String> = c
            .entries
            .iter()
            .map(|e| match e {
                Entry::Comment(c) => format!("comment {} {}", c.author, c.body),
                Entry::Review { verdict, comment } => {
                    format!("review {} {verdict:?} {}", comment.author, comment.body)
                }
                Entry::Thread {
                    path,
                    line,
                    resolved,
                    outdated,
                    comments,
                } => format!(
                    "thread {path}:{line:?} {resolved} {outdated} {}",
                    comments.len()
                ),
            })
            .collect();
        // 本文の無い COMMENTED と PENDING は除き, 時刻順. APPROVED は submittedAt で並ぶ
        assert_eq!(
            summary,
            vec![
                "thread src/b.rs:Some(7) true true 1",
                "comment bob c1",
                "thread src/a.rs:Some(12) false false 1",
                "review carol Commented 総評",
                "comment ghost 消えた人",
                "review alice Approved ",
            ]
        );
    }

    #[test]
    fn parse_conversation_missing_pr_is_error() {
        let json = r#"{"data":{"repository":{"pullRequest":null}}}"#;
        assert!(parse_conversation(json).is_err());
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
