#![allow(dead_code)] // 骨組み. 実装時に外す

use std::fmt;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// git config `prtui.devin-token` と `prtui.devin-org`. 両方そろったときだけ作る
#[derive(Clone, PartialEq)]
pub struct DevinConfig {
    pub token: String,
    /// `org-` で始まる organization ID
    pub org: String,
}

/// `{:?}` で token が出ないように伏せる
impl fmt::Debug for DevinConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DevinConfig")
            .field("token", &"***")
            .field("org", &self.org)
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DevinStatus {
    New,
    Claimed,
    Running,
    Exit,
    Error,
    Suspended,
    Resuming,
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DevinSession {
    /// `devin-` で始まる session ID
    pub id: String,
    pub status: DevinStatus,
    pub title: Option<String>,
    /// Devin の web 画面の URL
    pub url: String,
    pub updated_at: DateTime<Utc>,
    /// session が作った, または触った PR の URL
    pub pr_urls: Vec<String>,
    pub is_archived: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DevinMessage {
    pub from_devin: bool,
    pub username: Option<String>,
    pub text: String,
    pub created_at: DateTime<Utc>,
}

/// PR の URL に紐づく session のうち updated_at が最新のものと, 紐づく session の件数.
/// URL は末尾の `/` と大文字小文字を無視して比べる
pub fn session_for<'a>(
    sessions: &'a [DevinSession],
    pr_url: &str,
) -> Option<(&'a DevinSession, usize)> {
    let _ = (sessions, pr_url);
    todo!()
}

#[cfg(test)]
impl DevinSession {
    pub(crate) fn fixture(id: &str, pr_url: &str) -> DevinSession {
        DevinSession {
            id: id.into(),
            status: DevinStatus::Running,
            title: None,
            url: format!("https://app.devin.ai/sessions/{id}"),
            updated_at: Default::default(),
            pr_urls: vec![pr_url.into()],
            is_archived: false,
        }
    }
}
