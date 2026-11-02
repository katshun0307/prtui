# prtui 申し送り

検証用 repo のセッションで作り始めた. 続きはこの repo のセッションで行う.

## 目的

レビューに出す前の PR を手元でチェックアウトして確認し, 修正箇所をコメントする作業を楽にする. PR 一覧から状態の確認とチェックアウトを 1 画面で行う TUI.

## 決定済みの仕様

- 対象 repo: 起動ディレクトリ (既定は cwd, `-C <dir>` で上書き) の repo. `gh repo view` で `owner/name` を取る
- 対象 PR: open PR のうち author:@me または assignee:@me. GraphQL の `search` を 2 つ alias して 1 リクエストで取り, 番号で重複除去し, 更新日時の降順に並べる
- stacked PR: `baseRefName` が一覧内の別 PR の `headRefName` になっていれば親子とみなし, 親の直後に子を並べてまとめる (`domain/stack.rs` の `group_stacks`). stack 同士は stack 内で最も新しい更新日時の降順. 親が一覧に無い PR (他人の PR の上に積んだものなど) は根として扱うので stack にならない. 枝分かれした stack は深さ優先で並べ, 段数は最も深い枝に合わせる
- チェックアウト先: メイン checkout. worktree 内から起動しても `git rev-parse --git-common-dir` の親を対象にする. PR ごとの worktree は作らない (ユーザーの選択)
- チェックアウト: `gh pr checkout <n>`. 未コミットの変更 (untracked は除く) があれば実行しない. ローカルの同名ブランチが PR と分岐していると gh はブランチを切り替えた後に fast-forward できず失敗する (`Diverging branches` / `Not possible to fast-forward`. `domain/local.rs` の `is_diverged` で判定). このときはエラーの代わりにフッターで y/N を聞き, y なら `gh pr checkout --force <n>` でローカルブランチを PR の最新に reset する (2026-11-02 追加. ユーザーの指定). ローカルだけのコミットは消える (reflog には残る). `--force` でも未コミットの変更があれば実行しない. `--force` でも失敗したらもう聞かずにエラーを出す
- main の取り込み: `u` キー. stacked PR では update-branch API が base (親のブランチ) を取り込むので, 確認プロンプトとメッセージに base 名を出す. base が main 以外のときは取り込み済みかを判定できないので常に確認を出す. GitHub の update-branch API (`PUT repos/{repo}/pulls/{n}/update-branch`) で remote ブランチに merge commit を作る. ローカルは触らない. `expected_head_sha` に取得時の head oid を渡し, その後に head が動いていれば GitHub が 422 で拒否する. コンフリクトしている PR (`mergeable == CONFLICTING`) では無効にする (ユーザーの指定). 実行前に y/N で確認する
- PR の状態変更: `O` で open (Draft を外す, `gh pr ready`), `D` を 2 回で draft に戻す (`gh pr ready --undo`), `X` を 2 回で close (`gh pr close`). open は 1 キー, draft と close は 2 キーにする (ユーザーの指定). 1 回目でフッターに確認を出し, 同じキー以外を押すと取りやめる. 既に Draft の PR への draft と, Draft でない PR への open は何もしない. 実行はスレッドで行い, 終わったら再取得する
- review 依頼: `R` を 2 回で git config `prtui.reviewers` の相手に review を依頼する (`gh pr edit <n> --add-reviewer`). 相手は repo ごとに違うので git config で指定する (ユーザーの指定. 最初は alice と bobBuilder の固定だった). 値はカンマか空白で区切るか, `git config --add` で 1 人ずつ足す (`infra/git.rs` の `read_local_state` が `--get-all` で読む). 未設定なら設定を促すエラーを出す. open (Draft でない) PR に限り, Draft では `#<n> は Draft なので review 依頼しない` を出す. 既に依頼中の人と PR author 自身は除き, レビュー済みの人には再依頼する. 全員が依頼中なら何もしない. 反映待ちは全員が依頼中になるまで. login は大文字小文字を区別せずに比べる (API は `bobBuilder` を返すので, 完全一致だと反映待ちが終わらず 20 秒経つまで一覧が変わらなかった)
- Linear を開く: `l` キー. PR の `headRefName` (大文字小文字を無視, 大文字に正規化), 次に `title` から issue キー (`[A-Z][A-Z0-9]*-[0-9]+`) を探し, `https://linear.app/<workspace>/issue/<KEY>` を macOS の `open` で開く. workspace は repo ごとに違うので git config `prtui.linear-workspace` (`infra/git.rs` の `read_local_state` で読む) から取り, 未設定なら設定を促すエラーを出す. キーが無ければ `#<n> に Linear の issue キーが無い` を出す
- Linear review を開く: `L` キー. review の URL (`https://linear.app/<workspace>/review/<タイトル>-<slugId>`) の slugId は Linear 側の ID で, GitHub の情報からは組み立てられない. Linear の公開 GraphQL API にも GitHub の PR 番号から引くクエリが無い (`diff(id:)` は internal で Linear の ID が要る). そこで Linear の bot (linear-code) が PR に付けるコメントから取る. `gh api --paginate repos/{repo}/issues/{n}/comments` を UI とは別スレッドで呼び, `<!-- linear-review-link -->` の直後の `href` を `?openGuide=true` などの query を外して `open` する. 同じコメントには issue 本文の引用があり, そこに関連する別 PR の review URL が並ぶので, 目印より前は見ない. bot のコメントが無い PR (例: issue に紐付いていない PR) では `#<n> に Linear review のリンクが無い` を出す
- コピー: `c` で選択中の PR のコピー選択画面を一覧の上にポップアップで出し, 続く 1 キーで選んだ値を `pbcopy` でクリップボードに入れる (2026-11-02 追加. ユーザーの指定で `c` をチェックアウトから外し, チェックアウトは `Enter` だけにした). 項目は `b` ブランチ名, `n` `#番号`, `t` タイトル, `u` PR の URL, `m` Markdown リンク `[#番号 タイトル](URL)`, `i` Linear issue キー, `l` Linear issue の URL. 項目は開いたときの PR から `domain/copy.rs` の `copy_items` で作り, 値が作れない項目 (issue キーが無い, `prtui.linear-workspace` が未設定) は理由を DIM で出し, 押すとエラーにする. 項目以外のキーは何もせず閉じる. 両タブで使える
- キャッシュ: 全件取得に成功するたびに一覧を `$XDG_CACHE_HOME/prtui/<owner>__<name>.json` (未設定なら `~/.cache`) に書く. 起動時はこれを読んで即表示し, 裏で取り直す. ヘッダに `取得 Xm 前` を出し, 取り直しが終わるまで `(キャッシュ)` を付ける. `Pr` の形が変わって読めないキャッシュは捨てる. `--dump` はキャッシュを使わない
- フッターのメッセージ: 操作の結果 (チェックアウトした, 取得失敗など) はフッターにキー案内の代わりに出し, 次に何かキーを押すと消してキー案内に戻す. 押したキーが新しいメッセージを出せばそれに置き換わる
- 操作後の更新: `u`, `O`, `DD`, `XX`, `RR` の後は全件を取り直さず, その PR だけを `fetch_pr` (`repository.pullRequest` に同じ fragment) で取り直して差し替える. 止める条件は `WatchUntil` (`app/event.rs`) のデータで表す. 反映を待つため 2 秒おきに最大 10 回取り直す. 止める条件は, `u` なら head oid が変わる, open / draft なら `isDraft` が変わる, close なら open でなくなる (一覧から消す). 待っている間は一覧のタイトルに `#n を同期中…` を出す. 差し替え後は `arrange` で並びと stack をまとめ直し, 選択中の PR は番号で追う
- Devin session: PR に紐づく Devin session のやり取りを読み, メッセージを送る (2026-10-29 追加. 仕様はユーザーと質疑で決めた).
  - 設定は git config `prtui.devin-token` と `prtui.devin-org` (`org-...`). token は個人, org は組織ごとなので置き場所 (global / repo) は自由にした. 未設定なら Devin 列は空で, `v` で設定を促すエラーを出す
  - session の特定: Devin API v3 の session 一覧 (`GET /v3/organizations/{org}/sessions`) に PR で絞る引数が無い. そこで `repo_names=<owner/name>` と `updated_after=30 日前` で取り (200 件 × 最大 5 ページ), 各 session の `pull_requests[].pr_url` を PR の URL と照合する (末尾 `/` と大文字小文字は無視). PR 本文の Devin のリンクから取る案は, 人が作った PR に後から Devin を入れた場合に拾えないので採らなかった. 1 つの PR に複数あれば `updated_at` が最新の 1 つを使う. 他人の session でも PR が一致すれば出す
  - 取得は PR の全件取得のたび (起動時, 5 分ごと, `r`) に別スレッドで行う. PR の取得 (`loading`) とは独立 (`sessions_loading`) で, 失敗しても一覧は出す. キャッシュにも入れる
  - 一覧の Devin 列と詳細欄の `devin:` 行に status を出す. new / claimed / running / resuming は yellow, suspended は cyan, exit と archived は DIM, error は red. 複数なら `×n`
  - 会話ビュー (`v`): 全画面でメッセージを古い順に並べる (Devin は magenta, 自分側は cyan). 開いている間は 5 秒おきに取り直し, 末尾を見ていれば追従する. `j`/`k`, `Ctrl-d`/`Ctrl-u`, `g`/`G` でスクロール. `i` で 1 行入力 (Enter 送信, Esc でやめる), `e` で TUI を止めて `${VISUAL:-${EDITOR:-vi}}` を開き, 0 で終われば送信する (非 0 は取りやめ). `o` で session を Devin の web で開く. `r` ですぐ取り直す. `Esc`/`q` で一覧に戻る (送信中は無視)
  - 送信は status で制限しない (suspended は送信で再開する). API のエラー (409 / 410 など) は会話ビューのフッターに出す. 書きかけは送信が成功するまで残し, 失敗後に `i` か `e` で直せる
  - HTTP は crate を足さず `curl` を呼ぶ. token は argv に載せず `curl --config -` で stdin から渡す (`ps` に出さないため)
