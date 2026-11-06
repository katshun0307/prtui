# prtui

自分が author か assignee の open PR を 1 画面に並べ, 状態の確認, チェックアウト, main の取り込みを行う TUI. 別タブには自分がレビューを依頼された PR とレビューした PR を並べる. GitHub の検索クエリで絞ったタブを git config で足すこともできる. PR に紐づく Devin session の会話を読み, メッセージを送ることもできる.

GitHub と git の操作はすべて `gh` と `git` コマンドを呼んで行う.

## 必要なもの

- Rust (edition 2024)
- [`gh`](https://cli.github.com/) (`gh auth login` 済み)
- `git`
- `curl` (Devin 連携を使う場合)
- macOS の `open` (ブラウザで開く操作)
- [Task](https://taskfile.dev/) (任意)

## インストール

```sh
task               # fmt と clippy の確認, release ビルド, ~/.cargo/bin/prtui への install
# または
cargo install --path .
```

## 使い方

```sh
prtui              # カレントディレクトリの repo を対象にする
prtui -C <dir>     # 対象の repo を指定する
prtui --dump       # TUI を出さずに取得結果をテキストで出す
```

worktree の中から起動してもメインの checkout を対象にする. 一覧は `~/.cache/prtui/` にキャッシュし, 起動時はキャッシュを即表示して裏で取り直す. 起動中は 5 分ごとに取り直す.

## 一覧の列

| 列 | 内容 |
|---|---|
| Stack | stacked PR (base が一覧内の別 PR の head) の中の位置 `段/段数`. 親の直後に子を並べる |
| 状態 | Conflict / Draft / Ready |
| CI | 最新コミットのチェックの結果 |
| 未解決 | 未解決の review thread 数 |
| main | main にあって PR ブランチに無いコミット数 |
| 差分 | 追加行と削除行 |
| Reviewer | 承認 ✓, 変更要求 ✗, 依頼中 ●, コメントのみ ✎ の人数 |
| Local | ローカルの同名ブランチが PR の head と一致すれば「最新」, 違えば「差分あり」 |
| Devin | PR に紐づく最新の Devin session の status |

レビュータブでは Stack と main を外し, Author と自分の立場 (依頼中 / 再依頼 / 承認済 / 変更要求 / コメント済), 依頼 (レビュー依頼を受けてからの経過時間) を出す. 依頼中の PR を依頼の古い順に先頭から並べ, レビュー済みの PR はその下に更新の新しい順に並べる.

追加のタブ (`prtui.tab`) では Stack を外し, Author と Assignee (先頭の 1 人と残りの人数) を出す. stack はまとめず, 更新の新しい順に並べる.

## キー操作

### 一覧

| キー | 操作 |
|---|---|
| `j` / `k`, `↓` / `↑` | 移動. 端を越えたら反対の端に回る |
| `g` / `G` | 先頭 / 末尾 |
| `Tab` / `Shift-Tab`, `1`〜`9` | タブの切り替え. `Tab` は端を越えたら反対の端に回る. 数字は左から何番目のタブか |
| `t` | Draft だけに絞る / 戻す |
| `r` | 取り直す |
| `Enter` | メインの checkout で `gh pr checkout` する. 未コミットの変更があれば実行しない. ローカルの同名ブランチが PR と分岐して失敗したら y/N を聞き, y で `--force` を付けて PR の最新に合わせる |
| `c` | コピー選択画面を開き, 続くキーで選んだ値をクリップボードに入れる (`b` ブランチ名, `n` PR 番号, `t` タイトル, `u` URL, `m` Markdown リンク, `i` Linear issue キー, `l` Linear issue の URL) |
| `d` | `gh pr diff` |
| `o` | PR をブラウザで開く |
| `u` | main (stacked PR では親ブランチ) をリモートのブランチに取り込む. y/N で確認する. Conflict のときは無効 |
| `O` | Draft を外す |
| `D` `D` | Draft に戻す |
| `X` `X` | close する |
| `R` `R` | `prtui.reviewers` の相手に review を依頼する. レビュー済みの人には再依頼する |
| `l` | ブランチ名かタイトルの issue キーから Linear の issue を開く |
| `L` | Linear の bot のコメントから Linear review を開く |
| `v` | Devin session の会話ビューを開く |
| `V` | Devin Review を開く (自分タブ以外) |
| `q` / `Esc` | 終了 |

`u`, `O`, `D`, `X`, `R` は他人の PR を変えるので, 自分タブ以外 (レビュータブと追加のタブ) では使えない.

### 会話ビュー

| キー | 操作 |
|---|---|
| `j` / `k`, `Ctrl-d` / `Ctrl-u`, `g` / `G` | スクロール |
| `i` | 1 行入力. `Enter` で送信, `Esc` でやめる |
| `e` | `${VISUAL:-${EDITOR:-vi}}` で書いて送信する |
| `o` | session をブラウザで開く |
| `r` | 取り直す |
| `q` / `Esc` | 一覧に戻る |

## 設定

設定は git config で行う. repo ごとに変えたいものは repo の config に, 共通のものは `--global` に入れる.

```sh
# R R で review を依頼する相手. カンマか空白で区切るか, --add で 1 人ずつ足す
git config prtui.reviewers "alice,bob"

# l で開く Linear の workspace
git config prtui.linear-workspace my-workspace

# Devin 連携 (v). token は個人のもの, org は org-... の ID
git config --global prtui.devin-token <token>
git config prtui.devin-org org-xxxxxxxx

# 追加のタブ. "名前=検索クエリ" を --add で 1 タブずつ足す. 書いた順に自分タブとレビュータブの後ろに並ぶ
git config --add prtui.tab 'マージ予定=label:マージ予定'
git config --add prtui.tab '承認待ち=review:required -is:draft'
```

追加のタブのクエリは [GitHub の検索構文](https://docs.github.com/ja/search-github/searching-on-github/searching-issues-and-pull-requests) で書く. 前に `repo:<owner/name> is:pr is:open` を付けて検索するので, 対象は常にその repo の open PR になる. 最初の `=` で名前とクエリに分けるので, クエリに `=` が入ってもよい. 設定は起動時に読むので, 変えたら起動し直す. 書き間違えたクエリはエラーにならず 0 件になる.

Linear と Devin の連携は任意. 設定が無ければ, Devin 列, 詳細欄の `devin:` 行, `l` と `v` のキー案内, `c` の Linear issue の URL を出さない. `L` (Linear review) と `V` (Devin Review) は設定が無くても使える.

Devin session は Devin API v3 から直近 30 日に更新されたものを取り, PR の URL で照合する. 1 つの PR に複数あれば最新のものを使い, Devin 列に `×n` を付ける.

## 開発

```sh
task check         # cargo fmt --check と cargo clippy --all-targets -D warnings
task build         # check 後に release ビルド
cargo test
```

仕様の決定経緯と既知の課題は [`docs/handoff.md`](docs/handoff.md) にある.
