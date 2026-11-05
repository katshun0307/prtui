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

/// テスト用の手書きの fake. 返す結果は Mutex に入れて差し替え, 呼び出しは記録する.
/// `anyhow::Error` は Clone できないので, 結果は `Result<_, String>` で持つ
#[cfg(test)]
pub(crate) mod fake {
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    use anyhow::{Result, anyhow};
    use chrono::{DateTime, Utc};

    use super::{CacheStore, Desktop, Devin, Git, Github, Ports};
    use crate::domain::cache::Cache;
    use crate::domain::devin::{DevinConfig, DevinMessage, DevinSession};
    use crate::domain::local::LocalState;
    use crate::domain::op::PrOp;
    use crate::domain::pr::Pr;

    fn cloned<T: Clone>(r: &Mutex<Result<T, String>>) -> Result<T> {
        r.lock().unwrap().clone().map_err(|e| anyhow!(e))
    }

    pub struct FakeGh {
        /// 呼ばれた順の `<関数名> <引数>`
        pub calls: Mutex<Vec<String>>,
        pub prs: Mutex<Result<Vec<Pr>, String>>,
        pub reviews: Mutex<Result<Vec<Pr>, String>>,
        pub custom: Mutex<Result<Vec<Pr>, String>>,
        /// `fetch_pr` が順に返す結果. 最後の 1 つは使い切らずに返し続ける. 空なら Ok(None)
        pub pr: Mutex<VecDeque<Result<Option<Pr>, String>>>,
        /// update_branch, checkout, change_state, view_web の結果
        pub op: Mutex<Result<String, String>>,
        pub linear: Mutex<Result<Option<String>, String>>,
    }

    impl Default for FakeGh {
        fn default() -> Self {
            Self {
                calls: Mutex::default(),
                prs: Mutex::new(Ok(vec![])),
                reviews: Mutex::new(Ok(vec![])),
                custom: Mutex::new(Ok(vec![])),
                pr: Mutex::default(),
                op: Mutex::new(Ok(String::new())),
                linear: Mutex::new(Ok(None)),
            }
        }
    }

    impl FakeGh {
        fn record(&self, call: String) {
            self.calls.lock().unwrap().push(call);
        }

        pub fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl Github for FakeGh {
        fn fetch_prs(&self) -> Result<Vec<Pr>> {
            self.record("fetch_prs".into());
            cloned(&self.prs)
        }

        fn fetch_reviews(&self) -> Result<Vec<Pr>> {
            self.record("fetch_reviews".into());
            cloned(&self.reviews)
        }

        fn fetch_custom(&self, query: &str) -> Result<Vec<Pr>> {
            self.record(format!("fetch_custom {query}"));
            cloned(&self.custom)
        }

        fn fetch_pr(&self, number: u64) -> Result<Option<Pr>> {
            self.record(format!("fetch_pr {number}"));
            let mut q = self.pr.lock().unwrap();
            let res = if q.len() > 1 {
                q.pop_front()
            } else {
                q.front().cloned()
            };
            res.unwrap_or(Ok(None)).map_err(|e| anyhow!(e))
        }

        fn update_branch(&self, number: u64, head_oid: &str) -> Result<String> {
            self.record(format!("update_branch {number} {head_oid}"));
            cloned(&self.op)
        }

        fn checkout(&self, number: u64, force: bool) -> Result<String> {
            self.record(format!("checkout {number} {force}"));
            cloned(&self.op)
        }

        fn change_state(&self, number: u64, op: PrOp, targets: &[String]) -> Result<String> {
            self.record(format!(
                "change_state {number} {op:?} {}",
                targets.join(",")
            ));
            cloned(&self.op)
        }

        fn view_web(&self, number: u64) -> Result<()> {
            self.record(format!("view_web {number}"));
            cloned(&self.op).map(drop)
        }

        fn linear_review_url(&self, number: u64) -> Result<Option<String>> {
            self.record(format!("linear_review_url {number}"));
            cloned(&self.linear)
        }
    }

