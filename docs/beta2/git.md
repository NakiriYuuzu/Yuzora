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
| `yuzora-git-diff` | modal 或 tool pane | 預設開成 modal（dialog overlay），「保留」把它移到 pane（預設分割在目前 pane 右側，可改成新分頁）；並排／合併、字詞高亮、空白（精確／忽略頭尾／全部忽略）、context（3 行／整檔）、hunk 導覽、暫存／取消暫存／捨棄 hunk 或選取的行、Annotate、歷史；即時更新 |
| `yuzora-git-log` | tool pane | 提交紀錄＋分支圖（`GraphCell`）、篩選、詳情、右鍵選單、分支比較、檔案歷史 |
| `yuzora-git-merge` | tool pane | 衝突檔案清單、整檔接受 yours／theirs、三方合併（套用非衝突、自動解決簡單衝突、逐塊選擇或手動編輯） |
| `yuzora-git-rebase` | tool pane | 互動式 rebase 編輯器（pick／reword／edit／squash／fixup／drop、拖曳排序、update-ref）、記憶體內改寫訊息 |
| `yuzora-git-blame` | tool pane | Annotate：gutter、上一版 annotate、與上一版比較、在 log 中顯示 |
| `yuzora-git-repo` | tool pane | Stashes、Tags、Remotes、Worktrees（新增交給上游 `worktree` 對話框）、Submodules、Patches、身分與簽章 |
| `yuzora-git-settings` | 設定頁 | Yuzora `settings.json` 的 `git` 區段（見 A12） |

### pane 之間的約定

- 每個 mod 用 `open(cwd, on)` 取得 repo（同一個 `<machine>:<root>` 只開一次，watcher 也只有一個），之後的函式都帶 `repo.id`。
- 單一實例的 pane 用 shared key 交棒：`yuzora.git.diff`（`{cwd, on, path, orig, mode, rev?, to?, at}`）、`yuzora.git.merge`、`yuzora.git.rebase`。開啟方先 `setShared`，已有該 pane 就 `pane.focus`，否則 `tab.new {mod, props}`；pane 依 `at` 取較新的請求。log、blame、repo pane 用 props，可以同時開多個。
- diff 例外，依 Git 設定開啟（側欄、log 詳情、blame、repo pane 四個入口各帶一份相同的 `showDiff`／`placeDiff`，`scripts/yuzora-git-diff-open.test.ts` 守住四份一致、每個 mod 都 import 了它們呼叫的函式，以及各種設定）：
  - 已有 diff pane 且 `diffFollowPane`（預設開）：照上面的 shared key 交棒。
  - 否則 `diffOpenIn: "modal"`（預設）：`openOverlay({mod: "yuzora-git-diff", kind: "dialog", width: 0.9, height: 0.86, props})`。modal 只看自己的 props，不跟 shared key；Esc、✕ 或點旁邊關閉；Annotate、歷史開新分頁後也關閉。
  - `diffOpenIn: "pane"`：直接開在「保留」會放的位置。
  - 「保留」：依 `diffKeepIn`，`split`（預設）在焦點 pane 右側 `pane.split {direction: "right", mod, props}`，沒有焦點 pane 或設成 `tab` 時 `tab.new`；新 pane 帶著 modal 裡的檢視（並排／合併、空白、context），然後關閉 modal。`run()` 只把指令排進佇列、不回傳錯誤，所以 split 或 `tab.new` 失敗時 modal 照樣關閉，pane 也不會出現。
- 即時更新：`changed(repo, since)` 在 repo 的 generation 變了（watcher 或我們自己的寫入）時 resolve；遠端 repo 沒有 watcher，約 25 秒逾時一次，呼叫端藉此輪詢。本機 repo 的 watcher 啟動失敗時（例如系統的監看數用完），等待逾時就把整個 repo 與 refs 視為已變更，generation 前進，各 pane 照常重讀。逾時由一條共用的計時執行緒負責，不靠 watcher 的 tick（只在有人等待時存在）。
- 跨 lane：`yuzora.badge.git`＝變更檔案數（乾淨時 `null`），`yuzora.status.git`＝`{text: "⎇ main ↑2 · 4 變更", tone}`，tone：衝突 `blocked`、操作中或執行中 `working`、有變更 `accent`、乾淨 `muted`。

### virtual list 的限制

gpui-shell 不允許在 `v_virtual_list` 的 item renderer 裡註冊事件（會丟例外、整個清單不畫）。列的點擊走清單的 `on_item_click(key)`／`on_item_secondary_click(key, event)`；一列裡有多個可點區域（側欄的 +／−／↺、diff 的行號）時，清單外層用 `on_mouse_down` 記下按下的 x，再在 `on_item_click` 判斷。

## 引擎

