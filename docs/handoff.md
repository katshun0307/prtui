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
- マージ: `M` で選択中の PR のマージの確認画面を一覧の上にポップアップで出し, 方法のキーでマージする (2026-11-11 追加. ユーザーの指定で自分タブだけ, 確認のポップアップを経由する)
  - 開かない: Draft (`#<n> は Draft なのでマージできない. O で open にする`) とコンフリクト (`mergeable == CONFLICTING`) の PR は GitHub がマージしないので, 開かずにエラーを出す. それ以外 (CI 失敗, 承認なし, branch protection) は止めず, マージできるかは GitHub に任せてエラーをそのまま出す
  - 表示: タイトル, `head → base`, 注意点 (`domain/merge.rs` の `merge_warnings`. CI の失敗と実行中, 変更要求, 承認なし, 未解決のスレッド, base が main でない, この PR のブランチを base にしている子 PR) を yellow で出し, 無ければ green で問題なしと出す. その下に repo で許可された方法を `s` squash / `m` merge commit / `r` rebase の順で並べる
  - 方法: GitHub の repo には「既定のマージ方法」の設定は無く, 許可する方法を選ぶだけ. 許可された方法と `viewerDefaultMergeMethod` (自分がその repo で前回使った方法. GitHub の画面のボタンが最初に選んでいるもの) を `gh repo view --json mergeCommitAllowed,squashMergeAllowed,rebaseMergeAllowed,viewerDefaultMergeMethod` で取る. 確認画面を初めて開いたときに別スレッドで取り, 以後は取り直さない (取得中は `マージ方法を取得中…`. 失敗したら確認画面を閉じてエラーを出し, 次に開いたときに取り直す). `Enter` は `viewerDefaultMergeMethod` でマージする. 方法のキーと `Enter` 以外のキーは取りやめる. 取得中の方法のキーも取りやめる (誤ってマージしないため). `gh pr merge` は TTY が無いと方法の指定が要るので, 必ずどれかのフラグを付ける
  - 実行: GitHub の非同期のマージ API で行う (2026-11-11 に `gh pr merge` から変更. GitHub の stacked PR (2026-10 GA) に属する PR は同期のマージ API と GraphQL の mutation ではマージできず, `This pull request is part of a stack and must be merged using the asynchronous merge REST API.` で断られたため). `gh api -X PUT -H 'X-GitHub-Api-Version: 2026-03-10' repos/{repo}/pulls/{n}/merge-async -f merge_method=<方法> -f sha=<確認画面を開いたときの head oid>` で頼み, `status` が `pending` の間は `GET .../merge-async/{uuid}` を 2 秒おきに最大 30 回取り直す (`infra/gh.rs` の `merge`. スレッドの中で待つ). `merged` と `enqueued` で終わり, `failed` は `details.message` をエラーにする. 確認画面を見た後に push された変更は `sha` で断られる. `merge_action` は送らず既定 (merge queue があれば queue, 無ければ直接) に任せる. stack の途中の PR を頼むと, その下の PR もまとめてマージされる (GitHub の仕様). head ブランチは消さない (`u` と同じくローカルは触らない). リモートの head ブランチを消すかは repo の自動削除の設定に任せる. `merged` なら open でなくなる (一覧から消える) まで `WatchUntil::Gone` で 1 件を取り直し, 一緒にマージされた下の PR を消すため全件も取り直す. 全件の search の索引はマージ直後には遅れて `is:open` に当たるので, `parse_search` は node の `state` が OPEN のものだけを残す. `enqueued` なら `#<n> を merge queue に入れた` を出して 1 回だけ取り直す. 失敗したら 1 回だけ取り直す
- review 依頼: `R` で選択中の PR の review 依頼の選択画面を一覧の上にポップアップで出し, 選んだ相手に `gh pr edit <n> --add-reviewer a,b` で依頼する (2026-11-06 に選択画面にした. ユーザーの指定でラベルの選択画面と同じ操作. それまでは `RR` で `prtui.reviewers` の全員に依頼していた)
  - 設定: 個人は git config `prtui.reviewers` (カンマか空白で区切るか, `git config --add` で 1 人ずつ足す). 名前つきの組は `git config --add prtui.reviewer-set '<名前>=<login>,<login>'` で 1 組ずつ足す (`prtui.tab` と同じ書き方. 最初の `=` で分け, メンバーは `prtui.reviewers` と同じ区切り. `=` が無い値と, 名前かメンバーが空の値は捨てる). どちらも `infra/git.rs` の `read_local_state` が `--get-all` で読む. 相手は repo ごとに違うので git config で指定する (最初は alice と bobBuilder の固定だった)
  - 選択肢 (`domain/review.rs` の `reviewer_choices`): 組を設定の順に先に並べ (名前の前に `◆`, メンバーを DIM で), 次に個人を, 再依頼できるレビュー済みの人 (承認, 変更要求, コメントのみ), 設定にあって PR に居ない人 (`prtui.reviewers`, 組のメンバーの順), 依頼中の人の順に並べる. レビュー済みの人は設定に無くても出すので, コメントだけした人にも再依頼できる (ユーザーの指定). PR author は組からも除き, author しか居ない組は出さない. 個人の後ろに PR での今の状態 (依頼中 yellow, ✓ 承認 green, ✗ 変更要求 red, コメント DIM) を出す
  - 操作: ラベルの選択画面と同じ. 文字で組の名前, メンバー, login の部分一致 (大文字小文字を無視) で絞り込み, `↑`/`↓` (`Ctrl-p`/`Ctrl-n`) で移動, `Tab` で依頼する相手を選んで次に進む. 組を選ぶとメンバーのうち依頼中でない人をまとめて選び, 全員選ばれていればまとめて外す. `Enter` で選んだ相手に依頼し, 何も選んでいなければカーソルの項目に依頼する. `Esc` で閉じる. 印は 選んだ `[+]`, 組の一部だけ選んだ `[~]`, 依頼中 `[x]`, 選んでいない `[ ]`
  - 依頼中の人は選べない. 依頼を外す操作は入れていない (誤って外すのを避けるため. ユーザーの依頼にも無い). チームは依頼中としてだけ出る (`Reviewer` が slug しか持たず, 依頼に要る `org/slug` を作れないため)
  - open (Draft でない) PR に限り, Draft では `#<n> は Draft なので review 依頼しない` を出す. 選択肢が 1 つも無い (設定が無く, レビュー済みの人も居ない) ときは開かずに設定を促すエラーを出す. 開いている間に PR を取り直して依頼中になった人は送らず, 送る相手が居なければ開いたままにする
  - 反映待ちは依頼した全員が依頼中になるまで (`WatchUntil::Requested`). login は大文字小文字を区別せずに比べる (API は `bobBuilder` を返すので, 完全一致だと反映待ちが終わらず 20 秒経つまで一覧が変わらなかった)
