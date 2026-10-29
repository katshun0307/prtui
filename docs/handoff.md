# prtui 申し送り

検証用 repo のセッションで作り始めた. 続きはこの repo のセッションで行う.

## 目的

レビューに出す前の PR を手元でチェックアウトして確認し, 修正箇所をコメントする作業を楽にする. PR 一覧から状態の確認とチェックアウトを 1 画面で行う TUI.

## 決定済みの仕様

- 対象 repo: 起動ディレクトリ (既定は cwd, `-C <dir>` で上書き) の repo. `gh repo view` で `owner/name` を取る
- 対象 PR: open PR のうち author:@me または assignee:@me. GraphQL の `search` を 2 つ alias して 1 リクエストで取り, 番号で重複除去し, 更新日時の降順に並べる
- stacked PR: `baseRefName` が一覧内の別 PR の `headRefName` になっていれば親子とみなし, 親の直後に子を並べてまとめる (`github::group_stacks`). stack 同士は stack 内で最も新しい更新日時の降順. 親が一覧に無い PR (他人の PR の上に積んだものなど) は根として扱うので stack にならない. 枝分かれした stack は深さ優先で並べ, 段数は最も深い枝に合わせる
- チェックアウト先: メイン checkout. worktree 内から起動しても `git rev-parse --git-common-dir` の親を対象にする. PR ごとの worktree は作らない (ユーザーの選択)
- チェックアウト: `gh pr checkout <n>`. 未コミットの変更 (untracked は除く) があれば実行しない
- main の取り込み: `u` キー. stacked PR では update-branch API が base (親のブランチ) を取り込むので, 確認プロンプトとメッセージに base 名を出す. base が main 以外のときは取り込み済みかを判定できないので常に確認を出す. GitHub の update-branch API (`PUT repos/{repo}/pulls/{n}/update-branch`) で remote ブランチに merge commit を作る. ローカルは触らない. `expected_head_sha` に取得時の head oid を渡し, その後に head が動いていれば GitHub が 422 で拒否する. コンフリクトしている PR (`mergeable == CONFLICTING`) では無効にする (ユーザーの指定). 実行前に y/N で確認する
- PR の状態変更: `O` で open (Draft を外す, `gh pr ready`), `D` を 2 回で draft に戻す (`gh pr ready --undo`), `X` を 2 回で close (`gh pr close`). open は 1 キー, draft と close は 2 キーにする (ユーザーの指定). 1 回目でフッターに確認を出し, 同じキー以外を押すと取りやめる. 既に Draft の PR への draft と, Draft でない PR への open は何もしない. 実行はスレッドで行い, 終わったら再取得する
- review 依頼: `R` を 2 回で git config `prtui.reviewers` の相手に review を依頼する (`gh pr edit <n> --add-reviewer`). 相手は repo ごとに違うので git config で指定する (ユーザーの指定. 最初は alice と bobBuilder の固定だった). 値はカンマか空白で区切るか, `git config --add` で 1 人ずつ足す (`LocalState::read` が `--get-all` で読む). 未設定なら設定を促すエラーを出す. open (Draft でない) PR に限り, Draft では `#<n> は Draft なので review 依頼しない` を出す. 既に依頼中の人と PR author 自身は除き, レビュー済みの人には再依頼する. 全員が依頼中なら何もしない. 反映待ちは全員が依頼中になるまで. login は大文字小文字を区別せずに比べる (API は `bobBuilder` を返すので, 完全一致だと反映待ちが終わらず 20 秒経つまで一覧が変わらなかった)
- Linear を開く: `l` キー. PR の `headRefName` (大文字小文字を無視, 大文字に正規化), 次に `title` から issue キー (`[A-Z][A-Z0-9]*-[0-9]+`) を探し, `https://linear.app/<workspace>/issue/<KEY>` を macOS の `open` で開く. workspace は repo ごとに違うので git config `prtui.linear-workspace` (`LocalState::read` で読む) から取り, 未設定なら設定を促すエラーを出す. キーが無ければ `#<n> に Linear の issue キーが無い` を出す
- Linear review を開く: `L` キー. review の URL (`https://linear.app/<workspace>/review/<タイトル>-<slugId>`) の slugId は Linear 側の ID で, GitHub の情報からは組み立てられない. Linear の公開 GraphQL API にも GitHub の PR 番号から引くクエリが無い (`diff(id:)` は internal で Linear の ID が要る). そこで Linear の bot (linear-code) が PR に付けるコメントから取る. `gh api --paginate repos/{repo}/issues/{n}/comments` を UI とは別スレッドで呼び, `<!-- linear-review-link -->` の直後の `href` を `?openGuide=true` などの query を外して `open` する. 同じコメントには issue 本文の引用があり, そこに関連する別 PR の review URL が並ぶので, 目印より前は見ない. bot のコメントが無い PR (例: issue に紐付いていない PR) では `#<n> に Linear review のリンクが無い` を出す
- キャッシュ: 全件取得に成功するたびに一覧を `$XDG_CACHE_HOME/prtui/<owner>__<name>.json` (未設定なら `~/.cache`) に書く. 起動時はこれを読んで即表示し, 裏で取り直す. ヘッダに `取得 Xm 前` を出し, 取り直しが終わるまで `(キャッシュ)` を付ける. `Pr` の形が変わって読めないキャッシュは捨てる. `--dump` はキャッシュを使わない
- 操作後の更新: `u`, `O`, `DD`, `XX`, `RR` の後は全件を取り直さず, その PR だけを `fetch_pr` (`repository.pullRequest` に同じ fragment) で取り直して差し替える. 反映を待つため 2 秒おきに最大 10 回取り直す. 止める条件は, `u` なら head oid が変わる, open / draft なら `isDraft` が変わる, close なら open でなくなる (一覧から消す). 待っている間は一覧のタイトルに `#n を同期中…` を出す. 差し替え後は `arrange` で並びと stack をまとめ直し, 選択中の PR は番号で追う
- 実装: Rust (edition 2024), ratatui 0.30 (crossterm は `ratatui::crossterm` の re-export を使う), serde, anyhow, chrono. GitHub と git は `gh` と `git` コマンドを `std::process::Command` で呼ぶ. 非同期 runtime は使わず, 時間のかかる処理はスレッド + mpsc で UI をブロックしない

