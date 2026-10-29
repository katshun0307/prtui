use std::path::Path;
use std::process::Command;

use anyhow::{Result, bail};

/// dir でコマンドを実行し, 成功時の stdout を返す. 失敗時は stderr をエラーにする.
pub fn run(dir: &Path, program: &str, args: &[&str]) -> Result<String> {
    let out = Command::new(program).args(args).current_dir(dir).output()?;
    if !out.status.success() {
        bail!(
            "{program} {}: {}",
            args.first().unwrap_or(&""),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

pub fn open_url(dir: &Path, url: &str) -> Result<()> {
    run(dir, "open", &[url]).map(drop)
}