- Linear を開く: `l` キー. PR の `headRefName` (大文字小文字を無視, 大文字に正規化), 次に `title` から issue キー (`[A-Z][A-Z0-9]*-[0-9]+`) を探し, `https://linear.app/<workspace>/issue/<KEY>` を macOS の `open` で開く. workspace は repo ごとに違うので git config `prtui.linear-workspace` (`infra/git.rs` の `read_local_state` で読む) から取り, 未設定なら設定を促すエラーを出し, フッターの `l` も出さない (2026-11-06 変更). 設定の要らない `L` は残す. キーが無ければ `#<n> に Linear の issue キーが無い` を出す
- Linear review を開く: `L` キー. review の URL (`https://linear.app/<workspace>/review/<タイトル>-<slugId>`) の slugId は Linear 側の ID で, GitHub の情報からは組み立てられない. Linear の公開 GraphQL API にも GitHub の PR 番号から引くクエリが無い (`diff(id:)` は internal で Linear の ID が要る). そこで Linear の bot (linear-code) が PR に付けるコメントから取る. `gh api --paginate repos/{repo}/issues/{n}/comments` を UI とは別スレッドで呼び, `<!-- linear-review-link -->` の直後の `href` を `?openGuide=true` などの query を外して `open` する. 同じコメントには issue 本文の引用があり, そこに関連する別 PR の review URL が並ぶので, 目印より前は見ない. bot のコメントが無い PR (例: issue に紐付いていない PR) では `#<n> に Linear review のリンクが無い` を出す
- ラベルの付け外し: `a` で選択中の PR のラベル選択画面を一覧の上にポップアップで出す (2026-11-06 追加. ユーザーの指定で自分タブだけ)
  - 選択肢: 開くたびに `gh label list --repo <owner/name> --limit 1000 --json name,color,description` を別スレッドで取り直す. 前に取った一覧があればそれを出しておき, 届いたら差し替える (取得中はタイトルに `(取得中…)`). 失敗はポップアップに赤で出し, 前の一覧で続けられる. PR に付いているラベルを付いている順に先に, 残りを名前順 (大文字小文字を無視) に並べる. repo の一覧に無いラベルも PR に付いていれば出す
  - 操作: 文字を打つと名前の部分一致 (大文字小文字を無視) で絞り込む. `j` や `q` も絞り込みの文字になるので, 移動は `↑`/`↓` か `Ctrl-n`/`Ctrl-p` (端で回る), `Backspace` で 1 文字消し, `Ctrl-u` で全部消す. `Tab` でカーソルの項目の付け外しを選んで次に進む. `Enter` で選んだ差分をまとめて反映し, 何も選んでいなければカーソルの項目だけを切り替えて反映する (fzf と同じ. 打って Enter の 1 手で付け外しできる). `Esc` で何もせず閉じる. 絞り込みで何も無いときの `Enter` は何もしない
  - 表示: 印は 付いている `[x]`, 足す `[+]` (green), 外す `[-]` (red), 付いていない `[ ]` (DIM). 名前の前に GitHub のラベルの色の `●` を 24bit 色で出す (色が無ければ DIM の `○`). 説明は DIM
  - 反映: `gh pr edit <n> --add-label <a> --add-label <b> --remove-label <c>`. gh はフラグの値をカンマで分けるので, カンマを含むラベル名は扱えない. 成功したら `#<n> のラベルを変えた: +a -c` を出し, ラベルが狙いの集合 (順番と大文字小文字は無視) になるまで `WatchUntil::Labels` で 1 件を取り直す. 失敗時は 1 回だけ取り直す
  - 詳細欄の `labels:` も名前の前にラベルの色の `●` を出す (2026-11-06 追加. ユーザーの指定). 色は `fragment_pr.graphql` の `labels { nodes { name color } }` で PR と一緒に取るので, 選択画面を開かなくても出る. `Pr::labels` は名前と色の `Label` で, 付け外しの差分と反映待ちは名前 (`Pr::label_names`) で比べる. 名前に空白を含むラベルがあるので, ラベルの間は 2 つ空ける
  - 差分は開いたときに PR に付いていたラベルとの差なので, 開いている間に他の人が付け外ししたものは巻き戻さない
