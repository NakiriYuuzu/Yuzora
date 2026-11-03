# L2 · Git

beta.2 的 Git 以 JetBrains IntelliJ IDEA Community 的 Git 整合為功能基準（研究：[research/intellij-git-research.md](research/intellij-git-research.md)），採 **staging area 模式**：沒有 changelists、沒有 shelf，也不做 GitHub／GitLab 整合。持久化 log index 不在 beta.2 範圍，但留了接縫（見〈Log〉）。

- 原生層：`app/crates/yuzora-git`（JS `import … from "yuzora:git"`），全部 git 工作都在這裡，不在 UI 執行緒。
- JS mods：`app/defaults/mods/yuzora-sidebar-git`（側欄 Git 模式）與 `yuzora-git-*`（tool pane、popup、對話框、設定頁）。

## 架構

```
yuzora-sidebar-git ─┐                        ┌─ exec.rs   git 執行器（環境、鎖、取消、逾時、遠端）
yuzora-git-diff     │                        ├─ repo.rs   開啟的 repo、generation、.git 狀態
yuzora-git-log      │   yuzora:git           ├─ status.rs＋watch.rs   dirty scope、檔案監看
yuzora-git-merge    ├── (HostModule) ────────┼─ diff.rs＋diffview.rs  Myers／Patience、diff pane、部分暫存
yuzora-git-rebase   │   async 函式＋GraphCell ├─ log.rs＋graph.rs＋graph_view.rs  兩階段 log、分支圖
yuzora-git-branches │                        ├─ ops.rs    commit、分支、遠端、stash、歷史、tag、patch…
yuzora-git-push     │                        ├─ merge.rs  三方衝突    rebase.rs  互動式 rebase
yuzora-git-repo     │                        └─ settings.rs  Yuzora settings.json 的 `git` 區段
yuzora-git-blame    │
yuzora-git-settings ┘
```

### mods

| mod | 種類 | 內容 |
| --- | --- | --- |
| `yuzora-sidebar-git` | 側欄模式 | 分支按鈕（↑↓，開分支 popup）、操作中橫幅（繼續／中止／略過）、衝突／已暫存／變更三段清單（數量、全部暫存／取消、列尾 +／−／↺）、右鍵選單、commit 訊息（歷史、範本、amend、sign-off、略過 hooks、作者）、Commit／Commit 並 Push／Push ↑N、「分支圖」。寫 `yuzora.badge.git`、`yuzora.status.git` |
| `yuzora-git-branches` | popup | 搜尋、更新專案、Fetch、Push…、新分支、checkout revision、stash；最近／本地／遠端／tags；每個 ref 的動作（checkout、smart／強制 checkout、從此建分支、比較、merge／rebase 進目前分支、改名、upstream、刪除、tag push／刪除） |
| `yuzora-git-push` | 對話框 | 目標 remote／分支、要送出的 commits、force（lease／force，受保護分支禁止）、tags（none／follow／all）、略過 hooks、新分支自動 set-upstream、被拒時依設定自動更新 |
| `yuzora-git-diff` | tool pane | 並排／合併、字詞高亮、空白（精確／忽略頭尾／全部忽略）、context（3 行／整檔）、hunk 導覽、暫存／取消暫存／捨棄 hunk 或選取的行、Annotate、歷史；即時更新 |
| `yuzora-git-log` | tool pane | 提交紀錄＋分支圖（`GraphCell`）、篩選、詳情、右鍵選單、分支比較、檔案歷史 |
| `yuzora-git-merge` | tool pane | 衝突檔案清單、整檔接受 yours／theirs、三方合併（套用非衝突、自動解決簡單衝突、逐塊選擇或手動編輯） |
| `yuzora-git-rebase` | tool pane | 互動式 rebase 編輯器（pick／reword／edit／squash／fixup／drop、拖曳排序、update-ref）、記憶體內改寫訊息 |
| `yuzora-git-blame` | tool pane | Annotate：gutter、上一版 annotate、與上一版比較、在 log 中顯示 |
| `yuzora-git-repo` | tool pane | Stashes、Tags、Remotes、Worktrees（新增交給上游 `worktree` 對話框）、Submodules、Patches、身分與簽章 |
| `yuzora-git-settings` | 設定頁 | Yuzora `settings.json` 的 `git` 區段（見 A12） |

### pane 之間的約定