## 構成

| ファイル | 役割 |
|---|---|
| `src/main.rs` | 引数処理 (`-C`, `--dump`), 端末初期化, イベントループ, 外部コマンド実行時の TUI 一時停止 |
| `src/github.rs` | GraphQL クエリと `Pr` への変換, 全件取得 `fetch_prs` と 1 件取得 `fetch_pr`, `update_branch` |
| `src/cache.rs` | PR 一覧のディスクキャッシュの読み書き |
| `src/git.rs` | コマンド実行ヘルパ `run`, メイン worktree の解決, `LocalState` (現在ブランチ, dirty, refs/heads の oid) |
| `src/app.rs` | 状態とキー操作 |
| `src/ui.rs` | 描画 |

## 一覧の列

- Stack: stacked PR の中の位置 `段/段数` (cyan). main に近い側が 1. タイトル列は 2 段目以降を `└` で字下げする. 詳細欄の branch 行に `head ← base` を出す
- 状態: Conflict / Draft / Ready の順に判定. Conflict のときは Draft かどうかがこの列では分からない
- CI: 最新コミットの `statusCheckRollup`. CheckRun は status が COMPLETED 以外なら実行中, それ以外は conclusion (SUCCESS と NEUTRAL を成功, SKIPPED を除外, 他を失敗). StatusContext は state
- 未解決: 未解決の review thread 数 (`reviewThreads(first:100)`)
- main: main にあって PR ブランチに無いコミット数. `headRef { compare(headRef:"main") { aheadBy } }` で取る. base は main 固定 (検証用 repo は trunk-based で base が常に main のため)
- 差分: 追加行 `+x` (green) と削除行 `-y` (red). `additions` / `deletions`
- Review: `reviewDecision`. GitHub がブランチ保護ルールから計算する値なので, 必須レビューのルールが無い repo (検証用 repo など) では常に `-` になる
- Reviewer: 承認 ✓n (green), 変更要求 ✗n (red), 依頼中 ●n (yellow) の人数. `reviewRequests` (依頼中) と `latestReviews` (各人の最新レビュー) から作る. 承認後に再依頼された人は依頼中として数える. author 自身, DISMISSED, PENDING は除く. コメントのみの人 (devin などの bot を含む) は数えない. 数える人が居なければ「未設定」で, Draft なら DIM, それ以外は yellow. 詳細欄の `reviewers:` 行に名前と状態を全員分出す. Review 列が `-` の repo でも誰に頼んでいて誰が承認したかが分かるように足した
- Local: ローカルの同名ブランチの oid が PR の head と一致すれば「最新」, 違えば「差分あり」. 現在のブランチに ● を付ける