- コピー: `c` で選択中の PR のコピー選択画面を一覧の上にポップアップで出し, 続く 1 キーで選んだ値を `pbcopy` でクリップボードに入れる (2026-11-02 追加. ユーザーの指定で `c` をチェックアウトから外し, チェックアウトは `Enter` だけにした). 項目は `b` ブランチ名, `n` `#番号`, `t` タイトル, `u` PR の URL, `m` Markdown リンク `[#番号 タイトル](URL)`, `i` Linear issue キー, `l` Linear issue の URL. 項目は開いたときの PR から `domain/copy.rs` の `copy_items` で作り, 値が作れない項目 (issue キーが無い) は理由を DIM で出し, 押すとエラーにする. `prtui.linear-workspace` が未設定なら `l` の項目ごと出さない (2026-11-06 変更). 項目以外のキーは何もせず閉じる. 両タブで使える
- キャッシュ: 全件取得に成功するたびに一覧を `$XDG_CACHE_HOME/prtui/<owner>__<name>.json` (未設定なら `~/.cache`) に書く. 起動時はこれを読んで即表示し, 裏で取り直す. ヘッダに `取得 Xm 前` を出し, 取り直しが終わるまで `(キャッシュ)` を付ける. `Pr` の形が変わって読めないキャッシュは捨てる. `--dump` はキャッシュを使わない
- フッターのメッセージ: 操作の結果 (チェックアウトした, 取得失敗など) はフッターにキー案内の代わりに出し, 次に何かキーを押すと消してキー案内に戻す. 押したキーが新しいメッセージを出せばそれに置き換わる
- 操作後の更新: `u`, `O`, `DD`, `XX`, `R`, `a`, `M` の後は全件を取り直さず, その PR だけを `fetch_pr` (`repository.pullRequest` に同じ fragment) で取り直して差し替える. 止める条件は `WatchUntil` (`app/event.rs`) のデータで表す. 反映を待つため 2 秒おきに最大 10 回取り直す. 止める条件は, `u` なら head oid が変わる, open / draft なら `isDraft` が変わる, close と `M` なら open でなくなる (一覧から消す), `a` ならラベルが狙いの集合になる. 待っている間は一覧のタイトルに `#n を同期中…` を出す. 差し替え後は `arrange` で並びと stack をまとめ直し, 選択中の PR は番号で追う
- PR ビュー (`p`): 選択中の PR の説明とコメントを全画面で読む (2026-11-09 追加. ユーザーの指定). どのタブの PR でも開ける. 開くたびに GraphQL (`conversation.graphql`) で取り, キャッシュはしない. 説明を先頭に, 会話のコメント, レビューの本文, 行へのスレッド (`path:line` と未解決 / 解決済 / outdated. スレッド内のコメントは字下げ) を時刻の古い順に並べる. 本文の無い COMMENTED のレビュー (行へのコメントの入れ物) と未提出 (PENDING) のレビューは出さない. 本文は markdown のまま出し, HTML コメントだけ除く (PR テンプレートと bot が入れるため). 見出しは bold, 引用とコードブロックの囲みは DIM. 書き込みは新しい側の 100 件ずつ (スレッド内は先頭 100 件) を取り, 取らなかった件数を説明の下に出す. ヘッダは背景色付きの 2 行で, スクロールしても残る. 1 行目に番号とタイトル (と Draft), 2 行目に author, `head → base` と, コメント, レビュー, スレッドの件数と未解決の数を出す (2026-11-11 に 1 行から変更. ユーザーの指定. どの PR かを常に分かるようにするため). 説明と各書き込み (コメント, レビュー, スレッド) の先頭は `── <見出し> ───…` の欄の幅いっぱいの罫線にして境目を分ける (同日. 空行だけでは区切りが分かりにくいとの指摘). スレッド内の返信は罫線にせず字下げした名前の行で分ける. ヘッダの値は開いたときの `Pr` から `PrView::new` で写す (自分タブ以外の PR は後から引けないため). `j`/`k`, `Ctrl-d`/`Ctrl-u`, `Space`/`PageUp`, `g`/`G` でスクロール, `o` でブラウザ, `r` で取り直し, `Esc`/`q` で一覧に戻る
  - レビュー (`!`): 続けて `a` で承認, `c` でコメント (他のキーで取りやめ). 本文は `${VISUAL:-${EDITOR:-vi}}` で書き (会話ビューの `e` と同じ一時ファイルの仕組み. 戻った `Event::Composed` は `PrView::composing` が Some ならレビューに使う), `gh pr review <n> --approve|--comment --body` で送る. 承認は本文が空でも送り (`--body` を付けない), コメントは空なら取りやめる. 承認のエディタの初期値は git config `prtui.approve-body` (`LGTM 👍` など. repo ごとでも global でもよい. 2026-11-09 追加. ユーザーの指定) で, 消して保存すれば本文なしで承認する. コメントの初期値は常に空. エディタを非 0 で終えたら取りやめ. 自分の PR の承認などは GitHub のエラーをそのままフッターに出す (タブで制限しない). 成功したら説明とコメントとレビュータブを取り直す. 送信中は PR ビューを閉じない. 変更要求 (request changes) はユーザーの指定に無いので入れていない
  - スレッドの解決 (`]`/`[` と `x`): 説明とコメントのページで `]`/`[` で次 / 前の行へのスレッドを選び (見出しに背景を付け, 見出しを画面の先頭に出す), `x` で解決する. 解決済みなら未解決に戻す (2026-11-12 追加. ユーザーの指定). 選んでいないときの `]` は画面の先頭以降の最初のスレッド, `[` は先頭以前の最後のスレッドを選ぶ. 本文を折り返すので, 見出しの位置は描画が 1 行ずつの高さを足して `PrView::thread_rows` に書く. GraphQL の `resolveReviewThread` / `unresolveReviewThread` を `gh api graphql` で呼び, スレッドの node ID は `conversation.graphql` で一緒に取る. 成功したら取り直さずに手元の `resolved` を書き換える (取り直すと読んでいる位置がずれるため). 自分タブの PR なら一覧の未解決の数のため 1 件を取り直す. 権限が無いなどのエラーは GitHub のものをフッターに出す. 解決中は PR ビューを閉じない. 取り消しの確認は付けていない (`x` をもう一度押せば戻せるため)
  - Linear の bot のコメント: `<!-- linear-linkback -->` の後の `<details>` には issue の本文が丸ごと入っているので, `<summary>` (issue のキーとタイトル) と `(issue の本文は省略)` だけにする (2026-11-12. ユーザーの指定. `domain/linear.rs` の `fold_linkback` を `tidy_body` で呼ぶ). 入れ子の `<details>` は外側の終わりまでを除く. 後に続く `Review in Linear` の `<p><a href=…>` は HTML のまま出る
  - diff (`d`): PR ビューの中で説明とコメントのページと diff のページを切り替える (2026-11-12 追加. ユーザーの指定). diff は初めて開いたときに `gh pr diff <n> --color never` で取り, `domain/diff.rs` の `Diff::parse` でファイルごとに分ける. キャッシュはせず, PR ビューを閉じれば捨てる. `r` は表示中のページだけを取り直す. スクロール位置はページごとに持つ. 各ファイルの先頭は `── <path>  新規|削除|バイナリ  +n -m ───…` の罫線 (rename は `old → new`) で, 行は追加を green, 削除を red, hunk の見出しを cyan, `\ No newline at end of file` を DIM にする. 折り返さない (行の位置がずれず `]`/`[` で次 / 前のファイルの見出しに飛べるようにするため). 長い行は `h`/`l` (8 桁ずつ) と `0` で横に送り, ファイルの見出しは横に送らない. タブは空白 4 つにする. 大きな diff でも重くならないよう見えている行だけを作る. `Esc`/`q` は diff のページでは説明とコメントのページに戻り, そこでもう一度押すと一覧に戻る (2026-11-13 変更. ユーザーの指定. 直前の画面に戻るようにするため). 一覧の `d` (`gh pr diff` を端末に繋いでページャで読む) はそのまま残した. GitHub の diff の上限 (300 ファイル / 2 万行) を超える PR は gh のエラーをフッターに出す
  - 説明の編集 (`e`): PR の説明をエディタ (git config `prtui.editor`, 無ければ `${VISUAL:-${EDITOR:-vi}}`) で書き換える (2026-11-11 追加. ユーザーの指定). 表示中の説明 (HTML コメントを除く前の生の本文. CRLF は LF にする) を初期値にし, 戻った `Event::Composed` は `PrView::composing` が `Composing::Body` なら説明の更新に使う. 改行コードと末尾の空白だけの違いは変更とみなさず何もしない. 空にするのは許す. 更新は `gh pr edit <n> --body-file -` に stdin で渡す. GitHub の API に楽観ロックが無いので, 直前に `gh pr view --json body` で取り直し, エディタを開いたときの説明と違えば書き換えずにエラーにする (編集中に他の人や bot が書き換えた内容を消さないため). 失敗したら書いた内容を `PrView::body_draft` に残し, 次の `e` でそれを開く (`r` で取り直してから差分を手で直す). タブでは制限せず GitHub のエラーを出す. 成功したら説明とコメントを取り直す. 更新中は PR ビューを閉じず, `!` と `e` も重ねない
