//! `M` のマージの確認画面. 方法のキー (s / m / r) か, 既定の方法の Enter でマージし,
//! 他のキーで取りやめる. repo で許可された方法は初めて開いたときに取る

use ratatui::crossterm::event::{KeyCode, KeyEvent};

use crate::app::event::Effect;
use crate::app::state::{AppState, MergeMenu};
use crate::domain::merge::{MergeMethod, MergeSettings};

/// 選択中の PR の確認画面を開く. Draft とコンフリクトしている PR は GitHub がマージしないので開かない
pub(super) fn open_merge(state: &mut AppState) -> Vec<Effect> {
    if state.busy.is_some() {
        return vec![];
    }
    let Some(pr) = state.selected() else {
        return vec![];
    };
    let n = pr.number;
    if pr.is_draft {
        state.error(format!(
            "#{n} は Draft なのでマージできない. O で open にする"
        ));
        return vec![];
    }
    if pr.is_conflicting() {
        state.error(format!("#{n} はコンフリクトしているのでマージできない"));
        return vec![];
    }
    let menu = MergeMenu {
        number: n,
        head_oid: pr.head_ref_oid.clone(),
    };
    state.push(menu);
    if state.merge_settings.is_some() || state.merge_settings_loading {
        return vec![];
    }
    state.merge_settings_loading = true;
    vec![Effect::FetchMergeSettings]
}

/// 方法が決まればマージし, 決まらなければ閉じる. 方法の取得中は方法のキーも取りやめになる
pub(super) fn merge_key(state: &mut AppState, key: KeyEvent) -> Vec<Effect> {
    let Some(menu) = state.pop_merge_menu() else {
        return vec![];
    };
    let method = state
        .merge_settings
        .as_ref()
        .and_then(|s| chosen(s, key.code));
    let Some(method) = method else {
        state.info(format!("#{} のマージを取りやめた", menu.number));
        return vec![];
    };
    state.busy = Some(format!(
        "#{} を {} でマージしている…",
        menu.number,
        method.label()
    ));
    vec![Effect::Merge {
        number: menu.number,
        method,
        head_oid: menu.head_oid,
    }]
}

