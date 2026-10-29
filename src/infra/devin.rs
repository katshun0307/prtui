#![allow(dead_code)] // 骨組み. 実装時に外す

use anyhow::Result;
use chrono::{DateTime, Utc};

use crate::domain::devin::{DevinConfig, DevinMessage, DevinSession};

/// repo の session のうち since 以降に更新されたもの
pub fn fetch_sessions(
    cfg: &DevinConfig,
    repo: &str,
    since: DateTime<Utc>,
) -> Result<Vec<DevinSession>> {
    let _ = (cfg, repo, since);
    todo!()
}

/// session のメッセージを古い順にすべて
pub fn fetch_messages(cfg: &DevinConfig, id: &str) -> Result<Vec<DevinMessage>> {
    let _ = (cfg, id);
    todo!()
}

/// 送信後の session を返す. suspended の session は送信で再開する
pub fn send_message(cfg: &DevinConfig, id: &str, text: &str) -> Result<DevinSession> {
    let _ = (cfg, id, text);
    todo!()
}

/// session 一覧の 1 ページと次のページのカーソル
pub(crate) fn parse_sessions(json: &str) -> Result<(Vec<DevinSession>, Option<String>)> {
    let _ = json;
    todo!()
}

/// メッセージ一覧の 1 ページと次のページのカーソル
pub(crate) fn parse_messages(json: &str) -> Result<(Vec<DevinMessage>, Option<String>)> {
    let _ = json;
    todo!()
}

pub(crate) fn parse_session(json: &str) -> Result<DevinSession> {
    let _ = json;
    todo!()
}
