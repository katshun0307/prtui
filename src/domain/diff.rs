//! PR ビューの diff. `gh pr diff` の unified diff をファイルごとに分け, 描画する行の並びを決める

/// diff の 1 行の種類
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineKind {
    /// `@@ -a,b +c,d @@`
    Hunk,
    Add,
    Del,
    Context,
    /// `\ No newline at end of file`
    Note,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffLine {
    pub kind: LineKind,
    /// 先頭の `+` / `-` / ` ` を含めた行
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileStatus {
    Added,
    Deleted,
    Renamed,
    Modified,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileDiff {
    /// 変更後のパス. 削除なら変更前のパス
    pub path: String,
    /// rename のときの変更前のパス
    pub old_path: Option<String>,
    pub status: FileStatus,
    pub binary: bool,
    pub additions: usize,
    pub deletions: usize,
    /// 最初の hunk 以降の行. `index` や `---` などのヘッダ行は持たない
    pub lines: Vec<DiffLine>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Diff {
    pub files: Vec<FileDiff>,
}

/// 描画する 1 行. ファイルごとに見出しの行, diff の行を並べ, ファイルの間に空行を 1 つ挟む
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Row<'a> {
    File(&'a FileDiff),
    Line(&'a DiffLine),
    Blank,
}

impl Diff {
    /// unified diff を読む. 解釈できない行は捨てる
    pub fn parse(text: &str) -> Self {
        let mut files: Vec<FileDiff> = Vec::new();
        let mut in_hunk = false;
        for line in text.lines() {
            if let Some(rest) = line.strip_prefix("diff --git ") {
                files.push(FileDiff {
                    path: header_path(rest),
                    old_path: None,
                    status: FileStatus::Modified,
                    binary: false,
                    additions: 0,
                    deletions: 0,
                    lines: Vec::new(),
                });
                in_hunk = false;
                continue;
            }
            let Some(f) = files.last_mut() else {
                continue;
            };
            if line.starts_with("@@") {
                in_hunk = true;
                f.lines.push(diff_line(LineKind::Hunk, line));
                continue;
            }
            if in_hunk {
                // hunk の中の `---` や `+++` は内容の行なので, ヘッダとして読まない
                let kind = match line.as_bytes().first() {
                    Some(b'+') => {
                        f.additions += 1;
                        LineKind::Add
                    }
                    Some(b'-') => {
                        f.deletions += 1;
                        LineKind::Del
                    }
                    Some(b'\\') => LineKind::Note,
                    _ => LineKind::Context,
                };
                f.lines.push(diff_line(kind, line));
                continue;
            }
            if line.starts_with("new file mode") {
                f.status = FileStatus::Added;
            } else if line.starts_with("deleted file mode") {
                f.status = FileStatus::Deleted;
            } else if let Some(p) = line.strip_prefix("rename from ") {
                f.status = FileStatus::Renamed;
                f.old_path = Some(p.to_string());
            } else if let Some(p) = line.strip_prefix("rename to ") {
                f.path = p.to_string();
            } else if let Some(p) = line.strip_prefix("+++ b/") {
                f.path = p.to_string();
            } else if let Some(p) = line.strip_prefix("--- a/") {
                // 削除のときは `+++ /dev/null` なので変更前のパスを使う
                if f.status == FileStatus::Deleted {
                    f.path = p.to_string();
                }
            } else if line.starts_with("Binary files") || line == "GIT binary patch" {
                f.binary = true;
            }
        }
        Self { files }
    }

    /// 全ファイルの追加と削除の行数
    pub fn totals(&self) -> (usize, usize) {
        self.files
            .iter()
            .fold((0, 0), |(a, d), f| (a + f.additions, d + f.deletions))
    }

    pub fn rows(&self) -> impl Iterator<Item = Row<'_>> {
        self.files.iter().enumerate().flat_map(|(i, f)| {
            (i > 0)
                .then_some(Row::Blank)
                .into_iter()
                .chain(std::iter::once(Row::File(f)))
                .chain(f.lines.iter().map(Row::Line))
        })
    }

    pub fn row_count(&self) -> usize {
        self.file_starts().last().map_or(0, |&s| {
            s + 1 + self.files.last().map_or(0, |f| f.lines.len())
        })
    }

    /// 各ファイルの見出しの行の位置. `rows` の並びと合わせる
    pub fn file_starts(&self) -> Vec<usize> {
        let mut at = 0;
        self.files
            .iter()
            .map(|f| {
                let start = at;
                at += f.lines.len() + 2;
                start
            })
            .collect()
    }
}

fn diff_line(kind: LineKind, text: &str) -> DiffLine {
    DiffLine {
        kind,
        text: text.to_string(),
    }
}

/// `a/<path> b/<path>` から変更後のパスを取る. `+++` や `rename to` が無いとき (バイナリやモードだけの変更) に使う
fn header_path(rest: &str) -> String {
    let rest = rest.strip_prefix("a/").unwrap_or(rest);
    // rename でなければ a と b は同じパスなので, 真ん中の ` b/` で分ける
    let n = rest.len();
    if n > 3 && (n - 3).is_multiple_of(2) {
        let half = (n - 3) / 2;
        if rest.get(half..half + 3) == Some(" b/") {
            return rest[half + 3..].to_string();
        }
    }
    match rest.find(" b/") {
        Some(i) => rest[i + 3..].to_string(),
        None => rest.to_string(),
    }
}

/// 位置 at から delta 個先 (負なら前) のファイルの見出しの位置.
/// 前に戻るとき, 今のファイルの途中にいればその見出しに戻る. 端を越えたら None
pub fn jump_file(starts: &[usize], at: usize, delta: isize) -> Option<usize> {
    if delta > 0 {
        starts
            .iter()
            .filter(|&&s| s > at)
            .nth(delta as usize - 1)
            .copied()
    } else {
        starts
            .iter()
            .rev()
            .filter(|&&s| s < at)
            .nth(delta.unsigned_abs().saturating_sub(1))
            .copied()
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    const DIFF: &str = "\
diff --git a/src/a.rs b/src/a.rs
index 1111111..2222222 100644
--- a/src/a.rs
+++ b/src/a.rs
@@ -1,3 +1,3 @@ fn main() {
 fn a() {}
--- not a header
+++ not a header
\\ No newline at end of file
diff --git a/new.txt b/new.txt
new file mode 100644
index 0000000..3333333
--- /dev/null
+++ b/new.txt
@@ -0,0 +1 @@
+hello
diff --git a/old.txt b/old.txt
deleted file mode 100644
index 4444444..0000000
--- a/old.txt
+++ /dev/null
@@ -1 +0,0 @@
-bye
diff --git a/x y.rs b/z.rs
similarity index 90%
rename from x y.rs
rename to z.rs
diff --git a/img.png b/img.png
index 5555555..6666666 100644
Binary files a/img.png and b/img.png differ
";

    #[test]
    fn parses_files() {
        let d = Diff::parse(DIFF);
        let summary: Vec<_> = d
            .files
            .iter()
            .map(|f| {
                (
                    f.path.as_str(),
                    f.old_path.as_deref(),
                    f.status,
                    f.binary,
                    f.additions,
                    f.deletions,
                    f.lines.len(),
                )
            })
            .collect();
        assert_eq!(
            summary,
            vec![
                ("src/a.rs", None, FileStatus::Modified, false, 1, 1, 5),
                ("new.txt", None, FileStatus::Added, false, 1, 0, 2),
                ("old.txt", None, FileStatus::Deleted, false, 0, 1, 2),
                ("z.rs", Some("x y.rs"), FileStatus::Renamed, false, 0, 0, 0),
                ("img.png", None, FileStatus::Modified, true, 0, 0, 0),
            ]
        );
        let kinds: Vec<_> = d.files[0].lines.iter().map(|l| l.kind).collect();
        assert_eq!(
            kinds,
            vec![
                LineKind::Hunk,
                LineKind::Context,
                LineKind::Del,
                LineKind::Add,
                LineKind::Note
            ]
        );
        assert_eq!(d.totals(), (2, 2));
    }

    #[test]
    fn header_path_handles_spaces() {
        assert_eq!(header_path("a/a b.rs b/a b.rs"), "a b.rs");
        assert_eq!(header_path("a/x.rs b/y.rs"), "y.rs");
    }

    #[test]
    fn rows_match_file_starts() {
        let d = Diff::parse(DIFF);
        let rows: Vec<_> = d.rows().collect();
        assert_eq!(rows.len(), d.row_count());
        let starts: Vec<usize> = rows
            .iter()
            .enumerate()
            .filter(|(_, r)| matches!(r, Row::File(_)))
            .map(|(i, _)| i)
            .collect();
        assert_eq!(starts, d.file_starts());
        assert_eq!(starts, vec![0, 7, 11, 15, 17]);
        assert_eq!(Diff::default().row_count(), 0);
    }

    #[test]
    fn jumps_between_files() {
        let starts = [0, 7, 10];
        assert_eq!(jump_file(&starts, 0, 1), Some(7));
        assert_eq!(jump_file(&starts, 3, 1), Some(7));
        assert_eq!(jump_file(&starts, 10, 1), None);
        // 途中にいればそのファイルの先頭へ
        assert_eq!(jump_file(&starts, 8, -1), Some(7));
        assert_eq!(jump_file(&starts, 7, -1), Some(0));
        assert_eq!(jump_file(&starts, 0, -1), None);
    }
}