fn chosen(settings: &MergeSettings, code: KeyCode) -> Option<MergeMethod> {
    match code {
        KeyCode::Enter => settings.default,
        KeyCode::Char(c) => settings.by_key(c),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;
    use ratatui::crossterm::event::KeyCode;

    use super::super::test_util::{code, key, message, pr, state};
    use crate::app::event::{Effect, Event, WatchUntil};
    use crate::app::state::MergeMenu;
    use crate::app::update::{handle_event, handle_key};
    use crate::domain::merge::{MergeMethod, MergeOutcome, MergeSettings};

    fn settings() -> MergeSettings {
        MergeSettings::new(
            vec![MergeMethod::Squash, MergeMethod::Merge],
            Some(MergeMethod::Merge),
        )
    }

    #[test]
    fn open_fetches_settings_once() {
        let mut s = state(vec![pr(1)]);
        assert_eq!(
            handle_key(&mut s, key('M')),
            vec![Effect::FetchMergeSettings]
        );
        assert_eq!(
            s.merge_menu(),
            Some(&MergeMenu {
                number: 1,
                head_oid: "oid1".into()
            })
        );
        assert!(s.merge_settings_loading);
        handle_event(&mut s, Event::FetchedMergeSettings(Ok(settings())));
        assert!(!s.merge_settings_loading);
        assert_eq!(s.merge_settings, Some(settings()));

        // 2 回目は取り直さない
        handle_key(&mut s, code(KeyCode::Esc));
        assert!(handle_key(&mut s, key('M')).is_empty());
        assert!(s.merge_menu().is_some());
    }

    #[test]
    fn method_key_merges_with_head() {
        let mut s = state(vec![pr(1)]);
        s.merge_settings = Some(settings());
        assert!(handle_key(&mut s, key('M')).is_empty());
        assert_eq!(
            handle_key(&mut s, key('s')),
            vec![Effect::Merge {
                number: 1,
                method: MergeMethod::Squash,
                head_oid: "oid1".into(),
            }]
        );
        assert!(s.merge_menu().is_none());
        assert_eq!(s.busy.as_deref(), Some("#1 を squash でマージしている…"));
    }

    #[test]
    fn enter_uses_default() {
        let mut s = state(vec![pr(1)]);
        s.merge_settings = Some(settings());
        handle_key(&mut s, key('M'));
        assert_eq!(
            handle_key(&mut s, code(KeyCode::Enter)),
            vec![Effect::Merge {
                number: 1,
                method: MergeMethod::Merge,
                head_oid: "oid1".into(),
            }]
        );
    }

    #[test]
    fn other_keys_cancel() {
        // 許可されていない方法, 既定が無いときの Enter, 取得中の方法のキーはどれも取りやめ
        let no_default = MergeSettings::new(vec![MergeMethod::Squash], None);
        for (settings, k) in [
            (Some(settings()), key('r')),
            (Some(settings()), key('j')),
            (Some(settings()), code(KeyCode::Esc)),
            (Some(no_default), code(KeyCode::Enter)),
            (None, key('s')),
        ] {
            let mut s = state(vec![pr(1)]);
            s.merge_settings = settings;
            s.merge_settings_loading = s.merge_settings.is_none();
            handle_key(&mut s, key('M'));
            assert!(handle_key(&mut s, k).is_empty(), "{k:?}");
            assert!(s.merge_menu().is_none());
            assert!(s.busy.is_none());
            assert_eq!(message(&s), ("#1 のマージを取りやめた", false));
        }
    }

    #[test]
    fn draft_and_conflict_do_not_open() {
        let mut d = pr(1);
        d.is_draft = true;
        let mut c = pr(2);
        c.mergeable = "CONFLICTING".into();
        let mut s = state(vec![d, c]);
        assert!(handle_key(&mut s, key('M')).is_empty());
        assert_eq!(
            message(&s),
            ("#1 は Draft なのでマージできない. O で open にする", true)
        );
        handle_key(&mut s, key('j'));
        assert!(handle_key(&mut s, key('M')).is_empty());
        assert_eq!(
            message(&s),
            ("#2 はコンフリクトしているのでマージできない", true)
        );
        assert!(s.merge_menu().is_none());
    }

    #[test]
    fn settings_error_closes_menu() {
        let mut s = state(vec![pr(1)]);
        handle_key(&mut s, key('M'));
        handle_event(
            &mut s,
            Event::FetchedMergeSettings(Err(anyhow::anyhow!("HTTP 502"))),
        );
        assert!(s.merge_menu().is_none() && s.merge_settings.is_none());
        assert!(!s.merge_settings_loading);
        assert_eq!(message(&s), ("マージ方法の取得失敗: HTTP 502", true));
        // 次に開いたら取り直す
        assert_eq!(
            handle_key(&mut s, key('M')),
            vec![Effect::FetchMergeSettings]
        );
    }

    #[test]
    fn merged_watches_until_gone_and_refetches() {
        let mut s = state(vec![pr(1)]);
        s.busy = Some("…".into());
        assert_eq!(
            handle_event(
                &mut s,
                Event::Merged {
                    number: 1,
                    method: MergeMethod::Squash,
                    res: Ok(MergeOutcome::Merged),
                }
            ),
            vec![
                Effect::Watch {
                    number: 1,
                    until: WatchUntil::Gone,
                },
                Effect::Fetch
            ]
        );
        assert!(s.busy.is_none() && s.mine.loading);
        assert_eq!(message(&s), ("#1 を squash でマージした", false));

        assert_eq!(
            handle_event(
                &mut s,
                Event::Merged {
                    number: 1,
                    method: MergeMethod::Squash,
                    res: Ok(MergeOutcome::Enqueued),
                }
            ),
            vec![Effect::Watch {
                number: 1,
                until: WatchUntil::Immediately,
            }]
        );
        assert_eq!(message(&s), ("#1 を merge queue に入れた", false));

        assert_eq!(
            handle_event(
                &mut s,
                Event::Merged {
                    number: 1,
                    method: MergeMethod::Squash,
                    res: Err(anyhow::anyhow!("head branch was modified")),
                }
            ),
            vec![Effect::Watch {
                number: 1,
                until: WatchUntil::Immediately,
            }]
        );
        assert_eq!(
            message(&s),
            ("#1 のマージ失敗: head branch was modified", true)
        );
    }
}