- Devin session: PR に紐づく Devin session のやり取りを読み, メッセージを送る (2026-10-29 追加. 仕様はユーザーと質疑で決めた).
  - 設定は git config `prtui.devin-token` と `prtui.devin-org` (`org-...`). token は個人, org は組織ごとなので置き場所 (global / repo) は自由にした. 未設定なら session を取らず, Devin 列と詳細欄の `devin:` 行を出さない (キャッシュに session が残っていても出さない). フッターの `v` も出さないが, 押せば設定を促すエラーを出す. 設定の要らない `V` (Devin Review) は残す (2026-11-06 変更. それまでは未設定でも空の Devin 列を出していた)
  - session の特定: Devin API v3 の session 一覧 (`GET /v3/organizations/{org}/sessions`) に PR で絞る引数が無い. そこで `repo_names=<owner/name>` と `updated_after=30 日前` で取り (200 件 × 最大 5 ページ), 各 session の `pull_requests[].pr_url` を PR の URL と照合する (末尾 `/` と大文字小文字は無視). PR 本文の Devin のリンクから取る案は, 人が作った PR に後から Devin を入れた場合に拾えないので採らなかった. 1 つの PR に複数あれば `updated_at` が最新の 1 つを使う. 他人の session でも PR が一致すれば出す
  - 取得は PR の全件取得のたび (起動時, 5 分ごと, `r`) に別スレッドで行う. PR の取得 (`loading`) とは独立 (`sessions_loading`) で, 失敗しても一覧は出す. キャッシュにも入れる
  - 一覧の Devin 列と詳細欄の `devin:` 行に status を出す. new / claimed / running / resuming は yellow, suspended は cyan, exit と archived は DIM, error は red. 複数なら `×n`
  - 会話ビュー (`v`): 全画面でメッセージを古い順に並べる (Devin は magenta, 自分側は cyan). 開いている間は 5 秒おきに取り直し, 末尾を見ていれば追従する. `j`/`k`, `Ctrl-d`/`Ctrl-u`, `g`/`G` でスクロール. `i` で 1 行入力 (Enter 送信, Esc でやめる. `←`/`→` と `Home`/`End` (`C-b`/`C-f`, `C-a`/`C-e`) で入力位置を動かし, 文字はそこに挿す. `Delete` で入力位置の文字を消す. 入力位置の文字は反転で出す. 長い入力は折り返して入力欄を伸ばし, 画面の半分を超えたら入力位置の行を見せる), `e` で TUI を止めて `${VISUAL:-${EDITOR:-vi}}` を開き, 0 で終われば送信する (非 0 は取りやめ). `o` で session を Devin の web で開く. `r` ですぐ取り直す. `Esc`/`q` で一覧に戻る (送信中は無視)
  - 送信は status で制限しない (suspended は送信で再開する). API のエラー (409 / 410 など) は会話ビューのフッターに出す. 書きかけは送信が成功するまで残し, 失敗後に `i` か `e` で直せる
  - HTTP は crate を足さず `curl` を呼ぶ. token は argv に載せず `curl --config -` で stdin から渡す (`ps` に出さないため)
- レビュータブ: 自分がレビューを依頼された PR とレビューした PR を別タブに並べる (2026-10-30 追加. 仕様はユーザーと質疑で決めた)
  - 対象: open PR のうち `review-requested:@me` か `reviewed-by:@me` に当たるもの. どちらも `-author:@me` を付けて自分の PR を除く. `review-requested` はチーム宛ての依頼も拾う (`user-review-requested` にしなかったのはユーザーの選択). `reviewed-by` はコメントだけ残した PR も拾う. Draft も出す
  - 取得: `QUERY_REVIEW` で 2 つの alias と `viewer { login }` を 1 リクエストで取り, 依頼中側を先に見て番号で重複を除く. 自分タブとは別スレッドで, 取るのは同じタイミング (起動時, 5 分ごと, `r`). 失敗は一覧のタイトルに赤で出し, フッターには出さない. キャッシュは同じファイルの `reviews` に取得時刻と一緒に入れる
  - 切り替え: `Tab` / `Shift-Tab` と `1` / `2`. ヘッダに `1 自分 (n)  2 レビュー (m)` を出し, 今のタブを反転する. 選択位置と `t` の絞り込みはタブごとに持つ
  - 自分の立場 (`MyReview`): 依頼中の検索に当たれば requested. `latestReviews` から自分 (大文字小文字を無視) の最新のレビューを取る. 依頼中かつレビュー済みなら「再依頼」. `reviewRequests` に自分の login が無ければチーム経由とみなし, PR に依頼されているチームのうち自分の所属チームの slug を詳細欄に `依頼: <team> 経由` で出す. 所属チームは `QUERY_REVIEW` で `repositoryOwner(login:<owner>)` が Organization のとき `teams(role:MEMBER)` と `teams(role:ADMIN)` を取る (maintainer が MEMBER に入るか不明なので両方). 1 つも当たらなければ (owner が User, 子チーム経由で親チームに依頼された, など) 依頼されている全チームに戻す. `read:org` の無いトークンでこのクエリが失敗するかは確かめていない
  - 並び: 依頼中 (再依頼を含む) を先に, その中は依頼を受けた時刻の古い順 (古い依頼から片付けるため. ユーザーの指定). 依頼時刻が分からなければ更新日時で代える. レビュー済みはその後ろに更新日時の降順. stack はまとめない
  - 依頼時刻: `reviewRequests` に時刻が無いので, 依頼中側の alias だけ `fragment_requested.graphql` で `timelineItems(last:50, itemTypes:[REVIEW_REQUESTED_EVENT])` を取る. 直接の依頼なら自分宛ての最新の event (再依頼なら最後の依頼) を `MyReview::requested_at` にする. チーム経由なら, 今依頼されているチームごとの最新の event のうち最も古いものを使う. 所属チームで絞れず全チームに戻ったときに, 後から関係ないチームが依頼されても待ち時間を短く見せないため (サブエージェントのレビューの指摘). 依頼時刻が同じなら番号の昇順. 一覧の「依頼」列に経過時間を yellow で出す
  - 操作: `Enter`, `c`, `d`, `o`, `l`, `L`, `v`, `t`, `r` は使える. 他人の PR を変える `u` / `O` / `DD` / `XX` / `R` / `a` / `M` は無効にし, `<key> はレビュータブでは使えない` を出す (ユーザーの指定)
  - Devin Review: `V` で `https://app.devin.ai/review/<owner>/<name>/pull/<n>` を `open` で開く. repo と PR 番号だけで組み立てるので, Devin の設定 (token, org) も API も要らない. ユーザーの指定でレビュータブだけにした. 自分タブでは何もしない. 追加のタブはレビュータブに合わせて使えるようにした
