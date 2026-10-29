# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## 何をするツールか

自分が author か assignee の open PR を 1 画面に並べ, 状態確認とチェックアウト, main の取り込みを行う Rust 製 TUI. 対象 repo は起動ディレクトリ (`-C <dir>` で上書き) で, GitHub と git の操作はすべて `gh` と `git` コマンドを `std::process::Command` で呼んで行う. 仕様の決定経緯と既知の課題は `docs/handoff.md` にある. 仕様を変えたらそこも更新する.

## コマンド

```sh
task                            # check → build → install. ~/.cargo/bin/prtui を入れ替える
task check                      # cargo fmt --check と cargo clippy --all-targets -D warnings
task build                      # check 後に cargo build --release
```

`Taskfile.yml` が上の 3 つを定義している. 警告 0 件を維持するため clippy は `-D warnings` で落とす.

単体テストは `cargo test` (domain の純粋関数, GraphQL の JSON 解析, キー操作とイベント処理の状態遷移と `Effect`). 動作確認は次の 2 段階で行う.

```sh
# TUI なしで取得結果をテキスト出力する. GraphQL や変換の変更はまずこれで見る
./target/release/prtui -C /path/to/some-repo --dump

# TUI の描画確認. この repo 自体は remote が無いので, remote のある repo を -C で指す
tmux new-session -d -s prtui -x 160 -y 30 "prtui -C /path/to/some-repo" && sleep 12 && tmux capture-pane -p -t prtui
tmux capture-pane -p -e -t prtui   # エスケープコード付き. 配色の確認用
tmux send-keys -t prtui q
```

`gh auth login` 済みであることが前提. GitHub API を呼ぶので `--dump` も TUI も実際の repo に対して動く.

## 構造

スレッドと mpsc で UI をブロックしない設計. 非同期 runtime は使わない. 層は下に向かってのみ依存する.

| 層 | 依存してよい先 |
|---|---|
| `domain/` | std, chrono, serde のみ. crate 内の他モジュールと ratatui は使わない |
| `infra/` | domain. app と ui と ratatui は使わない |
| `app/{state,event,update}.rs` | domain と, ratatui のデータ型 (`KeyEvent`, `TableState`) だけ. infra と描画型は使わない |
| `app/runtime.rs` | app の他のファイル, infra, domain |
| `ui/`, `dump.rs` | `AppState` の読み取りと domain. dump だけは TUI を通らないので infra を直接呼ぶ |
| `main.rs` | app (`App`, `Action`), ui, dump, infra (`main_worktree`, `repo_name`) |

- `domain/` は純粋な型と関数. `pr.rs` (`Pr` と付随する型, `is_conflicting` など), `stack.rs` (`arrange` で更新日時の降順に並べて stack をまとめる), `review.rs` (`review_targets`), `linear.rs` (issue キーと review URL の抽出), `local.rs` (`LocalState` と「最新」「差分あり」の判定, `parse_reviewers`), `op.rs` (`PrOp`: open / draft / close / review)
- `infra/` は外部プロセスとファイル. `infra/process.rs` の `run` がコマンド実行の共通ヘルパで, 失敗時は stderr を `anyhow` のエラーにする. `infra/git.rs` は `main_worktree` (worktree 内から起動されてもメイン checkout を対象にする) と `read_local_state` (ブランチ名 → oid の表と git config). `infra/gh.rs` は gh コマンド (`fetch_prs`, `fetch_pr`, `update_branch`, `checkout`, `change_state`, `view_web`, `linear_review_url`). `update_branch` は REST の update-branch API を `expected_head_sha` 付きで呼び, 取得後に head が動いていれば GitHub の 422 で止まる. `infra/graphql.rs` はクエリ文字列 (`QUERY` は author と assignee を 2 つの alias で同時に取る, `QUERY_ONE` は同じ fragment で 1 件) と `Raw*` 型, `convert`, `parse_search` (番号で重複除去) と `parse_one` (open でなければ None) を持ち, `infra/gh.rs` はコマンドを叩いて JSON を渡すだけ. `infra/cache.rs` は PR 一覧を `~/.cache/prtui/<owner>__<name>.json` に読み書きする. 起動時に読んで即表示し, 裏で取り直す. 読めなければ捨てるので, `Pr` のフィールドを変えても移行処理は要らない
- `app/state.rs` の `AppState` が全状態を持つ (選択位置は ratatui の `TableState`). `replace_prs` は選択を PR 番号で保つ. 実行中は `busy` に文字列を入れて多重実行を防ぐ
- `app/event.rs` は型だけ. `Event` はスレッドの結果, `Effect` は update が runtime に求める副作用, `Action` は main への指示 (`Quit`, `Suspend`), `WatchUntil` は 1 件取り直しを止める条件 (head が動いた, Draft が変わった, open でなくなった, 全員が依頼中になった, など) で `is_done` を持つ
- `app/update.rs` は `handle_key` / `handle_event` / `refresh` で `&mut AppState` を更新し `Vec<Effect>` を返すだけの純粋関数. `thread::spawn` も gh も cache も呼ばない. 操作後は全件を取り直さず `Effect::Watch { number, until }` を返す. 結果の `Event::Reloaded` でその PR だけ差し替え, `arrange` し直して `Effect::SaveCache` を返す
- `app/runtime.rs` の `App` は `AppState` と mpsc を持ち, `perform` で Effect を実行する唯一の場所. 時間のかかる処理は `spawn` でスレッドに出し, 結果を `Event` として送る. `pump` が届いた Event を update に渡す. `watch` は `WatchUntil` を満たすまで 2 秒おきに最大 10 回 `fetch_pr` する. Quit と Suspend は `Action` にして main に返す
- `ui/` は描画のみで, `AppState` を読む. `mod.rs` (`draw`, ヘッダ, フッタ, 配色定数), `table.rs` (一覧の各列), `detail.rs` (詳細欄), `format.rs` (`ago`. dump も使う). 配色の意味 (green / red / yellow / cyan / magenta / dim) は `ui/mod.rs` 先頭のコメントに定義してあり, 新しい表示を足すときはそれに合わせる. `Color::DarkGray` は端末テーマで背景と同化するので使わず, `DIM` と `SELECTED_BG` の 256 色固定値を使う
- `dump.rs` は `--dump` のテキスト出力. `main.rs` は引数処理とイベントループ. 200ms ごとにキー入力を poll し, `app.pump()` で Event を処理し, `AUTO_REFRESH` (5 分) 経過で再取得する. `Action::Suspend` を受けると `ratatui::restore()` で TUI を抜けて外部コマンド (diff, ブラウザ) を端末に繋ぎ, 終了後に `ratatui::init()` で戻る

