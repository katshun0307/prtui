/// ブランチ名, 次にタイトルの順で Linear の issue キー (`[A-Z][A-Z0-9]*-[0-9]+`) を探す.
/// ブランチ名は `devin/abc-412-...` のように小文字のことがあるので大文字小文字を無視し, 大文字に正規化して返す.
pub fn linear_issue_key(branch: &str, title: &str) -> Option<String> {
    find_issue_key(branch, true).or_else(|| find_issue_key(title, false))
}

pub fn issue_url(workspace: &str, key: &str) -> String {
    format!("https://linear.app/{workspace}/issue/{key}")
}

/// `[A-Z][A-Z0-9]*-[0-9]+` に最初に一致する部分を大文字で返す. regex crate を足さずに手書きで探す.
fn find_issue_key(s: &str, ignore_case: bool) -> Option<String> {
    let b = s.as_bytes();
    let is_alpha = |c: u8| {
        if ignore_case {
            c.is_ascii_alphabetic()
        } else {
            c.is_ascii_uppercase()
        }
    };
    for start in 0..b.len() {
        if !is_alpha(b[start]) {
            continue;
        }
        let mut i = start + 1;
        while i < b.len() && (is_alpha(b[i]) || b[i].is_ascii_digit()) {
            i += 1;
        }
        if i >= b.len() || b[i] != b'-' {
            continue;
        }
        let digits = i + 1;
        let mut end = digits;
        while end < b.len() && b[end].is_ascii_digit() {
            end += 1;
        }
        if end == digits {
            continue;
        }
        // 一致部分は ASCII だけなので文字境界を跨がない
        return Some(s[start..end].to_ascii_uppercase());
    }
    None
}

/// bot のコメントは issue 本文の引用を含み, そこに関連 PR の review URL が並ぶことがある.
/// この PR 自身の「Review in Linear」リンクは目印のコメントの直後にあるので, 目印より後だけを見る.
pub(crate) const REVIEW_LINK_MARK: &str = "<!-- linear-review-link -->";

pub(crate) fn extract_review_url(body: &str) -> Option<String> {
    let after = &body[body.find(REVIEW_LINK_MARK)? + REVIEW_LINK_MARK.len()..];
    let start = after.find("href=\"")? + "href=\"".len();
    let url = &after[start..];
    let url = &url[..url.find('"')?];
    // ?openGuide=true などの query は付けずに開く
    let url = url.split('?').next().unwrap_or(url);
    url.starts_with("https://linear.app/")
        .then(|| url.to_string())
}

#[cfg(test)]
mod tests {
    use super::{extract_review_url, linear_issue_key};

    #[test]
    fn lowercase_branch_is_normalized() {
        assert_eq!(
            linear_issue_key("devin/abc-412-add-login-form", "タイトル").as_deref(),
            Some("ABC-412")
        );
    }

    #[test]
    fn title_leading_key_when_branch_has_none() {
        assert_eq!(
            linear_issue_key("feature/foo", "MNT-55 スマレジ API の 429 を リトライする")
                .as_deref(),
            Some("MNT-55")
        );
        assert_eq!(
            linear_issue_key("feature/foo", "ABC-388 add_users").as_deref(),
            Some("ABC-388")
        );
    }

    #[test]
    fn title_key_in_the_middle() {
        assert_eq!(
            linear_issue_key("feature/foo", "[migration] ABC-389 テーブルを足す").as_deref(),
            Some("ABC-389")
        );
    }

    #[test]
    fn none_when_neither_has_key() {
        assert_eq!(linear_issue_key("feature/foo", "キー無しのタイトル"), None);
    }

    #[test]
    fn branch_takes_precedence() {
        assert_eq!(
            linear_issue_key("abc-1-fix", "MNT-2 別の issue").as_deref(),
            Some("ABC-1")
        );
    }

    #[test]
    fn title_is_case_sensitive() {
        assert_eq!(linear_issue_key("feature/foo", "abc-1 は小文字"), None);
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