- 追加のタブ: git config `prtui.tab` に書いた検索クエリで, 自分タブとレビュータブの後ろにタブを足す (2026-11-05 追加. 仕様はユーザーと質疑で決めた. 例はラベル `マージ予定` の open PR の一覧)
  - 設定: `git config --add prtui.tab '<名前>=<検索クエリ>'` で 1 タブずつ足す (`prtui.reviewers` と同じ複数値のキー. 書き方はユーザーの指定で git config). 最初の `=` で名前とクエリに分ける. `=` が無い値と, 名前かクエリが空の値は捨てる. タブは `--get-all` の順 (書いた順) に並べる. タブの並びは起動時の設定で決め, 実行中に設定を変えても再起動まで反映しない. タブごとのセクション (`[prtui "tab.<名前>"]`) にする案は, 今の用途では書く量が増えるだけなので採らなかった
  - 対象: `repo:<owner/name> is:pr is:open <クエリ>` の search (`QUERY_CUSTOM`, `first:100`). 1 件の取り直しと操作が open を前提にしているので `is:open` は外せない. 自分の PR も除かない. GitHub の search は解釈できないクエリでもエラーにせず 0 件を返す
  - 取得: タブごとに別スレッドで, 自分タブと同じタイミング (起動時, 5 分ごと, `r`). 失敗はそのタブの一覧のタイトルに赤で出す. キャッシュは同じファイルの `custom` にクエリと取得時刻と一緒に入れ, 起動時はクエリが今の設定と一致するタブだけ戻す
  - 並び: 更新日時の降順 (`arrange_flat`). 他人の PR が混ざるので stack はまとめない
  - 切り替え: `Tab` / `Shift-Tab` で順に回し (端で反対の端に回る), `1`〜`9` で直接選ぶ. 10 個目以降は `Tab` でだけ選べる. タブ欄はヘッダの下の独立した行に出す (ヘッダの 1 行目に並べるとブランチ名が長いときに 3 つ目以降が画面外に切れたため)
  - 操作: レビュータブと同じ (ユーザーの指定). `u` / `O` / `DD` / `XX` / `R` / `a` / `M` は無効にし `<key> は<名前>タブでは使えない` を出す. `V` (Devin Review) は使える
  - 列: `# 状態 CI 未解決 main 差分 Reviewer Author Assignee Local Devin 更新 タイトル`. マージの判断に使うことが多いので main を出し, 自分と他人の PR が混ざるので Author と Assignee を出す (Assignee はユーザーの指定). Assignee は先頭の 1 人と `+n` (DIM). Stack は stack をまとめないので, レビュータブの自分と依頼はこのクエリで取らないので出さない. ラベルは絞り込みに使うことが多く重複するので一覧には出さない (詳細欄の `labels:` にはある). Review 列は無い (下の「一覧の列」を参照)
- 実装: Rust (edition 2024), ratatui 0.30 (crossterm は `ratatui::crossterm` の re-export を使う), serde, anyhow, chrono. GitHub と git は `gh` と `git` コマンドを `std::process::Command` で呼ぶ. 非同期 runtime は使わず, 時間のかかる処理はスレッド + mpsc で UI をブロックしない. dev-dependencies は insta (描画と `--dump` の snapshot), pretty_assertions, tempfile, apollo-compiler (クエリを schema に照らして検証)

## 構成

層は domain → infra → app → ui / dump → main の順に積み, 下に向かってのみ依存する. 依存規則と新機能を足すときに触るファイルは `CLAUDE.md` の「構造」節にある.

