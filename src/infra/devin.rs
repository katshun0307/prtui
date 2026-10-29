use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::domain::devin::{DevinConfig, DevinMessage, DevinSession, DevinStatus};
use crate::infra::process::run_with_stdin;

const BASE: &str = "https://api.devin.ai/v3/organizations";
/// session 一覧は 5 ページ (1000 件) で打ち切る
const SESSION_PAGES: usize = 5;
/// メッセージは 50 ページ (10000 件) で打ち切る
const MESSAGE_PAGES: usize = 50;

/// repo の session のうち since 以降に更新されたもの
pub fn fetch_sessions(
    cfg: &DevinConfig,
    repo: &str,
    since: DateTime<Utc>,
) -> Result<Vec<DevinSession>> {
    let base = format!(
        "{BASE}/{}/sessions?repo_names={}&updated_after={}&first=200",
        encode(&cfg.org),
        encode(repo),
        since.timestamp()
    );
    let mut out = Vec::new();
    let mut cursor: Option<String> = None;
    for _ in 0..SESSION_PAGES {
        let url = match &cursor {
            Some(c) => format!("{base}&after={}", encode(c)),
            None => base.clone(),
        };
        let (page, next) = parse_sessions(&request(cfg, "GET", &url, None)?)?;
        out.extend(page);
        match next {
            Some(c) => cursor = Some(c),
            None => break,
        }
    }
    Ok(out)
}

/// session のメッセージを古い順にすべて
pub fn fetch_messages(cfg: &DevinConfig, id: &str) -> Result<Vec<DevinMessage>> {
    let base = format!("{}/messages?first=200", session_url(cfg, id));
    let mut out = Vec::new();
    let mut cursor: Option<String> = None;
    for _ in 0..MESSAGE_PAGES {
        let url = match &cursor {
            Some(c) => format!("{base}&after={}", encode(c)),
            None => base.clone(),
        };
        let (page, next) = parse_messages(&request(cfg, "GET", &url, None)?)?;
        out.extend(page);
        match next {
            Some(c) => cursor = Some(c),
            None => break,
        }
    }
    // API も時系列で返すが, 念のため安定ソートで揃える
    out.sort_by_key(|m| m.created_at);
    Ok(out)
}

/// 送信後の session を返す. suspended の session は送信で再開する
pub fn send_message(cfg: &DevinConfig, id: &str, text: &str) -> Result<DevinSession> {
    let url = format!("{}/messages", session_url(cfg, id));
    let body = serde_json::json!({ "message": text }).to_string();
    parse_session(&request(cfg, "POST", &url, Some(&body))?)
}

fn session_url(cfg: &DevinConfig, id: &str) -> String {
    format!("{BASE}/{}/sessions/{}", encode(&cfg.org), encode(id))
}

/// curl で叩いて本文を返す. token は argv に載せず, stdin の設定でだけ渡す
fn request(cfg: &DevinConfig, method: &str, url: &str, body: Option<&str>) -> Result<String> {
    let args = curl_args(method, url, body);
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let out = run_with_stdin(&std::env::temp_dir(), "curl", &args, &auth_config(cfg))?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    if out.status.success() {
        return Ok(stdout.into_owned());
    }
    // 22 は --fail-with-body で HTTP 400 以上を受けたとき. 本文に ProblemDetail が入る
    if out.status.code() == Some(22) {
        bail!("{}", http_error(&stdout, &stderr));
    }
    bail!("curl: {}", stderr.trim())
}

/// token を含まない curl の引数. 認証は `--config -` で stdin から読ませる
fn curl_args(method: &str, url: &str, body: Option<&str>) -> Vec<String> {
    let mut args: Vec<String> = [
        "-sS",
        "--fail-with-body",
        "--max-time",
        "30",
        "--config",
        "-",
        "-X",
        method,
    ]
    .map(String::from)
    .to_vec();
    if let Some(b) = body {
        args.extend(["-H", "Content-Type: application/json", "--data-binary", b].map(String::from));
    }
    args.push(url.to_string());
    args
}

/// curl の設定ファイル形式の認証ヘッダ. 引用符の中では `\` と `"` をエスケープする
fn auth_config(cfg: &DevinConfig) -> String {
    let token = cfg.token.replace('\\', "\\\\").replace('"', "\\\"");
    format!("header = \"Authorization: Bearer {token}\"\n")
}

#[derive(Deserialize)]
struct RawProblem {
    detail: Option<Value>,
    title: Option<String>,
}

/// ProblemDetail の detail (無ければ title, それも無ければ本文) と, curl の stderr (HTTP status を含む)
fn http_error(stdout: &str, stderr: &str) -> String {
    let problem = serde_json::from_str::<RawProblem>(stdout).ok();
    let msg = problem
        .and_then(|p| {
            // 旧形式の detail は文字列以外 (検証エラーの配列) のこともある
            let detail = p.detail.and_then(|d| match d {
                Value::String(s) => Some(s),
                Value::Null => None,
                other => Some(other.to_string()),
            });
            detail.or(p.title)
        })
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| stdout.trim().to_string());
    let status = stderr.trim();
    match (msg.is_empty(), status.is_empty()) {
        (true, _) => status.to_string(),
        (false, true) => msg,
        (false, false) => format!("{msg} ({status})"),
    }
}

