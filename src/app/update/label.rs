//! `a` のラベル選択画面. 文字で絞り込み, Tab で付け外しを選び, Enter でまとめて反映する.
//! 何も選ばずに Enter を押したらカーソルの項目だけを切り替える (fzf と同じ)

use ratatui::crossterm::event::{KeyCode, KeyEvent};

use super::picker::{advance, clamp, picker_key};
use crate::app::event::Effect;
use crate::app::state::{AppState, LabelMenu};
use crate::domain::label::{has_label, label_diff, same_label, same_labels};

/// 選択中の PR のラベル選択画面を開き, repo のラベルを取り直す.
/// 前に取った一覧があればそれを出しておき, 届いたら差し替える
pub(super) fn open_labels(state: &mut AppState) -> Vec<Effect> {
    if state.busy.is_some() {
        return vec![];
    }
    let Some(pr) = state.selected() else {
        return vec![];
    };
    let menu = LabelMenu::new(pr.number, pr.label_names());
    state.push(menu);
    if state.labels_loading {
        return vec![];
    }
    state.labels_loading = true;
    vec![Effect::FetchLabels]
}

pub(super) fn label_key(state: &mut AppState, key: KeyEvent) -> Vec<Effect> {
    let len = state.label_choices().len();
    let Some(menu) = state.label_menu_mut() else {
        return vec![];
    };
    match key.code {
        KeyCode::Esc => {
            state.pop();
        }
        KeyCode::Enter => return apply(state),
        KeyCode::Tab => toggle_cursor(state),
        _ => {
            picker_key(&mut menu.query, &mut menu.list, len, key);
        }
    }
    vec![]
}

/// 選択肢の数が変わったときにカーソルを範囲に収める
pub(super) fn clamp_cursor(state: &mut AppState) {
    let len = state.label_choices().len();
    if let Some(menu) = state.label_menu_mut() {
        clamp(&mut menu.list, len);
    }
}

fn toggle(chosen: &mut Vec<String>, name: &str) {
    if has_label(chosen, name) {
        chosen.retain(|l| !same_label(l, name));
    } else {
        chosen.push(name.to_string());
    }
}

/// カーソルの項目を切り替えて次に進む. 末尾では止まる
fn toggle_cursor(state: &mut AppState) {
    let choices = state.label_choices();
    let Some(menu) = state.label_menu_mut() else {
        return;
    };
    let cur = menu.list.selected().unwrap_or(0);
    if let Some(l) = choices.get(cur) {
        toggle(&mut menu.chosen, &l.name);
        advance(&mut menu.list, choices.len());
    }
}

/// `+a -b` の形. メッセージに使う
pub(super) fn diff_text(add: &[String], remove: &[String]) -> String {
    add.iter()
        .map(|l| format!("+{l}"))
        .chain(remove.iter().map(|l| format!("-{l}")))
        .collect::<Vec<_>>()
        .join(" ")
}