- レビュータブ: 自分がレビューを依頼された PR とレビューした PR を別タブに並べる (2026-10-30 追加. 仕様はユーザーと質疑で決めた)
  - 対象: open PR のうち `review-requested:@me` か `reviewed-by:@me` に当たるもの. どちらも `-author:@me` を付けて自分の PR を除く. `review-requested` はチーム宛ての依頼も拾う (`user-review-requested` にしなかったのはユーザーの選択). `reviewed-by` はコメントだけ残した PR も拾う. Draft も出す
  - 取得: `QUERY_REVIEW` で 2 つの alias と `viewer { login }` を 1 リクエストで取り, 依頼中側を先に見て番号で重複を除く. 自分タブとは別スレッドで, 取るのは同じタイミング (起動時, 5 分ごと, `r`). 失敗は一覧のタイトルに赤で出し, フッターには出さない. キャッシュは同じファイルの `reviews` に取得時刻と一緒に入れる
  - 切り替え: `Tab` / `Shift-Tab` と `1` / `2`. ヘッダに `1 自分 (n)  2 レビュー (m)` を出し, 今のタブを反転する. 選択位置と `t` の絞り込みはタブごとに持つ
  - 自分の立場 (`MyReview`): 依頼中の検索に当たれば requested. `latestReviews` から自分 (大文字小文字を無視) の最新のレビューを取る. 依頼中かつレビュー済みなら「再依頼」. `reviewRequests` に自分の login が無ければチーム経由とみなし, PR に依頼されているチームのうち自分の所属チームの slug を詳細欄に `依頼: <team> 経由` で出す. 所属チームは `QUERY_REVIEW` で `repositoryOwner(login:<owner>)` が Organization のとき `teams(role:MEMBER)` と `teams(role:ADMIN)` を取る (maintainer が MEMBER に入るか不明なので両方). 1 つも当たらなければ (owner が User, 子チーム経由で親チームに依頼された, など) 依頼されている全チームに戻す. `read:org` の無いトークンでこのクエリが失敗するかは確かめていない
  - 並び: 依頼中 (再依頼を含む) を先に, その中は依頼を受けた時刻の古い順 (古い依頼から片付けるため. ユーザーの指定). 依頼時刻が分からなければ更新日時で代える. レビュー済みはその後ろに更新日時の降順. stack はまとめない
  - 依頼時刻: `reviewRequests` に時刻が無いので, 依頼中側の alias だけ `FRAGMENT_REQUESTED` で `timelineItems(last:50, itemTypes:[REVIEW_REQUESTED_EVENT])` を取る. 直接の依頼なら自分宛ての最新の event (再依頼なら最後の依頼) を `MyReview::requested_at` にする. チーム経由なら, 今依頼されているチームごとの最新の event のうち最も古いものを使う. 所属チームで絞れず全チームに戻ったときに, 後から関係ないチームが依頼されても待ち時間を短く見せないため (サブエージェントのレビューの指摘). 依頼時刻が同じなら番号の昇順. 一覧の「依頼」列に経過時間を yellow で出す
  - 操作: `Enter`, `c`, `d`, `o`, `l`, `L`, `v`, `t`, `r` は使える. 他人の PR を変える `u` / `O` / `DD` / `XX` / `RR` は無効にし, `<key> はレビュータブでは使えない` を出す (ユーザーの指定)
  - Devin Review: `V` で `https://app.devin.ai/review/<owner>/<name>/pull/<n>` を `open` で開く. repo と PR 番号だけで組み立てるので, Devin の設定 (token, org) も API も要らない. ユーザーの指定でレビュータブだけにした. 自分タブでは何もしない