### 執行器（`exec.rs`）

- 只用 git CLI。每個指令都帶 `-c core.quotepath=false -c log.showSignature=false -c color.ui=false`；讀取另外帶 `GIT_OPTIONAL_LOCKS=0` 與 `-c diff.autoRefreshIndex=false`，不碰 `.git/index`，watcher 才不會被自己的讀取喚醒。
- 環境：`LANGUAGE=` 清空、`LC_ALL=en_US.UTF-8`（macOS 本機）或 `C.UTF-8`，`GIT_TERMINAL_PROMPT=0`、`GIT_EDITOR=:`、`GIT_MERGE_AUTOEDIT=no`，輸出才能穩定解析，也不會卡在互動輸入。
- 讀取一律帶 `-c core.fsmonitor=false`：側欄一聚焦資料夾就自動跑 status，repo 自己的 `.git/config` 不能藉 fsmonitor hook 在使用者沒有任何動作時執行程式（`status_never_runs_the_repos_fsmonitor_program` 守住）。代價是設定了 fsmonitor 的大 repo，背景 status 會慢一些。
- 其他由 repo 設定指定程式的讀取：commit 詳情的簽章檢查（`%G?`）把 repo 本地設定的 `gpg.program`／`gpg.{openpgp,ssh,x509}.program` 換回使用者全域／系統的值或 git 預設（`signature_programs`）；blame 帶 `--no-textconv`；status 的 numstat 帶 `--no-ext-diff --no-textconv`，建立 patch 的 diff 帶 `--no-ext-diff`。diff 讀 blob 用 `cat-file --filters`，會照 repo 設定跑 smudge filter（例如 git-lfs），只在使用者打開 diff 時發生，與 checkout 相同。
- 信任資料夾（`trust.rs`）：repo 自己的設定（`.git/config`、它 include 的檔案、worktree 設定）若指定了 git 會執行的程式（`filter.*.clean/smudge/process`、`core.sshCommand`／`gitProxy`／`askPass`／`alternateRefsCommand`、指向程式的 `core.fsmonitor`（`true` 是 git 自己的 daemon，不算）、credential helper、`remote.*.uploadpack/receivepack/vcs`、`protocol.*.allow`、`merge.*.driver`、`diff.external`、`diff.*.command/textconv`、`gpg.*program`、`gpg.ssh.defaultKeyCommand`、`!` 開頭的 `submodule.*.update`），`status` 不跑 git，只回傳已知內容和 `trust`（那些 key）；側欄顯示「要信任這個資料夾嗎？」，按下後記在 `<app home>/yuzora/git/trusted.json`。自動 fetch 由引擎擋（`fetch {auto: true}` 在未信任時直接拒絕），而且一律不跑 hook（`core.hooksPath` 指向 app 自己的空目錄；fetch 會觸發 reference-transaction）、不遞迴 submodule（`--no-recurse-submodules`）。信任檢查也看 index 裡每個已 checkout 的 gitlink（巢狀的也算）自己的設定，不管 `.gitmodules` 有沒有列出它，也不管上層設定有沒有程式：git 的 status、diff 會遞迴進這些 submodule，用它們的設定跑 filter。判斷「已 checkout」照 git 自己的條件（資料夾裡有 `.git`），進去後只讀它自己（`GIT_DIR=.git`）；它的巢狀 gitlink 到它實際的工作樹去找：submodule 自己的 `core.worktree`（寫在它的 config、gitdir 的 config 或 `config.worktree`）可以把工作樹指到別處，git 的 status 會跟過去，掃描也用 `rev-parse --show-toplevel` 跟過去。每一層都跟 git 一樣先進到那個工作樹，再用 gitlink 在 index 裡的名稱原樣找 `.git`：從根拼成的絕對路徑可能超過 PATH_MAX（macOS 1024），git 用的相對路徑卻沒有，以前掃描因此略過，git 卻照樣進去。找不到 `.git` 時，只有確定它不在（路上某層資料夾進得去而且沒有下一段，或那一段是檔案）才略過；其他情況（例如資料夾進不去）當成讀不到。每個 repo（gitdir 加工作樹，取 `rev-parse` 的絕對路徑）只讀一次，整個掃描在同一個 shell 裡記住讀過哪些：在自己底下又遇到（工作樹指回上層，git 的 status 會無限遞迴）當成讀不到；在別處又遇到（兩個 gitfile 指向同一個 gitdir、衝突的 gitlink 在 index 出現好幾次）就略過，它的 key 已經列過。所以掃描量受實際 repo 數限制，不會像只限層數那樣在兩個指回頂層的 gitlink 上展開成 2^16 個。路徑被 git 加了引號（含 `"`、`\`、控制字元）、index 裡的 gitlink 名稱是絕對路徑（git 的 status 會走進那個路徑，掃描卻會在頂層底下找）、git 自己不會寫出的名稱（含 `.`、`..` 或空的路徑段、以 `/` 結尾、磁碟代號開頭；手改 index 可以放進去，用來把路徑拉長）、工作樹名稱含換行（`$(…)` 會刪掉結尾的換行，掃描就會走到同名但沒有換行的資料夾；所以 `rev-parse` 的回答後面補一個 `.`，去掉後必須剛好兩行，頂層自己也一樣）、工作樹從根算起超過 PATH_MAX（`rev-parse --absolute-git-dir` 讀不到），或超過 16 層時，都無法追蹤，也當成讀不到。掃描以命令列 alias 讓 git 在頂層執行，本機與遠端都只多一次呼叫；腳本 export `GIT_NO_LAZY_FETCH=1`。只要有一段沒跑完，關卡就維持關閉：gitlink 用 grep 從 `ls-files -s` 挑出，輸入最後補一行固定的記錄，grep 的輸出沒有它就當成讀不到（壞掉的 grep 不輸出也會 exit 0，看起來就像沒有 gitlink）；整個掃描以 `end` 記錄結尾，回答沒有它也當成讀不到。20 萬筆 entry 的 index 約 115 ms。掃描逾時（yuzora-git 60 秒、`programs_in` 30 秒）也當成讀不到，而不是錯誤：錯誤會讓 status 沒有 `trust` 欄位，側欄就無法提供信任。橫幅以 `路徑: key` 列出。設定讀不到、或回答超過 8 MiB（`git_trust::ANSWER_CAP`，截斷的回答可能剛好少了某個程式 key）時，當成未信任。hook 本身不列入信任條件：status 不跑 hook，而 husky 等工具會在許多 repo 設定 `core.hooksPath`。使用者全域／系統設定的程式（例如全域安裝的 git-lfs）不需要信任。未信任時 `yuzora:git` 對這個 repo 的其他呼叫（log、diff、merge、stage、commit、fetch…）一律在引擎拒絕（`trust::check`，錯誤以 `untrusted folder` 開頭），各 pane 顯示這個錯誤，等同 IntelliJ 的 Safe mode；只有 `status`（顯示提示）與 `trust`（回答）例外。不經過 `repo_fn` 的路徑各自在跑 git 之前檢查：log 的載入（開啟、`logRefresh`、跟著 refs 更新）與補抓 metadata，以及 diff／merge session 的寫入（`diffStage`／`diffUnstage`／`diffDiscard`、`conflictResolve`；session 開啟後 repo 設定可能多了程式，例如 `git lfs install --local`）。partial clone 讀到缺少的物件時，會用 repo 的 ssh 指令去抓，所以 log 也不能在信任前讀。
- 共用的信任 API（`yuzora_core::git_trust`）：上面的偵測（`PROGRAMS`、`names_program`、submodule 掃描）與信任清單放在 yuzora-core，給狀態列（L1）、檔案樹（L5）等也會自己跑 git 的地方共用；yuzora-git 用自己的執行器呼叫它，行為不變。
  - `pending(machine, top)`：回傳空清單才可以跑 `status`、`diff` 這類會比對工作樹或 index 的指令；`Err`（git 跑不起來）也當成不安全。`top` 是那台機器上 `git rev-parse --show-toplevel` 的輸出，遠端走 `hc exec --on`。
  - 清單裡的項目是 `key`（repo 自己的設定）或 `路徑: key`（submodule 的設定）。設定讀不到時，項目是 `UNREADABLE`（`(unreadable config)`）或 `路徑: (unreadable config)`：git 讀設定失敗、回答超過 `ANSWER_CAP`、掃描沒跑完或逾時、submodule 的工作樹指回上層、路徑被加了引號、gitlink 名稱是絕對路徑或 git 不會寫出的名稱（含 `.`、`..`、空的路徑段，或磁碟代號）、gitlink 可能在但看不到（例如資料夾進不去）、工作樹名稱以換行結尾、工作樹從根算起超過 PATH_MAX、超過 16 層。這些都維持關閉，但使用者仍可以信任這個資料夾。
  - `pending_from(file, machine, top)`：同 `pending`，信任清單從 `file` 讀（測試用暫存檔，不碰真實的 app home）。
  - `is_trusted(machine, top)`：只讀信任清單，不跑 git。
  - `folder_id(machine, top)`：信任清單裡的 id，`<machine>:<top>`，本機的 machine 是空字串。
  - `trust_at(file, id)`：把 id 加進 `file`。信任只能由 Git 側欄按下（yuzora-git 的 `trust`），其他 lane 只讀，不要呼叫它。
  - `programs_in(machine, top)`：不看信任清單，只列出 key（`pending` 在未信任時就是它）。
  - API 變動（#194 審查期間）：`repo_programs(run)` 拿掉了第一個參數 `gitmodules: bool`，submodule 一律從 index 掃，不再看 `.gitmodules`。`pending`、`programs_in`、`is_trusted` 的呼叫方式不變。自己傳 `run` 的呼叫端，要讓它能執行 `-c alias.…=!<script> yuzora-trust-scan`（本機與 `hc exec --on` 都可以），並照原樣回傳 exit code 與 stdout（經 `whole`）；逾時要回傳 `Ok((-1, vec![]))`（當成讀不到），只有 git 根本跑不起來才回 `Err`。
  - 未信任時仍可以執行的指令（前面加 `READ_FLAGS`，也就是 `--no-optional-locks -c core.fsmonitor=false`）：只讀 refs、不碰 index 與工作樹的指令，例如 `rev-parse --show-toplevel`、`symbolic-ref`、`for-each-ref`（不帶 `:track`）、`config --get`。
  - 走訪 commit 的指令（`rev-list --left-right --count`、`for-each-ref` 的 `%(upstream:track)`、`log`、`merge-base`）：repo 有 promisor remote（partial clone）時，ref 可能指向 clone 裡沒有的 commit，git 會用 repo 自己的 ssh 指令去抓。這種 repo 要加 `--no-lazy-fetch`（git 2.44 起），更舊的 git 就不要走訪。範例是 L1 的 `yuzora-shell` `gitStatus`（`a_partial_clones_missing_commit_is_not_fetched`）。
  - 必須先信任的指令：`status`、`diff`、`diff-files`、`diff-index`、`ls-files -m`，任何寫入工作樹或 index 的指令，以及 `fetch`、`pull`、`push`。
- 寫入指令先拿 repo 層級的寫入鎖（等待中可取消）；讀取不拿。
- 取消與逾時：本機行程開在自己的 process group，取消或逾時就 `kill(-pid, SIGKILL)` 整棵樹。Windows 沒有 process group：git 先以暫停狀態啟動，放進自己的 job object 後才恢復執行，取消或逾時用 `TerminateJobObject` 結束 hook、ssh、credential helper 等所有子孫行程。job 允許明確要求脫離的程式（`CREATE_BREAKAWAY_FROM_JOB`）離開，否則那些程式會無法啟動；這和 unix 上行程可以離開 process group 對等。credential helper 為登入而開的瀏覽器在 job 裡，取消那次 fetch 時會一起被關掉。job 建不起來或放不進去時不執行 git（回傳錯誤），不會在取消只能結束 git 本身的情況下執行。讀取預設 60 秒，fetch／pull／push 等網路操作 600 秒；輸出上限 64 MiB。
- pathspec 超過 7600 bytes 改用 `--pathspec-from-file=- --pathspec-file-nul`（指令不支援時退回不帶路徑、讀全部）。
- 遠端 space：同一個執行器改走 `hc exec --json --on <machine> --cwd --timeout --env … [--stdin] -- git …`（`yuzora_core::remote`）。
- 輸出可以邊讀邊解析（`run_streaming`），log 用這條路。

### Status（`status.rs`、`watch.rs`）

- `status --porcelain=v2 -z --branch --show-stash --untracked-files=no`，untracked 另走 `ls-files --others --exclude-standard`（與 IntelliJ 一樣分開，大量 untracked 不拖慢 tracked）；被忽略的目錄只用來過濾 watcher 事件。`+/-` 行數來自 `diff --numstat`（超過 3000 個變更就略過）。全量刷新時這幾個讀取並行執行。status 與 untracked 清單超過輸出上限（64 MiB）時會被截斷，所以超過就回傳錯誤，不把只有前段的清單當成完整的存進快取（下次全部重讀）。
- 不是 UTF-8 的路徑（unix 的檔名可以是任意位元組）在 JS 只能顯示成把壞位元組換成 U+FFFD 的字串，把這個字串交回 git 會變成另一個名稱，可能剛好是另一個真的叫這個名字的檔案。status 記下這些路徑的顯示名稱（`StatusCache::lossy`），`repo_fn` 對 `status`、`trust` 以外的呼叫檢查參數：任何字串等於這種顯示名稱，或是它所在的資料夾（本身也含 U+FFFD），就拒絕（`path.lossy`，請使用者改用終端機）。上層的正常資料夾（例如 `src`）照常可以操作，git 會用真正的位元組。watcher 看到不是 UTF-8 的路徑時整個重讀；部分刷新的路徑碰到這種顯示名稱時也改成全量，否則只用這個名稱查 git，只會列出真的叫這個名字的檔案，另一個就從快取消失。
- watcher（notify，300 ms debounce）只在有 view 使用 repo 時運作。呼叫和等待（`changed()`、log 的 `logWait`）都算使用；超過 2 分鐘兩者都沒有，就停掉 watcher。下一次呼叫或等待會在背景執行緒重新啟動 watcher（遞迴監看可能要走過整個工作樹，不能放在 UI 執行緒），並把整個 repo 視為已變更。watcher 啟動失敗時改成輪詢（見〈pane 之間的約定〉的 `changed()`）。watcher 把事件分成 dirty scope：`.git/index` → 全部重讀；`.git` 內其他（refs、HEAD、MERGE_HEAD…）→ 全部＋refs；`.git` 雜訊（objects、logs、hooks、lock 檔…）與被忽略的目錄略過；工作目錄檔案 → 只重讀那些路徑。
- 部分刷新只對 dirty 路徑跑 status（`GIT_LITERAL_PATHSPECS=1`），A／D／R／C 的路徑一併重讀以免拆散 rename；路徑數過多（擁擠的目錄先折疊成目錄，超過 50 000 就全量）時退回全量。submodule 裡的檔案改成查它的 gitlink（git 把裡面的變更報在 gitlink 上）；gitlink 從 index 列出（`ls-files -s` 的 `160000`），沒寫在 `.gitmodules` 的也算，與全量刷新的其他讀取並行。這份清單失敗或超過輸出上限時，不存前段（後面的 gitlink 會漏掉，裡面的變更就查不到），而是記成不完整，部分刷新一律改成全量。折疊時去掉已被目錄涵蓋的路徑，只查每個路徑的上層目錄，不做兩兩比對（watcher 在 debounce 執行緒上同步呼叫，5 萬個路徑兩兩比對要 25 億次）。重讀前從快取拿掉這些路徑底下的項目時也一樣：每個項目用自己和上層目錄去查重讀的路徑集合（10 萬個項目、5000 個路徑原本要 18 秒）。
- `.git` 的操作狀態（rebase-merge／rebase-apply、MERGE_HEAD、CHERRY_PICK_HEAD、REVERT_HEAD、BISECT_LOG）直接讀檔，對應 IntelliJ 的 `GitRepositoryReader`。

### Log 與分支圖（`log.rs`、`graph.rs`、`graph_view.rs`）

- **兩階段**（IntelliJ `GitLogProvider`）：先讀最近 `logRecentCount`（預設 2000）筆完整 metadata（`%x01%x01`／`%x02%x02`／`%x03%x03` 控制字元分隔，邊讀邊解析），拓撲排序後立刻建圖；再用 `--date-order --format=%H%x00%P%x00%ct` 讀全部 commit 的 hash／parents／時間，換上完整的圖。全部 refs 指向的 commit 都已在第一段時跳過第二段。
- **詳情延遲載入**：可見列缺 metadata 時，背景以 `log --no-walk=unsorted --stdin` 每批 500 筆補齊；commit 詳情（`--name-status -M`）放 LRU（1000）。
- **刷新**（VcsLogJoiner 的作法）：refs 變了只讀最近 1000 筆，接回既有的圖；有 parent 不在圖裡、ref 指到圖外或舊 head 消失時接不上，才全部重讀。
- **永久圖**：hash 存成連續 bytes、parents／children 用 CSR int 陣列；「唯一 parent 就在下一列」的 commit 不存邊（bitset），大多數線性歷史因此幾乎不佔 parent 空間。
- **layout index**：依 IntelliJ `GraphLayoutBuilder` 的 DFS，目前分支、main／master、其他本地、遠端、tag 依序先排，重要分支固定在左側；顏色由 layout index 決定，同一條分支的顏色穩定。
- **可見圖與列印元素**：依篩選得到可見列；每列的列印元素（節點欄、經過的邊、長邊箭頭，LONG_EDGE 30 列）在需要時才算，放 LRU（400）。元素排序移植 IntelliJ 的 `GraphElementComparatorByLayoutIndex`。
- **繪製**：`GraphCell` 是 HostModule 元件（GPUI `canvas`），直接讀 Rust 端的列印元素畫線段、箭頭與節點（HEAD 實心＋外框、merge 實心、一般空心）。
- **篩選**：branch、user、date、path、text（regex、區分大小寫）、hash、range、不含 merge。branch／path／range 篩選保留分支圖（`--parents` 改寫後的 parents）；user／text／date／hash／no-merges 顯示平面清單。篩選的 `--max-count` 分段 5 → 100 → 2000 → 全部（與 IntelliJ 一樣先給一屏）。branch／hash／range 是放在選項前面的 revision，開頭是 `-` 的一律拒絕，否則 git 會把它當成選項（例如 `--output=<file>` 會把 log 寫進那個檔案）；compare 的兩端也一樣。
- **持久化 index 的接縫**：圖只依賴 `RawCommit {hash, parents, time}`，`LogSession` 的第二階段來源可以換成讀 index；篩選目前全交給 git。

### Diff 與部分暫存（`diff.rs`、`diffview.rs`）

- 行 diff 在 Rust 計算：線性空間 Myers（middle snake），編輯數超過 `max(20000 + 10·√(n+m), 20000)`（IntelliJ `MyersLCS` 的門檻）就退回 Patience（唯一行錨點＋區段內有限 Myers）。單側超過 16 MiB 不比較並告知。
- 內容：`git cat-file --filters <rev>:<path>`（套用 textconv 以外的 filter，與 IntelliJ 的 `git.read.content.with=FILTERS` 相同）、index 用 `:<path>`、工作目錄直接讀檔（symlink 讀目標，unix 上保留原始位元組，不轉成 UTF-8）。遠端也一樣：`cat` 會讀穿 symlink，所以先在那台機器上用 `test -L`／`readlink`／`test -x` 判斷（`Repo::kind`），新檔部分暫存時的 mode（`120000`、`100755`）也照它。
- 檢視：並排或合併、字詞層級高亮（tokens：單字、空白段、單一符號）、空白策略 Exact／Trim／Ignore（對應 IntelliJ `ComparisonPolicy`）、context 摺疊。
- **部分暫存不動工作目錄**：把「index 版本＋選中的 hunk 或行」組成新內容，`git hash-object -w --stdin --path` 寫 blob，再 `update-index --add --cacheinfo <mode>,<oid>,<path>`。取消暫存同理（HEAD 版本＋反向選取）；捨棄 hunk 則改寫工作目錄檔案（寫入與刪除都不穿過工作樹裡的 symlink 資料夾：路徑上層有 symlink 就拒絕，與 git 的 `beyond a symbolic link` 相同，免得寫到 repo 外面；merge 寫回結果也一樣，本機與遠端都檢查）。整檔情況（新檔、刪除、全選）直接走 `add`／`reset`／`rm --cached`；逐行勾選了每個 hunk 的每一行也算全選（否則取消暫存一個新檔，會留下一個空檔在 index）。只有 mode 改變（例如 `chmod +x`）時，以 `git diff --raw` 讀兩邊的 mode（照 git 的判斷，含 `core.fileMode`），pane 顯示 mode 變更，暫存、取消暫存、捨棄都整檔處理，mode 一起帶上。已暫存的 rename 比較 HEAD 裡舊路徑的 mode 與 index 裡新路徑的 mode（`--no-renames` 會把它看成新增）。部分暫存是用開啟時讀到的內容組出來的，所以先確認工作目錄的檔案沒有被其他編輯器改過（捨棄也是），改過就要求重新載入，免得暫存檔案裡已經不存在的行。CRLF 與無結尾換行會保留。
- 空白不是 Exact 時不允許暫存（畫面上的 hunk 與實際內容不一致）。

### Merge 與衝突（`merge.rs`）

- 讀 index 的 stage 1／2／3（base／ours／theirs）做 diff3 分塊：equal、ours、theirs、same、conflict。
- 「自動解決簡單衝突」：對 conflict 塊再做 token 層級三方合併，兩側改到不重疊的地方時給出 auto 解答（對應 IntelliJ 的 magic resolve）。塊裡有不是 UTF-8 的行時不提供 auto（token 合併以文字進行，那些位元組會變成 U+FFFD）；塊以外的行照原本的位元組寫回。
- 結果寫回檔案後 `git add`；整檔 accept yours／theirs 用 `checkout --ours/--theirs`（刪除衝突用 `git rm`）。symlink 的衝突以 `120000` 寫回 index，結果的每個位元組都是目標路徑（結尾的換行也是），不另外去掉。
- 任一側或工作目錄的檔案超過 16 MiB（與 diff 相同的上限）時只讀得到開頭，合併那段會丟掉檔尾：view 回傳 `tooLarge`、沒有分塊，`result`／`conflictResolve` 拒絕，merge pane 只提供整檔接受。
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

- `GIT_SEQUENCE_EDITOR` 指向一段 shell，把我們寫好的 todo（`.git/yuzora-rebase/todo`）複製過去；`GIT_EDITOR` 指向另一段 shell，依 `rebase-merge/done` 的最後一行找出要 reword／squash 的 commit，換上我們事先寫好的訊息。不需要外部 helper 程式，本機與遠端都能用。base 放在 `rebase -i` 的選項後面（列出 commit 的 `log` 也是），開頭是 `-` 的一律拒絕，否則 `--exec=<cmd>` 會執行指令、`--output=<file>` 會寫檔。
- todo 只列非 merge 的 commit：沒有 `--rebase-merges` 時 git 會把 merge 攤平，`pick` 也不接受 merge commit。範圍內有 merge 時 `hasMerges` 為 true，pane 顯示警告。
- todo 驗證（第一行不能是 squash／fixup、action 必須合法）；支援 `--update-refs`；edit 或衝突停下後由 `sequencer(continue|abort|skip)` 接手，continue 也用同一組 editor。
- **記憶體內歷史編輯（評估）**：IntelliJ 的 in-memory 操作（git-revise 啟發）不動工作目錄與 index。beta.2 先做 **reword**：沿線性歷史重建該 commit 之後的鏈，最後 `update-ref`。每個 commit 照原本的位元組重放（`hash-object -t commit`），只換 parent 與 committer（`git var GIT_COMMITTER_IDENT`）、拿掉已失效的簽章；作者、`encoding` header 與之後的訊息都不經過文字轉換，非 UTF-8 的訊息也不會變。被改寫的那個 commit 換上新訊息（UTF-8，拿掉 `encoding`）。遇到 merge 就拒絕並改走一般互動式 rebase。drop／fixup／reorder 需要 tree 層級的三方合併（`merge-tree --write-tree`，git ≥ 2.38），留待後續。

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
- 🟡 Git 執行檔：設定頁可指定路徑（`executable`），每次呼叫時讀取，已開啟的 repo 也立刻換用；版本顯示在 repo pane 的「身分與簽章」；沒有自動下載。
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

- ✅ 分支 popup：搜尋、最近、本地（↑↓、upstream 消失、在其他 worktree）、遠端、tags；新增、checkout（本地、遠端建追蹤分支、tag／revision 分離）、從此建分支、改名、刪除（未合併再確認）、刪遠端分支（遠端分支的 remote 以設定的 remote 名稱最長相符來判斷，`team/origin` 這種含斜線的名稱也對）、設定／取消 upstream、tag push／刪除。建立 tag 時名稱放在 `--` 之後，像選項的名稱（`-d`）不會變成刪除，git 會拒絕這種名稱。⏳ favorites、依目錄分組、checkout and rebase onto current。
- ✅ Smart checkout 與強制 checkout（本地變更擋住時詢問）。
- ✅ 比較：兩分支（只在一邊的 commits 與差異檔案）、與工作目錄比較的 diff 模式（`compare`）。
- ⏳ Branch cleanup（刪除已合併分支）。
- ✅ Protected branches（regex）：禁止 force push。
- ✅ Tags：建立（輕量或附註）、push 單一或全部、刪除本地或遠端、checkout。
- ✅ Remotes：新增、改 URL、改名、移除、逐一 fetch。`remote -v` 以 tab 與最後一個空白切開，URL 裡的空白（本機路徑）保留。
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
- 🟡 二進位檔與超過 16 MiB 的檔案只能整檔接受。⏳ 語言特化的 imports 衝突解決。

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
- ✅ Patches：建立（commits、已暫存、未暫存；複製或存檔）、套用（`git apply`：check、3way、只進 index、reverse）。`git diff` 不支援 `--pathspec-from-file`，選取的路徑太長時分批產生再串起來（依路徑排序），不會退回整個 repo。patch 超過 64 MiB（git 的輸出會被截斷）時拒絕，不回傳只有前段的 patch。

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
- ✅ Diff／Annotate／Merge（實機點擊截圖：`evidence/l2-diff-modal.png`、`l2-diff-keep.png`、`l2-diff-follow.png`）：diff 開啟方式（modal／pane）、保留的位置（分割在目前 pane 旁／新分頁）、已開的 diff pane 是否顯示下一個檔案、預設檢視、空白、context、annotate 忽略空白與偵測移動、rebase 時對調左右。
- 🟡 GPG 與身分在 repo pane 編輯（repo 或 global 層）。⏳ credential helper 選項、多 repo 同步。

## 已知限制與後續

- **側欄沒開過時沒有徽章**（實機確認：重啟後停在工作區模式，shared 裡沒有任何 git key；切到 Git 模式一次後才出現，之後切走仍會持續更新）：herdr 只在 stack 子節點第一次顯示時建立 mod，`yuzora.badge.git`／`yuzora.status.git` 要等 Git 模式開過一次才開始更新（之後切到別的模式仍會持續）。需要 L1／殼層在啟動時建立側欄各模式，或由狀態列自己訂閱。
- **三欄合併的欄寬**：欄寬用字元數估算，視窗 1280 寬時 Theirs 欄會被裁掉一部分（可橫向捲動）；要改成依可用寬度分配。
- **popup／dialog 的截圖**：herdr CLI 沒辦法帶 props 開 overlay，分支 popup 與 push 對話框的實機截圖是開在分頁裡（`tab.new --mod`）。
- **信任只管 Git**：repo 本地設定會自動執行程式時，Git 的所有操作都先等信任，但終端機與其他模式不受影響。狀態列（L1）與檔案樹（L5）的 git 呼叫要改用 `yuzora_core::git_trust`（#194 合併後）。整個 app 的信任模型要由殼層（L1）提供，Git 再改用它。
- **遠端 repo 沒有 watcher**：靠 `changed()` 約 25 秒逾時輪詢一次 status。
- **持久化 log index**：未做；圖只依賴 `RawCommit`，第二階段來源可以換成 index（見〈Log〉）。
- **記憶體內編輯**只有 reword。
- **diff 的跟隨請求不分 pane**（Codex 對 #208 的 P2，延後）：先關掉 `diffFollowPane`、開出好幾個 diff pane、再打開它時，`yuzora.git.diff` 會讓每個 diff pane 都換檔，`showDiff` 卻只 focus 其中一個。mod pane 不知道自己的 pane id（herdr 只把 `props().pane` 給 pane 標頭 mod，`run()` 回傳 `null`），要改上游 API 或另做 opener 與 pane 的 key 對應。
- **props 沒帶 `at` 的 diff pane 會顯示舊檔案**（既有，reviewer 對 #208 指出）：`wanted()` 把沒有 `at` 的 props 當成 0，已持久化的 `yuzora.git.diff` 請求永遠比它新。agent 照 `defaults/AGENTS.md` 用 `tab.new --props` 開 diff 時，可能看到上一次的檔案。
- 列出的 ⏳ 項目。

### #194 合併後的 follow-up

合併關卡調整後，不影響安全、修正需要另外決定的項目延到合併後的 follow-up PR：

- **diff 的記憶體上限**（`diff.rs` 的 Myers，對 `ebb78888` 的留言）：兩個 16 MiB、每行一個位元組的檔案，`V` 緩衝區照 n+m 配置約 512 MiB，`lines()` 的切片與行號又約 700 MiB。修法：`V` 依編輯預算（`budget / 2 + 2`）配置，並設行數上限（超過就跟超過 16 MiB 一樣不比較並告知）；上限值與畫面文字要一起決定。
- **淺色主題（yuzora-light）的對比**（L1 對 `ebb78888` 的清單，5 處，標準為文字 4.5:1、大字與圖示 3:1）：
  - log 圖的 lane 色與 HEAD 點（`graph_view.rs` 的預設色在白底只有 1.53–2.65）：`tokens.json` 新增 `git_graph {lanes, head}`（深色與 light 都要），log pane 傳給 `GraphCell`。`tokens.json` 與 `yuzora-core` 的 `tokens.rs`（對比測試）屬於 L1，由總指揮授權修改。
  - ref chip 的字改用 `foreground`（底色與邊框保留）。
  - 信任按鈕的字改用 `accent_foreground`，hover 改用透明度回饋。
  - selected／hover 上的紅綠文字改用 `file_status.deleted`／`diff.added`。
  - rebase 的 `dim` 不再乘透明度；Git 設定頁停用列的透明度至少 0.75。
  - 用 yuzora-light 在隔離環境實機截圖：graph、ref chip、信任按鈕各一張。
- **Codex 對 `9e0c88b0` 的 6 則 P2**。前 3 則跟信任或讀取範圍有關，優先處理：
  - `git_trust.rs`：未信任的 repo 的 remote URL 是 `ext::` 等會執行程式的形式，而使用者全域設定允許 `protocol.ext` 時，自動 fetch 會執行它。repo 本地的 `protocol.*.allow` 已會先等信任。修法：未信任時，自動 fetch 擋下會執行程式的 remote URL，或把它列入信任條件。
  - `log.rs`：依路徑篩選的 log 在未信任的 partial clone 上可能 lazy fetch。修法：log 的讀取設 `GIT_NO_LAZY_FETCH=1`。
  - `status.rs`：計算 untracked 行數時會跟著 symlink 讀到目標檔案（可能在工作樹外）。修法：用 `symlink_metadata`，symlink 計算目標字串本身。
  - `merge.rs`：衝突已在外面解決後，選了不存在的一側會被當成刪除去跑 `git rm`。修法：先確認路徑仍有 unmerged stage，否則要求重新載入。
  - `repo.rs`：工作樹路徑含換行時，`rev-parse` 的輸出用 `lines()` 拆會錯。修法：改用不會出現在路徑裡的分隔方式，或分開查詢。
  - `ops.rs`：submodule 路徑含空白時，`split_whitespace` 會拆錯。修法：改從保留路徑原樣的來源讀（index 的 gitlink 或 `.gitmodules`）。
