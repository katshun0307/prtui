use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::domain::devin::DevinSession;
use crate::domain::pr::Pr;

/// 前回取得した PR 一覧. 起動直後にこれを出し, 裏で取り直す
#[derive(Serialize, Deserialize)]
pub struct Cache {
    pub fetched_at: DateTime<Utc>,
    pub prs: Vec<Pr>,
    /// 前回取得した Devin session. Devin を設定していなければ空
    #[serde(default)]
    pub devin_sessions: Vec<DevinSession>,
    /// レビュータブの PR 一覧と, それを取得した時刻. 未取得なら None
    #[serde(default)]
    pub reviews: Option<(DateTime<Utc>, Vec<Pr>)>,
    /// 追加のタブの PR 一覧. 起動時にはクエリが今の設定と一致するタブだけ戻す
    #[serde(default)]
    pub custom: Vec<CustomCache>,
}

#[derive(Serialize, Deserialize)]
pub struct CustomCache {
    pub query: String,
    pub fetched_at: DateTime<Utc>,
    pub prs: Vec<Pr>,
}