fn apply(state: &mut AppState) -> Vec<Effect> {
    let choices = state.label_choices();
    let Some(menu) = state.label_menu_mut() else {
        return vec![];
    };
    if same_labels(&menu.chosen, &menu.before) {
        let Some(l) = menu.list.selected().and_then(|i| choices.get(i)) else {
            // 絞り込みで何も無ければ開いたままにする
            return vec![];
        };
        toggle(&mut menu.chosen, &l.name);
    }
    let Some(menu) = state.pop_label_menu() else {
        return vec![];
    };
    let n = menu.number;
    let (add, remove) = label_diff(&menu.before, &menu.chosen);
    state.busy = Some(format!(
        "#{n} のラベルを変えている… {}",
        diff_text(&add, &remove)
    ));
    vec![Effect::EditLabels {
        number: n,
        add,
        remove,
    }]
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    use super::super::test_util::{code, custom_state, key, message, pr, state};
    use super::super::{handle_event, handle_key};
    use crate::app::event::{Effect, Event, WatchUntil};
    use crate::app::state::AppState;
    use crate::domain::label::Label;

    fn label(name: &str) -> Label {
        Label {
            name: name.into(),
            color: "ededed".into(),
            description: String::new(),
        }
    }

    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    /// #1 に bug が付いていて, repo に bug, docs, feature がある
    fn labeled() -> AppState {
        let mut p = pr(1);
        p.labels = vec![label("bug")];
        let mut s = state(vec![p]);
        s.repo_labels = vec![label("feature"), label("bug"), label("docs")];
        s
    }

    fn type_str(s: &mut AppState, text: &str) {
        for c in text.chars() {
            assert!(handle_key(s, key(c)).is_empty());
        }
    }

    fn names(s: &AppState) -> Vec<String> {
        s.label_choices().into_iter().map(|l| l.name).collect()
    }

    fn cursor(s: &AppState) -> Option<usize> {
        s.label_menu().unwrap().list.selected()
    }

    #[test]
    fn open_fetches_labels_once() {
        let mut s = labeled();
        assert_eq!(handle_key(&mut s, key('a')), vec![Effect::FetchLabels]);
        assert!(s.labels_loading);
        let menu = s.label_menu().unwrap();
        assert_eq!(menu.number, 1);
        assert_eq!(menu.before, ["bug"]);
        // 付いているものが先, 残りは名前順
        assert_eq!(names(&s), ["bug", "docs", "feature"]);

        // 取得中に開き直しても取りに行かない
        handle_key(&mut s, code(KeyCode::Esc));
        assert!(s.label_menu().is_none());
        assert!(handle_key(&mut s, key('a')).is_empty());
        assert!(s.label_menu().is_some());
    }

    #[test]
    fn typing_filters_and_resets_cursor() {
        let mut s = labeled();
        handle_key(&mut s, key('a'));
        handle_key(&mut s, code(KeyCode::Down));
        assert_eq!(cursor(&s), Some(1));
        // j や q も絞り込みの文字として扱う
        type_str(&mut s, "fe");
        assert_eq!(names(&s), ["feature"]);
        assert_eq!(cursor(&s), Some(0));
        handle_key(&mut s, code(KeyCode::Backspace));
        assert_eq!(names(&s), ["feature"]);
        handle_key(&mut s, code(KeyCode::Backspace));
        assert_eq!(names(&s).len(), 3);
        type_str(&mut s, "q");
        assert!(names(&s).is_empty());
        handle_key(&mut s, ctrl('u'));
        assert_eq!(s.label_menu().unwrap().query, "");
    }

    #[test]
    fn cursor_wraps() {
        let mut s = labeled();
        handle_key(&mut s, key('a'));
        handle_key(&mut s, code(KeyCode::Up));
        assert_eq!(cursor(&s), Some(2));
        handle_key(&mut s, ctrl('n'));
        assert_eq!(cursor(&s), Some(0));
        handle_key(&mut s, ctrl('p'));
        assert_eq!(cursor(&s), Some(2));
    }

    #[test]
    fn tab_toggles_then_enter_applies_all() {
        let mut s = labeled();
        handle_key(&mut s, key('a'));
        // bug を外し, docs を足す. Tab は次に進む
        handle_key(&mut s, code(KeyCode::Tab));
        handle_key(&mut s, code(KeyCode::Tab));
        assert_eq!(cursor(&s), Some(2));
        assert_eq!(s.label_menu().unwrap().chosen, ["docs"]);
        assert_eq!(
            handle_key(&mut s, code(KeyCode::Enter)),
            vec![Effect::EditLabels {
                number: 1,
                add: vec!["docs".into()],
                remove: vec!["bug".into()],
            }]
        );
        assert!(s.label_menu().is_none());
        assert_eq!(
            s.busy.as_deref(),
            Some("#1 のラベルを変えている… +docs -bug")
        );
    }

    #[test]
    fn enter_without_choice_toggles_cursor() {
        let mut s = labeled();
        handle_key(&mut s, key('a'));
        type_str(&mut s, "feat");
        assert_eq!(
            handle_key(&mut s, code(KeyCode::Enter)),
            vec![Effect::EditLabels {
                number: 1,
                add: vec!["feature".into()],
                remove: vec![],
            }]
        );
    }

    #[test]
    fn enter_on_empty_filter_keeps_menu() {
        let mut s = labeled();
        handle_key(&mut s, key('a'));
        type_str(&mut s, "zzz");
        assert!(handle_key(&mut s, code(KeyCode::Enter)).is_empty());
        assert!(s.label_menu().is_some());
        assert!(s.busy.is_none());
    }

    #[test]
    fn readonly_tabs_reject() {
        let mut s = custom_state(vec![pr(1)], vec![pr(20)]);
        handle_key(&mut s, key('3'));
        assert!(handle_key(&mut s, key('a')).is_empty());
        assert_eq!(message(&s), ("a はマージ予定タブでは使えない", true));
        assert!(s.label_menu().is_none());
    }

    #[test]
    fn busy_does_not_open() {
        let mut s = labeled();
        s.busy = Some("…".into());
        assert!(handle_key(&mut s, key('a')).is_empty());
        assert!(s.label_menu().is_none());
    }

    #[test]
    fn fetched_labels_replace_and_clamp_cursor() {
        let mut s = labeled();
        handle_key(&mut s, key('a'));
        handle_key(&mut s, code(KeyCode::Up));
        assert_eq!(cursor(&s), Some(2));
        let effects = handle_event(&mut s, Event::FetchedLabels(Ok(vec![label("bug")])));
        assert!(effects.is_empty());
        assert!(!s.labels_loading);
        assert_eq!(names(&s), ["bug"]);
        assert_eq!(cursor(&s), Some(0));
    }

    #[test]
    fn fetch_error_goes_to_menu_or_footer() {
        let mut s = labeled();
        handle_key(&mut s, key('a'));
        handle_event(
            &mut s,
            Event::FetchedLabels(Err(anyhow::anyhow!("HTTP 502"))),
        );
        assert_eq!(
            s.label_menu().unwrap().error.as_deref(),
            Some("ラベル一覧の取得失敗: HTTP 502")
        );
        // 以前の一覧は残す
        assert_eq!(names(&s).len(), 3);

        let mut s = labeled();
        s.labels_loading = true;
        handle_event(
            &mut s,
            Event::FetchedLabels(Err(anyhow::anyhow!("HTTP 502"))),
        );
        assert_eq!(message(&s), ("ラベル一覧の取得失敗: HTTP 502", true));
    }

    #[test]
    fn edited_watches_until_labels_match() {
        let mut s = labeled();
        s.busy = Some("…".into());
        let effects = handle_event(
            &mut s,
            Event::EditedLabels {
                number: 1,
                add: vec!["docs".into()],
                remove: vec!["bug".into()],
                res: Ok(String::new()),
            },
        );
        assert!(s.busy.is_none());
        assert_eq!(message(&s), ("#1 のラベルを変えた: +docs -bug", false));
        assert_eq!(
            effects,
            vec![Effect::Watch {
                number: 1,
                until: WatchUntil::Labels(vec!["docs".into()]),
            }]
        );
    }

    #[test]
    fn edit_failure_is_error_and_reloads() {
        let mut s = labeled();
        let effects = handle_event(
            &mut s,
            Event::EditedLabels {
                number: 1,
                add: vec!["nope".into()],
                remove: vec![],
                res: Err(anyhow::anyhow!("'nope' not found")),
            },
        );
        assert_eq!(message(&s), ("#1 のラベル変更失敗: 'nope' not found", true));
        assert_eq!(
            effects,
            vec![Effect::Watch {
                number: 1,
                until: WatchUntil::Immediately,
            }]
        );
    }
}
