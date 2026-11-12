//! PR の説明とコメント (`p` で開く PR ビュー). 会話のコメント, レビュー, 行へのスレッドを時刻順に並べる

use chrono::{DateTime, Utc};

use super::linear::fold_linkback;

/// 1 件の書き込み. 削除されたユーザーの author は `ghost`
#[derive(Debug, Clone, PartialEq)]
pub struct Comment {
    pub author: String,
    pub created_at: DateTime<Utc>,
    /// markdown のまま
    pub body: String,
}

/// レビューの結論
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Approved,
    ChangesRequested,
    Commented,
    Dismissed,
}

impl Verdict {
    /// GitHub の `PullRequestReviewState`. PENDING (未提出) と不明な値は None
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "APPROVED" => Some(Self::Approved),
            "CHANGES_REQUESTED" => Some(Self::ChangesRequested),
            "COMMENTED" => Some(Self::Commented),
            "DISMISSED" => Some(Self::Dismissed),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Approved => "承認",
            Self::ChangesRequested => "変更要求",
            Self::Commented => "レビュー",
            Self::Dismissed => "却下されたレビュー",
        }
    }
}

/// PR ビューの `!` で送るレビュー
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewAction {
    Approve,
    Comment,
}

impl ReviewAction {
    /// 選択のキー
    pub fn key(self) -> char {
        match self {
            Self::Approve => 'a',
            Self::Comment => 'c',
        }
    }