- 每個 mod 用 `open(cwd, on)` 取得 repo（同一個 `<machine>:<root>` 只開一次，watcher 也只有一個），之後的函式都帶 `repo.id`。
- 單一實例的 pane 用 shared key 交棒：`yuzora.git.diff`（`{cwd, on, path, orig, mode, rev?, to?, at}`）、`yuzora.git.merge`、`yuzora.git.rebase`。開啟方先 `setShared`，已有該 pane 就 `pane.focus`，否則 `tab.new {mod, props}`；pane 依 `at` 取較新的請求。log、blame、repo pane 用 props，可以同時開多個。
- 即時更新：`changed(repo, since)` 在 repo 的 generation 變了（watcher 或我們自己的寫入）時 resolve；遠端 repo 沒有 watcher，約 25 秒逾時一次，呼叫端藉此輪詢。
- 跨 lane：`yuzora.badge.git`＝變更檔案數（乾淨時 `null`），`yuzora.status.git`＝`{text: "⎇ main ↑2 · 4 變更", tone}`，tone：衝突 `blocked`、操作中或執行中 `working`、有變更 `accent`、乾淨 `muted`。

### virtual list 的限制

gpui-shell 不允許在 `v_virtual_list` 的 item renderer 裡註冊事件（會丟例外、整個清單不畫）。列的點擊走清單的 `on_item_click(key)`／`on_item_secondary_click(key, event)`；一列裡有多個可點區域（側欄的 +／−／↺、diff 的行號）時，清單外層用 `on_mouse_down` 記下按下的 x，再在 `on_item_click` 判斷。

## 引擎

### 執行器（`exec.rs`）

