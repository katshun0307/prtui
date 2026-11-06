use std::path::Path;

use anyhow::{Context, Result};

use crate::domain::label::Label;
use crate::domain::linear::{REVIEW_LINK_MARK, extract_review_url};
use crate::domain::op::PrOp;
use crate::domain::pr::Pr;
use crate::domain::review::arrange_reviews;
use crate::domain::stack::{arrange, arrange_flat};
use crate::infra::graphql::{
    QUERY, QUERY_CUSTOM, QUERY_ONE, QUERY_REVIEW, parse_custom, parse_one, parse_review,
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
    let out = gh(dir, &fetch_prs_args(repo))?;
    let prs = parse_search(&out)?;
    Ok(arrange(prs))
}

/// repo の open PR のうち, 自分にレビューが依頼されている (チーム経由を含む) か,
/// 自分がレビューしたものを返す. 自分が author の PR は除く. 依頼中を依頼の古い順に先に並べる.
pub fn fetch_reviews(dir: &Path, repo: &str) -> Result<Vec<Pr>> {
    let out = gh(dir, &fetch_reviews_args(repo)?)?;
    Ok(arrange_reviews(parse_review(&out)?))
}

/// 追加のタブ. repo の open PR のうち `query` (GitHub の検索クエリ) に当たるものを
/// 更新日時の降順で返す. 他人の PR が混ざるので stack はまとめない.
pub fn fetch_custom(dir: &Path, repo: &str, query: &str) -> Result<Vec<Pr>> {
    let out = gh(dir, &fetch_custom_args(repo, query))?;
    Ok(arrange_flat(parse_custom(&out)?))
}

/// PR を 1 件だけ取り直す. open でなくなっていれば None.
pub fn fetch_pr(dir: &Path, repo: &str, number: u64) -> Result<Option<Pr>> {
    let out = gh(dir, &fetch_pr_args(repo, number)?)?;
    parse_one(&out)
}

/// PR ブランチに base (main) を merge commit で取り込む. merge は GitHub 側で非同期に行われる.
/// 取得時から head が動いていれば GitHub が 422 を返すので, 確認していない変更は巻き込まない.
pub fn update_branch(dir: &Path, repo: &str, number: u64, head_oid: &str) -> Result<String> {
    gh(dir, &update_branch_args(repo, number, head_oid))
}

/// Linear の bot (linear-code) が PR に付けるコメントから Linear review の URL を取る.
/// bot が未投稿 (Linear の GitHub 連携が PR をまだ拾っていない) なら None.
pub fn linear_review_url(dir: &Path, repo: &str, number: u64) -> Result<Option<String>> {
    let bodies = gh(dir, &linear_review_url_args(repo, number))?;
    Ok(extract_review_url(&bodies))
}

/// force ならローカルの同名ブランチを PR の最新に reset する. ローカルだけのコミットは消える
pub fn checkout(dir: &Path, number: u64, force: bool) -> Result<String> {
    gh(dir, &checkout_args(number, force))
}

/// PR の状態を変える. Review のときは targets を `--add-reviewer` に渡す
pub fn change_state(dir: &Path, number: u64, op: PrOp, targets: &[String]) -> Result<String> {
    gh(dir, &change_state_args(number, op, targets))
}

/// repo のラベルを全件返す. 並びは選択画面で決めるので gh の既定のまま
pub fn fetch_labels(dir: &Path, repo: &str) -> Result<Vec<Label>> {
    let out = gh(dir, &fetch_labels_args(repo))?;
    parse_labels(&out)
}

/// PR のラベルを付け外す. add も remove も空なら gh を呼ばない
pub fn edit_labels(dir: &Path, number: u64, add: &[String], remove: &[String]) -> Result<String> {
    if add.is_empty() && remove.is_empty() {
        return Ok(String::new());
    }
    gh(dir, &edit_labels_args(number, add, remove))
}

fn parse_labels(json: &str) -> Result<Vec<Label>> {
    serde_json::from_str(json).context("ラベル一覧の JSON を読めない")
}

pub fn view_web(dir: &Path, number: u64) -> Result<String> {
    run(dir, "gh", &["pr", "view", &number.to_string(), "--web"])
}

/// 組み立てた引数で gh を実行する
fn gh(dir: &Path, args: &[String]) -> Result<String> {
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    run(dir, "gh", &args)
}

fn strings<const N: usize>(args: [&str; N]) -> Vec<String> {
    args.map(String::from).to_vec()
}

/// `owner/name` を owner と name に分ける
fn split_repo(repo: &str) -> Result<(&str, &str)> {
    repo.split_once('/')
        .with_context(|| format!("repo 名が owner/name の形でない: {repo}"))
}