- 実装: Rust (edition 2024), ratatui 0.30 (crossterm は `ratatui::crossterm` の re-export を使う), serde, anyhow, chrono. GitHub と git は `gh` と `git` コマンドを `std::process::Command` で呼ぶ. 非同期 runtime は使わず, 時間のかかる処理はスレッド + mpsc で UI をブロックしない

## 構成

層は domain → infra → app → ui / dump → main の順に積み, 下に向かってのみ依存する. 依存規則と新機能を足すときに触るファイルは `CLAUDE.md` の「構造」節にある.

| ファイル | 役割 |
|---|---|
| `src/main.rs` | 引数処理 (`-C`, `--dump`), 端末初期化, イベントループ, 外部コマンド実行時の TUI 一時停止 |
| `src/dump.rs` | `--dump` のテキスト出力 |
| `src/domain/` | 純粋な型と関数. `pr.rs` (`Pr`, `MyReview`), `stack.rs` (`arrange`, `group_stacks`), `review.rs` (`review_targets`, `arrange_reviews`), `linear.rs` (issue キーと review URL の抽出), `copy.rs` (`copy_items`), `local.rs` (`LocalState`, `parse_reviewers`), `op.rs` (`PrOp`), `devin.rs` (Devin の型と `session_for`, `review_url`) |
| `src/infra/process.rs` | コマンド実行ヘルパ `run`, `open_url` |
| `src/infra/git.rs` | メイン worktree の解決 `main_worktree`, `read_local_state` (現在ブランチ, dirty, refs/heads の oid, git config) |
| `src/infra/gh.rs` | gh コマンド. 全件取得 `fetch_prs`, レビュータブの `fetch_reviews`, 1 件取得 `fetch_pr`, `update_branch`, `checkout`, `change_state`, `view_web`, `linear_review_url` |
| `src/infra/graphql.rs` | GraphQL クエリ, `Raw*` 型と `Pr` への変換, JSON の解析 `parse_search` / `parse_review` / `parse_one` |
| `src/infra/devin.rs` | Devin API v3 (`fetch_sessions`, `fetch_messages`, `send_message`) を curl で呼ぶ. JSON の解析 `parse_*` |
| `src/infra/cache.rs` | PR 一覧と Devin session 一覧のディスクキャッシュの読み書き |
| `src/app/state.rs` | 全状態 `AppState`, タブ `Tab` とタブごとの一覧 `PrList`, 会話ビューの `ChatView` |
| `src/app/event.rs` | `Event` (スレッドの結果), `Effect` (副作用の要求), `Action` (main への指示), `WatchUntil` |
| `src/app/update.rs` | キー操作, イベント処理, 時間の遷移 (`tick`). `AppState` を更新して `Vec<Effect>` を返す純粋関数 |
| `src/app/runtime.rs` | `App`. Effect を実行し, スレッドの起動と infra の呼び出しをここに閉じる |
| `src/ui/` | 描画. `mod.rs` (ヘッダ, フッタ, 配色), `table.rs` (一覧), `detail.rs` (詳細欄), `chat.rs` (会話ビュー), `format.rs` |

