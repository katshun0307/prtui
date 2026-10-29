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

単体テストは `cargo test` (issue キーと review URL の抽出, stack のまとめ方). 動作確認は次の 2 段階で行う.

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

スレッドと mpsc で UI をブロックしない設計. 非同期 runtime は使わない.

- `main.rs` はイベントループ. 200ms ごとにキー入力を poll し, `app.rx` に届いた `Event` を処理し, `AUTO_REFRESH` (5 分) 経過で再取得する. `Action::Suspend` を受けると `ratatui::restore()` で TUI を抜けて外部コマンド (diff, ブラウザ) を端末に繋ぎ, 終了後に `ratatui::init()` で戻る
- `app.rs` の `App` が全状態を持つ. 時間のかかる処理 (`fetch_prs`, `gh pr checkout`, `update_branch`) は `thread::spawn` して結果を `Event` として `tx` に送る. 操作後は全件を取り直さず `watch` でその PR だけを反映されるまで `fetch_pr` し直し (`Event::Reloaded`), `set_prs` で差し替える. `set_prs` は選択を PR 番号で保ち, キャッシュも書く. `handle_key` は `Action` を返すだけで, 描画やコマンド実行はしない. 実行中は `busy` に文字列を入れて多重実行を防ぐ
- `github.rs` は GraphQL クエリ 1 本 (`QUERY`) で author と assignee を 2 つの alias として同時に取り, `RawPr` → `Pr` に `convert` する. 番号で重複除去し, `arrange` で更新日時の降順に並べて stack をまとめる. `fetch_pr` は同じ fragment を `repository.pullRequest` で使って 1 件だけ取り, open でなければ None を返す. `update_branch` は REST の update-branch API を `expected_head_sha` 付きで呼び, 取得後に head が動いていれば GitHub の 422 で止まるようにしている
- `cache.rs` は PR 一覧を `~/.cache/prtui/<owner>__<name>.json` に読み書きする. 起動時に読んで即表示し, 裏で取り直す. 読めなければ捨てるので, `Pr` のフィールドを変えても移行処理は要らない
- `git.rs` の `run` がコマンド実行の共通ヘルパで, 失敗時は stderr を `anyhow` のエラーにする. `main_worktree` は worktree 内から起動されてもメイン checkout を対象にする. `LocalState` はローカルブランチ名 → oid の表を持ち, PR の head oid と比べて「最新」「差分あり」を出す
- `ui.rs` は描画のみ. 配色の意味 (green / red / yellow / cyan / magenta / dim) はファイル先頭のコメントに定義してあり, 新しい表示を足すときはそれに合わせる. `Color::DarkGray` は端末テーマで背景と同化するので使わず, `DIM` と `SELECTED_BG` の 256 色固定値を使う

## 前提になっている判断

- main 列は `main` 固定で `headRef { compare(headRef:"main") { aheadBy } }` から出す. stacked PR (base が一覧内の別 PR の head) は `group_stacks` で親の直後に並べ, Stack 列に `段/段数` を出す. `u` は update-branch API なので stacked PR では親ブランチを取り込む
- チェックアウト先はメイン checkout で, PR ごとの worktree は作らない. 未コミットの変更 (untracked は除く) があれば `gh pr checkout` を実行しない
- `u` (main 取り込み) はリモートに merge commit を作るだけでローカルは触らない. `mergeable == CONFLICTING` のときは無効. 実行前に y/N を聞く
- crossterm は `ratatui::crossterm` の re-export を使い, 直接依存に足さない
- 状態列は Conflict / Draft / Ready の順に判定する. Conflict のとき Draft かどうかは列からは分からない
- `O` は open (Draft を外す) を 1 キーで実行し, `D` (draft に戻す) と `X` (close) は同じキーを 2 回押して実行する. 1 回目は `confirm_op` に入れてフッターに確認を出す
- `R` を 2 回で git config `prtui.reviewers` (カンマか空白区切り, `--add` で複数値も可) の相手に review を依頼する. Draft の PR では無効. 依頼中の人と author 自身は除き, レビュー済みの人には再依頼する. login は大文字小文字を区別せずに比べる
- `l` は PR のブランチ名 (大文字小文字を無視), 次にタイトルから Linear の issue キー (`[A-Z][A-Z0-9]*-[0-9]+`) を手書きの走査で探し, `https://linear.app/<workspace>/issue/<KEY>` を `open` で開く. workspace は git config `prtui.linear-workspace` から `LocalState::read` で読む (repo ごとに違うため). regex crate は足さない
- `L` は Linear review を開く. URL の slugId は GitHub 側から組み立てられないので, Linear の bot が PR に付けるコメントの `<!-- linear-review-link -->` の直後の href を使う. コメント中のそれより前には関連する別 PR の review URL が並ぶので見ない
