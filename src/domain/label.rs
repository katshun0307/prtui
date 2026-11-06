//! `a` のラベル選択画面で使う, repo のラベルと付け外しの差分

use serde::{Deserialize, Serialize};

/// repo のラベル. `gh label list --json name,color,description` の 1 件
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Label {
    pub name: String,
    /// `d73a4a` の形の 16 進. PR にだけ付いていて repo の一覧に無いものは空
    #[serde(default)]
    pub color: String,
    #[serde(default)]
    pub description: String,
}

impl Label {
    /// 色を (r, g, b) にする. 6 桁の 16 進でなければ None
    pub fn rgb(&self) -> Option<(u8, u8, u8)> {
        let c = self.color.trim_start_matches('#');
        if c.len() != 6 {
            return None;
        }
        let byte = |i| u8::from_str_radix(c.get(i..i + 2)?, 16).ok();
        Some((byte(0)?, byte(2)?, byte(4)?))
    }
}

/// GitHub のラベル名は大文字小文字を区別しない
pub fn same_label(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}

pub fn has_label(labels: &[String], name: &str) -> bool {
    labels.iter().any(|l| same_label(l, name))
}

/// 選択画面に並べるラベル. PR に付いているもの (`attached`) を先に付いている順で, 残りを名前順に並べる.
/// repo の一覧に無いラベル (一覧の取得前など) も `attached` にあれば出す.
/// `query` は名前の部分一致 (大文字小文字を無視) で絞る
pub fn label_choices(repo: &[Label], attached: &[String], query: &str) -> Vec<Label> {
    let q = query.to_lowercase();
    let mut rest: Vec<&Label> = repo
        .iter()
        .filter(|l| !has_label(attached, &l.name))
        .collect();
    rest.sort_by_key(|l| l.name.to_lowercase());
    attached
        .iter()
        .map(|name| {
            repo.iter()
                .find(|l| same_label(&l.name, name))
                .cloned()
                .unwrap_or_else(|| Label {
                    name: name.clone(),
                    ..Label::default()
                })
        })
        .chain(rest.into_iter().cloned())
        .filter(|l| l.name.to_lowercase().contains(&q))
        .collect()
}

/// 付け外しの差分. before に無く after にあるものを足し, before にあって after に無いものを外す
pub fn label_diff(before: &[String], after: &[String]) -> (Vec<String>, Vec<String>) {
    let add = after
        .iter()
        .filter(|l| !has_label(before, l))
        .cloned()
        .collect();
    let remove = before
        .iter()
        .filter(|l| !has_label(after, l))
        .cloned()
        .collect();
    (add, remove)
}

/// labels に add を足して remove を外した結果. 反映待ちの終了条件に使う
pub fn apply_diff(labels: &[String], add: &[String], remove: &[String]) -> Vec<String> {
    labels
        .iter()
        .filter(|l| !has_label(remove, l))
        .chain(add.iter().filter(|l| !has_label(labels, l)))
        .cloned()
        .collect()
}

/// 2 つのラベルの集合が等しいか. 順番と大文字小文字は見ない
pub fn same_labels(a: &[String], b: &[String]) -> bool {
    a.iter().all(|l| has_label(b, l)) && b.iter().all(|l| has_label(a, l))
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    fn label(name: &str) -> Label {
        Label {
            name: name.into(),
            color: "d73a4a".into(),
            description: String::new(),
        }
    }

    fn strings(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    fn names(v: &[Label]) -> Vec<&str> {
        v.iter().map(|l| l.name.as_str()).collect()
    }

    #[test]
    fn rgb_parses_hex() {
        assert_eq!(label("a").rgb(), Some((0xd7, 0x3a, 0x4a)));
        let mut l = label("a");
        l.color = "#00ff10".into();
        assert_eq!(l.rgb(), Some((0, 255, 16)));
        l.color = "fff".into();
        assert_eq!(l.rgb(), None);
        l.color = "zzzzzz".into();
        assert_eq!(l.rgb(), None);
        l.color = String::new();
        assert_eq!(l.rgb(), None);
    }

    #[test]
    fn choices_attached_first_then_by_name() {
        let repo = vec![label("wontfix"), label("Bug"), label("docs"), label("api")];
        let attached = strings(&["docs", "only-on-pr"]);
        let c = label_choices(&repo, &attached, "");
        assert_eq!(names(&c), ["docs", "only-on-pr", "api", "Bug", "wontfix"]);
        // repo の色を使い, 一覧に無いものは色無し
        assert_eq!(c[0].color, "d73a4a");
        assert_eq!(c[1].color, "");
    }

    #[test]
    fn choices_filtered_case_insensitively() {
        let repo = vec![label("Bug"), label("debug-only"), label("docs")];
        let c = label_choices(&repo, &strings(&["docs"]), "BUG");
        assert_eq!(names(&c), ["Bug", "debug-only"]);
        assert!(label_choices(&repo, &[], "nothing").is_empty());
    }

    #[test]
    fn choices_attached_matches_repo_case_insensitively() {
        let repo = vec![label("Bug")];
        let c = label_choices(&repo, &strings(&["bug"]), "");
        assert_eq!(names(&c), ["Bug"]);
    }

    #[test]
    fn diff_add_and_remove() {
        let before = strings(&["bug", "docs"]);
        let after = strings(&["Docs", "feature"]);
        assert_eq!(
            label_diff(&before, &after),
            (strings(&["feature"]), strings(&["bug"]))
        );
        assert_eq!(label_diff(&before, &before), (vec![], vec![]));
    }

    #[test]
    fn apply_diff_and_compare() {
        let labels = strings(&["bug", "docs"]);
        let after = apply_diff(&labels, &strings(&["feature", "BUG"]), &strings(&["docs"]));
        assert_eq!(after, strings(&["bug", "feature"]));
        assert!(same_labels(&after, &strings(&["Feature", "bug"])));
        assert!(!same_labels(&after, &strings(&["bug"])));
        assert!(!same_labels(&strings(&["bug"]), &after));
    }
}