/// 自分が author のものと assignee のものを 2 つの alias で同時に取る
fn fetch_prs_args(repo: &str) -> Vec<String> {
    let base = format!("repo:{repo} is:pr is:open");
    vec![
        "api".into(),
        "graphql".into(),
        "-f".into(),
        format!("query={QUERY}"),
        "-f".into(),
        format!("author={base} author:@me"),
        "-f".into(),
        format!("assignee={base} assignee:@me"),
    ]
}

/// 依頼中とレビュー済みを 2 つの alias で取る. owner は所属チームを引くのに使う
fn fetch_reviews_args(repo: &str) -> Result<Vec<String>> {
    let base = format!("repo:{repo} is:pr is:open -author:@me");
    let (owner, _) = split_repo(repo)?;
    Ok(vec![
        "api".into(),
        "graphql".into(),
        "-f".into(),
        format!("query={QUERY_REVIEW}"),
        "-f".into(),
        format!("requested={base} review-requested:@me"),
        "-f".into(),
        format!("reviewed={base} reviewed-by:@me"),
        "-f".into(),
        format!("owner={owner}"),
    ])
}

fn fetch_custom_args(repo: &str, query: &str) -> Vec<String> {
    vec![
        "api".into(),
        "graphql".into(),
        "-f".into(),
        format!("query={QUERY_CUSTOM}"),
        "-f".into(),
        format!("q=repo:{repo} is:pr is:open {query}"),
    ]
}

/// number は GraphQL の Int なので `-F` で型付きで渡す
fn fetch_pr_args(repo: &str, number: u64) -> Result<Vec<String>> {
    let (owner, name) = split_repo(repo)?;
    Ok(vec![
        "api".into(),
        "graphql".into(),
        "-f".into(),
        format!("query={QUERY_ONE}"),
        "-f".into(),
        format!("owner={owner}"),
        "-f".into(),
        format!("name={name}"),
        "-F".into(),
        format!("number={number}"),
    ])
}

fn update_branch_args(repo: &str, number: u64, head_oid: &str) -> Vec<String> {
    vec![
        "api".into(),
        "-X".into(),
        "PUT".into(),
        format!("repos/{repo}/pulls/{number}/update-branch"),
        "-f".into(),
        format!("expected_head_sha={head_oid}"),
        "-q".into(),
        ".message".into(),
    ]
}