### レイヤー化した理由

refactor 前は 1 つの `App` に状態, キー判断, スレッド起動, gh 呼び出しが混ざっていて, キー操作の振る舞いを gh を呼ばずに確かめる手段が無かった. そこで判断 (update) と実行 (runtime) を分け, update は「何をするか」を `Effect` のデータとして返すだけにした. gh / git の差し替え境界は trait ではなくこの `Effect` にした. キー操作とイベント処理のテストは `AppState` と返った `Vec<Effect>` の assert で書け, gh の実装が 1 つしか無いうちに trait を作る必要が無いため. runtime は薄く保って `--dump` と tmux で手動確認する. runtime もテストしたくなったら, その時点で `infra/gh.rs` の関数群を trait にする. watch の終了条件はクロージャだと `Effect` が `Debug` / `PartialEq` を持てず assert できないので, `WatchUntil` の enum にした. `AppState` は ratatui の `TableState` を持つ (選択位置とスクロール offset を毎フレーム作り直すと振る舞いが変わるため). `KeyEvent` もそのまま受け, 独自の入力型は作らない. 新旧で `--dump` の出力と TUI の画面が一致することを確かめてから merge した.

## 一覧の列

- Stack: stacked PR の中の位置 `段/段数` (cyan). main に近い側が 1. タイトル列は 2 段目以降を `└` で字下げする. 詳細欄の branch 行に `head ← base` を出す
- 状態: Conflict / Draft / Ready の順に判定. Conflict のときは Draft かどうかがこの列では分からない
- CI: 最新コミットの `statusCheckRollup`. CheckRun は status が COMPLETED 以外なら実行中, それ以外は conclusion (SUCCESS と NEUTRAL を成功, SKIPPED を除外, 他を失敗). StatusContext は state
- 未解決: 未解決の review thread 数 (`reviewThreads(first:100)`)
- main: main にあって PR ブランチに無いコミット数. `headRef { compare(headRef:"main") { aheadBy } }` で取る. base は main 固定 (検証用 repo は trunk-based で base が常に main のため)
- 差分: 追加行 `+x` (green) と削除行 `-y` (red). `additions` / `deletions`
- Review: `reviewDecision`. GitHub がブランチ保護ルールから計算する値なので, 必須レビューのルールが無い repo (検証用 repo など) では常に `-` になる
- Reviewer: 承認 ✓n (green), 変更要求 ✗n (red), 依頼中 ●n (yellow), コメントのみ ✎n (DIM) の人数. `reviewRequests` (依頼中) と `latestReviews` (各人の最新レビュー) から作る. 承認後に再依頼された人は依頼中として数える. author 自身, Bot (`latestReviews` の author の `__typename` が `Bot`. devin-ai-integration など), DISMISSED, PENDING は除く. Bot は GitHub の Reviewers 欄に出ないため. コメントのみの人は ✎n で別に数え, 承認・変更要求・依頼中が居なければ「未設定」を前に付ける (例 `未設定 ✎2`).「未設定」は Draft なら DIM, それ以外は yellow. 詳細欄の `reviewers:` 行に名前と状態を全員分出す. Review 列が `-` の repo でも誰に頼んでいて誰が承認したかが分かるように足した
- Local: ローカルの同名ブランチの oid が PR の head と一致すれば「最新」, 違えば「差分あり」. 現在のブランチに ● を付ける
- Devin: PR に紐づく最新の Devin session の status. 複数なら `×n`. Devin を設定していなければ空