- 只用 git CLI。每個指令都帶 `-c core.quotepath=false -c log.showSignature=false -c color.ui=false`；讀取另外帶 `GIT_OPTIONAL_LOCKS=0` 與 `-c diff.autoRefreshIndex=false`，不碰 `.git/index`，watcher 才不會被自己的讀取喚醒。
- 環境：`LANGUAGE=` 清空、`LC_ALL=en_US.UTF-8`（macOS 本機）或 `C.UTF-8`，`GIT_TERMINAL_PROMPT=0`、`GIT_EDITOR=:`、`GIT_MERGE_AUTOEDIT=no`，輸出才能穩定解析，也不會卡在互動輸入。
- 讀取一律帶 `-c core.fsmonitor=false`：側欄一聚焦資料夾就自動跑 status，repo 自己的 `.git/config` 不能藉 fsmonitor hook 在使用者沒有任何動作時執行程式（`status_never_runs_the_repos_fsmonitor_program` 守住）。代價是設定了 fsmonitor 的大 repo，背景 status 會慢一些。
- 其他由 repo 設定指定程式的讀取：commit 詳情的簽章檢查（`%G?`）把 repo 本地設定的 `gpg.program`／`gpg.{openpgp,ssh,x509}.program` 換回使用者全域／系統的值或 git 預設（`signature_programs`）；blame 帶 `--no-textconv`；status 的 numstat 帶 `--no-ext-diff --no-textconv`，建立 patch 的 diff 帶 `--no-ext-diff`。diff 讀 blob 用 `cat-file --filters`，會照 repo 設定跑 smudge filter（例如 git-lfs），只在使用者打開 diff 時發生，與 checkout 相同。
- 信任資料夾（`trust.rs`）：repo 自己的設定（`.git/config`、它 include 的檔案、worktree 設定）若指定了 git 會執行的程式（`filter.*.clean/smudge/process`、`core.sshCommand`／`gitProxy`／`askPass`／`alternateRefsCommand`、指向程式的 `core.fsmonitor`（`true` 是 git 自己的 daemon，不算）、credential helper、`remote.*.uploadpack/receivepack/vcs`、`protocol.*.allow`、`merge.*.driver`、`diff.external`、`diff.*.command/textconv`、`gpg.*program`、`gpg.ssh.defaultKeyCommand`、`!` 開頭的 `submodule.*.update`），`status` 不跑 git，只回傳已知內容和 `trust`（那些 key）；側欄顯示「要信任這個資料夾嗎？」，按下後記在 `<app home>/yuzora/git/trusted.json`。自動 fetch 由引擎擋（`fetch {auto: true}` 在未信任時直接拒絕），而且一律不跑 hook（`core.hooksPath` 指向 app 自己的空目錄；fetch 會觸發 reference-transaction）、不遞迴 submodule（`--no-recurse-submodules`）。信任檢查也看 index 裡每個已 checkout 的 gitlink（巢狀的也算）自己的設定，不管 `.gitmodules` 有沒有列出它，也不管上層設定有沒有程式：git 的 status、diff 會遞迴進這些 submodule，用它們的設定跑 filter。判斷「已 checkout」照 git 自己的條件（資料夾裡有 `.git`），進去後只讀它自己（`GIT_DIR=.git`）；路徑被 git 加了引號（含 `"`、`\`、控制字元）時無法追蹤，當成讀不到。掃描以命令列 alias 讓 git 在頂層執行，本機與遠端都只多一次呼叫。橫幅以 `路徑: key` 列出。設定讀不到、或回答超過 8 MiB（`git_trust::ANSWER_CAP`，截斷的回答可能剛好少了某個程式 key）時，當成未信任。hook 本身不列入信任條件：status 不跑 hook，而 husky 等工具會在許多 repo 設定 `core.hooksPath`。使用者全域／系統設定的程式（例如全域安裝的 git-lfs）不需要信任。未信任時 `yuzora:git` 對這個 repo 的其他呼叫（log、diff、merge、stage、commit、fetch…）一律在引擎拒絕（`trust::check`，錯誤以 `untrusted folder` 開頭），各 pane 顯示這個錯誤，等同 IntelliJ 的 Safe mode；只有 `status`（顯示提示）與 `trust`（回答）例外。不經過 `repo_fn` 的路徑各自在跑 git 之前檢查：log 的載入（開啟、`logRefresh`、跟著 refs 更新）與補抓 metadata，以及 diff／merge session 的寫入（`diffStage`／`diffUnstage`／`diffDiscard`、`conflictResolve`；session 開啟後 repo 設定可能多了程式，例如 `git lfs install --local`）。partial clone 讀到缺少的物件時，會用 repo 的 ssh 指令去抓，所以 log 也不能在信任前讀。
- 共用的信任 API（`yuzora_core::git_trust`）：上面的偵測（`PROGRAMS`、`names_program`、submodule 掃描）與信任清單放在 yuzora-core，給狀態列（L1）、檔案樹（L5）等也會自己跑 git 的地方共用；yuzora-git 用自己的執行器呼叫它，行為不變。
  - `pending(machine, top)`：回傳空清單才可以跑 `status`、`diff` 這類會比對工作樹或 index 的指令；`Err`（git 跑不起來）也當成不安全。`top` 是那台機器上 `git rev-parse --show-toplevel` 的輸出，遠端走 `hc exec --on`。
  - `is_trusted(machine, top)`：只讀信任清單，不跑 git。
  - 未信任時仍可以執行的指令（前面加 `READ_FLAGS`，也就是 `--no-optional-locks -c core.fsmonitor=false`）：只讀 refs、不碰 index 與工作樹的指令，例如 `rev-parse --show-toplevel`、`symbolic-ref`、`for-each-ref`（不帶 `:track`）、`config --get`。
  - 走訪 commit 的指令（`rev-list --left-right --count`、`for-each-ref` 的 `%(upstream:track)`、`log`、`merge-base`）：repo 有 promisor remote（partial clone）時，ref 可能指向 clone 裡沒有的 commit，git 會用 repo 自己的 ssh 指令去抓。這種 repo 要加 `--no-lazy-fetch`（git 2.44 起），更舊的 git 就不要走訪。範例是 L1 的 `yuzora-shell` `gitStatus`（`a_partial_clones_missing_commit_is_not_fetched`）。
  - 必須先信任的指令：`status`、`diff`、`diff-files`、`diff-index`、`ls-files -m`，任何寫入工作樹或 index 的指令，以及 `fetch`、`pull`、`push`。
- 寫入指令先拿 repo 層級的寫入鎖（等待中可取消）；讀取不拿。
- 取消與逾時：本機行程開在自己的 process group，取消或逾時就 `kill(-pid, SIGKILL)` 整棵樹。Windows 沒有 process group：git 先以暫停狀態啟動，放進自己的 job object 後才恢復執行，取消或逾時用 `TerminateJobObject` 結束 hook、ssh、credential helper 等所有子孫行程。job 允許明確要求脫離的程式（`CREATE_BREAKAWAY_FROM_JOB`）離開，否則那些程式會無法啟動；這和 unix 上行程可以離開 process group 對等。credential helper 為登入而開的瀏覽器在 job 裡，取消那次 fetch 時會一起被關掉。讀取預設 60 秒，fetch／pull／push 等網路操作 600 秒；輸出上限 64 MiB。
- pathspec 超過 7600 bytes 改用 `--pathspec-from-file=- --pathspec-file-nul`（指令不支援時退回不帶路徑、讀全部）。
- 遠端 space：同一個執行器改走 `hc exec --json --on <machine> --cwd --timeout --env … [--stdin] -- git …`（`yuzora_core::remote`）。
- 輸出可以邊讀邊解析（`run_streaming`），log 用這條路。

### Status（`status.rs`、`watch.rs`）

- `status --porcelain=v2 -z --branch --show-stash --untracked-files=no`，untracked 另走 `ls-files --others --exclude-standard`（與 IntelliJ 一樣分開，大量 untracked 不拖慢 tracked）；被忽略的目錄只用來過濾 watcher 事件。`+/-` 行數來自 `diff --numstat`（超過 3000 個變更就略過）。全量刷新時這幾個讀取並行執行。
- watcher（notify，300 ms debounce）只在有 view 使用 repo 時運作。呼叫和等待（`changed()`、log 的 `logWait`）都算使用；超過 2 分鐘兩者都沒有，就停掉 watcher。下一次呼叫或等待會在背景執行緒重新啟動 watcher（遞迴監看可能要走過整個工作樹，不能放在 UI 執行緒），並把整個 repo 視為已變更。watcher 把事件分成 dirty scope：`.git/index` → 全部重讀；`.git` 內其他（refs、HEAD、MERGE_HEAD…）→ 全部＋refs；`.git` 雜訊（objects、logs、hooks、lock 檔…）與被忽略的目錄略過；工作目錄檔案 → 只重讀那些路徑。
- 部分刷新只對 dirty 路徑跑 status（`GIT_LITERAL_PATHSPECS=1`），A／D／R／C 的路徑一併重讀以免拆散 rename；路徑數過多（擁擠的目錄先折疊成目錄，超過 50 000 就全量）時退回全量。
- `.git` 的操作狀態（rebase-merge／rebase-apply、MERGE_HEAD、CHERRY_PICK_HEAD、REVERT_HEAD、BISECT_LOG）直接讀檔，對應 IntelliJ 的 `GitRepositoryReader`。

### Log 與分支圖（`log.rs`、`graph.rs`、`graph_view.rs`）

- **兩階段**（IntelliJ `GitLogProvider`）：先讀最近 `logRecentCount`（預設 2000）筆完整 metadata（`%x01%x01`／`%x02%x02`／`%x03%x03` 控制字元分隔，邊讀邊解析），拓撲排序後立刻建圖；再用 `--date-order --format=%H%x00%P%x00%ct` 讀全部 commit 的 hash／parents／時間，換上完整的圖。全部 refs 指向的 commit 都已在第一段時跳過第二段。
- **詳情延遲載入**：可見列缺 metadata 時，背景以 `log --no-walk=unsorted --stdin` 每批 500 筆補齊；commit 詳情（`--name-status -M`）放 LRU（1000）。
- **刷新**（VcsLogJoiner 的作法）：refs 變了只讀最近 1000 筆，接回既有的圖；有 parent 不在圖裡、ref 指到圖外或舊 head 消失時接不上，才全部重讀。
- **永久圖**：hash 存成連續 bytes、parents／children 用 CSR int 陣列；「唯一 parent 就在下一列」的 commit 不存邊（bitset），大多數線性歷史因此幾乎不佔 parent 空間。
- **layout index**：依 IntelliJ `GraphLayoutBuilder` 的 DFS，目前分支、main／master、其他本地、遠端、tag 依序先排，重要分支固定在左側；顏色由 layout index 決定，同一條分支的顏色穩定。
- **可見圖與列印元素**：依篩選得到可見列；每列的列印元素（節點欄、經過的邊、長邊箭頭，LONG_EDGE 30 列）在需要時才算，放 LRU（400）。元素排序移植 IntelliJ 的 `GraphElementComparatorByLayoutIndex`。
- **繪製**：`GraphCell` 是 HostModule 元件（GPUI `canvas`），直接讀 Rust 端的列印元素畫線段、箭頭與節點（HEAD 實心＋外框、merge 實心、一般空心）。
- **篩選**：branch、user、date、path、text（regex、區分大小寫）、hash、range、不含 merge。branch／path／range 篩選保留分支圖（`--parents` 改寫後的 parents）；user／text／date／hash／no-merges 顯示平面清單。篩選的 `--max-count` 分段 5 → 100 → 2000 → 全部（與 IntelliJ 一樣先給一屏）。
- **持久化 index 的接縫**：圖只依賴 `RawCommit {hash, parents, time}`，`LogSession` 的第二階段來源可以換成讀 index；篩選目前全交給 git。

### Diff 與部分暫存（`diff.rs`、`diffview.rs`）

- 行 diff 在 Rust 計算：線性空間 Myers（middle snake），編輯數超過 `max(20000 + 10·√(n+m), 20000)`（IntelliJ `MyersLCS` 的門檻）就退回 Patience（唯一行錨點＋區段內有限 Myers）。單側超過 16 MiB 不比較並告知。
- 內容：`git cat-file --filters <rev>:<path>`（套用 textconv 以外的 filter，與 IntelliJ 的 `git.read.content.with=FILTERS` 相同）、index 用 `:<path>`、工作目錄直接讀檔（symlink 讀目標字串）。遠端也一樣：`cat` 會讀穿 symlink，所以先在那台機器上用 `test -L`／`readlink`／`test -x` 判斷（`Repo::kind`），新檔部分暫存時的 mode（`120000`、`100755`）也照它。
- 檢視：並排或合併、字詞層級高亮（tokens：單字、空白段、單一符號）、空白策略 Exact／Trim／Ignore（對應 IntelliJ `ComparisonPolicy`）、context 摺疊。
- **部分暫存不動工作目錄**：把「index 版本＋選中的 hunk 或行」組成新內容，`git hash-object -w --stdin --path` 寫 blob，再 `update-index --add --cacheinfo <mode>,<oid>,<path>`。取消暫存同理（HEAD 版本＋反向選取）；捨棄 hunk 則改寫工作目錄檔案。整檔情況（新檔、刪除、全選）直接走 `add`／`reset`／`rm --cached`。CRLF 與無結尾換行會保留。
- 空白不是 Exact 時不允許暫存（畫面上的 hunk 與實際內容不一致）。

### Merge 與衝突（`merge.rs`）

- 讀 index 的 stage 1／2／3（base／ours／theirs）做 diff3 分塊：equal、ours、theirs、same、conflict。
- 「自動解決簡單衝突」：對 conflict 塊再做 token 層級三方合併，兩側改到不重疊的地方時給出 auto 解答（對應 IntelliJ 的 magic resolve）。
- 結果寫回檔案後 `git add`；整檔 accept yours／theirs 用 `checkout --ours/--theirs`（刪除衝突用 `git rm`）。
- 手動編輯一塊（`{text}`）：
  - 文字以 `\n` 分行，結尾的 `\n` 表示最後一行有行尾，不會多出一行。
  - `""` 代表這塊沒有任何行（刪掉整塊）；`"\n"` 代表一行空白。
  - 檔案中間的塊一定接著換行；只有檔案最後一行可以沒有行尾，所以沒有 `\n` 結尾的文字只會影響檔尾。
  - merge pane 的編輯框照原樣顯示每一行的換行：沒有行是空框，一行空白是一個換行。所以開啟編輯後不改就按「使用」，結果不變。
  - 結尾的換行被刪掉時，依來源那一側（view 的 `eol`）補回；只有檔案最後一行原本就沒有換行時不補。編輯框清空就送出 `""`，也就是刪掉整塊。
  - 想留一行空白，就在編輯框裡按一次換行。
  - 測試：`bun test scripts/yuzora-git-merge.test.ts`。
- rebase 期間 yours／theirs 依設定 `swapSidesInRebase`（預設開）對調，與 IntelliJ `isReverseRoot` 一致，標籤也跟著換。

### 互動式 rebase（`rebase.rs`）

- `GIT_SEQUENCE_EDITOR` 指向一段 shell，把我們寫好的 todo（`.git/yuzora-rebase/todo`）複製過去；`GIT_EDITOR` 指向另一段 shell，依 `rebase-merge/done` 的最後一行找出要 reword／squash 的 commit，換上我們事先寫好的訊息。不需要外部 helper 程式，本機與遠端都能用。
- todo 驗證（第一行不能是 squash／fixup、action 必須合法）；支援 `--update-refs`；edit 或衝突停下後由 `sequencer(continue|abort|skip)` 接手，continue 也用同一組 editor。
- **記憶體內歷史編輯（評估）**：IntelliJ 的 in-memory 操作（git-revise 啟發）不動工作目錄與 index。beta.2 先做 **reword**：沿線性歷史用 `commit-tree` 重建該 commit 之後的鏈（保留作者與時間），最後 `update-ref`；遇到 merge 就拒絕並改走一般互動式 rebase。drop／fixup／reorder 需要 tree 層級的三方合併（`merge-tree --write-tree`，git ≥ 2.38），留待後續。

## 效能量測

量測方式：`YUZORA_GIT_PERF_REPO=<repo> cargo test -p yuzora-git --lib --config 'profile.dev.package.yuzora-git.opt-level=2' -- --ignored --nocapture perf`（`log.rs` 的 `perf_on_a_large_repository`；依賴照 dev profile 是 opt-level 2，yuzora-git 本身這次也開到 2）。資料：本機 `/opt/homebrew` 的 clone（52,937 commits、3,406 個檔案，`git clone` 到 `/tmp` 後才量，原 repo 未動）。機器：Apple Silicon，量測時其他 lane 同時在編譯（數字偏保守）。

| 項目 | 結果 | 目標 |
| --- | --- | --- |
| log 首屏（前 40 列有 subject） | 冷快取 315 ms；熱快取 82–105 ms | ≤ 300 ms |
| 整張圖（52,937 commits） | 0.52–0.75 s（背景，不擋畫面） | — |
| 捲動：40 列的列印元素 | 平均 ~52 µs、最差 99–480 µs（200 個分散位置、冷 LRU） | 不掉幀（一幀 16.7 ms） |
| 深處列的 metadata（第 26,468 列起 40 列） | 19–32 ms | — |
| status 全量刷新 | 並行化前 480–740 ms；並行化後 238–467 ms（背景） | 不擋 UI |

## A1–A12 完成度

對照 [research/intellij-git-research.md](research/intellij-git-research.md) 的 A 段。標記：✅ 完成　🟡 部分　⏳ 未做（後續）　⛔ 不做（範圍外）。

### A1. Clone、init、環境、多 root

- ⏳ Clone（含 shallow／unshallow）、init：beta.2 由 Workspace（L1）開資料夾，Git 側欄不提供 clone／init。
- 🟡 Git 執行檔：設定頁可指定路徑（`executable`），版本顯示在 repo pane 的「身分與簽章」；沒有自動下載。
- ✅ Repo 偵測：側欄依焦點 pane 的資料夾（否則 space 的資料夾）找所在的 repo，跟著焦點切換；本機與遠端 space（`hc exec --on`）都可以。
- ⛔ Multi-root 同步分支操作：一次看一個 repo。
- 🟡 Submodules：列出狀態、`submodule update`；子模組本身當成另一個 repo 開啟。
- 🟡 Safe mode（未受信任專案）：Git 有自己的信任資料夾（見〈執行器〉）：repo 本地設定會自動執行程式時，Git 的所有操作先等使用者信任；自動執行的讀取另外關掉 `core.fsmonitor`，簽章檢查不用 repo 本地指定的程式，blame 不跑 textconv。⏳ 整個 app 共用的信任模型（終端機、其他模式）屬於殼層。

### A2. Status、Changes view、Partial commit

- ✅ 已暫存／變更／衝突三段，untracked 併在「變更」以 U 標示；數量、`+/-` 行數、全部暫存／取消。
- ⛔ Changelists：採 staging area 模式（IntelliJ 啟用 staging area 時同樣停用 changelists）。
- ✅ 部分暫存：hunk 或行，暫存、取消暫存、捨棄，不動工作目錄（`hash-object` 加 `update-index`）。
- 🟡 Ignored：被忽略的目錄不觸發刷新；沒有編輯 `.gitignore`／`info/exclude` 的 UI。
- ✅ 非 modal 的 commit 區（側欄底部）。

### A3. Commit 選項

- ✅ amend（最後一筆）、sign-off、略過 hooks、cleanup（設定 `commitCleanup`）、作者；日期由 API 支援（`date`），側欄未開欄位。
- ⏳ amend 指定 commit（`--allow-empty` 後 squash）：可以用互動式 rebase 的 fixup 達成。
- 🟡 GPG：與 IntelliJ 一樣改 git config（`commit.gpgSign`、`user.signingKey`、`gpg.format`），commit 詳情顯示 `%G?` 簽章狀態；沒有內建 pinentry。
- ✅ Hooks 由 git 執行，可逐次略過，預設值在設定頁。
- ✅ 訊息：歷史（每個 repo，數量可設定）、`commit.template`、amend 時帶入上一則訊息。⏳ `fixup!`／`squash!` 補全、Co-Authored-By 補全、長度檢查。
- ✅ Pre-commit 檢查：身分、衝突、分離 HEAD／rebase 中、沒有東西可提交、大檔案（門檻可設）、CRLF；阻擋型直接擋，其餘可「仍要提交」。
- ✅ Commit 並 Push：依設定開 push 對話框或直接推。

### A4. Shelf 與 Stash

- ⛔ Shelf。
- ✅ Stash：push（訊息、keep index、含 untracked）、apply、pop（可還原 index）、drop、從 stash 建分支、clear；看 stash 的檔案與 diff。
- ✅ 更新時保存本地變更：`git pull --autostash`（設定 `autoStash`）；smart checkout 用 stash 收起再還原。

### A5. Branches、Tags、Remotes

- ✅ 分支 popup：搜尋、最近、本地（↑↓、upstream 消失、在其他 worktree）、遠端、tags；新增、checkout（本地、遠端建追蹤分支、tag／revision 分離）、從此建分支、改名、刪除（未合併再確認）、刪遠端分支、設定／取消 upstream、tag push／刪除。⏳ favorites、依目錄分組、checkout and rebase onto current。
- ✅ Smart checkout 與強制 checkout（本地變更擋住時詢問）。
- ✅ 比較：兩分支（只在一邊的 commits 與差異檔案）、與工作目錄比較的 diff 模式（`compare`）。
- ⏳ Branch cleanup（刪除已合併分支）。
- ✅ Protected branches（regex）：禁止 force push。
- ✅ Tags：建立（輕量或附註）、push 單一或全部、刪除本地或遠端、checkout。
- ✅ Remotes：新增、改 URL、改名、移除、逐一 fetch。
- ✅ Fetch（prune、tags 模式 auto／always／never）、Pull…（分支 popup 的對話框：merge／rebase／ff-only，merge 可加 --no-ff、--squash、--no-commit；API 另支援 no-verify）、更新專案（依設定的更新方式）；自動 fetch（分鐘數可設）。
- ✅ Push：預覽（目標、commits、會帶上的 tags）、force-with-lease、force（受保護分支禁止、要再按一次確認；受保護分支的 pattern 用引擎的 Rust regex 語法，設定頁以同一套語法檢查，`checkPattern`。讀不懂的 pattern 等於保護所有分支，因為 JavaScript 接受的 look-around 在引擎會編譯失敗）、tags（none／follow／all）、略過 hooks、新分支自動 set-upstream、被拒時自動更新後重推（設定）。⏳ 記住 push target、Push up to here。

### A6. Merge、Rebase、Cherry-pick、Revert、Reset、Undo

- ✅ Merge：API 支援 no-ff、ff-only、squash、no-commit、訊息、no-verify、allow-unrelated；分支 popup 的「合併進目前分支」。操作中橫幅可繼續／中止。
- ✅ Rebase：API 支援 onto、rebase-merges、update-refs、autosquash、keep-empty、root；繼續／中止／略過。
- ✅ 互動式 rebase 編輯器：pick／reword／edit／squash／fixup／drop／update-ref、拖曳排序、訊息編輯；`GIT_SEQUENCE_EDITOR`／`GIT_EDITOR` 指向 Yuzora 的 shell helper。
- 🟡 記憶體內歷史編輯：reword ✅；drop／fixup／reorder ⏳（見〈互動式 rebase〉）。
- ✅ Cherry-pick（`-x` 後綴依設定、由舊到新）、Revert（由新到舊）、Reset（soft／mixed／hard／keep）、Undo commit（變更留在已暫存）。
- ⏳ 只套用或只 revert 選取的檔案／hunk（Apply Selected Changes）。

### A7. 衝突解決

- ✅ 三方資料（index stage 1／2／3）、跨檔案清單、整檔接受 yours／theirs（刪除衝突用 `git rm`）。
- ✅ 三欄合併：套用所有非衝突變更、自動解決簡單衝突（token 層級）、逐塊選擇或手動編輯、解完 `git add`。
- ✅ rebase 中 yours／theirs 對調（設定 `swapSidesInRebase`）。
- 🟡 二進位檔只能整檔接受。⏳ 語言特化的 imports 衝突解決。

### A8. Log

- ✅ 兩階段載入、分支圖（IntelliJ layout index、穩定顏色、長邊箭頭）、即時接上新 commit。
- 🟡 排序：拓撲加時間（Standard）；⏳ LinearBek、FirstParent、折疊線性分支。
- ✅ 篩選：branch、user、date、path、text（regex、大小寫）、hash、range、不含 merge。
- ✅ 詳情（訊息、作者、committer、parents、簽章、檔案）、右鍵選單（複製 hash、checkout、建分支、建 tag、cherry-pick、revert、reset、undo、改寫訊息、從此互動式 rebase、與目前分支比較、建立 patch）。
- ⏳ Highlighters、Show Repository at Revision（`tree`／`fileAt` 已在 API）、log 內的 Branches 面板、Go to hash 以外的跳轉。

### A9. File history、Blame、Compare、Patches

- ✅ 檔案歷史：log 加路徑篩選＋`--follow`。⏳ 選取範圍的歷史。
- ✅ Annotate：`git blame --porcelain -l -t`，忽略空白、偵測移動（none／inner `-M`／outer `-C`）、上一版 annotate（跟著改名）、與上一版比較、在 log 中顯示。
- ✅ Diff viewer：並排、合併、字詞高亮、三種空白策略、context 摺疊；二進位與過大檔案明示。⏳ 外部 diff 工具。
- ✅ Patches：建立（commits、已暫存、未暫存；複製或存檔）、套用（`git apply`：check、3way、只進 index、reverse）。

### A10. Worktrees 與其他

- ✅ Worktrees：列出、lock／unlock、移除、prune；新增沿用上游 `worktree` 對話框（不改上游 mod）。
- ⛔ Local History（IDE 自己的檔案歷史）。
- ⏳ Search Everywhere 搜 commit／分支（L1 的 palette）。
- ⛔ Git hosting 連結、GitHub／GitLab 整合。

### A11. 認證與 SSH

- 🟡 走 git 自己的 credential helper 與原生 OpenSSH；`GIT_TERMINAL_PROMPT=0` 讓需要互動輸入的認證直接失敗並顯示錯誤，而不是卡住。⏳ askpass helper（在 Yuzora 裡輸入帳密／passphrase）、記住 HTTP 認證。

### A12. Git 設定頁

`yuzora-git-settings`（Yuzora `settings.json` 的 `git` 區段）：

- ✅ 一般：Git 執行檔、CRLF 警告、分離 HEAD 警告、大檔案警告與門檻。
- ✅ Commit：sign-off 預設、略過 hooks 預設、cleanup、cherry-pick 後綴、訊息歷史數量。
- ✅ Push：force-with-lease、被拒時自動更新、push tags、顯示 push 對話框、受保護分支。
- ✅ Update／Fetch：更新方式、更新時自動 stash、fetch tags、fetch prune、自動 fetch 間隔。
- ✅ Log：最近 commit 數（第一階段）、顯示 tags、顯示遠端分支。
- ✅ Diff／Annotate／Merge：預設檢視、空白、context、annotate 忽略空白與偵測移動、rebase 時對調左右。
- 🟡 GPG 與身分在 repo pane 編輯（repo 或 global 層）。⏳ credential helper 選項、多 repo 同步。

## 已知限制與後續

- **側欄沒開過時沒有徽章**（實機確認：重啟後停在工作區模式，shared 裡沒有任何 git key；切到 Git 模式一次後才出現，之後切走仍會持續更新）：herdr 只在 stack 子節點第一次顯示時建立 mod，`yuzora.badge.git`／`yuzora.status.git` 要等 Git 模式開過一次才開始更新（之後切到別的模式仍會持續）。需要 L1／殼層在啟動時建立側欄各模式，或由狀態列自己訂閱。
- **三欄合併的欄寬**：欄寬用字元數估算，視窗 1280 寬時 Theirs 欄會被裁掉一部分（可橫向捲動）；要改成依可用寬度分配。
- **popup／dialog 的截圖**：herdr CLI 沒辦法帶 props 開 overlay，分支 popup 與 push 對話框的實機截圖是開在分頁裡（`tab.new --mod`）。
- **信任只管 Git**：repo 本地設定會自動執行程式時，Git 的所有操作都先等信任，但終端機與其他模式不受影響。狀態列（L1）與檔案樹（L5）的 git 呼叫要改用 `yuzora_core::git_trust`（#194 合併後）。整個 app 的信任模型要由殼層（L1）提供，Git 再改用它。
- **遠端 repo 沒有 watcher**：靠 `changed()` 約 25 秒逾時輪詢一次 status。
- **持久化 log index**：未做；圖只依賴 `RawCommit`，第二階段來源可以換成 index（見〈Log〉）。
- **記憶體內編輯**只有 reword。
- 列出的 ⏳ 項目。