/// query と path に入れる値のパーセントエンコード. 英数と `-_.~` 以外を `%XX` にする
fn encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// API の時刻は整数. 単位が未確認なので 1e12 を超えれば ms, それ以外は秒とみなす.
/// 読めなければ 1970 年にする
fn timestamp(v: Option<f64>) -> DateTime<Utc> {
    let Some(v) = v else {
        return DateTime::default();
    };
    let ms = if v > 1e12 { v } else { v * 1000.0 };
    DateTime::from_timestamp_millis(ms as i64).unwrap_or_default()
}

/// 1 件の形が崩れていても全体を落とさないよう, 要素ごとに解析して読めないものは捨てる
fn lenient<T: DeserializeOwned>(items: Vec<Value>) -> Vec<T> {
    items
        .into_iter()
        .filter_map(|v| serde_json::from_value(v).ok())
        .collect()
}

#[derive(Deserialize)]
struct RawPage {
    items: Vec<Value>,
    #[serde(default)]
    has_next_page: bool,
    #[serde(default)]
    end_cursor: Option<String>,
}

impl RawPage {
    fn next(&self) -> Option<String> {
        self.end_cursor.clone().filter(|_| self.has_next_page)
    }
}

#[derive(Deserialize)]
struct RawSession {
    session_id: String,
    #[serde(default)]
    url: String,
    #[serde(default)]
    status: Option<DevinStatus>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    updated_at: Option<f64>,
    #[serde(default)]
    pull_requests: Option<Vec<Value>>,
    #[serde(default)]
    is_archived: Option<bool>,
}

#[derive(Deserialize)]
struct RawPullRequest {
    pr_url: String,
}

#[derive(Deserialize)]
struct RawMessage {
    #[serde(default)]
    source: Option<String>,
    #[serde(default)]
    username: Option<String>,
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    created_at: Option<f64>,
}

fn convert_session(r: RawSession) -> DevinSession {
    let pr_urls = lenient::<RawPullRequest>(r.pull_requests.unwrap_or_default())
        .into_iter()
        .map(|p| p.pr_url)
        .collect();
    DevinSession {
        id: r.session_id,
        status: r.status.unwrap_or(DevinStatus::Unknown),
        title: r.title,
        url: r.url,
        updated_at: timestamp(r.updated_at),
        pr_urls,
        is_archived: r.is_archived.unwrap_or(false),
    }
}

fn convert_message(r: RawMessage) -> DevinMessage {
    DevinMessage {
        from_devin: r.source.as_deref() == Some("devin"),
        username: r.username.filter(|s| !s.is_empty()),
        text: r.message.unwrap_or_default(),
        created_at: timestamp(r.created_at),
    }
}

/// session 一覧の 1 ページと次のページのカーソル
pub(crate) fn parse_sessions(json: &str) -> Result<(Vec<DevinSession>, Option<String>)> {
    let page: RawPage =
        serde_json::from_str(json).context("Devin の session 一覧を解析できない")?;
    let next = page.next();
    let sessions = lenient::<RawSession>(page.items)
        .into_iter()
        .map(convert_session)
        .collect();
    Ok((sessions, next))
}

/// メッセージ一覧の 1 ページと次のページのカーソル
pub(crate) fn parse_messages(json: &str) -> Result<(Vec<DevinMessage>, Option<String>)> {
    let page: RawPage = serde_json::from_str(json).context("Devin のメッセージを解析できない")?;
    let next = page.next();
    let messages = lenient::<RawMessage>(page.items)
        .into_iter()
        .map(convert_message)
        .collect();
    Ok((messages, next))
}

