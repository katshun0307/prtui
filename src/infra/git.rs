use std::path::{Path, PathBuf};

use anyhow::{Result, bail};

use crate::domain::devin::DevinConfig;
use crate::domain::local::{LocalState, parse_reviewer_sets, parse_reviewers, parse_tabs};
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
    let config = |key: &str| {
        run(dir, "git", &["config", "--get", key])
            .ok()
            .filter(|s| !s.is_empty())
    };
    let linear_workspace = config("prtui.linear-workspace");
    let reviewers = run(dir, "git", &["config", "--get-all", "prtui.reviewers"])
        .map(|s| parse_reviewers(&s))
        .unwrap_or_default();
    let reviewer_sets = run(dir, "git", &["config", "--get-all", "prtui.reviewer-set"])
        .map(|s| parse_reviewer_sets(&s))
        .unwrap_or_default();
    let tabs = run(dir, "git", &["config", "--get-all", "prtui.tab"])
        .map(|s| parse_tabs(&s))
        .unwrap_or_default();
    Ok(LocalState {
        current_branch,
        dirty: !status.is_empty(),
        branches,
        linear_workspace,
        reviewers,
        reviewer_sets,
        devin: config("prtui.devin-token")
            .zip(config("prtui.devin-org"))
            .map(|(token, org)| DevinConfig { token, org }),
        tabs,
    })
}
