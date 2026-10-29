use std::path::Path;
use std::process::{Command, Output};

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

/// stdin に文字列を流してコマンドを実行し, 終了状態と出力をそのまま返す.
/// 失敗時の stdout も要る呼び出し (curl の --fail-with-body) 向け
pub fn run_with_stdin(dir: &Path, program: &str, args: &[&str], stdin: &str) -> Result<Output> {
    let _ = (dir, program, args, stdin);
    todo!()
}

pub fn open_url(dir: &Path, url: &str) -> Result<()> {
    run(dir, "open", &[url]).map(drop)
}