pub(crate) fn parse_session(json: &str) -> Result<DevinSession> {
    let raw: RawSession = serde_json::from_str(json).context("Devin の session を解析できない")?;
    Ok(convert_session(raw))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SESSION: &str = r#"{
        "session_id": "devin-abc",
        "url": "https://app.devin.ai/sessions/abc",
        "status": "suspended",
        "status_detail": "inactivity",
        "title": "Fix bug",
        "created_at": 1761700000,
        "updated_at": 1761714000,
        "acus_consumed": 1.5,
        "tags": [],
        "org_id": "org-x",
        "is_archived": false,
        "pull_requests": [
            {"pr_url": "https://github.com/o/r/pull/1", "pr_state": "open"},
            {"pr_state": null}
        ]
    }"#;

    #[test]
    fn session_fields() {
        let s = parse_session(SESSION).unwrap();
        assert_eq!(s.id, "devin-abc");
        assert_eq!(s.status, DevinStatus::Suspended);
        assert_eq!(s.title.as_deref(), Some("Fix bug"));
        assert_eq!(s.url, "https://app.devin.ai/sessions/abc");
        assert_eq!(s.updated_at.timestamp(), 1761714000);
        // pr_url の無い要素は捨てる
        assert_eq!(s.pr_urls, vec!["https://github.com/o/r/pull/1"]);
        assert!(!s.is_archived);
    }

    #[test]
    fn session_minimal_and_unknown_status() {
        let s = parse_session(
            r#"{"session_id":"devin-x","status":"paused","updated_at":1761714000123,"pull_requests":null}"#,
        )
        .unwrap();
        assert_eq!(s.status, DevinStatus::Unknown);
        // 1e12 を超える値は ms とみなす
        assert_eq!(s.updated_at.timestamp_millis(), 1761714000123);
        assert!(s.pr_urls.is_empty());
        assert!(s.url.is_empty());
    }

    #[test]
    fn sessions_page_and_cursor() {
        let json = format!(
            r#"{{"items":[{SESSION},{{"no_id":true}}],"has_next_page":true,"end_cursor":"c1","total":null}}"#
        );
        let (sessions, next) = parse_sessions(&json).unwrap();
        // 形の崩れた要素は捨てて残りを返す
        assert_eq!(sessions.len(), 1);
        assert_eq!(next.as_deref(), Some("c1"));

        let (_, next) =
            parse_sessions(r#"{"items":[],"has_next_page":false,"end_cursor":"c2"}"#).unwrap();
        assert_eq!(next, None);
        let (sessions, next) = parse_sessions(r#"{"items":[]}"#).unwrap();
        assert!(sessions.is_empty() && next.is_none());
        assert!(parse_sessions("not json").is_err());
    }

    #[test]
    fn messages_page() {
        let json = r#"{"items":[
            {"event_id":"e1","source":"user","message":"hi","created_at":1761700000,"username":"kat","user_id":"u1","origin":"web"},
            {"event_id":"e2","source":"devin","message":"line1\nline2","created_at":1761700060}
        ],"has_next_page":false,"end_cursor":null}"#;
        let (msgs, next) = parse_messages(json).unwrap();
        assert!(next.is_none());
        assert_eq!(msgs.len(), 2);
        assert!(!msgs[0].from_devin);
        assert_eq!(msgs[0].username.as_deref(), Some("kat"));
        assert_eq!(msgs[0].created_at.timestamp(), 1761700000);
        assert!(msgs[1].from_devin);
        assert_eq!(msgs[1].text, "line1\nline2");
        assert_eq!(msgs[1].username, None);
    }

    #[test]
    fn problem_detail_message() {
        let stderr = "curl: (22) The requested URL returned error: 409\n";
        assert_eq!(
            http_error(
                r#"{"title":"Conflict","status":409,"detail":"session is archived"}"#,
                stderr
            ),
            "session is archived (curl: (22) The requested URL returned error: 409)"
        );
        // detail が無ければ title
        assert_eq!(http_error(r#"{"title":"Gone","status":410}"#, ""), "Gone");
        // 旧形式の検証エラーは JSON のまま出す
        assert!(http_error(r#"{"detail":[{"msg":"bad"}]}"#, "").contains("bad"));
        // JSON でなければ本文
        assert_eq!(http_error("Bad Gateway\n", "e"), "Bad Gateway (e)");
        assert_eq!(http_error("", "e"), "e");
    }

    #[test]
    fn token_only_in_stdin_config() {
        let cfg = DevinConfig {
            token: "cog_SECRET\"x".into(),
            org: "org-1".into(),
        };
        let args = curl_args("POST", "https://example.com/x", Some(r#"{"message":"hi"}"#));
        assert!(args.iter().all(|a| !a.contains("SECRET")));
        assert_eq!(
            args,
            [
                "-sS",
                "--fail-with-body",
                "--max-time",
                "30",
                "--config",
                "-",
                "-X",
                "POST",
                "-H",
                "Content-Type: application/json",
                "--data-binary",
                r#"{"message":"hi"}"#,
                "https://example.com/x",
            ]
        );
        assert_eq!(
            auth_config(&cfg),
            "header = \"Authorization: Bearer cog_SECRET\\\"x\"\n"
        );
        assert!(!format!("{cfg:?}").contains("SECRET"));
    }

    #[test]
    fn percent_encoding() {
        assert_eq!(encode("owner/repo"), "owner%2Frepo");
        assert_eq!(encode("a-b_c.d~e9"), "a-b_c.d~e9");
        assert_eq!(encode("x+y=z&"), "x%2By%3Dz%26");
        assert_eq!(encode("あ"), "%E3%81%82");
    }

    #[test]
    fn timestamp_units() {
        assert_eq!(timestamp(Some(1761700000.0)).timestamp(), 1761700000);
        assert_eq!(
            timestamp(Some(1761700000123.0)).timestamp_millis(),
            1761700000123
        );
        assert_eq!(timestamp(None).timestamp(), 0);
    }
}
