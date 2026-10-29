use std::path::{Path, PathBuf};

use anyhow::{Result, bail};

use crate::domain::local::{LocalState, parse_reviewers};
use crate::infra::process::run;

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

pub fn read_local_state(dir: &Path) -> Result<LocalState> {
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
    Ok(LocalState {
        current_branch,
        dirty: !status.is_empty(),
        branches,
        linear_workspace,
        reviewers,
        devin: None, // TODO(A): git config から読む
    })
}