| ファイル | 役割 |
|---|---|
| `src/main.rs` | 引数処理 (`-C`, `--dump`), 端末初期化, イベントループ, 外部コマンド実行時の TUI 一時停止 |
| `src/dump.rs` | `--dump` のテキスト出力. 取得は `run`, 文字列の組み立ては純粋関数 (`header`, `mine`, `review`, `custom`) で, snapshot は `src/snapshots/` |
| `src/domain/` | 純粋な型と関数. `conversation.rs` (PR ビューの `Conversation`, `Entry`, `Verdict`, `ReviewAction`, 本文の整形 `tidy_body`, 説明の編集の `editable_body` と `same_body`), `diff.rs` (PR ビューの diff `Diff`, 描画する行の並び `rows`, ファイルの移動 `jump_file`), `pr.rs` (`Pr`, `MyReview`), `stack.rs` (`arrange`, `arrange_flat`, `group_stacks`), `review.rs` (`review_targets`, `reviewer_choices`, `arrange_reviews`), `linear.rs` (issue キーと review URL の抽出), `copy.rs` (`copy_items`), `local.rs` (`LocalState`, `parse_reviewers`, `ReviewerSet`, `parse_reviewer_sets`, `TabQuery`, `parse_tabs`), `op.rs` (`PrOp`), `label.rs` (`Label`, `label_choices`, `label_diff`), `devin.rs` (Devin の型と `session_for`, `review_url`), `cache.rs` (キャッシュの型 `Cache`, `CustomCache`) |
| `src/infra/process.rs` | コマンド実行ヘルパ `run`, `open_url` |
| `src/infra/git.rs` | メイン worktree の解決 `main_worktree`, `read_local_state` (現在ブランチ, dirty, refs/heads の oid, git config) |
| `src/infra/gh.rs` | gh コマンド. 全件取得 `fetch_prs`, レビュータブの `fetch_reviews`, 追加のタブの `fetch_custom`, 1 件取得 `fetch_pr`, PR ビューの `fetch_conversation`, `fetch_diff`, `submit_review`, `edit_body`, `update_branch`, `checkout`, `change_state`, `request_reviews`, `fetch_labels`, `edit_labels`, `view_web`, `linear_review_url`. 引数の組み立ては `*_args` の純粋関数にしてテストする |
| `src/infra/graphql/` | `*.graphql` (クエリと fragment の本文), `schema.graphql` (GitHub の公開 schema. `task schema` で取り直す), `query.rs` (`.graphql` を `include_str!` して送る文書を組み立てる. schema に照らす検証テストもここ), `raw.rs` (`Raw*` 型), `mod.rs` (`Pr` への変換と JSON の解析 `parse_search` / `parse_review` / `parse_custom` / `parse_one`) |
| `src/infra/devin.rs` | Devin API v3 (`fetch_sessions`, `fetch_messages`, `send_message`) を curl で呼ぶ. JSON の解析 `parse_*` |
| `src/infra/cache.rs` | PR 一覧と Devin session 一覧のディスクキャッシュの置き場所と読み書き |
| `src/app/state.rs` | 全状態 `AppState`, タブ `Tab` とタブごとの一覧 `PrList`, 追加のタブ `CustomTab`, 一覧の上に開く画面 `Screen` とその積み `screens`, 会話ビューの `ChatView`, ラベル選択画面の `LabelMenu`, review 依頼の選択画面の `ReviewerMenu`, マージの確認画面の `MergeMenu`, PR ビューの `PrView` |
| `src/app/event.rs` | `Event` (スレッドの結果), `Effect` (副作用の要求), `Action` (main への指示), `WatchUntil` |
| `src/app/update/` | キー操作, イベント処理, 時間の遷移 (`tick`). `AppState` を更新して `Vec<Effect>` を返す純粋関数. `mod.rs` (`refresh`, `tick`, `handle_key` と一覧の操作), `event.rs` (`handle_event`), `chat.rs` (会話ビューのキー), `pr_view.rs` (PR ビューのキー, ページの切り替え, レビューの送信, 説明の編集), `label.rs` (ラベル選択画面のキー), `reviewer.rs` (review 依頼の選択画面のキー), `picker.rs` (2 つの選択画面で共通の絞り込みとカーソル移動) |
| `src/app/ports.rs` | runtime が外とやりとりする trait (`Github`, `Git`, `Devin`, `CacheStore`, `Desktop`) とその束 `Ports`, 待ち時間の `RuntimeConfig`. テスト用の fake もここ |
| `src/app/adapters.rs` | ports の実装. infra の関数に委譲する. `real_ports` |
| `src/app/runtime.rs` | `App`. Effect を実行し, スレッドの起動と ports の呼び出しをここに閉じる |
| `src/ui/` | 描画. `mod.rs` (ヘッダ, フッタ, 配色), `table.rs` (一覧. 列はタブごとの `Column` の表), `detail.rs` (詳細欄), `chat.rs` (会話ビュー), `pr_view.rs` (PR ビュー), `copy.rs`, `label.rs`, `reviewer.rs` (ポップアップ. 配置は `mod.rs` の `popup_area`), `format.rs`. snapshot は `src/ui/snapshots/` |

### レイヤー化した理由

refactor 前は 1 つの `App` に状態, キー判断, スレッド起動, gh 呼び出しが混ざっていて, キー操作の振る舞いを gh を呼ばずに確かめる手段が無かった. そこで判断 (update) と実行 (runtime) を分け, update は「何をするか」を `Effect` のデータとして返すだけにした. gh / git の差し替え境界は trait ではなくこの `Effect` にした. キー操作とイベント処理のテストは `AppState` と返った `Vec<Effect>` の assert で書け, gh の実装が 1 つしか無いうちに trait を作る必要が無いため. runtime は薄く保って `--dump` と tmux で手動確認する. watch の終了条件はクロージャだと `Effect` が `Debug` / `PartialEq` を持てず assert できないので, `WatchUntil` の enum にした. `AppState` は ratatui の `TableState` を持つ (選択位置とスクロール offset を毎フレーム作り直すと振る舞いが変わるため). `KeyEvent` もそのまま受け, 独自の入力型は作らない. 新旧で `--dump` の出力と TUI の画面が一致することを確かめてから merge した.

### runtime の DI (2026-11-05)

runtime にも checkout の dirty チェック, watch の打ち切り, キャッシュの読み戻し (追加のタブはクエリが一致するものだけ) といった分岐が増え, 手動確認だけでは足りなくなったので, 外部呼び出しを trait 越しにした.

- trait は使う側の `app/ports.rs` に置き, domain にだけ依存させる. 実装は `app/adapters.rs` で infra に委譲する. infra に `impl` を書くと infra が app に依存するため
- 粒度はサービス単位 (`Github::fetch_prs` など) にし, `process::run` の単位では差し替えない. argv と JSON を丸ごと偽装すると gh の CLI 仕様に結合するため. argv は `gh.rs` の `*_args` を単体テストで見る
- dir と repo は adapter が持ち, trait の引数から外す. fake が簡単になる
- `Arc<dyn Trait>` (supertrait `Send + Sync + 'static`) の束 `Ports` を `Clone` してスレッドに渡す. 実装は本物と fake の 2 つだけで, `App` をジェネリックにすると main と ui に型が波及するため
- `Clock` / `Spawner` の trait と mockall は入れない. Effect 1 つが Event 1 つを返すので, テストは `pump_blocking(n)` で決定的に待てる. watch の sleep は `RuntimeConfig::watch_interval` を 0 にして消す
- `Cache` 型は serde の純データなので `domain/cache.rs` に移した. ports が infra に依存しないため
- `--dump` は TUI を通らないので infra を直接呼ぶまま
- ureq で curl を置き換える案は, エラー文言や TLS 依存が変わる設計変更なので見送った. やるなら `Devin` の adapter の差し替えで済む

