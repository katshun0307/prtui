//! スレッドの結果 (Event) を AppState に反映する.

use chrono::Utc;

use super::chat::send;
use super::label::{clamp_cursor, diff_text};
use crate::app::event::{Effect, Event, WatchUntil};
use crate::app::state::{AppState, PrList};
use crate::domain::label::apply_diff;
use crate::domain::local::is_diverged;
use crate::domain::op::PrOp;
use crate::domain::pr::Pr;
use crate::domain::stack::arrange;

/// 自分タブ以外の全件取得の結果を反映する. 失敗はフッターではなくそのタブの一覧に出し,
/// 自分タブの表示を邪魔しない
fn fetched_list(list: &mut PrList, res: anyhow::Result<Vec<Pr>>) -> Vec<Effect> {
    list.loading = false;
    match res {
        Ok(prs) => {
            list.fetched_at = Some(Utc::now());
            list.from_cache = false;
            list.error = None;
            list.replace(prs);
            vec![Effect::SaveCache]
        }
        Err(e) => {
            list.error = Some(format!("取得失敗: {e}"));
            vec![]
        }
    }
}

pub fn handle_event(state: &mut AppState, ev: Event) -> Vec<Effect> {
    match ev {
        Event::Fetched(res) => {
            state.mine.loading = false;
            match res {
                Ok(prs) => {
                    state.mine.fetched_at = Some(Utc::now());
                    state.mine.from_cache = false;
                    state.mine.replace(prs);
                    vec![Effect::SaveCache]
                }
                Err(e) => {
                    state.error(format!("取得失敗: {e}"));
                    vec![]
                }
            }
        }
        Event::FetchedReviews(res) => fetched_list(&mut state.review, res),
        Event::FetchedCustom(i, res) => match state.custom.get_mut(i) {
            Some(c) => fetched_list(&mut c.list, res),
            None => vec![],
        },
        Event::Reloaded(n, res) => {
            state.syncing.remove(&n);
            match *res {
                Ok(pr) => {
                    // drain すると replace_prs が選択中の PR を引けなくなるので複製する
                    let mut prs: Vec<Pr> = state
                        .mine
                        .prs
                        .iter()
                        .filter(|p| p.number != n)
                        .cloned()
                        .collect();
                    prs.extend(pr);
                    state.mine.replace(arrange(prs));
                    vec![Effect::SaveCache]
                }
                Err(e) => {
                    state.error(format!("#{n} の再取得失敗: {e}"));
                    vec![]
                }
            }
        }
        Event::UpdatedBranch(n, res) => {
            state.busy = None;
            match res {
                Ok(base) => {
                    state.info(format!("#{n} に {base} を取り込んだ"));
                    // merge commit が積まれて head が動くまで待つ
                    let old = state.pr(n).map(|p| p.head_ref_oid.clone());
                    vec![Effect::Watch {
                        number: n,
                        until: WatchUntil::HeadMovedFrom(old),
                    }]
                }
                Err(e) => {
                    state.error(format!("#{n}: {e:#}"));
                    // head が動いていて 422 になったときなどに最新を出す
                    vec![Effect::Watch {
                        number: n,
                        until: WatchUntil::Immediately,
                    }]
                }
            }
        }
        Event::LinearReview(n, res) => {
            state.busy = None;
            match res {
                Ok(Some(url)) => vec![Effect::OpenUrl(url)],
                Ok(None) => {
                    state.error(format!("#{n} に Linear review のリンクが無い"));
                    vec![]
                }
                Err(e) => {
                    state.error(format!("#{n} の Linear review 取得失敗: {e}"));
                    vec![]
                }
            }
        }
        Event::ChangedState(n, op, res) => {
            state.busy = None;
            match res {
                Ok(_) => {
                    state.info(format!("#{n} を {}した", op.phrase(&state.local.reviewers)));
                    let until = match op {
                        PrOp::Open => WatchUntil::NotDraft,
                        PrOp::Draft => WatchUntil::IsDraft,
                        PrOp::Close => WatchUntil::Gone,
                        PrOp::Review => WatchUntil::Requested(state.local.reviewers.clone()),
                    };
                    vec![Effect::Watch { number: n, until }]
                }
                Err(e) => {
                    state.error(format!("#{n} の {} 失敗: {e}", op.label()));
                    vec![Effect::Watch {
                        number: n,
                        until: WatchUntil::Immediately,
                    }]
                }
            }
        }
        Event::CheckedOut(n, force, res) => {
            state.busy = None;
            match res {
                Ok(_) if force => {
                    state.info(format!("#{n} を PR の最新に合わせてチェックアウトした"))
                }
                Ok(_) => state.info(format!("#{n} をチェックアウトした")),
                // 分岐していれば, ローカルのコミットを捨てて合わせるかを聞く
                Err(e) if !force && is_diverged(&e.to_string()) => {
                    let branch = state
                        .mine
                        .prs
                        .iter()
                        .chain(&state.review.prs)
                        .find(|p| p.number == n)
                        .map(|p| p.head_ref_name.clone())
                        .unwrap_or_default();
                    state.confirm_force_checkout = Some((n, branch));
                }
                Err(e) => state.error(format!("#{n} のチェックアウト失敗: {e}")),
            }
            vec![Effect::ReloadLocal]
        }
        Event::DevinSessions(res) => {
            state.sessions_loading = false;
            match res {
                // 開いている session が一覧から消えても会話ビューはそのまま残す
                Ok(sessions) => {
                    state.devin_sessions = sessions;
                    vec![Effect::SaveCache]
                }
                Err(e) => {
                    state.error(format!("Devin session の取得失敗: {e}"));
                    vec![]
                }
            }
        }
        Event::DevinMessages(id, res) => {
            // 閉じたか別の session に移った後に届いた結果は捨てる
            let Some(chat) = state.chat.as_mut().filter(|c| c.session_id == id) else {
                return vec![];
            };
            chat.fetching = false;
            match res {
                Ok(messages) => {
                    // 送信失敗のエラーを次のポーリングで消さない. 消すのは送信と r, i, e
                    chat.messages = messages;
                    chat.loaded = true;
                }
                Err(e) => chat.error = Some(format!("メッセージの取得失敗: {e}")),
            }
            vec![]
        }
        Event::DevinSent(id, res) => {
            let Some(chat) = state.chat.as_mut().filter(|c| c.session_id == id) else {
                return vec![];
            };
            chat.sending = false;
            match res {
                Ok(session) => {
                    chat.set_draft(String::new());
                    chat.typing = false;
                    // 送ったメッセージをすぐ出すため次の tick で取り直す
                    chat.last_poll = None;
                    match state.devin_sessions.iter_mut().find(|s| s.id == session.id) {
                        Some(s) => *s = session,
                        None => state.devin_sessions.push(session),
                    }
                    vec![Effect::SaveCache]
                }
                Err(e) => {
                    chat.error = Some(format!("送信失敗: {e}"));
                    vec![]
                }
            }
        }
        Event::FetchedLabels(res) => {
            state.labels_loading = false;
            match res {
                Ok(labels) => {
                    state.repo_labels = labels;
                    clamp_cursor(state);
                }
                // 開いていれば選択画面に出し, 以前の一覧で続けられるようにする
                Err(e) => {
                    let text = format!("ラベル一覧の取得失敗: {e}");
                    match state.label_menu.as_mut() {
                        Some(m) => m.error = Some(text),
                        None => state.error(text),
                    }
                }
            }
            vec![]
        }
        Event::EditedLabels {
            number: n,
            add,
            remove,
            res,
        } => {
            state.busy = None;
            match res {
                Ok(_) => {
                    state.info(format!(
                        "#{n} のラベルを変えた: {}",
                        diff_text(&add, &remove)
                    ));
                    let labels = state
                        .pr(n)
                        .map(|p| apply_diff(&p.label_names(), &add, &remove))
                        .unwrap_or_else(|| add.clone());
                    vec![Effect::Watch {
                        number: n,
                        until: WatchUntil::Labels(labels),
                    }]
                }
                Err(e) => {
                    state.error(format!("#{n} のラベル変更失敗: {e}"));
                    vec![Effect::Watch {
                        number: n,
                        until: WatchUntil::Immediately,
                    }]
                }
            }
        }
        Event::Composed(res) => {
            let Some(chat) = state.chat.as_mut() else {
                return vec![];
            };
            match res {
                Ok(Some(text)) => {
                    chat.set_draft(text.trim_end_matches(['\n', '\r']).to_string());
                    send(chat)
                }
                // エディタを非 0 で終えたら取りやめ. draft は変えない
                Ok(None) => vec![],
                Err(e) => {
                    chat.error = Some(format!("エディタの実行失敗: {e}"));
                    vec![]
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use anyhow::anyhow;
    use pretty_assertions::assert_eq;

    use super::super::test_util::{
        chat, chat_state, custom_state, key, message, pr, review_state, state,
    };
    use super::super::{handle_key, refresh};
    use super::handle_event;
    use crate::app::event::{Effect, Event, WatchUntil};
    use crate::domain::devin::DevinSession;
    use crate::domain::op::PrOp;

    #[test]
    fn custom_tab_fetch() {
        let mut s = custom_state(vec![], vec![pr(20)]);
        assert_eq!(
            refresh(&mut s),
            vec![Effect::Fetch, Effect::FetchReviews, Effect::FetchCustom(0)]
        );
        // 取得中は出し直さない
        assert_eq!(refresh(&mut s), vec![]);

        let effects = handle_event(&mut s, Event::FetchedCustom(0, Err(anyhow!("boom"))));
        assert!(effects.is_empty());
        assert!(!s.custom[0].list.loading);
        assert_eq!(s.custom[0].list.prs.len(), 1);
        assert_eq!(s.custom[0].list.error.as_deref(), Some("取得失敗: boom"));
        assert!(s.message.is_none());

        s.custom[0].list.loading = true;
        let effects = handle_event(&mut s, Event::FetchedCustom(0, Ok(vec![pr(21), pr(22)])));
        assert_eq!(effects, vec![Effect::SaveCache]);
        assert!(s.custom[0].list.error.is_none() && !s.custom[0].list.loading);
        assert_eq!(s.custom[0].list.prs.len(), 2);
    }

    #[test]
    fn fetched_reviews_error_stays_in_review_tab() {
        let mut s = review_state(vec![], vec![pr(10)]);
        s.review.loading = true;
        let effects = handle_event(&mut s, Event::FetchedReviews(Err(anyhow!("boom"))));
        assert!(effects.is_empty());
        assert!(!s.review.loading);
        assert_eq!(s.review.prs.len(), 1);
        assert_eq!(s.review.error.as_deref(), Some("取得失敗: boom"));
        assert!(s.message.is_none());

        let effects = handle_event(&mut s, Event::FetchedReviews(Ok(vec![pr(11)])));
        assert_eq!(effects, vec![Effect::SaveCache]);
        assert!(s.review.error.is_none());
        assert!(s.review.fetched_at.is_some());
        assert_eq!(s.review.prs[0].number, 11);
    }

    #[test]
    fn diverged_checkout_asks_then_forces() {
        let mut s = state(vec![pr(1)]);
        let err = anyhow!(
            "gh pr: Switched to branch 'b1'hint: Diverging branches can't be fast-forwarded"
        );
        let effects = handle_event(&mut s, Event::CheckedOut(1, false, Err(err)));
        assert_eq!(effects, vec![Effect::ReloadLocal]);
        assert_eq!(s.confirm_force_checkout, Some((1, "b1".into())));
        assert!(s.message.is_none());
        assert_eq!(
            handle_key(&mut s, key('y')),
            vec![Effect::Checkout {
                number: 1,
                force: true
            }]
        );
        assert!(s.confirm_force_checkout.is_none());
    }

    #[test]
    fn diverged_checkout_cancelled_by_other_key() {
        let mut s = state(vec![pr(1)]);
        let err = anyhow!("fatal: Not possible to fast-forward, aborting.");
        handle_event(&mut s, Event::CheckedOut(1, false, Err(err)));
        assert!(handle_key(&mut s, key('n')).is_empty());
        assert_eq!(message(&s), ("b1 を PR に合わせるのを取りやめた", false));
    }

    #[test]
    fn other_checkout_error_and_forced_failure_are_errors() {
        let mut s = state(vec![pr(1)]);
        handle_event(&mut s, Event::CheckedOut(1, false, Err(anyhow!("boom"))));
        assert!(s.confirm_force_checkout.is_none());
        assert_eq!(message(&s), ("#1 のチェックアウト失敗: boom", true));
        // --force でも失敗したらもう聞かない
        let err = anyhow!("Diverging branches");
        handle_event(&mut s, Event::CheckedOut(1, true, Err(err)));
        assert!(s.confirm_force_checkout.is_none());
        assert!(message(&s).1);
    }

    #[test]
    fn fetched_replaces_and_saves() {
        let mut s = state(vec![pr(1)]);
        s.mine.loading = true;
        s.mine.from_cache = true;
        let effects = handle_event(&mut s, Event::Fetched(Ok(vec![pr(2), pr(3)])));
        assert_eq!(effects, vec![Effect::SaveCache]);
        assert!(!s.mine.loading);
        assert!(!s.mine.from_cache);
        assert!(s.mine.fetched_at.is_some());
        let numbers: Vec<u64> = s.mine.prs.iter().map(|p| p.number).collect();
        assert_eq!(numbers, vec![2, 3]);
    }

    #[test]
    fn fetched_err_keeps_prs() {
        let mut s = state(vec![pr(1)]);
        s.mine.loading = true;
        let effects = handle_event(&mut s, Event::Fetched(Err(anyhow!("boom"))));
        assert!(effects.is_empty());
        assert!(!s.mine.loading);
        assert_eq!(s.mine.prs.len(), 1);
        assert_eq!(message(&s), ("取得失敗: boom", true));
    }

    #[test]
    fn reloaded_none_removes_pr() {
        let mut s = state(vec![pr(1), pr(2)]);
        s.syncing.insert(1);
        let effects = handle_event(&mut s, Event::Reloaded(1, Box::new(Ok(None))));
        assert_eq!(effects, vec![Effect::SaveCache]);
        assert!(s.pr(1).is_none());
        assert!(s.pr(2).is_some());
        assert!(!s.syncing.contains(&1));
    }

    #[test]
    fn changed_state_open_watches_not_draft() {
        let mut s = state(vec![pr(1)]);
        s.busy = Some("x".into());
        let effects = handle_event(
            &mut s,
            Event::ChangedState(1, PrOp::Open, Ok(String::new())),
        );
        assert_eq!(
            effects,
            vec![Effect::Watch {
                number: 1,
                until: WatchUntil::NotDraft,
            }]
        );
        assert!(s.busy.is_none());
        assert_eq!(message(&s), ("#1 を open にした", false));
    }

    #[test]
    fn messages_for_other_session_are_dropped() {
        let mut s = chat_state();
        s.chat.as_mut().unwrap().fetching = true;
        let effects = handle_event(&mut s, Event::DevinMessages("devin-b".into(), Ok(vec![])));
        assert!(effects.is_empty());
        assert!(chat(&s).fetching);
        assert!(!chat(&s).loaded);
    }

    #[test]
    fn messages_err_goes_to_chat() {
        let mut s = chat_state();
        s.chat.as_mut().unwrap().fetching = true;
        handle_event(
            &mut s,
            Event::DevinMessages("devin-a".into(), Err(anyhow!("boom"))),
        );
        assert!(!chat(&s).fetching);
        assert_eq!(
            chat(&s).error.as_deref(),
            Some("メッセージの取得失敗: boom")
        );
    }

    #[test]
    fn sent_ok_clears_draft_and_updates_session() {
        let mut s = chat_state();
        {
            let c = s.chat.as_mut().unwrap();
            c.set_draft("hi".into());
            c.typing = true;
            c.sending = true;
            c.last_poll = Some(Instant::now());
        }
        let mut sent = DevinSession::fixture("devin-a", "https://github.com/o/r/pull/1");
        sent.status = crate::domain::devin::DevinStatus::Resuming;
        let effects = handle_event(&mut s, Event::DevinSent("devin-a".into(), Ok(sent)));
        assert_eq!(effects, vec![Effect::SaveCache]);
        let c = chat(&s);
        assert!(!c.sending && !c.typing && c.draft.is_empty() && c.last_poll.is_none());
        assert_eq!(c.cursor, 0);
        assert_eq!(s.devin_sessions.len(), 1);
        assert_eq!(
            s.chat_session().map(|x| x.status),
            Some(crate::domain::devin::DevinStatus::Resuming)
        );
    }

    #[test]
    fn sent_err_keeps_draft() {
        let mut s = chat_state();
        {
            let c = s.chat.as_mut().unwrap();
            c.draft = "hi".into();
            c.sending = true;
        }
        let effects = handle_event(
            &mut s,
            Event::DevinSent("devin-a".into(), Err(anyhow!("409 conflict"))),
        );
        assert!(effects.is_empty());
        let c = chat(&s);
        assert!(!c.sending);
        assert_eq!(c.draft, "hi");
        assert_eq!(c.error.as_deref(), Some("送信失敗: 409 conflict"));

        // 次のポーリングが成功しても送信失敗は消さない. i で消す
        handle_event(&mut s, Event::DevinMessages("devin-a".into(), Ok(vec![])));
        assert_eq!(chat(&s).error.as_deref(), Some("送信失敗: 409 conflict"));
        handle_key(&mut s, key('i'));
        assert!(chat(&s).error.is_none());
    }

    #[test]
    fn composed_sends_or_cancels() {
        let mut s = chat_state();
        s.chat.as_mut().unwrap().draft = "old".into();
        assert!(handle_event(&mut s, Event::Composed(Ok(None))).is_empty());
        assert_eq!(chat(&s).draft, "old");

        let effects = handle_event(&mut s, Event::Composed(Ok(Some("a\nb\n\n".into()))));
        assert_eq!(chat(&s).draft, "a\nb");
        assert_eq!(chat(&s).cursor, 3);
        assert_eq!(
            effects,
            vec![Effect::SendMessage {
                session_id: "devin-a".into(),
                text: "a\nb".into(),
            }]
        );

        let mut s = chat_state();
        assert!(handle_event(&mut s, Event::Composed(Ok(Some(" \n".into())))).is_empty());
        assert!(!chat(&s).sending);
        handle_event(&mut s, Event::Composed(Err(anyhow!("boom"))));
        assert!(chat(&s).error.is_some());
    }

    #[test]
    fn sessions_event() {
        let mut s = chat_state();
        s.sessions_loading = true;
        let effects = handle_event(&mut s, Event::DevinSessions(Ok(vec![])));
        assert_eq!(effects, vec![Effect::SaveCache]);
        assert!(!s.sessions_loading);
        // 開いている session が一覧から消えても会話ビューは残す
        assert!(s.chat.is_some());
        assert!(s.chat_session().is_none());

        s.sessions_loading = true;
        assert!(handle_event(&mut s, Event::DevinSessions(Err(anyhow!("boom")))).is_empty());
        assert!(!s.sessions_loading);
        assert_eq!(message(&s), ("Devin session の取得失敗: boom", true));
    }
}