## 確認済み

- 2026-10-30: レビュータブを追加. clippy 0 件, fmt 通過, `cargo test` 98 件通過. 検証用 repo で `--dump` の `# review` に依頼中 2 件と承認済 2 件が依頼中を先にして出ること, tmux で `Tab` / `1` / `2` の切り替え, レビュータブの列, `u` のエラー表示, 再起動後にキャッシュから `(キャッシュ)` 付きで出ることを確認
- 2026-10-30: レビュータブの依頼中を依頼時刻の古い順に並べ, 「依頼」列を追加. `cargo test` 101 件通過. 検証用 repo の `--dump` と tmux で依頼中 2 件が 1h → 5m の順に並び, レビュー済みがその下にまとまることを確認
- 2026-10-30: チーム経由の依頼を自分の所属チームに絞る. `cargo test` 102 件通過. 検証用 repo の `--dump` でクエリが通ることを確認. その時点で依頼中の PR が無かったので, チーム経由の絞り込みは単体テストでだけ確認した

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
- 2026-10-29: レイヤー構成に refactor した. clippy 0 件, fmt 通過, `cargo test` 43 件通過. `--dump` の出力と tmux の画面が refactor 前と一致することを確認
- 2026-10-29: Devin session 連携を追加. fable に設計を相談して層ごとの契約を骨組みとして先に書き, domain+infra, app+main, ui+dump を 3 つの opus subagent に worktree で並行実装させて merge した. clippy 0 件, fmt 通過, `cargo test` 88 件通過. Devin 未設定の 検証用 repo で tmux 表示 (Devin 列が空) と `v` の設定エラーを確認. 偽の token で curl を叩き 403 の ProblemDetail がエラーになることを確認
- 2026-10-29: merge 後に fable に統合レビューを頼み, 次の 5 点を直した. `cargo test` 89 件通過
  - キャッシュの session を起動時に復元していなかった
  - config が消えると `sessions_loading` が立ったまま戻らなかった
  - 送信失敗のエラーが次のポーリングで消えていた
  - 送信中に打った文字が成功時に消えていた
  - 会話ビューのヘッダが選択中の PR を引いていて, PR が一覧から消えるとずれた. `ChatView` に PR 番号を持たせた