### 画面のスタック (2026-11-13)

一覧の上に開く画面 (PR ビュー, 会話ビュー, コピー, ラベル, review 依頼, マージの確認) を, 画面ごとの `Option` から `AppState::screens: Vec<Screen>` の積みにした (ユーザーの指定). `Option` の並びではどれを前に出すかが `handle_key` と `draw` の `if` の順で暗に決まり, `Esc` も画面ごとに「閉じる」と書いていたので, PR ビューの diff のページで `Esc` を押すと説明とコメントのページを飛ばして一覧に戻っていた. 今後 PR ビューから会話ビューやマージの確認画面を開くような, 画面の上に別の画面を開く操作を足す前に寄せておいた.

- 積むのは「どの画面の上に開いたか」で, 通った画面の履歴ではない. `d` のような画面の中のページの切り替えは積まず, 画面が自分の状態 (`PrView::page`) として持つ. 履歴にすると `d` を押すたびに積まれ, `Esc` で同じ 2 ページを行き来するため
- `Esc` (と PR ビューと会話ビューの `q`) は, 画面の中に戻る先があればそこへ (diff のページ → 説明とコメントのページ, 1 行入力 → 会話ビュー), 無ければ `pop` で下の画面に戻る. 一覧の `Esc` は何もしない (以前は終了だった. 戻りすぎてアプリが終わらないよう `q` だけで終える)
- キーは一番上の画面に渡す (`AppState::top`). 一覧のフッターの確認 (`u` の y/N, `DD`, `XX`, checkout の `--force`) は一覧に属する一時的な問いなので積まず, 今までどおりポップアップより先に見る. 積みに全画面の画面があれば (その上のポップアップでも) 見ない. フッターのメッセージも同じ
- 描画は一番上の全画面の画面 (`Screen::is_full`. 無ければ一覧) を描き, その上に積んだポップアップを順に重ねる
- 種類ごとの `pr_view()` / `pr_view_mut()` は積みの中で一番上にあるものを引く. `pop_pr_view()` などは一番上がその種類のときだけ閉じる (積みの途中を抜くと, その上に開いた画面が浮くため). `push` は中身の型 (`PrView` など) をそのまま受ける. どれも `state.rs` の `screen_accessors!` で作る. スレッドから届く結果は `pr_view_for(n)` / `chat_for(id)` で積み全体から番号か session ID で引き, 一番上でなくても反映する. 下に隠れた PR ビューの取得結果を失わないため

## 一覧の列

- Stack: stacked PR の中の位置 `段/段数` (cyan). main に近い側が 1. タイトル列は 2 段目以降を `└` で字下げする. 詳細欄の branch 行に `head ← base` を出す
- 状態: Conflict / Draft / Ready の順に判定. Conflict のときは Draft かどうかがこの列では分からない
- CI: 最新コミットの `statusCheckRollup`. CheckRun は status が COMPLETED 以外なら実行中, それ以外は conclusion (SUCCESS と NEUTRAL を成功, SKIPPED を除外, 他を失敗). StatusContext は state
- 未解決: 未解決の review thread 数 (`reviewThreads(first:100)`)
- main: main にあって PR ブランチに無いコミット数. `headRef { compare(headRef:"main") { aheadBy } }` で取る. base は main 固定 (検証用 repo は trunk-based で base が常に main のため)
- 差分: 追加行 `+x` (green) と削除行 `-y` (red). `additions` / `deletions`
- Review 列は外した (2026-11-05. ユーザーの指定). `reviewDecision` は GitHub がブランチ保護ルールから計算する値で, 必須レビューのルールが無い repo (検証用 repo など) では常に `-` になり判断に使えないため. 承認の状況は Reviewer 列で見る. 値は `Pr::review_decision` に取ったままで, `--dump` の `review=` に出す
- Reviewer: 承認 ✓n (green), 変更要求 ✗n (red), 依頼中 ●n (yellow), コメントのみ ✎n (DIM) の人数. `reviewRequests` (依頼中) と `latestReviews` (各人の最新レビュー) から作る. 承認後に再依頼された人は依頼中として数える. author 自身, Bot (`latestReviews` の author の `__typename` が `Bot`. devin-ai-integration など), DISMISSED, PENDING は除く. Bot は GitHub の Reviewers 欄に出ないため. コメントのみの人は ✎n で別に数え, 承認・変更要求・依頼中が居なければ「未設定」を前に付ける (例 `未設定 ✎2`).「未設定」は Draft なら DIM, それ以外は yellow. 詳細欄の `reviewers:` 行に名前と状態を全員分出す. Review 列 (`reviewDecision`) が `-` の repo でも誰に頼んでいて誰が承認したかが分かるように足した
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
- 2026-11-05: 追加のタブ (`prtui.tab`) を足した. clippy 0 件, fmt 通過, `cargo test` 121 件通過. 検証用 repo の設定は書き換えず `GIT_CONFIG_COUNT` の環境変数で `マージ予定=label:マージ予定` を渡し, `--dump` の `# マージ予定` 節に該当の 1 件が出ること, tmux でタブ欄, 列, `3` と `Tab` での切り替え, `u` のエラー表示, 再起動後にキャッシュから件数が出ることを確認
- 2026-11-02: 一覧の `j`/`k`/矢印の移動を, 端を越えたら反対の端に回るようにした (ユーザーの指定). 会話ビューのスクロールは回さない
- 2026-11-05: モジュール化, テスト性, DI の refactor をした. fable に計画を相談し, runtime の DI, ui と dump, update の分割, gh と graphql の 4 本を opus subagent に worktree で並行させて cherry-pick した. clippy 0 件, fmt 通過, `cargo test` 154 件通過 (121 件から runtime, gh の argv, 描画と `--dump` の snapshot を足した). 検証用 repo で `--dump` の出力 (経過時間の 1 分差を除く) と, tmux の自分タブとレビュータブの画面 (エスケープコード込み) が refactor 前のビルドと一致することを確認
- 2026-11-05: GraphQL のクエリを `.graphql` ファイルに移し, GitHub の公開 schema (`https://docs.github.com/public/fpt/schema.docs.graphql`) に照らして `cargo test` で検証するようにした (ユーザーの指定). 応答の型は手書きの `Raw*` のままにし, graphql_client による型生成は変換の書き直しとビルド時間の増加に見合わないので見送った. フィールド名を誤らせると位置付きのエラーで落ちることを確認. 検証用 repo で `--dump` が変更前のビルドと一致することを確認 (1 回目は `mergeable` の計算待ちで Ready / Conflict が揺れたが, 取り直すと一致した)
- 2026-11-06: `a` (ラベルの付け外し) を足した. clippy 0 件, fmt 通過, `cargo test` 180 件通過. 検証用 repo の tmux で選択画面の表示 (付いているものが先, ラベルの色, 説明), 英字と日本語での絞り込み, `Tab` で `[+]` が付くこと, `Ctrl-u`, `Esc` で何もせず閉じることを確認. 詳細欄の `●` が GitHub のラベルの色 (refactoring の `#ED3FE9` など) で出ることを tmux のエスケープコードで確認 (`cargo test` 181 件). 実際の `gh pr edit` は共有 repo の PR を変えるので実行していない
- 2026-11-06: Devin と Linear の設定が無いとき, 空になるだけの Devin 列, 詳細欄の `devin:` 行, フッターの `v` と `l`, `c` の Linear issue の URL を出さないようにした (ユーザーの指定). `cargo test` 182 件通過. 未設定の画面は snapshot `without_devin_and_linear` で固定した

