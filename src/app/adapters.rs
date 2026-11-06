//! ports の trait を infra の関数に繋ぐ. app から infra を呼ぶのはここだけ

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use chrono::{DateTime, Utc};

use super::ports::{CacheStore, Desktop, Devin, Git, Github, Ports};
use crate::domain::cache::Cache;
use crate::domain::devin::{DevinConfig, DevinMessage, DevinSession};
use crate::domain::label::Label;
use crate::domain::local::LocalState;
use crate::domain::op::PrOp;
use crate::domain::pr::Pr;
use crate::infra::{cache, devin, gh, git, process};

pub struct GhCli {
    pub dir: PathBuf,
    pub repo: String,
}

impl Github for GhCli {
    fn fetch_prs(&self) -> Result<Vec<Pr>> {
        gh::fetch_prs(&self.dir, &self.repo)
    }

    fn fetch_reviews(&self) -> Result<Vec<Pr>> {
        gh::fetch_reviews(&self.dir, &self.repo)
    }

    fn fetch_custom(&self, query: &str) -> Result<Vec<Pr>> {
        gh::fetch_custom(&self.dir, &self.repo, query)
    }

    fn fetch_pr(&self, number: u64) -> Result<Option<Pr>> {
        gh::fetch_pr(&self.dir, &self.repo, number)
    }

    fn update_branch(&self, number: u64, head_oid: &str) -> Result<String> {
        gh::update_branch(&self.dir, &self.repo, number, head_oid)
    }

    fn checkout(&self, number: u64, force: bool) -> Result<String> {
        gh::checkout(&self.dir, number, force)
    }

    fn change_state(&self, number: u64, op: PrOp) -> Result<String> {
        gh::change_state(&self.dir, number, op)
    }

    fn request_reviews(&self, number: u64, targets: &[String]) -> Result<String> {
        gh::request_reviews(&self.dir, number, targets)
    }

    fn view_web(&self, number: u64) -> Result<()> {
        gh::view_web(&self.dir, number).map(drop)
    }

    fn linear_review_url(&self, number: u64) -> Result<Option<String>> {
        gh::linear_review_url(&self.dir, &self.repo, number)
    }

    fn fetch_labels(&self) -> Result<Vec<Label>> {
        gh::fetch_labels(&self.dir, &self.repo)
    }

    fn edit_labels(&self, number: u64, add: &[String], remove: &[String]) -> Result<String> {
        gh::edit_labels(&self.dir, number, add, remove)
    }
}

pub struct GitCli {
    pub dir: PathBuf,
}

impl Git for GitCli {
    fn read_local_state(&self) -> Result<LocalState> {
        git::read_local_state(&self.dir)
    }
}

pub struct DevinCurl {
    pub repo: String,
}

impl Devin for DevinCurl {
    fn fetch_sessions(&self, cfg: &DevinConfig, since: DateTime<Utc>) -> Result<Vec<DevinSession>> {
        devin::fetch_sessions(cfg, &self.repo, since)
    }

    fn fetch_messages(&self, cfg: &DevinConfig, id: &str) -> Result<Vec<DevinMessage>> {
        devin::fetch_messages(cfg, id)
    }

    fn send_message(&self, cfg: &DevinConfig, id: &str, text: &str) -> Result<DevinSession> {
        devin::send_message(cfg, id, text)
    }
}

pub struct FileCache {
    pub repo: String,
}

impl CacheStore for FileCache {
    fn load(&self) -> Option<Cache> {
        cache::load(&self.repo)
    }

    fn save(&self, c: &Cache) -> Result<()> {
        cache::save(&self.repo, c)
    }
}

/// `open` と `pbcopy` を使う
pub struct MacDesktop {
    pub dir: PathBuf,
}

impl Desktop for MacDesktop {
    fn open_url(&self, url: &str) -> Result<()> {
        process::open_url(&self.dir, url)
    }

    fn copy(&self, text: &str) -> Result<()> {
        process::copy_to_clipboard(&self.dir, text)
    }
}

/// dir (メイン checkout) と repo (`owner/name`) に対して実際のコマンドを叩く ports
pub fn real_ports(dir: PathBuf, repo: String) -> Ports {
    Ports {
        gh: Arc::new(GhCli {
            dir: dir.clone(),
            repo: repo.clone(),
        }),
        git: Arc::new(GitCli { dir: dir.clone() }),
        devin: Arc::new(DevinCurl { repo: repo.clone() }),
        cache: Arc::new(FileCache { repo }),
        desktop: Arc::new(MacDesktop { dir }),
    }
}
