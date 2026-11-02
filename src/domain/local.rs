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

#[cfg(test)]
mod tests {
    use super::parse_reviewers;

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
}
