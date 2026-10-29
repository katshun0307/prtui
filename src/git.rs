use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Result, bail};

use crate::github::Pr;

/// dir でコマンドを実行し, 成功時の stdout を返す. 失敗時は stderr をエラーにする.
pub fn run(dir: &Path, program: &str, args: &[&str]) -> Result<String> {
    let out = Command::new(program).args(args).current_dir(dir).output()?;
    if !out.status.success() {
        bail!(
            "{program} {}: {}",
            args.first().unwrap_or(&""),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// dir が worktree 内であってもメインの checkout のパスを返す.
pub fn main_worktree(dir: &Path) -> Result<PathBuf> {
    let common = run(
        dir,
        "git",
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )?;
    match Path::new(&common).parent() {
        Some(p) => Ok(p.to_path_buf()),
        None => bail!("git common dir の親が取れない: {common}"),
    }
}

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
}

impl LocalState {
    pub fn read(dir: &Path) -> Result<Self> {
        let current_branch = run(dir, "git", &["branch", "--show-current"])?;
        let status = run(
            dir,
            "git",
            &["status", "--porcelain", "--untracked-files=no"],
        )?;
        let refs = run(
            dir,
            "git",
            &[
                "for-each-ref",
                "--format=%(refname:short) %(objectname)",
                "refs/heads",
            ],
        )?;
        let branches = refs
            .lines()
            .filter_map(|l| l.split_once(' '))
            .map(|(n, o)| (n.to_string(), o.to_string()))
            .collect();
        // 未設定だと git config は非 0 で終わるので, 失敗は None として扱う
        let linear_workspace = run(dir, "git", &["config", "--get", "prtui.linear-workspace"])
            .ok()
            .filter(|s| !s.is_empty());
        let reviewers = run(dir, "git", &["config", "--get-all", "prtui.reviewers"])
            .map(|s| parse_reviewers(&s))
            .unwrap_or_default();
        Ok(Self {
            current_branch,
            dirty: !status.is_empty(),
            branches,
            linear_workspace,
            reviewers,
        })
    }

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
fn parse_reviewers(s: &str) -> Vec<String> {
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
}
