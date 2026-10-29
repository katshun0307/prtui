use std::collections::HashSet;
use std::path::PathBuf;
use std::time::Instant;

use chrono::{DateTime, Utc};
use ratatui::widgets::TableState;

use crate::domain::local::LocalState;
use crate::domain::op::PrOp;
use crate::domain::pr::Pr;

pub struct Message {
    pub text: String,
    pub is_error: bool,
}

pub struct AppState {
    pub dir: PathBuf,
    pub repo: String,
    pub prs: Vec<Pr>,
    pub local: LocalState,
    pub draft_only: bool,
    pub table: TableState,
    pub message: Option<Message>,
    pub loading: bool,
    pub busy: Option<String>,
    /// base 取り込みの確認待ちの PR 番号と base ブランチ名
    pub confirm_update: Option<(u64, String)>,
    /// 2 回目のキー入力待ちの状態変更 (draft, close)
    pub confirm_op: Option<(u64, PrOp)>,
    pub last_fetch: Option<Instant>,
    /// `prs` の全件を取得した時刻. キャッシュから読んだときはキャッシュを書いた時刻
    pub fetched_at: Option<DateTime<Utc>>,
    /// `prs` がキャッシュのままで, 起動後の取得がまだ終わっていない
    pub from_cache: bool,
    /// 操作の反映を待って取り直している PR 番号
    pub syncing: HashSet<u64>,
}

impl AppState {
    pub fn new(dir: PathBuf, repo: String, local: LocalState) -> Self {
        Self {
            dir,
            repo,
            prs: Vec::new(),
            local,
            draft_only: false,
            table: TableState::default().with_selected(0),
            message: None,
            loading: false,
            busy: None,
            confirm_update: None,
            confirm_op: None,
            last_fetch: None,
            fetched_at: None,
            from_cache: false,
            syncing: HashSet::new(),
        }
    }

    pub fn visible(&self) -> Vec<&Pr> {
        self.prs
            .iter()
            .filter(|p| !self.draft_only || p.is_draft)
            .collect()
    }

    pub fn selected(&self) -> Option<&Pr> {
        let i = self.table.selected()?;
        self.visible().get(i).copied()
    }

    pub fn pr(&self, n: u64) -> Option<&Pr> {
        self.prs.iter().find(|p| p.number == n)
    }

    /// 一覧を差し替える. 並びが変わっても同じ PR を選んだままにする
    pub fn replace_prs(&mut self, prs: Vec<Pr>) {
        let sel = self.selected().map(|p| p.number);
        self.prs = prs;
        if let Some(i) = sel.and_then(|n| self.visible().iter().position(|p| p.number == n)) {
            self.table.select(Some(i));
        }
        self.clamp_selection();
    }

    pub fn move_by(&mut self, delta: isize) {
        let len = self.visible().len();
        if len == 0 {
            return;
        }
        let cur = self.table.selected().unwrap_or(0) as isize;
        self.table
            .select(Some((cur + delta).clamp(0, len as isize - 1) as usize));
    }

    pub fn clamp_selection(&mut self) {
        let len = self.visible().len();
        let cur = self.table.selected().unwrap_or(0);
        self.table.select(Some(cur.min(len.saturating_sub(1))));
    }

    pub fn info(&mut self, text: String) {
        self.message = Some(Message {
            text,
            is_error: false,
        });
    }

    pub fn error(&mut self, text: String) {
        self.message = Some(Message {
            text,
            is_error: true,
        });
    }
}
