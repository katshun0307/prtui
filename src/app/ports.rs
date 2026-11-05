//! runtime が外とやりとりする口. 実装は adapters.rs で infra に繋ぎ, テストでは fake に差し替える.
//! スレッドに渡すので `Send + Sync + 'static` にする

use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use chrono::{DateTime, Utc};

use crate::domain::cache::Cache;
use crate::domain::devin::{DevinConfig, DevinMessage, DevinSession};
use crate::domain::local::LocalState;
use crate::domain::op::PrOp;
use crate::domain::pr::Pr;

/// GitHub. 対象の repo と gh を実行するディレクトリは実装が持つ.
/// `fetch_*` は gh.rs と同じく並べ終えた一覧を返す (自分タブは `arrange`, レビュータブは
/// `arrange_reviews`, 追加のタブは `arrange_flat`)
pub trait Github: Send + Sync + 'static {
    fn fetch_prs(&self) -> Result<Vec<Pr>>;
    fn fetch_reviews(&self) -> Result<Vec<Pr>>;
    fn fetch_custom(&self, query: &str) -> Result<Vec<Pr>>;
    /// open でなくなっていれば None
    fn fetch_pr(&self, number: u64) -> Result<Option<Pr>>;
    fn update_branch(&self, number: u64, head_oid: &str) -> Result<String>;
    fn checkout(&self, number: u64, force: bool) -> Result<String>;
    fn change_state(&self, number: u64, op: PrOp, targets: &[String]) -> Result<String>;
    fn view_web(&self, number: u64) -> Result<()>;
    /// Linear の bot のコメントに見つからなければ None
    fn linear_review_url(&self, number: u64) -> Result<Option<String>>;
}

/// メイン checkout のブランチと git config
pub trait Git: Send + Sync + 'static {
    fn read_local_state(&self) -> Result<LocalState>;
}

/// Devin API. session 一覧を絞る repo は実装が持つ
pub trait Devin: Send + Sync + 'static {
    /// repo の session のうち since 以降に更新されたもの
    fn fetch_sessions(&self, cfg: &DevinConfig, since: DateTime<Utc>) -> Result<Vec<DevinSession>>;
    fn fetch_messages(&self, cfg: &DevinConfig, id: &str) -> Result<Vec<DevinMessage>>;
    /// 送信後の session を返す
    fn send_message(&self, cfg: &DevinConfig, id: &str, text: &str) -> Result<DevinSession>;
}

/// PR 一覧のキャッシュ
pub trait CacheStore: Send + Sync + 'static {
    /// 無い, 読めないときは None
    fn load(&self) -> Option<Cache>;
    fn save(&self, cache: &Cache) -> Result<()>;
}

/// ブラウザとクリップボード
pub trait Desktop: Send + Sync + 'static {
    fn open_url(&self, url: &str) -> Result<()>;
    fn copy(&self, text: &str) -> Result<()>;
}

#[derive(Clone)]
pub struct Ports {
    pub gh: Arc<dyn Github>,
    pub git: Arc<dyn Git>,
    pub devin: Arc<dyn Devin>,
    pub cache: Arc<dyn CacheStore>,
    pub desktop: Arc<dyn Desktop>,
}

/// runtime の待ち時間と期間
#[derive(Clone, Copy)]
pub struct RuntimeConfig {
    /// 操作の反映を待って PR を取り直す間隔と回数. update-branch は GitHub 側で非同期に merge される
    pub watch_interval: Duration,
    pub watch_tries: usize,
    /// Devin session 一覧を取る期間
    pub sessions_since_days: i64,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            watch_interval: Duration::from_secs(2),
            watch_tries: 10,
            sessions_since_days: 30,
        }
    }
}