## 確認済み

- `cargo clippy --all-targets` の警告 0 件, `cargo fmt --check` 通過
- `--dump` で 検証用 repo の実 PR 28 件と main 列が取れる
- tmux 上で一覧, 詳細, 移動, Draft への絞り込みが表示どおり動く (main 取り込み機能を足す前の版で確認)
- 2026-10-28: main 取り込み機能を足した版で `cargo build --release`, clippy 0 件, fmt 通過. 検証用 repo で `--dump` 28 件取得, tmux で一覧 (main 列 `↓4` など) と `u` の y/N プロンプト表示, `n` で取り消し, `q` 終了を確認
- 2026-10-28: `cargo install --path .` を実行し `~/.cargo/bin/prtui` は最新版
- 2026-10-28: Reviewer 列を追加. 検証用 repo で `--dump` と tmux 表示を確認 (#8244 が `✓1 ●2`, bot コメントのみの Ready PR が yellow の「未設定」). `Taskfile.yml` を追加し `task` で check, build, install が通り `~/.cargo/bin/prtui` を入れ替えた
- 2026-10-28: `L` (Linear review) と差分列を追加. 検証用 repo で review URL の抽出が #8241 と #8148 で正しく, #8244 は None になることを確認. tmux で `L` を押し #8152 の review がブラウザで開くこと, 差分列の表示を確認

- 2026-10-28: stacked PR のまとめ表示を追加. 検証用 repo で #8246 → #8152 (1/2, 2/2) と #8221 → #8224 → #8226 (1/3〜3/3) がまとまり, #8152 で `u` を押すと親ブランチ名で y/N が出ることを tmux で確認. `group_stacks` の単体テスト (並び, 枝分かれ, 親が一覧に無い, 循環) を追加

- 2026-10-28: `O` / `DD` / `XX` を追加. tmux で `D` と `X` の確認表示と取りやめを確認. 実際の状態変更は実行していない

- 2026-10-28: キャッシュと PR 単位の取り直しを追加. 検証用 repo でキャッシュがあると起動 1.5 秒で `取得 0m 前 (キャッシュ)` と 26 件が出て, 取得後に `(キャッシュ)` が消えることを tmux で確認. `fetch_pr` のクエリは open の PR で `OPEN`, merge 済みの PR で `MERGED` (→ None) を返すことを `gh api graphql` で確認

- 2026-10-29: `RR` (review 依頼) を追加. clippy 0 件, fmt 通過, `cargo test` 通過
- 2026-10-29: `RR` 後に Reviewer 列が変わらない不具合を直した. 原因は login の大文字小文字. 依頼相手を git config `prtui.reviewers` に移した

## 未確認


- 操作後の `#n を同期中…` からの差し替え (実際の `u`, `O`, `DD`, `XX` を実行していないため)

- `O`, `DD`, `XX` で実際に PR の状態を変えること

- `Enter` でのチェックアウト, `u` での main 取り込み (y を押した先), `d` (diff), `o` (ブラウザ) は実際には実行していない. 最初に手元で試す
- `l` は実際にブラウザを開いて確認していない (issue キー抽出は `cargo test` の単体テストで確認済み)

## 気付いている課題

- update-branch の反映を 20 秒 (2 秒 × 10 回) 待っても head が動かなければ, その時点の結果を出して止める. そのときは main 列が古いままになるので `r` で取り直す
- 1 件の取り直しと全件取得が並行すると, 後に届いた方で上書きされる. 全件取得が操作前の状態を返すと一時的に古く見える (次の取得で直る)
- `差分あり` はローカルが遅れているのか進んでいるのかを区別しない
- `o` (`gh pr view --web`) は UI スレッドで同期実行している
- stacked PR の main 列は main との差なので, 親の変更を取り込めているかは分からない
- PR が 100 件を超えると search の `first:100` で切れる