    /// `#n を<phrase>` の形で使う
    pub fn phrase(self) -> &'static str {
        match self {
            Self::Approve => "承認した",
            Self::Comment => "レビューコメントした",
        }
    }

    /// 本文が空でも送れるか. 承認は本文なしで送れる
    pub fn allows_empty(self) -> bool {
        self == Self::Approve
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Entry {
    /// 会話タブのコメント
    Comment(Comment),
    /// レビューの本文. 行へのコメントは `Thread` に出す
    Review { verdict: Verdict, comment: Comment },
    /// 行へのコメントのスレッド. line は今の差分での行で, outdated なら元の行.
    /// id は GraphQL の node ID で, 解決と未解決に戻すのに使う
    Thread {
        id: String,
        path: String,
        line: Option<u64>,
        resolved: bool,
        outdated: bool,
        comments: Vec<Comment>,
    },
}

impl Entry {
    /// 並べる基準の時刻. スレッドは最初のコメント
    pub fn at(&self) -> Option<DateTime<Utc>> {
        match self {
            Entry::Comment(c) | Entry::Review { comment: c, .. } => Some(c.created_at),
            Entry::Thread { comments, .. } => comments.first().map(|c| c.created_at),
        }
    }
}

/// PR の説明と, それに続く書き込み
#[derive(Debug, Clone, PartialEq)]
pub struct Conversation {
    pub author: String,
    pub created_at: DateTime<Utc>,
    pub body: String,
    /// 時刻の古い順
    pub entries: Vec<Entry>,
    /// 件数の上限で取らなかった古い書き込みの数 (コメント, レビュー, スレッドの合計)
    pub omitted: usize,
}

impl Conversation {
    /// 書き込みを時刻の古い順に並べて作る. 同じ時刻なら元の順 (コメント, レビュー, スレッド) を保つ
    pub fn new(
        author: String,
        created_at: DateTime<Utc>,
        body: String,
        mut entries: Vec<Entry>,
        omitted: usize,
    ) -> Self {
        entries.sort_by_key(Entry::at);
        Self {
            author,
            created_at,
            body,
            entries,
            omitted,
        }
    }

    /// スレッドの ID と解決済みかを, 並びの順に
    pub fn threads(&self) -> Vec<(&str, bool)> {
        self.entries
            .iter()
            .filter_map(|e| match e {
                Entry::Thread { id, resolved, .. } => Some((id.as_str(), *resolved)),
                _ => None,
            })
            .collect()
    }

    /// スレッドの解決済みを書き換える. 無ければ何もしない
    pub fn set_resolved(&mut self, thread: &str, value: bool) {
        for e in &mut self.entries {
            if let Entry::Thread { id, resolved, .. } = e
                && id == thread
            {
                *resolved = value;
            }
        }
    }

    /// `(コメント, レビュー, スレッド, 未解決のスレッド)` の件数
    pub fn counts(&self) -> (usize, usize, usize, usize) {
        let mut n = (0, 0, 0, 0);
        for e in &self.entries {
            match e {
                Entry::Comment(_) => n.0 += 1,
                Entry::Review { .. } => n.1 += 1,
                Entry::Thread { resolved, .. } => {
                    n.2 += 1;
                    if !resolved {
                        n.3 += 1;
                    }
                }
            }
        }
        n
    }
}

/// 描画用に本文を整える. Linear の bot のコメントは issue の本文を除き (`fold_linkback`),
/// HTML コメント (`<!-- -->`. PR テンプレートや bot が入れる) を除き,
/// 続く空行を 1 行にまとめ, 前後の空行と行末の空白を落とす. 閉じていない HTML コメントは残す
pub fn tidy_body(body: &str) -> String {
    let body = fold_linkback(body);
    let mut text = String::with_capacity(body.len());
    let mut rest = body.as_ref();
    while let Some(start) = rest.find("<!--") {
        let Some(len) = rest[start..].find("-->") else {
            break;
        };
        text.push_str(&rest[..start]);
        rest = &rest[start + len + 3..];
    }
    text.push_str(rest);

    let mut lines: Vec<&str> = vec![];
    for l in text.lines().map(str::trim_end) {
        if l.is_empty() && lines.last().is_none_or(|p| p.is_empty()) {
            continue;
        }
        lines.push(l);
    }
    while lines.last().is_some_and(|l| l.is_empty()) {
        lines.pop();
    }
    lines.join("\n")
}

/// `e` でエディタに渡す説明. GitHub の本文は CRLF のことがあるので LF に揃える
pub fn editable_body(body: &str) -> String {
    body.replace("\r\n", "\n")
}

/// 2 つの説明が同じか. 改行コード (CRLF / LF) と末尾の空白は見ない.
/// エディタで変えていないかと, 編集中に GitHub 側で変わっていないかの判定に使う
pub fn same_body(a: &str, b: &str) -> bool {
    editable_body(a).trim_end() == editable_body(b).trim_end()
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    fn at(secs: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(secs, 0).unwrap()
    }

    fn comment(author: &str, secs: i64) -> Comment {
        Comment {
            author: author.into(),
            created_at: at(secs),
            body: String::new(),
        }
    }

    #[test]
    fn entries_sorted_by_time_and_thread_by_first_comment() {
        let thread = Entry::Thread {
            id: "T1".into(),
            path: "a.rs".into(),
            line: Some(1),
            resolved: false,
            outdated: false,
            comments: vec![comment("t1", 20), comment("t2", 50)],
        };
        let c = Conversation::new(
            "me".into(),
            at(0),
            String::new(),
            vec![
                Entry::Comment(comment("c", 30)),
                Entry::Review {
                    verdict: Verdict::Approved,
                    comment: comment("r", 10),
                },
                thread,
                Entry::Comment(comment("same", 30)),
            ],
            0,
        );
        let order: Vec<_> = c.entries.iter().map(|e| e.at().unwrap()).collect();
        assert_eq!(order, vec![at(10), at(20), at(30), at(30)]);
        // 同じ時刻なら元の順
        assert_eq!(c.entries[2], Entry::Comment(comment("c", 30)));
        assert_eq!(c.counts(), (2, 1, 1, 1));
        assert_eq!(c.threads(), vec![("T1", false)]);
        let mut c = c;
        c.set_resolved("T1", true);
        c.set_resolved("none", false);
        assert_eq!(c.counts(), (2, 1, 1, 0));
    }

    #[test]
    fn verdict_parse() {
        assert_eq!(Verdict::parse("APPROVED"), Some(Verdict::Approved));
        assert_eq!(
            Verdict::parse("CHANGES_REQUESTED"),
            Some(Verdict::ChangesRequested)
        );
        assert_eq!(Verdict::parse("PENDING"), None);
    }

    #[test]
    fn tidy_body_strips_html_comments_and_blank_runs() {
        let body = "<!-- テンプレート\n説明 -->\n\n## 概要\r\n直す  \n\n\n\n<!-- x -->詳細\n\n";
        assert_eq!(tidy_body(body), "## 概要\n直す\n\n詳細");
        // 閉じていなければ残す
        assert_eq!(tidy_body("a <!-- b"), "a <!-- b");
        assert_eq!(tidy_body("<!-- only -->"), "");
        // Linear の bot のコメントは issue の本文を除く
        assert_eq!(
            tidy_body(
                "<!-- linear-linkback -->\n<details><summary>ABC-1 t</summary>本文</details>"
            ),
            "ABC-1 t\n(issue の本文は省略)"
        );
    }

    #[test]
    fn same_body_ignores_newline_style_and_trailing_space() {
        assert_eq!(editable_body("a\r\nb\r\n"), "a\nb\n");
        assert!(same_body("a\r\nb\r\n", "a\nb"));
        assert!(same_body("", "\n\n"));
        assert!(!same_body("a\nb", "a\n b"));
        assert!(!same_body("a", "b"));
    }
}
