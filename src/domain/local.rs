use std::collections::HashMap;

use crate::domain::devin::DevinConfig;
use crate::domain::pr::Pr;

#[derive(Debug, Default, Clone)]
pub struct LocalState {
    pub current_branch: String,
    pub dirty: bool,
    /// branch 名 → ローカルの HEAD oid
    pub branches: HashMap<String, String>,
    /// git config `prtui.linear-workspace` の値. 未設定なら None
    pub linear_workspace: Option<String>,
    /// git config `prtui.reviewers` の値. `RR` で review を依頼する相手の login.
    /// カンマか空白で区切って並べるか, `--add` で 1 人ずつ足す
    pub reviewers: Vec<String>,
    /// git config `prtui.devin-token` と `prtui.devin-org`. どちらかが未設定なら None
    pub devin: Option<DevinConfig>,
    /// git config `prtui.tab` の値. 自分タブとレビュータブの後ろに足すタブ
    pub tabs: Vec<TabQuery>,
}

/// 追加のタブ 1 つ分. git config `prtui.tab` に `名前=検索クエリ` で書く
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TabQuery {
    pub name: String,
    /// GitHub の検索クエリ. 取得時に `repo:<owner/name> is:pr is:open` を前に付ける
    pub query: String,
}

/// `gh pr checkout` の失敗が, ローカルの同名ブランチが PR と分岐していて fast-forward できないためか.
/// このとき gh はブランチの切り替えまでは済ませている
pub fn is_diverged(err: &str) -> bool {
    err.contains("Diverging branches") || err.contains("Not possible to fast-forward")
}

impl LocalState {
    pub fn is_current(&self, pr: &Pr) -> bool {
        self.current_branch == pr.head_ref_name
    }

    /// PR の head とローカルブランチの関係. ローカルにブランチが無ければ None.
    pub fn sync_label(&self, pr: &Pr) -> Option<&'static str> {
        let oid = self.branches.get(&pr.head_ref_name)?;
        Some(if *oid == pr.head_ref_oid {
            "最新"
        } else {
            "差分あり"
        })
    }
}

/// `git config --get-all prtui.reviewers` の出力を login の並びにする. 重複は先に出た方を残す
pub(crate) fn parse_reviewers(s: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for name in s.split(|c: char| c == ',' || c.is_whitespace()) {
        if !name.is_empty() && !out.iter().any(|o| o.eq_ignore_ascii_case(name)) {
            out.push(name.to_string());
        }
    }
    out
}

/// `git config --get-all prtui.tab` の出力をタブの並びにする. 1 行が 1 タブで, 最初の `=` で
/// 名前とクエリに分ける. `=` が無い行と, 名前かクエリが空の行は捨てる
pub(crate) fn parse_tabs(s: &str) -> Vec<TabQuery> {
    s.lines()
        .filter_map(|l| l.split_once('='))
        .map(|(name, query)| TabQuery {
            name: name.trim().to_string(),
            query: query.trim().to_string(),
        })
        .filter(|t| !t.name.is_empty() && !t.query.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{TabQuery, parse_reviewers, parse_tabs};

    #[test]
    fn reviewers_from_comma_and_multiple_values() {
        assert_eq!(
            parse_reviewers("alice, bobBuilder\nfoo\nALICE"),
            vec!["alice", "bobBuilder", "foo"]
        );
        assert!(parse_reviewers("").is_empty());
    }

    #[test]
    fn reviewers_from_whitespace() {
        assert_eq!(parse_reviewers("  a\tb  c ,, d "), vec!["a", "b", "c", "d"]);
    }

    #[test]
    fn diverged_messages() {
        assert!(super::is_diverged(
            "gh pr: Switched to branch 'b'hint: Diverging branches can't be fast-forwarded"
        ));
        assert!(super::is_diverged(
            "fatal: Not possible to fast-forward, aborting."
        ));
        assert!(!super::is_diverged("gh pr: could not find pull request"));
    }

    #[test]
    fn tabs_split_at_first_equal() {
        let tab = |name: &str, query: &str| TabQuery {
            name: name.into(),
            query: query.into(),
        };
        assert_eq!(
            parse_tabs(
                "マージ予定=label:マージ予定\n 承認待ち = review:required a=b \nno-equal\n=q\nname=\n"
            ),
            vec![
                tab("マージ予定", "label:マージ予定"),
                tab("承認待ち", "review:required a=b"),
            ]
        );
        assert!(parse_tabs("").is_empty());
    }
}
