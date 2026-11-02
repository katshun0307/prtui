use std::path::Path;

use anyhow::{Context, Result};

use crate::domain::linear::{REVIEW_LINK_MARK, extract_review_url};
use crate::domain::op::PrOp;
use crate::domain::pr::Pr;
use crate::domain::review::arrange_reviews;
use crate::domain::stack::arrange;
use crate::infra::graphql::{
    FRAGMENT, FRAGMENT_REQUESTED, QUERY, QUERY_ONE, QUERY_REVIEW, parse_one, parse_review,
    parse_search,
};
use crate::infra::process::run;

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
    let prs = parse_search(&out)?;
    Ok(arrange(prs))
}

/// repo の open PR のうち, 自分にレビューが依頼されている (チーム経由を含む) か,
/// 自分がレビューしたものを返す. 自分が author の PR は除く. 依頼中を依頼の古い順に先に並べる.
pub fn fetch_reviews(dir: &Path, repo: &str) -> Result<Vec<Pr>> {
    let base = format!("repo:{repo} is:pr is:open -author:@me");
    let (owner, _) = repo
        .split_once('/')
        .with_context(|| format!("repo 名が owner/name の形でない: {repo}"))?;
    let out = run(
        dir,
        "gh",
        &[
            "api",
            "graphql",
            "-f",
            &format!("query={QUERY_REVIEW}{FRAGMENT}{FRAGMENT_REQUESTED}"),
            "-f",
            &format!("requested={base} review-requested:@me"),
            "-f",
            &format!("reviewed={base} reviewed-by:@me"),
            "-f",
            &format!("owner={owner}"),
        ],
    )?;
    Ok(arrange_reviews(parse_review(&out)?))
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
    parse_one(&out)
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

/// force ならローカルの同名ブランチを PR の最新に reset する. ローカルだけのコミットは消える
pub fn checkout(dir: &Path, number: u64, force: bool) -> Result<String> {
    let n = number.to_string();
    let mut args = vec!["pr", "checkout", &n];
    if force {
        args.push("--force");
    }
    run(dir, "gh", &args)
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
