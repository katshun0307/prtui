mod adapters;
mod event;
mod ports;
mod runtime;
mod state;
mod update;

pub use event::Action;
pub use runtime::App;
/// ui の snapshot がラベルの選択画面を開いた状態を作るのに使う
#[cfg(test)]
pub use state::LabelMenu;
pub use state::{AppState, Tab};