- 2026-11-06: `RR` を review 依頼の選択画面 (`R`) にし, 名前つきの組 (`prtui.reviewer-set`) と, コメントだけした人への再依頼を足した (ユーザーの指定). `PrOp::Review` を外して `Effect::RequestReviews` / `Event::RequestedReviews` に分けた. clippy 0 件, fmt 通過, `cargo test` 202 件通過. 描画は snapshot `reviewer_menu` で固定した
- 2026-11-13: 一覧の上の画面を `screens` の積みにした. clippy 0 件, fmt 通過, `cargo test` 268 件通過 (snapshot は変更なし). 仕上げに subagent に差分を評価させ, `take_*` を一番上だけ閉じる `pop_*` に, フッターの案内を一番上の画面の `match` に, イベントの画面の引き方を `pr_view_for` / `chat_for` に揃え, 全画面の画面の上のポップアップでは一覧の確認を見ないようにした. 検証用 repo の tmux で `p` → `d` → `Esc` で説明とコメントのページ, もう一度 `Esc` で一覧, 一覧の `Esc` で終わらないこと, `a` のラベル選択画面が一覧の上に出て `Esc` で閉じること, `q` で終わることを確認

## 未確認

- `M` で実際にマージすること. 自分の PR がマージされるので試していない. 非同期のマージ API の argv と応答の解析は単体テストで, キーから Effect と反映待ちまでは update と runtime のテストでだけ確かめた. `GET .../merge-async/{uuid}` は版ヘッダの有無どちらでも path が通る (存在しない uuid で 404 と async merge の `documentation_url` が返る) ことを omni-hub で確かめた. `PUT` は試していない. merge queue のある repo で `merge_method` を送ると断られないか (API の説明では `merge_method` は直接マージだけに効く), stack の途中の PR をマージしたときの一覧の更新は未確認. `gh repo view --json` のマージ設定の 4 つのフィールドは cli/cli で実際に返ることを確かめた

- `R` の選択画面から実際に依頼し, 再依頼で Reviewer 列が依頼中に戻ること. `gh pr edit --add-reviewer` は前の `RR` と同じ argv で, Effect から反映待ちまでは fake でだけ確かめた

- `a` の `Enter` で実際にラベルが付け外しされ, `WatchUntil::Labels` の反映待ちが止まること. `gh pr edit` の argv は単体テストで, Effect から反映待ちまでは fake でだけ確かめた

- レビュータブの「再依頼」, 「変更要求」, チーム経由の表示. 検証用 repo に該当する PR が無かったので, `parse_review` の単体テストでだけ確かめた

- Devin API を正しい token で叩くこと全般. 特に次の 4 点
  - 時刻の単位 (秒と仮定し, 1e12 を超えれば ms とみなしている)
  - `repo_names` が 1 値の query で効くか
  - POST messages が SessionResponse を返すか
  - `pull_requests[].pr_url` が GitHub の PR URL と同じ形か
- 会話ビューの実画面 (スクロール, 入力, エディタからの復帰と一時ファイルの削除)
- PR ビューの `e` で実際に説明を書き換えること. argv と判定は単体テストで, キーから Effect と結果の反映までは update と runtime のテストでだけ確かめた
- PR ビューの `x` で実際にスレッドを解決すること. 他人に見える状態が変わるので試していない. mutation は schema に照らした検証と argv の単体テストで, キーから結果の反映までは update と runtime のテストでだけ確かめた. `]` での選択と見出しへのスクロールは feedforce/omni-hub の実 PR で tmux で確かめた
- PR ビューの diff は feedforce/omni-hub の実 PR (4 ファイル) で tmux で表示, `]`/`[`, `d` での切り替えを確かめた. 上限を超える大きな diff と, rename やバイナリを含む実 PR では試していない (解析は単体テストでだけ確かめた)
- PR ビューの `!` で実際にレビューを送ること. 他人の PR に承認やコメントが残るので試していない. `gh pr review` の argv は単体テストで, キーから Effect と結果の反映までは update のテストでだけ確かめた. 表示 (取得, スクロール, `!` の取りやめ) は feedforce/omni-hub の実 PR で tmux で確かめた


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
- 自分タブで操作した PR の取り直し (`Reloaded`) は自分タブだけを差し替える. 同じ PR が追加のタブやレビュータブにも出ていれば, 次の全件取得まで古いまま
- PR ビューは markdown を描画しない (表, リンク, `<details>` などの HTML はそのまま出る). bot のコメントは HTML が多く読みにくい
- PR ビューの書き込みは種類ごとに新しい側の 100 件まで. 超えた分は件数だけ出すので `o` でブラウザで見る
- 詳細欄は高さ 10 で固定. CI の失敗と実行中の行と devin 行が全部出ると URL の行が欄からはみ出ることがある
