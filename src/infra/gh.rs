use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result, bail};

use crate::domain::conversation::{Conversation, ReviewAction, same_body};
use crate::domain::label::Label;
use crate::domain::linear::{REVIEW_LINK_MARK, extract_review_url};
use crate::domain::merge::{MergeMethod, MergeOutcome, MergeSettings};
use crate::domain::op::PrOp;
use crate::domain::pr::Pr;
use crate::domain::review::arrange_reviews;
use crate::domain::stack::{arrange, arrange_flat};
use crate::infra::graphql::{
    QUERY, QUERY_CONVERSATION, QUERY_CUSTOM, QUERY_ONE, QUERY_REVIEW, parse_conversation,
    parse_custom, parse_one, parse_review, parse_search,
};
use crate::infra::process::{run, run_with_stdin};

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

/// PR ビューに出す説明とコメント. open でない PR も取れる
pub fn fetch_conversation(dir: &Path, repo: &str, number: u64) -> Result<Conversation> {
    let out = gh(dir, &pr_query_args(QUERY_CONVERSATION, repo, number)?)?;
    parse_conversation(&out)
}

/// PR にレビュー (承認かコメント) を送る
pub fn submit_review(dir: &Path, number: u64, action: ReviewAction, body: &str) -> Result<String> {
    gh(dir, &submit_review_args(number, action, body))
}

