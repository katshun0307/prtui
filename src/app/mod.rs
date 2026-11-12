mod adapters;
mod event;
mod ports;
mod runtime;
mod state;
mod update;

pub use event::Action;
pub use runtime::App;
pub use state::{AppState, Tab};
/// ui の snapshot がラベルと review 依頼の選択画面, マージの確認画面を開いた状態を作るのに使う
#[cfg(test)]
pub use state::{LabelMenu, MergeMenu, ReviewerMenu};
pub use state::{PrPage, PrView};
