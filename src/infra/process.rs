use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};

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
    let mut child = Command::new(program)
        .args(args)
        .current_dir(dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    // 流す先 (curl の設定, pbcopy, gh pr edit の本文) はどれも stdin を読み切ってから出力するので,
    // 書き終えてから出力を読んでも詰まらない.
    // ブロックを抜けて stdin を閉じないと, 子が EOF を待ち続ける
    if let Some(mut w) = child.stdin.take() {
        w.write_all(stdin.as_bytes())?;
    }
    Ok(child.wait_with_output()?)
}

pub fn open_url(dir: &Path, url: &str) -> Result<()> {
    run(dir, "open", &[url]).map(drop)
}

/// `open` と同じく macOS 前提で pbcopy を使う
pub fn copy_to_clipboard(dir: &Path, text: &str) -> Result<()> {
    let out = run_with_stdin(dir, "pbcopy", &[], text)?;
    if !out.status.success() {
        bail!("pbcopy: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(())
}