/// PR の説明を body に書き換える. 編集中に他の人が書き換えた内容を消さないよう,
/// 直前に取り直した説明が original と違えば書き換えない (GitHub の API に楽観ロックが無いため).
/// 本文は長く改行を含むので argv ではなく stdin で渡す
pub fn edit_body(dir: &Path, number: u64, original: &str, body: &str) -> Result<String> {
    let current = gh(dir, &view_body_args(number))?;
    if !same_body(&current, original) {
        bail!("編集中に GitHub 側で説明が変わった. r で取り直してからやり直す");
    }
    let args = edit_body_args(number);
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let out = run_with_stdin(dir, "gh", &args, body)?;
    if !out.status.success() {
        bail!("gh pr: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
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

/// PR の状態を変える
pub fn change_state(dir: &Path, number: u64, op: PrOp) -> Result<String> {
    gh(dir, &change_state_args(number, op))
}

/// repo で許可されたマージ方法と, 自分が前回使った方法
pub fn fetch_merge_settings(dir: &Path, repo: &str) -> Result<MergeSettings> {
    let out = gh(dir, &fetch_merge_settings_args(repo))?;
    parse_merge_settings(&out)
}

/// 非同期のマージ API の版. 既定の版 (2022-11-28) には無い
const MERGE_ASYNC_API_VERSION: &str = "2026-03-10";
/// マージの結果を取り直す間隔と回数
const MERGE_POLL_INTERVAL: Duration = Duration::from_secs(2);
const MERGE_POLL_TRIES: usize = 30;

/// PR を method でマージし, 終わるまで待つ. stacked PR は同期のマージ API (`gh pr merge`) では
/// マージできないので, 非同期のマージ API (`PUT .../merge-async`) で頼み, 結果を `GET .../merge-async/{uuid}`
/// で取り直す. head が head_oid から動いていれば GitHub がマージしない. merge queue があれば queue に入れる.
/// stack の途中の PR を頼むと, その下の PR もまとめてマージされる.
/// head ブランチは消さない. リモートのブランチを消すかは repo の設定に任せる
pub fn merge(
    dir: &Path,
    repo: &str,
    number: u64,
    method: MergeMethod,
    head_oid: &str,
) -> Result<MergeOutcome> {
    let mut res = parse_async_merge(&gh(dir, &merge_args(repo, number, method, head_oid))?)?;
    for _ in 0..MERGE_POLL_TRIES {
        let AsyncMerge::Pending(uuid) = res else {
            break;
        };
        std::thread::sleep(MERGE_POLL_INTERVAL);
        res = parse_async_merge(&gh(dir, &merge_status_args(repo, number, &uuid))?)?;
    }
    match res {
        AsyncMerge::Done(outcome) => Ok(outcome),
        AsyncMerge::Pending(_) => bail!(
            "マージの完了を {} 秒待っても終わらない. r で取り直して確かめる",
            MERGE_POLL_INTERVAL.as_secs() * MERGE_POLL_TRIES as u64
        ),
    }
}

/// targets に review を依頼する. レビュー済みの人に依頼すると再依頼になる
pub fn request_reviews(dir: &Path, number: u64, targets: &[String]) -> Result<String> {
    gh(dir, &request_reviews_args(number, targets))
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

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawMergeSettings {
    merge_commit_allowed: bool,
    squash_merge_allowed: bool,
    rebase_merge_allowed: bool,
    #[serde(default)]
    viewer_default_merge_method: String,
}

fn parse_merge_settings(json: &str) -> Result<MergeSettings> {
    let raw: RawMergeSettings =
        serde_json::from_str(json).context("マージの設定の JSON を読めない")?;
    let allowed = [
        (MergeMethod::Squash, raw.squash_merge_allowed),
        (MergeMethod::Merge, raw.merge_commit_allowed),
        (MergeMethod::Rebase, raw.rebase_merge_allowed),
    ]
    .into_iter()
    .filter_map(|(m, ok)| ok.then_some(m))
    .collect();
    Ok(MergeSettings::new(
        allowed,
        MergeMethod::from_github(&raw.viewer_default_merge_method),
    ))
}

/// 非同期のマージ API の応答. 頼んだときと結果の取り直しで同じ形
#[derive(serde::Deserialize)]
struct RawAsyncMerge {
    status: String,
    #[serde(default)]
    details: RawAsyncMergeDetails,
}

#[derive(serde::Deserialize, Default)]
struct RawAsyncMergeDetails {
    #[serde(default)]
    message: String,
    #[serde(default)]
    uuid: String,
}

#[derive(Debug, PartialEq)]
enum AsyncMerge {
    /// 結果を取り直す uuid
    Pending(String),
    Done(MergeOutcome),
}

/// failed は details.message をエラーにする
fn parse_async_merge(json: &str) -> Result<AsyncMerge> {
    let raw: RawAsyncMerge =
        serde_json::from_str(json).context("マージの応答の JSON を読めない")?;
    let message = raw.details.message;
    match raw.status.as_str() {
        "pending" if raw.details.uuid.is_empty() => bail!("マージの応答に uuid が無い: {message}"),
        "pending" => Ok(AsyncMerge::Pending(raw.details.uuid)),
        "merged" => Ok(AsyncMerge::Done(MergeOutcome::Merged)),
        "enqueued" => Ok(AsyncMerge::Done(MergeOutcome::Enqueued)),
        "failed" if message.is_empty() => bail!("GitHub がマージしなかった"),
        "failed" => bail!("{message}"),
        other => bail!("マージの応答の status が分からない: {other}"),
    }
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

fn fetch_pr_args(repo: &str, number: u64) -> Result<Vec<String>> {
    pr_query_args(QUERY_ONE, repo, number)
}

/// owner, name, number を変数に取る PR 1 件のクエリ.
/// number は GraphQL の Int なので `-F` で型付きで渡す
fn pr_query_args(query: &str, repo: &str, number: u64) -> Result<Vec<String>> {
    let (owner, name) = split_repo(repo)?;
    Ok(vec![
        "api".into(),
        "graphql".into(),
        "-f".into(),
        format!("query={query}"),
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

fn change_state_args(number: u64, op: PrOp) -> Vec<String> {
    let n = number.to_string();
    match op {
        PrOp::Open => strings(["pr", "ready", &n]),
        PrOp::Draft => strings(["pr", "ready", &n, "--undo"]),
        PrOp::Close => strings(["pr", "close", &n]),
    }
}

fn fetch_merge_settings_args(repo: &str) -> Vec<String> {
    strings([
        "repo",
        "view",
        repo,
        "--json",
        "mergeCommitAllowed,squashMergeAllowed,rebaseMergeAllowed,viewerDefaultMergeMethod",
    ])
}

/// merge_action は既定 (merge queue があれば queue, 無ければ直接) のまま送らない
fn merge_args(repo: &str, number: u64, method: MergeMethod, head_oid: &str) -> Vec<String> {
    let method = match method {
        MergeMethod::Squash => "squash",
        MergeMethod::Merge => "merge",
        MergeMethod::Rebase => "rebase",
    };
    vec![
        "api".into(),
        "-X".into(),
        "PUT".into(),
        "-H".into(),
        format!("X-GitHub-Api-Version: {MERGE_ASYNC_API_VERSION}"),
        format!("repos/{repo}/pulls/{number}/merge-async"),
        "-f".into(),
        format!("merge_method={method}"),
        "-f".into(),
        format!("sha={head_oid}"),
    ]
}

fn merge_status_args(repo: &str, number: u64, uuid: &str) -> Vec<String> {
    vec![
        "api".into(),
        "-H".into(),
        format!("X-GitHub-Api-Version: {MERGE_ASYNC_API_VERSION}"),
        format!("repos/{repo}/pulls/{number}/merge-async/{uuid}"),
    ]
}

fn view_body_args(number: u64) -> Vec<String> {
    strings([
        "pr",
        "view",
        &number.to_string(),
        "--json",
        "body",
        "--jq",
        ".body",
    ])
}

/// 本文は stdin から読ませる
fn edit_body_args(number: u64) -> Vec<String> {
    strings(["pr", "edit", &number.to_string(), "--body-file", "-"])
}

/// 承認は本文が空なら `--body` を付けない
fn submit_review_args(number: u64, action: ReviewAction, body: &str) -> Vec<String> {
    let flag = match action {
        ReviewAction::Approve => "--approve",
        ReviewAction::Comment => "--comment",
    };
    let mut args = strings(["pr", "review", &number.to_string(), flag]);
    if !body.is_empty() {
        args.extend(["--body".into(), body.into()]);
    }
    args
}

fn request_reviews_args(number: u64, targets: &[String]) -> Vec<String> {
    strings([
        "pr",
        "edit",
        &number.to_string(),
        "--add-reviewer",
        &targets.join(","),
    ])
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
    fn submit_review_flags() {
        assert_eq!(
            submit_review_args(3, ReviewAction::Approve, ""),
            ["pr", "review", "3", "--approve"]
        );
        assert_eq!(
            submit_review_args(3, ReviewAction::Approve, "LGTM"),
            ["pr", "review", "3", "--approve", "--body", "LGTM"]
        );
        assert_eq!(
            submit_review_args(3, ReviewAction::Comment, "a\nb"),
            ["pr", "review", "3", "--comment", "--body", "a\nb"]
        );
    }

    #[test]
    fn edit_body_reads_stdin() {
        assert_eq!(
            view_body_args(3),
            ["pr", "view", "3", "--json", "body", "--jq", ".body"]
        );
        assert_eq!(edit_body_args(3), ["pr", "edit", "3", "--body-file", "-"]);
    }

    #[test]
    fn conversation_by_owner_name_number() {
        let args = pr_query_args(QUERY_CONVERSATION, "o/r", 42).unwrap();
        assert!(args[3].starts_with("query="));
        assert!(args[3].contains("reviewThreads"));
        assert!(!args[3].contains("fragment pr on PullRequest"));
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
    fn merge_uses_async_api() {
        assert_eq!(
            merge_args("o/r", 7, MergeMethod::Squash, "abc"),
            [
                "api",
                "-X",
                "PUT",
                "-H",
                "X-GitHub-Api-Version: 2026-03-10",
                "repos/o/r/pulls/7/merge-async",
                "-f",
                "merge_method=squash",
                "-f",
                "sha=abc"
            ]
        );
        assert_eq!(
            merge_args("o/r", 7, MergeMethod::Merge, "abc")[7],
            "merge_method=merge"
        );
        assert_eq!(
            merge_args("o/r", 7, MergeMethod::Rebase, "abc")[7],
            "merge_method=rebase"
        );
        assert_eq!(
            merge_status_args("o/r", 7, "u-1"),
            [
                "api",
                "-H",
                "X-GitHub-Api-Version: 2026-03-10",
                "repos/o/r/pulls/7/merge-async/u-1"
            ]
        );
    }

    #[test]
    fn parse_async_merge_by_status() {
        let pending = r#"{"status":"pending","details":{"message":"accepted","uuid":"u-1","merge_method":"squash","merge_action":"direct_merge","expected_head_sha":"abc","bypass_rules":false}}"#;
        assert_eq!(
            parse_async_merge(pending).unwrap(),
            AsyncMerge::Pending("u-1".into())
        );
        assert_eq!(
            parse_async_merge(r#"{"status":"merged","details":{"message":"ok","sha":"def"}}"#)
                .unwrap(),
            AsyncMerge::Done(MergeOutcome::Merged)
        );
        assert_eq!(
            parse_async_merge(r#"{"status":"enqueued","details":{"message":"queued"}}"#).unwrap(),
            AsyncMerge::Done(MergeOutcome::Enqueued)
        );
        let failed = parse_async_merge(
            r#"{"status":"failed","details":{"message":"Required status check \"build\" is failing"}}"#,
        )
        .unwrap_err();
        assert_eq!(
            failed.to_string(),
            r#"Required status check "build" is failing"#
        );
        assert!(parse_async_merge(r#"{"status":"pending","details":{"message":"x"}}"#).is_err());
        assert!(parse_async_merge(r#"{"status":"weird"}"#).is_err());
    }

    #[test]
    fn merge_settings_from_json() {
        assert_eq!(
            fetch_merge_settings_args("o/r"),
            [
                "repo",
                "view",
                "o/r",
                "--json",
                "mergeCommitAllowed,squashMergeAllowed,rebaseMergeAllowed,viewerDefaultMergeMethod"
            ]
        );
        let s = parse_merge_settings(
            r#"{"mergeCommitAllowed":true,"rebaseMergeAllowed":false,"squashMergeAllowed":true,"viewerDefaultMergeMethod":"SQUASH"}"#,
        )
        .unwrap();
        assert_eq!(
            s,
            MergeSettings::new(
                vec![MergeMethod::Squash, MergeMethod::Merge],
                Some(MergeMethod::Squash)
            )
        );
        // 既定が許可されていなければ None
        let s = parse_merge_settings(
            r#"{"mergeCommitAllowed":false,"rebaseMergeAllowed":true,"squashMergeAllowed":false,"viewerDefaultMergeMethod":"MERGE"}"#,
        )
        .unwrap();
        assert_eq!(s.allowed, [MergeMethod::Rebase]);
        assert_eq!(s.default, None);
    }

    #[test]
    fn change_state_per_op() {
        assert_eq!(change_state_args(7, PrOp::Open), ["pr", "ready", "7"]);
        assert_eq!(
            change_state_args(7, PrOp::Draft),
            ["pr", "ready", "7", "--undo"]
        );
        assert_eq!(change_state_args(7, PrOp::Close), ["pr", "close", "7"]);
    }

    #[test]
    fn request_reviews_joins_targets() {
        let targets = ["a".to_string(), "b".to_string()];
        assert_eq!(
            request_reviews_args(7, &targets),
            ["pr", "edit", "7", "--add-reviewer", "a,b"]
        );
    }
}