- 2026-11-02: Reviewer 列にコメントのみの人数 ✎n (DIM) を足した. 列幅を 10 から 13 に広げた. 検証用 repo の tmux 表示で `✓1 ✎2`, `未設定 ✎1` を確認. `cargo test` 103 件通過
- 2026-11-02: Bot (devin-ai-integration など) のレビューを reviewer から外した. GitHub の Reviewers 欄に出ず, ✎n に数えると誤解を招くため
- 2026-11-02: チェックアウトがブランチの分岐で失敗したら y/N を聞き, `--force` で PR の最新に合わせるようにした. `cargo test` で状態遷移だけ確認し, 実際の `--force` は未実行
- 2026-11-02: `c` をコピー選択画面にした. 検証用 repo の tmux 表示でポップアップの桁揃えと, `m` で Markdown リンクが `pbpaste` に入ることを確認. `cargo test` 109 件通過

## 未確認

- レビュータブの「再依頼」, 「変更要求」, チーム経由の表示. 検証用 repo に該当する PR が無かったので, `parse_review` の単体テストでだけ確かめた

- Devin API を正しい token で叩くこと全般. 特に次の 4 点
  - 時刻の単位 (秒と仮定し, 1e12 を超えれば ms とみなしている)
  - `repo_names` が 1 値の query で効くか
  - POST messages が SessionResponse を返すか
  - `pull_requests[].pr_url` が GitHub の PR URL と同じ形か
- 会話ビューの実画面 (スクロール, 入力, エディタからの復帰と一時ファイルの削除)


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
- PR が 100 件を超えると search の `first:100` で切れる. レビュータブの `reviewed-by:@me` は close されるまで残るので, 長く open のままの PR が溜まりうる
- Devin session は直近 30 日に更新されたもので, 最大 1000 件しか見ない. それより古い session の PR は Devin 列が空になる
- 詳細欄は高さ 10 で固定. CI の失敗と実行中の行と devin 行が全部出ると URL の行が欄からはみ出ることがある
