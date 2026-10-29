use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::domain::pr::Pr;

/// 前回取得した PR 一覧. 起動直後にこれを出し, 裏で取り直す
#[derive(Serialize, Deserialize)]
pub struct Cache {
    pub fetched_at: DateTime<Utc>,
    pub prs: Vec<Pr>,
}

/// `$XDG_CACHE_HOME/prtui/<owner>__<name>.json`. XDG_CACHE_HOME が無ければ `~/.cache`.
fn path(repo: &str) -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))?;
    Some(
        base.join("prtui")
            .join(format!("{}.json", repo.replace('/', "__"))),
    )
}

/// 無い, 読めない, `Pr` の形が変わって解析できないときは None. 次の取得で上書きされる.
pub fn load(repo: &str) -> Option<Cache> {
    let s = fs::read_to_string(path(repo)?).ok()?;
    serde_json::from_str(&s).ok()
}

pub fn save(repo: &str, cache: &Cache) -> Result<()> {
    let p = path(repo).context("キャッシュの置き場所が決まらない (HOME が未設定)")?;
    if let Some(dir) = p.parent() {
        fs::create_dir_all(dir)?;
    }
    // 書きかけのファイルを読まないよう, 一時ファイルに書いてから置き換える
    let tmp = p.with_extension("json.tmp");
    fs::write(&tmp, serde_json::to_vec(cache)?)?;
    fs::rename(&tmp, &p)?;
    Ok(())
}
