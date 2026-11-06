//! ラベルと review 依頼の選択画面で共通の, 絞り込みの入力とカーソル移動.
//! 文字は絞り込みに使うので, 移動は ↑↓ か Ctrl-p / Ctrl-n で行う

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::widgets::ListState;

/// 絞り込みとカーソル移動のキーなら処理して true を返す. len は今の選択肢の数
pub(super) fn picker_key(
    query: &mut String,
    list: &mut ListState,
    len: usize,
    key: KeyEvent,
) -> bool {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    match key.code {
        KeyCode::Up => move_cursor(list, len, -1),
        KeyCode::Down => move_cursor(list, len, 1),
        KeyCode::Char('p') if ctrl => move_cursor(list, len, -1),
        KeyCode::Char('n') if ctrl => move_cursor(list, len, 1),
        KeyCode::Char('u') if ctrl => set_query(query, list, String::new()),
        KeyCode::Backspace => {
            let mut q = query.clone();
            q.pop();
            set_query(query, list, q);
        }
        KeyCode::Char(c) if !ctrl => {
            let q = format!("{query}{c}");
            set_query(query, list, q);
        }
        _ => return false,
    }
    true
}

/// 絞り込みを変えたら先頭に戻る
fn set_query(query: &mut String, list: &mut ListState, q: String) {
    *query = q;
    list.select(Some(0));
}

/// 端を越えたら反対の端に回る
fn move_cursor(list: &mut ListState, len: usize, delta: isize) {
    if len == 0 {
        return;
    }
    let cur = list.selected().unwrap_or(0) as isize;
    list.select(Some((cur + delta).rem_euclid(len as isize) as usize));
}

/// 選択肢の数が変わったときにカーソルを範囲に収める
pub(super) fn clamp(list: &mut ListState, len: usize) {
    let cur = list.selected().unwrap_or(0);
    list.select(Some(cur.min(len.saturating_sub(1))));
}

/// カーソルを 1 つ進める. 末尾では止まる. Tab で選んだ後に使う
pub(super) fn advance(list: &mut ListState, len: usize) {
    let cur = list.selected().unwrap_or(0);
    list.select(Some((cur + 1).min(len.saturating_sub(1))));
}