    #[derive(Default)]
    pub struct FakeGit {
        pub local: Mutex<LocalState>,
    }

    impl Git for FakeGit {
        fn read_local_state(&self) -> Result<LocalState> {
            Ok(self.local.lock().unwrap().clone())
        }
    }

    pub struct FakeDevin {
        pub calls: Mutex<Vec<String>>,
        pub sessions: Mutex<Result<Vec<DevinSession>, String>>,
        pub messages: Mutex<Result<Vec<DevinMessage>, String>>,
        pub sent: Mutex<Result<DevinSession, String>>,
    }

    impl Default for FakeDevin {
        fn default() -> Self {
            Self {
                calls: Mutex::default(),
                sessions: Mutex::new(Ok(vec![])),
                messages: Mutex::new(Ok(vec![])),
                sent: Mutex::new(Err("送信結果が未設定".into())),
            }
        }
    }

    impl FakeDevin {
        fn record(&self, call: String) {
            self.calls.lock().unwrap().push(call);
        }
    }

    impl Devin for FakeDevin {
        fn fetch_sessions(
            &self,
            _: &DevinConfig,
            since: DateTime<Utc>,
        ) -> Result<Vec<DevinSession>> {
            self.record(format!("fetch_sessions {}", since.timestamp()));
            cloned(&self.sessions)
        }

        fn fetch_messages(&self, _: &DevinConfig, id: &str) -> Result<Vec<DevinMessage>> {
            self.record(format!("fetch_messages {id}"));
            cloned(&self.messages)
        }

        fn send_message(&self, _: &DevinConfig, id: &str, text: &str) -> Result<DevinSession> {
            self.record(format!("send_message {id} {text}"));
            cloned(&self.sent)
        }
    }

    /// ファイルの代わりにメモリに持つキャッシュ
    #[derive(Default)]
    pub struct MemCache {
        pub cache: Mutex<Option<Cache>>,
    }

    impl CacheStore for MemCache {
        fn load(&self) -> Option<Cache> {
            self.cache.lock().unwrap().clone()
        }

        fn save(&self, cache: &Cache) -> Result<()> {
            *self.cache.lock().unwrap() = Some(cache.clone());
            Ok(())
        }
    }

    #[derive(Default)]
    pub struct FakeDesktop {
        pub opened: Mutex<Vec<String>>,
        pub copied: Mutex<Vec<String>>,
        /// Some ならこのメッセージで失敗させる
        pub fail: Mutex<Option<String>>,
    }

    impl FakeDesktop {
        fn check(&self) -> Result<()> {
            match self.fail.lock().unwrap().as_ref() {
                Some(e) => Err(anyhow!(e.clone())),
                None => Ok(()),
            }
        }
    }

    impl Desktop for FakeDesktop {
        fn open_url(&self, url: &str) -> Result<()> {
            self.check()?;
            self.opened.lock().unwrap().push(url.into());
            Ok(())
        }

        fn copy(&self, text: &str) -> Result<()> {
            self.check()?;
            self.copied.lock().unwrap().push(text.into());
            Ok(())
        }
    }

    /// fake 一式. `ports` で App に渡し, テストからは各 fake を直接見る
    #[derive(Default)]
    pub struct Fakes {
        pub gh: Arc<FakeGh>,
        pub git: Arc<FakeGit>,
        pub devin: Arc<FakeDevin>,
        pub cache: Arc<MemCache>,
        pub desktop: Arc<FakeDesktop>,
    }

    impl Fakes {
        pub fn ports(&self) -> Ports {
            Ports {
                gh: self.gh.clone(),
                git: self.git.clone(),
                devin: self.devin.clone(),
                cache: self.cache.clone(),
                desktop: self.desktop.clone(),
            }
        }
    }
}
