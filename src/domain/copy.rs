use crate::domain::linear::{issue_url, linear_issue_key};
use crate::domain::pr::Pr;

/// `c` のコピー選択画面の 1 行. 値が作れない項目は理由を持って灰色で出す
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CopyItem {
    pub key: char,
    pub label: &'static str,
    /// コピーする文字列. 作れなければ Err に理由を入れる
    pub value: Result<String, String>,
}

/// PR からコピーできる値を選択キーの順に並べる
pub fn copy_items(pr: &Pr, linear_workspace: Option<&str>) -> Vec<CopyItem> {
    let key = linear_issue_key(&pr.head_ref_name, &pr.title);
    let no_key = || "issue キーが無い".to_string();
    let item = |key, label, value| CopyItem { key, label, value };
    vec![
        item('b', "ブランチ名", Ok(pr.head_ref_name.clone())),
        item('n', "PR 番号", Ok(format!("#{}", pr.number))),
        item('t', "タイトル", Ok(pr.title.clone())),
        item('u', "PR の URL", Ok(pr.url.clone())),
        item(
            'm',
            "Markdown リンク",
            Ok(format!("[#{} {}]({})", pr.number, pr.title, pr.url)),
        ),
        item('i', "Linear issue キー", key.clone().ok_or_else(no_key)),
        item(
            'l',
            "Linear issue の URL",
            match (linear_workspace, &key) {
                (None, _) => Err("prtui.linear-workspace が未設定".into()),
                (_, None) => Err(no_key()),
                (Some(ws), Some(k)) => Ok(issue_url(ws, k)),
            },
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::copy_items;
    use crate::domain::pr::Pr;

    fn value(items: &[super::CopyItem], key: char) -> Result<&str, &str> {
        let item = items.iter().find(|i| i.key == key).unwrap();
        item.value.as_deref().map_err(String::as_str)
    }

    #[test]
    fn values_from_pr() {
        let mut pr = Pr::fixture(12, "abc-7-fix", "main");
        pr.title = "直す".into();
        pr.url = "https://github.com/o/r/pull/12".into();
        let items = copy_items(&pr, Some("ws"));
        assert_eq!(value(&items, 'b'), Ok("abc-7-fix"));
        assert_eq!(value(&items, 'n'), Ok("#12"));
        assert_eq!(
            value(&items, 'm'),
            Ok("[#12 直す](https://github.com/o/r/pull/12)")
        );
        assert_eq!(value(&items, 'i'), Ok("ABC-7"));
        assert_eq!(value(&items, 'l'), Ok("https://linear.app/ws/issue/ABC-7"));
    }

    #[test]
    fn linear_needs_key_and_workspace() {
        let mut pr = Pr::fixture(1, "feature/foo", "main");
        pr.title = "キー無し".into();
        let items = copy_items(&pr, None);
        assert!(value(&items, 'i').is_err());
        assert_eq!(value(&items, 'l'), Err("prtui.linear-workspace が未設定"));
        let items = copy_items(&pr, Some("ws"));
        assert_eq!(value(&items, 'l'), Err("issue キーが無い"));
    }

    #[test]
    fn keys_are_unique() {
        let items = copy_items(&Pr::fixture(1, "b", "main"), None);
        let mut keys: Vec<char> = items.iter().map(|i| i.key).collect();
        keys.sort();
        keys.dedup();
        assert_eq!(keys.len(), items.len());
    }
}