新しい操作を足すときは, キーの判断を `update.rs` に書き, 外部とのやりとりが要れば `Effect` (と結果の `Event`) を足して `infra/gh.rs` の関数を `runtime.rs` の `perform` から呼ぶ. 判定ロジックは domain に置く. 列を足すときは `infra/graphql.rs` の fragment と `convert`, `domain/pr.rs` の `Pr`, `ui/table.rs`, `dump.rs` を触る.

テストは各ファイル末尾の `#[cfg(test)] mod tests` に置く. domain は純粋関数を直接, `infra/graphql.rs` は JSON 文字列から `parse_*` を, `app/update.rs` は gh を呼ばずに `AppState` と返った `Vec<Effect>` を assert する. `Pr::fixture` がテスト用の PR を作る. runtime と infra の gh 呼び出しは `--dump` と tmux で手動確認する.

## 前提になっている判断

- main 列は `main` 固定で `headRef { compare(headRef:"main") { aheadBy } }` から出す. stacked PR (base が一覧内の別 PR の head) は `domain/stack.rs` の `group_stacks` で親の直後に並べ, Stack 列に `段/段数` を出す. `u` は update-branch API なので stacked PR では親ブランチを取り込む
- チェックアウト先はメイン checkout で, PR ごとの worktree は作らない. 未コミットの変更 (untracked は除く) があれば `gh pr checkout` を実行しない
- `u` (main 取り込み) はリモートに merge commit を作るだけでローカルは触らない. `mergeable == CONFLICTING` のときは無効. 実行前に y/N を聞く
- crossterm は `ratatui::crossterm` の re-export を使い, 直接依存に足さない
- 状態列は Conflict / Draft / Ready の順に判定する. Conflict のとき Draft かどうかは列からは分からない
- `O` は open (Draft を外す) を 1 キーで実行し, `D` (draft に戻す) と `X` (close) は同じキーを 2 回押して実行する. 1 回目は `confirm_op` に入れてフッターに確認を出す
- `R` を 2 回で git config `prtui.reviewers` (カンマか空白区切り, `--add` で複数値も可) の相手に review を依頼する. Draft の PR では無効. 依頼中の人と author 自身は除き, レビュー済みの人には再依頼する. login は大文字小文字を区別せずに比べる
- `l` は PR のブランチ名 (大文字小文字を無視), 次にタイトルから Linear の issue キー (`[A-Z][A-Z0-9]*-[0-9]+`) を手書きの走査で探し, `https://linear.app/<workspace>/issue/<KEY>` を `open` で開く. workspace は git config `prtui.linear-workspace` から `infra/git.rs` の `read_local_state` で読む (repo ごとに違うため). regex crate は足さない
- `L` は Linear review を開く. URL の slugId は GitHub 側から組み立てられないので, Linear の bot が PR に付けるコメントの `<!-- linear-review-link -->` の直後の href を使う. コメント中のそれより前には関連する別 PR の review URL が並ぶので見ない