fn linear_review_url_args(repo: &str, number: u64) -> Vec<String> {
    vec![
        "api".into(),
        "--paginate".into(),
        format!("repos/{repo}/issues/{number}/comments"),
        "--jq".into(),
        format!(r#".[] | select(.body | contains("{REVIEW_LINK_MARK}")) | .body"#),
    ]
}

/// gh の既定は 30 件なので上限を上げる
fn fetch_labels_args(repo: &str) -> Vec<String> {
    strings([
        "label",
        "list",
        "--repo",
        repo,
        "--limit",
        "1000",
        "--json",
        "name,color,description",
    ])
}

/// gh は `--add-label` の値をカンマで分けるので, 1 つずつフラグにしてもカンマを含む名前は扱えない
fn edit_labels_args(number: u64, add: &[String], remove: &[String]) -> Vec<String> {
    let mut args = strings(["pr", "edit", &number.to_string()]);
    for l in add {
        args.extend(["--add-label".into(), l.clone()]);
    }
    for l in remove {
        args.extend(["--remove-label".into(), l.clone()]);
    }
    args
}

fn checkout_args(number: u64, force: bool) -> Vec<String> {
    let mut args = strings(["pr", "checkout", &number.to_string()]);
    if force {
        args.push("--force".into());
    }
    args
}

fn change_state_args(number: u64, op: PrOp, targets: &[String]) -> Vec<String> {
    let n = number.to_string();
    match op {
        PrOp::Open => strings(["pr", "ready", &n]),
        PrOp::Draft => strings(["pr", "ready", &n, "--undo"]),
        PrOp::Close => strings(["pr", "close", &n]),
        PrOp::Review => strings(["pr", "edit", &n, "--add-reviewer", &targets.join(",")]),
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    /// `key=` で始まる引数の値. 無ければ落とす
    fn field<'a>(args: &'a [String], key: &str) -> &'a str {
        let prefix = format!("{key}=");
        args.iter()
            .find_map(|a| a.strip_prefix(&prefix))
            .unwrap_or_else(|| panic!("{key} が無い: {args:?}"))
    }

    #[test]
    fn prs_author_and_assignee() {
        let args = fetch_prs_args("o/r");
        assert_eq!(args[..3], ["api", "graphql", "-f"]);
        assert!(args[3].starts_with("query="));
        assert!(args[3].contains("fragment pr on PullRequest"));
        assert_eq!(field(&args, "author"), "repo:o/r is:pr is:open author:@me");
        assert_eq!(
            field(&args, "assignee"),
            "repo:o/r is:pr is:open assignee:@me"
        );
    }

    #[test]
    fn reviews_requested_and_reviewed_excluding_own() {
        let args = fetch_reviews_args("o/r").unwrap();
        assert!(args[3].starts_with("query="));
        assert!(args[3].contains("fragment pr on PullRequest"));
        assert!(args[3].contains("fragment requested on PullRequest"));
        assert_eq!(
            field(&args, "requested"),
            "repo:o/r is:pr is:open -author:@me review-requested:@me"
        );
        assert_eq!(
            field(&args, "reviewed"),
            "repo:o/r is:pr is:open -author:@me reviewed-by:@me"
        );
        assert_eq!(field(&args, "owner"), "o");
    }

    #[test]
    fn repo_without_owner_is_error() {
        let err = fetch_reviews_args("noslash").unwrap_err();
        assert_eq!(err.to_string(), "repo 名が owner/name の形でない: noslash");
        assert!(fetch_pr_args("noslash", 1).is_err());
    }

    #[test]
    fn custom_query_after_repo_filter() {
        let args = fetch_custom_args("o/r", "label:bug -author:@me");
        assert!(args[3].starts_with("query="));
        assert!(args[3].contains("fragment pr on PullRequest"));
        assert_eq!(
            field(&args, "q"),
            "repo:o/r is:pr is:open label:bug -author:@me"
        );
    }

    #[test]
    fn one_pr_by_owner_name_number() {
        let args = fetch_pr_args("o/r", 42).unwrap();
        assert!(args[3].starts_with("query="));
        assert!(args[3].contains("fragment pr on PullRequest"));
        assert_eq!(field(&args, "owner"), "o");
        assert_eq!(field(&args, "name"), "r");
        assert_eq!(args[args.len() - 2..], ["-F", "number=42"]);
    }

    #[test]
    fn update_branch_expects_head() {
        assert_eq!(
            update_branch_args("o/r", 7, "abc123"),
            [
                "api",
                "-X",
                "PUT",
                "repos/o/r/pulls/7/update-branch",
                "-f",
                "expected_head_sha=abc123",
                "-q",
                ".message",
            ]
        );
    }

    #[test]
    fn linear_review_from_comments() {
        let args = linear_review_url_args("o/r", 7);
        assert_eq!(
            args[..4],
            ["api", "--paginate", "repos/o/r/issues/7/comments", "--jq"]
        );
        assert!(args[4].contains(REVIEW_LINK_MARK));
    }

    #[test]
    fn labels_list_all() {
        assert_eq!(
            fetch_labels_args("o/r"),
            [
                "label",
                "list",
                "--repo",
                "o/r",
                "--limit",
                "1000",
                "--json",
                "name,color,description"
            ]
        );
    }

    #[test]
    fn labels_parse() {
        let json = r#"[{"name":"bug","color":"d73a4a","description":"壊れている"},
            {"name":"good first issue","color":"7057ff","description":""}]"#;
        assert_eq!(
            parse_labels(json).unwrap(),
            [
                Label {
                    name: "bug".into(),
                    color: "d73a4a".into(),
                    description: "壊れている".into(),
                },
                Label {
                    name: "good first issue".into(),
                    color: "7057ff".into(),
                    description: String::new(),
                },
            ]
        );
        assert!(parse_labels("oops").is_err());
    }

    #[test]
    fn edit_labels_flag_per_label() {
        let add = ["good first issue".to_string(), "docs".to_string()];
        let remove = ["bug".to_string()];
        assert_eq!(
            edit_labels_args(7, &add, &remove),
            [
                "pr",
                "edit",
                "7",
                "--add-label",
                "good first issue",
                "--add-label",
                "docs",
                "--remove-label",
                "bug"
            ]
        );
        assert_eq!(
            edit_labels_args(7, &[], &remove),
            ["pr", "edit", "7", "--remove-label", "bug"]
        );
    }

    #[test]
    fn checkout_force_only_when_asked() {
        assert_eq!(checkout_args(7, false), ["pr", "checkout", "7"]);
        assert_eq!(checkout_args(7, true), ["pr", "checkout", "7", "--force"]);
    }

    #[test]
    fn change_state_per_op() {
        let targets = ["a".to_string(), "b".to_string()];
        assert_eq!(
            change_state_args(7, PrOp::Open, &targets),
            ["pr", "ready", "7"]
        );
        assert_eq!(
            change_state_args(7, PrOp::Draft, &targets),
            ["pr", "ready", "7", "--undo"]
        );
        assert_eq!(
            change_state_args(7, PrOp::Close, &targets),
            ["pr", "close", "7"]
        );
        assert_eq!(
            change_state_args(7, PrOp::Review, &targets),
            ["pr", "edit", "7", "--add-reviewer", "a,b"]
        );
    }
}
