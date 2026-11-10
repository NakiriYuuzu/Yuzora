# L2 · Git

> beta.4 起，Files／Git／Database／SSH 是右側 dock 的模式（`panel` 右邊的模式列，沒有 panel 的 layout 是抽屜），不再是左側欄。本文原稱「側欄」「側欄模式」的地方已改稱「右側模式」；截圖與 beta.2／3 時期的設計描述（例如圖中的左側欄）未重寫。

beta.2 的 Git 以 JetBrains IntelliJ IDEA Community 的 Git 整合為功能基準（研究：[research/intellij-git-research.md](research/intellij-git-research.md)），採 **staging area 模式**：沒有 changelists、沒有 shelf，也不做 GitHub／GitLab 整合。持久化 log index 不在 beta.2 範圍，但留了接縫（見〈Log〉）。

- 原生層：`app/crates/yuzora-git`（JS `import … from "yuzora:git"`），全部 git 工作都在這裡，不在 UI 執行緒。
- JS mods：`app/defaults/mods/yuzora-sidebar-git`（右側模式 Git 模式）與 `yuzora-git-*`（tool pane、popup、對話框、設定頁）。

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
| `yuzora-sidebar-git` | 右側模式 | 分支按鈕（↑↓，開分支 popup）、操作中橫幅（繼續／中止／略過）、衝突／已暫存／變更三段清單（數量、全部暫存／取消、列尾 +／−／↺）、右鍵選單、commit 訊息（歷史、範本、amend、sign-off、略過 hooks、作者）、Commit／Commit 並 Push／Push ↑N、「分支圖」；頂端的 repo 選擇器（跟隨焦點，或替這個 Space 釘選一個 repo，見〈同一個 Space 的多個 repo〉）。寫 `yuzora.badge.git`、`yuzora.status.git` |
| `yuzora-git-branches` | popup | 搜尋、更新專案、Fetch、Push…、新分支、checkout revision、stash；最近／本地／遠端／tags；每個 ref 的動作（checkout、smart／強制 checkout、從此建分支、比較、merge／rebase 進目前分支、改名、upstream、刪除、tag push／刪除） |
| `yuzora-git-push` | 對話框 | 目標 remote／分支、要送出的 commits、force（lease／force，受保護分支禁止）、tags（none／follow／all）、略過 hooks、新分支自動 set-upstream、被拒時依設定自動更新 |
| `yuzora-git-diff` | modal 或 tool pane | 預設開成 modal（dialog overlay），「保留」把它移到 pane（預設分割在目前 pane 右側，可改成新分頁）；並排（左右各半，中間分隔線可拖，長行橫向捲動）／合併、字詞高亮、空白（精確／忽略頭尾／全部忽略）、context（3 行／整檔）、hunk 導覽、暫存／取消暫存／捨棄 hunk 或選取的行、選取列（點、Shift＋點、拖曳）後複製（⌘C／右鍵：複製行、hunk、patch）、Annotate、歷史；即時更新 |
| `yuzora-git-log` | tool pane | 提交紀錄＋分支圖（`GraphCell`）、篩選、詳情、右鍵選單、分支比較、檔案歷史 |
| `yuzora-git-merge` | tool pane | 衝突檔案清單、整檔接受 yours／theirs、三方合併（套用非衝突、自動解決簡單衝突、逐塊選擇或手動編輯） |
| `yuzora-git-rebase` | tool pane | 互動式 rebase 編輯器（pick／reword／edit／squash／fixup／drop、拖曳排序、update-ref）、記憶體內改寫訊息 |
| `yuzora-git-blame` | tool pane | Annotate：gutter、上一版 annotate、與上一版比較、在 log 中顯示 |
| `yuzora-git-repo` | tool pane | Stashes、Tags、Remotes、Worktrees（新增交給上游 `worktree` 對話框）、Submodules、Patches、身分與簽章 |
| `yuzora-git-settings` | 設定頁 | Yuzora `settings.json` 的 `git` 區段（見 A12） |

### pane 之間的約定

- 每個 mod 用 `open(cwd, on)` 取得 repo（同一個 `<machine>:<root>` 只開一次，watcher 也只有一個），之後的函式都帶 `repo.id`。
- 找 pane 只在**目前顯示的 Space** 的分頁裡找：`panes()` 列出每個 Space 的 pane，而 `pane.focus` 會切到那個 pane 所在的 Space，在 Space B 找到 Space A 的 pane 就會跳回 A。判斷收成幾個函式（`shownSpace`、`findPane`、`requestKey`、`requested`、`openRequested`，以及 log 的 `findLog`、`claimLog`），寫在每個用到它們的 Git mod 的 `main.js` 開頭（一個 mod 不能 import 另一個 mod 的檔案，也不新增 mod 資料夾裡的共用檔，因為 `src/mods/lifecycle_tests.rs` 把 `main.js` 單獨放進暫存資料夾載入）。`scripts/yuzora-git-pane-reuse.test.ts` 守住同名函式各處逐字相同、呼叫它們的 mod 都有它們；並且守住 `panes` 這個名字在 Git mod 裡只出現在 `findPane`、`findLog` 與兩個不用 mod 名稱查找的讀取（依 pane id、列出目前 Space 各 pane 的資料夾），不管查找怎麼寫（跨行、解構、先存進變數都擋）；新增的讀取要先確認它查的是什麼，再加進測試的清單。每個 opener 的行為另由測試用「每個 Git mod 在別的 Space 各有一個 pane」的假 Herdr 逐一驗證，不會對它們 `pane.focus`。
- diff、merge、rebase 每個 Space 一個 pane，用 shared key 交棒，而且**請求按 Space 分開存**：開啟方寫 `yuzora.git.diff.<space id>`（`{cwd, on, path, orig, mode, rev?, to?, at}`）、`yuzora.git.merge.<space id>`、`yuzora.git.rebase.<space id>`，pane 只讀自己 Space 的那一格（沿用 `yuzora.git.repo.<space id>` 的做法）。別的 Space 發的請求不會蓋掉它，所以回到這個 Space 時，pane 還是它最後一次被要求的檔案，選取的行與目前的 hunk 也都在。開啟方在目前 Space 找到該 pane 才寫入並 `pane.focus`；找不到就 `tab.new {mod, space, props}`，不寫 shared。**不帶 Space 的請求**（agent 或腳本直接 `setShared("yuzora.git.diff", {…})`，key 不加 Space）照舊可用：每個 Space 的 pane 都跟隨，它和該 Space 自己那一格取較新的（`at`）。它是「這一次執行期間發出的指令」，不是跨重啟保存的狀態：shared 會存檔，beta.4 又把每一筆請求都寫在這個 key，升級後它不會再被覆蓋，所以**每次執行第一次有 pane 讀請求時，先清掉上一次執行留下的三種不帶 Space 的請求**（`yuzora-git-{diff,merge,rebase}` 各帶同一份 `retirePlain`，以 `yuzora:core` 的 `session()` 記在 shared `yuzora.git.run`；自己 Space 那一格不動）。清掉之後寫的才會被跟隨，有沒有 `at` 都一樣；在這次執行第一個 Git pane 畫出來之前寫的，也會被清掉。要跨重啟保留的請求寫在 `<space id>` 的 key。pane 依 `at` 取較新的請求；props 沒帶 `at` 的 pane（agent 用 `tab.new --props` 開的）以開啟時間計，不會被更早留下的請求蓋過。merge、rebase 的 pane 在第一次畫出來時才讀請求（`init` 跑在快照發布之前，見〈已知限制〉）：props 沒有 `cwd` 的 pane（從「新增 pane」選單開的、重啟後還原的）拿自己 Space 最後一次的請求，沒有就是空的；不會拿到別的 Space 的，也不會拿到上一次執行留下的不帶 Space 的請求（升級自 beta.4 的使用者，shared.json 裡那一筆在第一次讀時就被清掉）。
- log 的分支圖用 `(Space, repo 根目錄, machine)` 判斷是不是同一個：開啟方（Git 側欄的「分支圖」）讀不到 mod pane 的 props，所以每個分支圖 pane 把自己顯示的 repo 登記到 shared `yuzora.git.logs`。**登記表以分頁為鍵**：`{ [tab id]: { space, root, on } }`，每個 pane 只寫自己那一格。`root` 是 `open()` 回傳的 repo 根目錄，不是 props 的 cwd（結尾斜線、子資料夾、agent 用 `tab.new --props` 開的都對得上）；登記在 repo 開好、而且第一次畫出來（才知道自己的分頁，不是 init，見〈已知限制〉）之後。這一格是「這個分頁裡的圖顯示哪個 repo」的事實，分頁還在就一直成立，所以 **pane 關閉不取消登記**：Cmd+Q 會對每個已載入的 pane 呼叫 `close()`，而重啟後不在畫面上的圖分頁沒有 pane 在跑、不會重新登記，開啟方靠的就是這一格還在 shared.json。開啟方在目前 Space 的分頁裡找 mod 是 log、而且登記的 `(root, on)` 相符的 pane，找到才 `pane.focus`，否則在目前 Space `tab.new`。同一個 repo 開了兩個圖時各有自己的格子，其中一個關掉不影響另一個；同一個 Space 換了 repo 會另開一個那個 repo 的圖，不會聚焦舊的；同一個資料夾在另一台 machine 是另一個 repo。寫入時順便清掉分頁已不存在於任何 Space 的格子；遠端 machine 的 Space 要連線後才列出（Space id 帶 machine 前綴），所以它們的格子在 Space 還沒列出時不清。檔案歷史、檔案比較、依 commit 篩選的 log pane（props 有 `filter` 或 `compare`）不登記，不會被當成分支圖。blame、repo pane 用 props，可以同時開多個。
- diff 例外，依 Git 設定開啟（右側模式、log 詳情、blame、repo pane 四個入口各帶一份相同的 `showDiff`／`placeDiff`，`scripts/yuzora-git-diff-open.test.ts` 守住四份一致、每個 mod 都 import 了它們呼叫的函式，以及各種設定）：
  - 已有 diff pane 且 `diffFollowPane`（預設開）：照上面的 shared key 交棒。
  - 否則 `diffOpenIn: "modal"`（預設）：`openOverlay({mod: "yuzora-git-diff", kind: "dialog", width: 0.9, height: 0.86, props})`。modal 只看自己的 props，不跟 shared key；Esc、✕ 或點旁邊關閉；Annotate、歷史開新分頁後也關閉。
  - `diffOpenIn: "pane"`：直接開在「保留」會放的位置。
  - 「保留」：依 `diffKeepIn`，`split`（預設）在焦點 pane 右側 `pane.split {direction: "right", mod, props}`，沒有焦點 pane 或設成 `tab` 時 `tab.new`；新 pane 帶著 modal 裡的檢視（並排／合併、空白、context），然後關閉 modal。`run()` 只把指令排進佇列、不回傳錯誤，所以 split 或 `tab.new` 失敗時 modal 照樣關閉，pane 也不會出現。
- 即時更新：`changed(repo, since)` 在 repo 的 generation 變了（watcher 或我們自己的寫入）時 resolve；遠端 repo 沒有 watcher，約 25 秒逾時一次，呼叫端藉此輪詢。本機 repo 的 watcher 啟動失敗時（例如系統的監看數用完），等待逾時就把整個 repo 與 refs 視為已變更，generation 前進，各 pane 照常重讀。逾時由一條共用的計時執行緒負責，不靠 watcher 的 tick（只在有人等待時存在）。
- `changes(repo, since)`：同 `changed`，另外回傳這段期間變動的路徑（`{generation, paths}`）。路徑來自 watcher 看到的工作樹，以及我們自己的 stage／unstage／discard 指定的路徑；無法判斷時 `paths` 是 `null`，包括 git 自己寫的 index 或 refs、rescan、整個 status 失效，以及一次超過 1000 個路徑，或已經超出保留範圍（repo 保留最近 256 個 generation；每個 generation 的路徑去重）。diff pane 只在變動可能碰到它的檔案時重讀：檔案本身、改名前的檔案、上層資料夾（`.` 是頂層）、檔案底下的路徑（submodule）、任何 `.gitattributes`。watcher、git 與 pathspec 對同一個檔案的寫法可能不同，所以兩邊都先換成同一種寫法再比對：`\` 當成 `/`（watcher 在每個系統上都這樣寫）、不分大小寫、轉成 NFC（macOS 交出的是磁碟上的名稱，常是 NFD；git 存的是 NFC）、去掉開頭的 `./` 與結尾的 `/`。含 `~` 加數字的路徑可能是 Windows 的 8.3 短檔名（watcher 可能回報長或短的名稱），和 `paths` 是 `null` 一樣一律重讀。staged diff 也會因為同一個檔案的工作樹變動重讀：watcher 回報的路徑分不出是工作樹還是 index。commit 的 diff 不跟隨。
- 跨 lane：`yuzora.badge.git`＝變更檔案數（乾淨時 `null`），`yuzora.status.git`＝`{text: "⎇ main ↑2 · 4 變更", tone}`，tone：衝突 `blocked`、操作中或執行中 `working`、有變更 `accent`、乾淨 `muted`。兩者都是右側模式目前選中的 repo 的值（跟隨焦點時就是焦點所在的 repo）。
- `yuzora.git.repo.<space id>`＝這個 Space 釘選的 repo（`{root, on}`），沒有就是跟隨焦點；見下一節。

### 同一個 Space 的多個 repo（beta.3）

情境：同一個 Space 裡有好幾個 repo，例如外層資料夾放了好幾個專案，或 monorepo 裡的子 repo。不跨 Space，也不做 submodule／worktree 的切換。

- 右側模式頂端的 repo 選擇器：預設「跟隨焦點」，和以前一樣看焦點 pane 的資料夾（否則 Space 的資料夾，`location()`）；也可以替這個 Space 釘選一個 repo，之後焦點怎麼換都看那個 repo。每個 Space 各自記住選擇：存在 shared 的 `yuzora.git.repo.<space id>`（`{root, on}`），shared 會存檔，所以重啟後仍然有效；選「跟隨焦點」就清掉。選擇器標示目前的 repo，釘選時名稱前有一個點。
- 鍵盤：指令 `yuzora-sidebar-git.pick-repo`（和其他 mod 指令一樣用點號寫進 `keys.json`，或 `herdr-app do`）把選擇器置中打開。選擇器裡 ↑↓ 在「跟隨焦點」與各 repo 之間移動（頭尾相接，一開始停在目前選的那一列），Enter 選定，Esc 關閉。和分支 popup 一樣，選到的列不會自動捲進畫面。
- 跟著選中的 repo：右側模式本身、`yuzora.badge.git`、`yuzora.status.git`，以及從右側模式開的 Git Log、diff、分支 popup、Push 對話框、merge（都用右側模式的 `repo`）。狀態列底部的 git 摘要不在範圍內，仍跟著焦點的終端機。
- 列出 repo：`repos(folder, on, cwds)`（`repos.rs`），只讀檔案系統，不跑 git，所以不論信任與否都不會執行 repo 設定的程式。列出的順序：
  1. Space 資料夾所在的 repo：資料夾本身，或往上最近一個有 `.git` 的資料夾（照真實路徑，和 git 往上找的方式相同；Space 資料夾是某個 repo 的子資料夾時就是那個 repo）。
  2. Space 內各 pane 的資料夾所在的 repo（同樣往上找，可以在 Space 資料夾外面）。
  3. Space 資料夾往下一層、兩層、含 `.git` 的資料夾（`.git` 是資料夾或 gitfile，指向它們的 symlink 也算；FIFO 之類的不算），依路徑的位元組排序。
  - 不進入 `.git`、`node_modules`、`target`。指向資料夾的 symlink 會跟著走：最多兩層，所以指回上層的 symlink 自然停住；同一個 repo 照真實路徑只列一次。
  - 名稱含換行的資料夾不列，也不進入。資料夾只看真實路徑，真實路徑裡有換行的（pane 的資料夾、經過這種資料夾的 symlink）也不算，Space 資料夾本身有換行就是問不到。原因是遠端的 `find` 一行印一個名稱：名稱一切開，後半段會被當成路徑，相對路徑落在遠端 home（`hc exec --on` 在那裡執行），就會列出 Space 外的 repo。所以 `find` 以 `-name "*<LF>*"` prune 掉這些名稱，Rust 照樣略過，兩邊連計數都相同。
  - 最多 50 個，超過時 `truncated`，選擇器顯示「只列出前 N 個」。往下看的資料夾最多 5000 個：依檔案系統列出的順序走（每個資料夾之後緊接著它底下的，和 `find` 走訪的順序相同），看滿就不再讀後面的資料夾，同樣 `truncated`；看過的這些才依位元組排序。所以超過 5000 個時，列出哪些取決於檔案系統的順序，不一定是路徑排序的前 5000 個。
  - 每一筆有 `root`（真實路徑，git 的寫法；Windows 去掉 `\\?\`、用 `/`）、`name`、`rel`（`.`、Space 資料夾底下的路徑，否則就是 `root`）、`trusted`（已經在信任清單裡；不跑 git 就只知道這麼多，所以「不在清單」只代表還沒信任過，不代表需要信任，選擇器只標「已信任」）。
  - 遠端 Space：同樣的規則寫成 POSIX sh（`repos::SCRIPT`），經 `hc exec --on` 一次呼叫完成，不會每個資料夾來回一次。`repos::tests` 在本機用 sh 跑這個腳本，每個情境都要和 Rust 的走訪結果完全相同；換行名稱的情境讓腳本在一個底下有對應 repo 的資料夾裡執行（模擬遠端 home），特殊名稱（空白、引號、`$(…)`、反引號、反斜線、`-` 開頭、`*`、`[ab]`、`;`、printf 格式、tab，以及 pane 的資料夾是 `-`、`--`）都要列成名稱本身，不能執行任何東西。
- 信任：每個 repo 各自過原本的信任關卡。只有選中的 repo 會跑 status（以及信任掃描）；列出 repo 不會對任何一個跑 git。
- 釘選的 repo 消失：打開它失敗、之後 status 失敗，或打開的不是它（見下一點）時，再問一次檔案系統（`repos(root, on, [])`）。確定不見才取消釘選，跳出提示，改回跟隨焦點。確定不見是指：那個資料夾還在、可以進入，但已經沒有 `.git`；或資料夾本身（或路上某一層）不存在而它的上一層可以進入；或那一層不是資料夾。問不到時保留釘選並顯示錯誤，例如遠端機器離線、沒有權限進入那個資料夾或它的上層、路上有解析不了的 symlink（可能指向沒有權限的資料夾）。這些情況下 `repos` 回傳錯誤，不回空清單，遠端的 sh 也一樣（以 `exit 1` 回報，不印 `gone`）。`.git` 還在（錯誤是別的原因）時也保留釘選。
- 打開成功也要是同一個 repo：資料夾的 `.git` 沒了，git 會往上打開外層的 repo，`open` 照樣成功。所以釘選時 `open` 回傳的 `root` 不是釘選的 `root`，就照上一點問檔案系統：確定沒有 `.git` 就取消釘選。`.git` 還在但 git 不接受（例如讀不到），或問不到時，保留釘選並顯示「git 在這裡打開的是 …」，不改用外層的 repo。git 的寫法和釘選的不同、其實是同一個資料夾時（例如 Windows 的 subst 磁碟機：`canonicalize` 解析成實際位置，git 可能保留磁碟機代號），先問 `repos(git 的 root, on, [])`：它的頂層真實路徑就是釘選的 `root`，就照常打開。Windows 上這一點沒有實測。
- 已打開的 repo 只在它的頂層找 `.git`：yuzora-git 對已打開的 repo 跑的每個 git 指令，都帶 `GIT_CEILING_DIRECTORIES=<root 的上一層>`。本機和遠端（`hc exec --env`）都一樣；`open` 從資料夾往上找 repo 時不帶。
  - 正常情況下行為不變：git 在 root 就找到 `.git`。
  - `.git` 在 repo 開著時不見了：git 不會往上找到外層 repo。status 會回錯誤，或者信任掃描讀不到這個 repo 的設定，改成詢問（`(unreadable config)`）；commit 等寫入都會失敗。不會頂著這個 repo 的名稱去讀寫外層 repo，跟隨焦點和釘選都一樣。
  - 釘選的 repo 在 status 失敗時，照前面的規則取消釘選；出現信任提示的話，信任之後 status 失敗，再取消釘選。
  - `GIT_CEILING_DIRECTORIES` 是清單，Windows 上的 git 用 `;` 分隔，其他系統用 `:`。上一層的路徑裡有分隔字元時，git 會把它切開，限制就失效了。yuzora-git 不判斷跑的是哪一種 git（遠端可能和本機不同），所以上一層的路徑裡只要有 `:` 或 `;`（開頭的磁碟機代號，例如 `C:`，不算），就**不帶任何環境變數**（不設 ceiling，也**絕不**用 `GIT_DIR`／`GIT_WORK_TREE` 代替：hook 會繼承它們，見下）。遠端是 Windows 時，`C:` 是磁碟機代號，所以照常用 ceiling（`C:/x/repo` → `C:/x`，`C:/repo` → `C:/`，`//host/share/repo` → `//host/share`）。上一層用字串算出，只在 `/` 切開（git 輸出的 root 一律用 `/`，Windows 也是；unix 的資料夾名稱可以含 `\`，不能當分隔字元，`…/re\po` 的上一層是 `…`），本機、遠端、unix、Windows 的路徑都用同一套規則，`exec::tests::a_top_place_is_kept_in_its_repo_whichever_git_runs` 列出這些情況（含 `re\po`），所有平台都跑。
  - 沒有 ceiling 時的補救：上一層含分隔字元的 repo（`needs_git_check`），**每一個** git 指令（讀取、寫入都一樣，包括 fetch、push、tag push、刪除遠端 tag、status）在執行前，先問 git 自己：同樣的資料夾、同樣不帶 ceiling 的環境，跑 `git rev-parse --show-toplevel --absolute-git-dir`（`--absolute-git-dir` 自 git 2.13，不用要 2.31 的 `--path-format`），兩個回答都要對：work tree 必須就是 root，**而且** git dir 必須就是 `open` 時記下的那個（`Place.git_dir`，linked worktree 是 `.git/worktrees/<n>`、submodule 是 `.git/modules/<n>`；`open` 也用 `--absolute-git-dir` 取得，兩邊同一個寫法）。只比 work tree 不夠：外層 repo 的 `core.worktree` 指到 root（絕對、相對 `../a:b/inner`、大小寫不同都行）時，git 回答的 work tree 就是 root，指令卻會在外層 repo 跑（它的 HEAD、它的 hook）；`.git` 換成指向外層 git dir 的 gitfile 也一樣，只有 git dir 能分辨。比較：本機比真實路徑（`canonicalize`，APFS 這類不分大小寫的檔案系統回傳磁碟上實際的大小寫），遠端照 git 寫的字串比；處理結尾 `/`、Windows 的 `C:/`、`\\?\`、`\`；**只有 Windows 形式的路徑（磁碟機代號或 `//` 開頭）忽略大小寫**，unix 路徑大小寫敏感（大小寫敏感的檔案系統上 `/srv/Repo` 與 `/srv/repo` 是兩個資料夾）。不是（git 失敗、work tree 或 git dir 不符）就回傳 `git finds no repository at <root> (…): git is not run there …`，不執行原本的指令（fail closed）。不再問 `.git` 存不存在：git 不接受的 `.git`（空資料夾、空的或不見的 `HEAD`、沒有 `objects`／`refs`）會被略過而往上找，只有 git 自己的回答算數。fetch、push 這類在引擎裡是讀取的指令，會寫 refs、跑 hook、連 remote，status 會跑 clean filter，所以檢查和寫入鎖脫鉤；寫入指令仍在拿到 repo 寫入鎖之後才檢查。檢查和執行是兩次分開的 git 啟動（遠端是兩次 `hc exec`，本機是兩次程式啟動），所以兩者之間有時間差：這段時間內 `.git` 被拿掉，指令仍會往上找到外層 repo，本機也一樣。寫入鎖只排開引擎自己的寫入（寫入的檢查在拿到鎖之後，`exec::tests::a_write_asks_git_which_repo_it_finds_only_after_it_has_the_write_lock` 守住），擋不住外部（終端機、別的程式）在這段時間刪掉 `.git`；ceiling 沒有這個時間差，所以只有上一層含分隔字元的 repo 需要這個檢查。檢查帶著原指令的取消與逾時（逾時最多 60 秒），機器不回應時取消照樣返回。有 ceiling 的 repo 不需要這個檢查。
  - `rev-parse --show-toplevel --absolute-git-dir` 不執行 repo 自帶的程式：它只讀設定裡的路徑（`core.worktree`、`gitdir`），不讀 index、不跑 filter／hook／`core.fsmonitor`、不連 remote（`core.sshCommand` 用不到）。只有讀取指令帶 `core.fsmonitor=false`（`base_args`），這個檢查不帶：它不需要，因為它不碰 index；它會讀整份設定（包含 `include` 的檔案），include 指到 FIFO 之類會卡住，直到逾時（最多 60 秒）或取消，但不會執行任何程式。`exec::tests::with_a_colon_above_no_command_reaches_the_repo_around_an_open_one` 的外層 repo 設了 clean filter、uploadpack、pre-push hook，檢查本身不會觸發任何一個。
  - 已知的例外：`core.worktree` 指到 `.git` 所在資料夾的子資料夾（很少見）時，root 是那個子資料夾，上一層就是 `.git` 所在的資料夾；上一層不含分隔字元時 ceiling 擋住 git 往上找 `.git`，指令會失敗（沒有 ceiling 的版本能正常運作）。這種 repo 目前不支援；上一層含分隔字元時不受影響（沒有 ceiling，檢查通過）。
  - 範圍：只有 yuzora-git 引擎的指令。上游的 `changes` mod 直接 `exec("git")`，另外 `worktree`、`spaces`、`menu`、`files` mod 與 `yuzora-files` 的檔案樹標記、狀態列也直接跑 git，不經過引擎，沒有 ceiling、也沒有這個檢查（既有行為，不在這個 PR 的範圍）。
  - 代價：hook 只繼承 ceiling（一般 repo 本來就沒有 `GIT_DIR`／`GIT_WORK_TREE`，這裡也不加）。hook 如果在這個 repo 的上一層底下的其他資料夾（例如 `w/libs/R` 的 hook 在 `w/libs/other` 跑 git；上一層 `w/libs` 本身、外層 repo 的其他資料夾 `w/app` 都找得到）跑 git，會找不到外層 repo；hook 換到別的 repo 或 `cd frontend && git add -u` 這類寫法都不受影響。曾經用 `GIT_DIR`／`GIT_WORK_TREE` 當退路，被獨立審查擋下：hook 繼承後在別處跑 git 會對著這個 repo，monorepo hook 的 `git add -u` 甚至讓 commit 刪光所有檔案。測試：`exec::tests::a_hook_gets_the_ceiling_and_never_a_git_dir_and_a_monorepo_hook_deletes_nothing`。linked worktree 的 hook 本來就拿得到 git 自己設的 `GIT_DIR`，與這裡無關。
- 和總指揮提案不同的地方：
  - 多列了 Space 資料夾所在的 repo（往上找）：Space 資料夾是 repo 的子資料夾時，跟隨焦點本來就會打開它，選擇器裡沒有它就切不回去。
  - symlink 不跳過而是跟著走兩層：指向別處的專案資料夾也列得出來；兩層的限制就足以擋住迴圈。
  - 另外限制往下看的資料夾數（5000）：走訪時就停，子資料夾上萬的 Space 資料夾不會讓列表變慢（遠端的 `find` 也接在 `head -n 5001` 後面：`head` 讀滿就結束，`find` 下一次寫出時跟著停）。

### Git 模式的位置、記住的 repo 與動作的對象（v0.1.0 E1）

使用者回報「切換專案時 Git 模式要等」。偵察與實測（本機兩個小 repo 之間切換，截圖取樣約 60 ms）：切換後 Git 模式換成新內容要 153–1047 ms，均勻散開；原因是它每 1 秒才比一次「現在看哪裡」，其次才是 attach 的行程數，而且 attach 一開始就把清單清成空白。

- **位置在畫出來的那一幀就看**：`render()` 比對 `key()` 與 `this.ctx`，不同就 `attach`。herdr 在 `spaces`、`panes`、`focused` 或 shared 變動時重畫讀過它們的 mod，所以 Space 或焦點 pane 的資料夾一變，下一個畫面就開始換，不等計時器。1 秒計時器留給沒有畫出來的 copy（切到別的模式時，徽章與 `yuzora.status.git` 仍要跟著焦點）。狀態列的 git 區塊同樣：第一次畫到一個資料夾就問，不等 2 秒計時器；每個資料夾各有自己的詢問，慢的那個不擋新的（以前全部共用一個 `busy`）；計時器只剩「每 15 秒再問一次」。
- **每個 repo 一份視圖**（`repoView`，`main.js` 最上面）：最後讀到的 status、這個 repo 的 commit 草稿（訊息、amend、作者）、動作的 `note`／`busy`／`warnings`、確認中的 discard、選取列、收合的段落、訊息歷史。畫面上的是 `this.v`；以 `repo.id` 為鍵，最近用過的 8 個留著，`seen` 記住 64 個位置各是哪個 repo。**手上有東西的視圖不被擠掉**（`holds`）：打著的 commit 訊息（只是 commit template 的不算）、作者、勾著的 amend、等著回答的警告（Commit anyway）、進行中的動作、動作失敗留下的錯誤訊息（`note` 的 `tone` 是 `error`，例如切走之後被拒的 push；下一次動作開頭會清掉，不會永久釘住）；所以留著的視圖數不設上限，但只有在使用者手上有未完成的事時才超過 8。
- **顯示規則**：位置以前看過 → 同一個畫面先用那個 repo 上次的狀態，背景照原本的路徑（`open`→`status`→`messages`→`changed`）再讀一次；沒看過 → 在 `open` 回答它是哪個 repo 之前，畫面不動，所以 `cd` 進同一個 repo 的子資料夾、或焦點移到同一個 repo 的另一個 pane，清單不重置也不重讀；回答是另一個 repo → 換成它的視圖（沒讀過就是「讀取中」），不是 repo → 錯誤畫面。只有畫面上的 repo 被追蹤變動（`changed`）；離開的 repo 不再讀。看過的 repo 重新顯示時，第一次重讀就失敗（資料夾被刪、repo 壞了）→ 丟掉記住的清單改顯示錯誤，不再把已經不能確認的清單留在畫面上；之後 `changed` 觸發的重讀失敗則保留清單並顯示錯誤，和以前一樣。

  **記住的清單先看、讀過才動手**（`v.confirmed`）：一個視圖每次被放上畫面（`show`）就是「未確認」，直到「放上畫面之後才開始」的 status 讀取回來（`reload`；上一次造訪晚到的讀取不算，以 `v.shown` 的造訪次數區分）。未確認時清單與徽章、狀態列照樣同一幀顯示（量到的即時感不變，畫面上的像素和確認後完全相同），**能看的照常能點**（開 diff、圖、分支、收合、選取、歷史訊息、勾選項），**會改 repo 或詢問要不要改的不動作**：列上的 ＋／−／↺、Stage all／Unstage all、Commit、Commit and push、Push、Commit anyway、操作列的 Continue／Abort／Skip、信任按鈕、右鍵選單、自動 fetch。取捨：舊清單可能已不是真的（回訪前檔案被改、repo 現在要信任），在 status 回來之前，點到的 ＋ 不會 stage 一個畫面上看不到的新狀態，也不會有人在一份舊的 trust 提示上按下「信任」；代價是本機約 20–130 ms（遠端到一次往返加 status）內的點擊被忽略，和位置切換的那一幀一樣是寧可沒反應。status 一回來帶著 `trust`，下一幀就換成信任提示；寫入在引擎端本來就先經 `trust::check`，這一層沒有動它。徽章與 `yuzora.status.git` 在這段時間顯示記住的值（和清單同一份資料）。**確認只等於「引擎的快取在這次讀取時是這樣」，不等於「磁碟現在是這樣」**（沒修，在引擎端，要碰 watcher）：離開超過 120 s（`repo::IDLE`）後 watcher 會放開；回來時 `open` 的 `used()` 在另一條執行緒重啟它，`invalidate_refs()` 要等 `watch::start` 回來才標記為髒（閒置時 9–12 ms，機器忙時 E1 量到 0.4–6 s），這段時間內第一次 status 回的是離開前的快取，視圖卻已經是 `confirmed`；遠端的 repo 沒有 watcher，離開後也沒有人輪詢，第一次 status 同樣是離開前（最多 25 s 前）的快取（這一點以前就是這樣，只是以前先顯示「讀取中」）。之後 `changed` 把它補上。所以 `confirmed` 擋的是「還沒讀過就動手」，擋不了「讀到的是舊快取」；建議的修法在引擎：`used()` 決定重啟 watcher 時同步 `invalidate_refs()`（再讓啟動完成後的那一次照舊），遠端回訪的第一次讀取 `force`。會刪東西的動作（確認框的 Yes、右鍵選單的 Discard）另外處理，見下。

  **問的問題就是看到的問題**：Discard 的確認框不跨越切換（離開 repo 就取消，回來不再有）；確認框開著時下一次讀取如果顯示該檔案已變成另一種變更（例如被移出 index 成了未追蹤）或不再是變更，確認框取消（`discard` 在引擎端是照當下的狀態分流，未追蹤的檔會被整個刪掉，畫面上問的卻是 `M`）；確認框的 Yes 只捨棄它畫出來時問的那個檔案，而且按下 Yes 時先強制讀一次 git（`discardAsked`，一次完整的 refresh），那個檔案現在還是被問的那種變更才捨棄，否則什麼都不刪、顯示「已經不是你被問的那個變更」（`discard.changed`）：右鍵選單的 Discard（開著的 popup 自己 `open` 再 `discard`）也一樣：選單開啟時帶著該檔案被列為哪種變更（`kind`），按第二下確認後同樣先強制讀一次再決定。這兩個是會刪東西的路徑；上一段說的快取可能是舊的，而引擎的 `discard` 是照當下的狀態分流，未追蹤的檔會被整個刪掉。其他寫入不另外讀：stage、unstage 可逆；commit 本身作用在 git 的 index，但 `commitChecks` 用的是不強制重讀的 `status::refresh`（`ops.rs`），快取乾淨時什麼都不做，所以在上一段說的舊快取時窗裡，它的警告（large、crlf、`nothing`）可能依據舊快取：可能誤擋（`nothing`，安全的方向）或漏掉警告，不會提交錯的內容。Commit anyway 的警告框照舊跟著視圖（它是使用者剛按下 Commit 提出的問題，晚到也回到那個 repo），但要等確認才能按。
- **動作只作用在畫面上那份清單所屬的 repo**：列的 key 含 repo id（`rowKey`），畫面換成別的 repo 之後，在舊畫面上開始的點擊找不到列，什麼都不做（即使兩個 repo 有同名檔案）；**畫面上每一個可以點的東西都由 `on(v, fn)` 做出來**（`v` 是畫出這一幀的 repo）：窗口把點擊交給「上一個畫出的 frame」，而 `open` 回來可以在兩次繪製之間把另一個 repo 換上畫面，這時舊 frame 裡的 Commit、Push、Commit anyway、amend、操作列的 Abort、分支、圖、repo 選擇器都不做事；⌘Enter 交給最後畫出的 frame（`this.drawn`），同一個規則。每個可點的東西的 id 都含 repo id（`name:<repo id>`，由 `button`／`checkbox`／`operationBar` 與 render 裡的 `id()` 統一加上），一次按壓不會在換畫面後的另一個 repo 的同名控制項上放開而算作點擊。`act`／`commit`／`push`／`toggleAmend` 沒有預設的視圖參數，漏接 `v` 會丟例外而不是作用在畫面上的 repo。`act`／`commit`／`push` 在開始時綁定當時的視圖：進行中的 commit、push 切走之後結果（錯誤、警告、清空訊息）回到它自己的視圖，不會出現在新位置的畫面上；晚到的 `open`（位置已換）直接丟掉；晚到的 `status`、`messages` 寫進它的視圖，不碰畫面；徽章與 `yuzora.status.git` 只發畫面上那個 repo 的。切走之後的 Push 不擋別的 repo 的按鈕（`busy` 是各 repo 的）。
- **commit 訊息草稿**：以前整個側欄只有一個輸入框與一個作者欄，在 alpha 打的訊息切到 beta 之後按 Commit 會把它提交到 beta。現在草稿跟著 repo：換畫面時把現在框裡的存回離開的 repo，再把新 repo 的放進去。
- **沒做的**：`open` 的快取（`rev-parse` 每次位置變動都跑一次，要碰 `check_top` 與信任，歸 trust 審查）；沒畫出來的 copy 跟隨位置仍是最多 1 秒（徽章與狀態文字）；位置切換的那一幀裡剛好按下的點擊會被丟掉（寧可沒反應，也不作用到別的 repo）。
- 測試：`scripts/yuzora-git-sidebar.test.ts`（載入出貨的 `main.js`，每個 git 呼叫都等測試回答：位置跟隨、快取、`cd`、晚到的回應、點擊與按鈕的對象、訊息／作者草稿、進行中的 commit 與 push；v0.1.0 E1 r1：「畫出一幀、另一個 repo 在繪製之外換上畫面、再把那一幀的每個 handler 呼叫一遍」的全控制項掃描（merge 與 rebase 兩種操作列），確認沒有任何 git 呼叫、pane、popup、toast；每個可點的 id 含 repo id；未確認的回訪畫面做同樣的掃描，寫入類呼叫為 0，確認後才出現）、`scripts/yuzora-statusbar.test.ts`；`lifecycle_tests.rs` 的 `yuzora_git_sidebar_follows_its_place_in_the_frame_it_changes` 在真的 QuickJS 與視窗重畫下確認不靠計時器。

### virtual list 的限制

gpui-shell 不允許在 `v_virtual_list` 的 item renderer 裡註冊事件（會丟例外、整個清單不畫）。列的點擊走清單的 `on_item_click(key)`／`on_item_secondary_click(key, event)`；一列裡有多個可點區域（右側模式的 +／−／↺、diff 的行號）時，清單外層用 `on_mouse_down` 記下按下的 x，再在 `on_item_click` 判斷。

## 引擎

### 執行器（`exec.rs`）

- 只用 git CLI。每個指令都帶 `-c core.quotepath=false -c log.showSignature=false -c color.ui=false`；讀取另外帶 `GIT_OPTIONAL_LOCKS=0` 與 `-c diff.autoRefreshIndex=false`，不碰 `.git/index`，watcher 才不會被自己的讀取喚醒。
- 環境：`LANGUAGE=` 清空、`LC_ALL=en_US.UTF-8`（macOS 本機）或 `C.UTF-8`，`GIT_TERMINAL_PROMPT=0`、`GIT_EDITOR=:`、`GIT_MERGE_AUTOEDIT=no`，輸出才能穩定解析，也不會卡在互動輸入。
- 讀取一律帶 `-c core.fsmonitor=false`：右側模式一聚焦資料夾就自動跑 status，repo 自己的 `.git/config` 不能藉 fsmonitor hook 在使用者沒有任何動作時執行程式（`status_never_runs_the_repos_fsmonitor_program` 守住）。代價是設定了 fsmonitor 的大 repo，背景 status 會慢一些。
- 其他由 repo 設定指定程式的讀取：commit 詳情的簽章檢查（`%G?`）把 repo 本地設定的 `gpg.program`／`gpg.{openpgp,ssh,x509}.program` 換回使用者全域／系統的值或 git 預設（`signature_programs`）；blame 帶 `--no-textconv`；status 的 numstat 帶 `--no-ext-diff --no-textconv`，建立 patch 的 diff 帶 `--no-ext-diff`。diff 讀 blob 用 `cat-file --filters`，會照 repo 設定跑 smudge filter（例如 git-lfs），只在使用者打開 diff 時發生，與 checkout 相同。
- 信任資料夾（`trust.rs`）：repo 自己的設定（`.git/config`、它 include 的檔案、worktree 設定）若指定了 git 會執行的程式（`filter.*.clean/smudge/process`、`core.sshCommand`／`gitProxy`／`askPass`／`alternateRefsCommand`、指向程式的 `core.fsmonitor`（`true` 是 git 自己的 daemon，不算）、credential helper、`remote.*.uploadpack/receivepack/vcs`、會執行 remote helper 的 remote URL（`remote.*.url`／`pushurl` 是 `<scheme>::…`，例如 `ext::` 會執行它的指令；或是 `<scheme>://…` 而 scheme 不是 git 自己處理的 file、git、ssh、git+ssh、ssh+git、http、https、ftp、ftps（大小寫照 git 比對，`HTTPS://` 也算），git 會交給 `git-remote-<scheme>`，例如 `ext://x/run` 會執行 repo 裡的 `ext:/x/run`；都照 git 的 `transport_get` 判斷；remote 的名稱本身是這種 URL 也算，不論是哪個 key，因為 remote 沒有 URL 時 git 拿名稱當 URL；放在需要 remote 名稱處的值也算：`branch.*.remote`／`pushRemote`、`remote.pushDefault`、`remotes.<group>` 的成員、`extensions.partialClone`；`submodule.*.url` 也算；`url.*.insteadOf`／`pushInsteadOf` 的替換字串只要接上 URL 其餘部分就可能成為這種 URL 也算，例如 `ext:` 接上 `:sh …`）、`protocol.*.allow`、`merge.*.driver`、`diff.external`、`diff.*.command/textconv`、`gpg.*program`、`gpg.ssh.defaultKeyCommand`、`!` 開頭的 `submodule.*.update`；另外 common dir 的 `remotes/`、`branches/` 裡有任何檔案也算，git 除了設定之外也從這兩處讀 remote；`modules/` 底下每個 git dir 的設定與它 common dir 的 `remotes/`、`branches/` 也算（巢狀的也算，git dir 與 common dir 的 `modules/` 都讀。哪些資料夾是 git dir 由 git 判斷：有 `HEAD`，再加上 `commondir` 或 `objects/`＋`refs/` 的資料夾，都以 `GIT_DIR` 交給 git 讀設定；這是 git 的 `is_git_directory` 接受一個資料夾的必要條件，而且照檔案系統開啟的方式判斷，不分大小寫時 `head` 也算，指向不存在 ref 的 symlink 也算。`modules/` 底下開頭是 `gitdir: ` 的檔案（gitfile，或指向它的 symlink；git 不讀超過 1 MiB 的 gitfile，掃描看到 2 MiB）也交給 git：git 在自己的 process 裡打開 submodule 時會跟著它走（例如 push 檢查 submodule 的 commit），所以讀它指向的 git dir，以及那個 git dir 自己的 `modules/`。git 不接受的當成讀不到。另外 `submodule.<name>.gitdir`（git 2.54 的 `extensions.submodulePathConfig`）可以把 git dir 放在任何地方，這個 key 本身就要等信任），以 `.git/modules/<name>: key` 列出（linked worktree 有自己的一份，`.git/worktrees/<id>/modules/<name>`）：未 checkout 的 submodule 把 git dir 留在這裡，`submodule update --init` 會重用它，checkout 時跑它的 filter，fetch 時用它的 `core.sshCommand`／`gitProxy`；已 checkout 的照原本的 gitlink 名稱只列一次），`status` 不跑 git，只回傳已知內容和 `trust`（那些 key）；右側模式顯示「要信任這個資料夾嗎？」，按下後記在 `<app home>/yuzora/git/trusted.json`。自動 fetch 由引擎擋（`fetch {auto: true}` 在有待信任項目時才直接拒絕），而且一律不跑 hook（`core.hooksPath` 指向 app 自己的空目錄；fetch 會觸發 reference-transaction）、不遞迴 submodule（`--no-recurse-submodules`）。使用者自己按的 fetch、pull 在信任前也加 `--no-recurse-submodules`，不遞迴抓取或更新 submodule，push 加 `--recurse-submodules=no`，不遞迴檢查或推送 submodule。pull 合併時仍可能讀 submodule：這裡的安全性靠的是先前的完整信任掃描，不是這些旗標。信任後照 git 自己的設定遞迴。會連到 remote 的指令（fetch、pull、push、刪除遠端分支或 tag、推 tag、submodule update）在資料夾信任前經 `trust::reach`：`GIT_ALLOW_PROTOCOL` 只列 git 自己的傳輸方式（file、git、http、https、ssh）裡使用者設定允許的，照 git 的順序判斷（`protocol.<name>.allow`、`protocol.allow`、預設值）；submodule update 的 URL 不是使用者給的，`file` 照 git 的預設不開。remote helper 一律不用。這一層只管傳輸方式：helper 叫不起來，但 filter、`core.sshCommand`、`gitProxy` 不經過傳輸方式，只能靠掃描事先擋。掃描看不到的 helper 寫法靠這一層擋：tracked 的 `.gitmodules`（包括 `--recursive` 途中遇到的巢狀 `.gitmodules`）、使用者自己的 `insteadOf` 改寫出來的 helper URL。submodule update 不另外要求信任（掃描沒有列出項目時就能跑）：它會重用 `.git/modules/<name>`，而 git 能當成 git dir 開啟的都在掃描範圍內；沒有 git dir 的 submodule 是新 clone，設定只有 clone 寫入的內容與使用者自己的全域設定。git 擋下的 helper（訊息是 `transport '<name>' not allowed`）從此列入待信任項目（`transport '<name>'`，只記在這次執行的記憶體），右側模式就會出現信任提示。信任檢查也看 index 裡每個已 checkout 的 gitlink（巢狀的也算）自己的設定，不管 `.gitmodules` 有沒有列出它，也不管上層設定有沒有程式：git 的 status、diff 會遞迴進這些 submodule，用它們的設定跑 filter。判斷「已 checkout」照 git 自己的條件（資料夾裡有 `.git`），進去後只讀它自己（`GIT_DIR=.git`）；它的巢狀 gitlink 到它實際的工作樹去找：submodule 自己的 `core.worktree`（寫在它的 config、gitdir 的 config 或 `config.worktree`）可以把工作樹指到別處，git 的 status 會跟過去，掃描也用 `rev-parse --show-toplevel` 跟過去。每一層都跟 git 一樣先進到那個工作樹，再用 gitlink 在 index 裡的名稱原樣找 `.git`：從根拼成的絕對路徑可能超過 PATH_MAX（macOS 1024），git 用的相對路徑卻沒有，以前掃描因此略過，git 卻照樣進去。找不到 `.git` 時，只有確定它不在（路上某層資料夾進得去而且沒有下一段，或那一段是檔案）才略過；其他情況（例如資料夾進不去）當成讀不到；資料夾進不去時 git 自己也進不去，這裡是刻意保守。每個 repo（gitdir 加工作樹，取 `rev-parse` 的絕對路徑）只讀一次，整個掃描在同一個 shell 裡記住讀過哪些：在自己底下又遇到（工作樹指回上層，git 的 status 會無限遞迴）當成讀不到；在別處又遇到（兩個 gitfile 指向同一個 gitdir、衝突的 gitlink 在 index 出現好幾次）就略過，它的 key 已經列過。所以掃描量受實際 repo 數限制，不會像只限層數那樣在兩個指回頂層的 gitlink 上展開成 2^16 個。路徑被 git 加了引號（含 `"`、`\`、控制字元）、index 裡的 gitlink 名稱是絕對路徑（git 的 status 會走進那個路徑，掃描卻會在頂層底下找）、git 自己不會寫出的名稱（含 `.`、`..` 或空的路徑段、以 `/` 結尾；手改 index 可以放進去，用來把路徑拉長）、Windows 主機上 Win32 會另外解讀的名稱（含 `:`，那是磁碟代號或 stream；路徑段以 `.` 或空白結尾，Win32 會去掉，git 就走到別處。以 `OS=Windows_NT` 或 `uname -s` 是 CYGWIN／MINGW／MSYS 判斷；其他主機上這些都是普通名稱，`a:b` 照常讀）、工作樹名稱含換行（`$(…)` 會刪掉結尾的換行，掃描就會走到同名但沒有換行的資料夾；所以 `rev-parse` 的回答後面補一個 `.`，去掉後必須剛好兩行，頂層自己也一樣）、gitdir 從根算起超過 PATH_MAX（macOS 上約 1019 bytes 起 `rev-parse --absolute-git-dir` 失敗；短幾個 bytes 時是 index 開不起來），或超過 16 層時，都無法追蹤，也當成讀不到。掃描以命令列 alias 讓 git 在頂層執行，設定的讀取也在同一個 shell 裡，所以本機與遠端都只要一次呼叫（遠端一次 `hc exec`）：腳本先照原樣跑 `git config --show-scope -z --get-regexp`，印出 `config\0<exit code>\0`，再掃描 submodule。頂層的 `.git` 是 gitfile 時（linked worktree、submodule 當頂層），git 會替 alias 設 `GIT_DIR`（git 2.50.1 實測；一般 repo 不會設）：這段設定照它讀，讀到的就是 discovery 會找到的那個 repo；找到頂層後腳本 `unset GIT_DIR GIT_WORK_TREE`，其餘讀取都回到 discovery。實測 linked worktree、submodule 當頂層與一般 repo，有沒有這個 `GIT_DIR` 掃描的輸出都相同。回答裡沒有這筆記錄，或它不在記錄的開頭位置（設定的回答在中途被截斷），都當成讀不到。腳本 export `GIT_NO_LAZY_FETCH=1` 與空的 `GIT_ALLOW_PROTOCOL`（git 2.44 以前不認得前者，後者會拒絕所有傳輸方式）。只要有一段沒跑完，關卡就維持關閉：gitlink 用 grep 從 `ls-files -s` 挑出，輸入最後補一行固定的記錄，grep 的輸出沒有它就當成讀不到（壞掉的 grep 不輸出也會 exit 0，看起來就像沒有 gitlink）；整個掃描以 `end` 記錄結尾，回答沒有它也當成讀不到。20 萬筆 entry 的 index 約 115 ms。掃描逾時（yuzora-git 60 秒、`programs_in` 30 秒）也當成讀不到，而不是錯誤：錯誤會讓 status 沒有 `trust` 欄位，右側模式就無法提供信任。橫幅以 `路徑: key` 列出。設定讀不到、或回答超過 8 MiB（`git_trust::ANSWER_CAP`，截斷的回答可能剛好少了某個程式 key）時，當成未信任。hook 本身不列入信任條件：status 不跑 hook，而 husky 等工具會在許多 repo 設定 `core.hooksPath`。使用者全域／系統設定的程式（例如全域安裝的 git-lfs）不需要信任。未信任時 `yuzora:git` 對這個 repo 的其他呼叫（log、diff、merge、stage、commit、fetch…）一律在引擎拒絕（`trust::check`，錯誤以 `untrusted folder` 開頭），各 pane 顯示這個錯誤，等同 IntelliJ 的 Safe mode；只有 `status`（顯示提示）與 `trust`（回答）例外。不經過 `repo_fn` 的路徑各自在跑 git 之前檢查：log 的載入（開啟、`logRefresh`、跟著 refs 更新）與補抓 metadata，以及 diff／merge session 的寫入（`diffStage`／`diffUnstage`／`diffDiscard`、`conflictResolve`；session 開啟後 repo 設定可能多了程式，例如 `git lfs install --local`）。**結構性保證**：yuzora-git 啟動 git 只有一個入口（`exec::run_streaming`，`Repo::run`、log 的串流讀取、`open()` 的探測都經過它，本機與 `hc exec --on` 也在這裡分流）。資料夾信任前，每個指令都在這裡加上空的 `GIT_ALLOW_PROTOCOL` 與 `GIT_NO_LAZY_FETCH=1`（`trust::guard`；git 2.44 以前不認得後者，靠前者擋）：不管 repo 用哪種寫法指到 remote helper，都叫不起來，也不會 lazy fetch。這不靠上面的掃描，掃描只負責讓右側模式事先問。例外只有自己指定清單的指令：`trust::reach`（fetch、pull、push 與 submodule update，見下）列出 git 自己的傳輸方式，`trust::offline`／`own_read`（自動讀取）同樣離線但不記錄。`trust::tests::every_git_command_in_an_untrusted_folder_runs_offline` 在本機與 `hc exec` 兩條路徑上，確認 read、write 指令都帶著這兩個變數。信任前沒有任何指令需要 lazy fetch，因為 partial clone（設了 `extensions.partialClone`，或有標成 `promisor` 或設了 `partialCloneFilter` 的 remote，照 git 的 `repo_has_promisor_remote`）一開始就列入待信任項目（共用的 `git_trust` 掃描就認得這些 key，所以 Files 的資料夾監看也會等信任）：它讀取時要向 remote 抓缺的物件，離線讀會失敗或缺內容（diff 讀不到一側時會當成沒有這個檔案），所以先問。右側模式的信任提示因此也會出現在 partial clone，取代 #207 log pane 的「信任並抓取」。
- 共用的信任 API（`yuzora_core::git_trust`）：上面的偵測（`PROGRAMS`、`names_program`、submodule 掃描）與信任清單放在 yuzora-core，給狀態列（L1）、檔案樹（L5）等也會自己跑 git 的地方共用；yuzora-git 用自己的執行器呼叫它，行為不變。
  - `pending(machine, top)`：回傳空清單才可以跑 `status`、`diff` 這類會比對工作樹或 index 的指令；`Err`（git 跑不起來）也當成不安全。`top` 是那台機器上 `git rev-parse --show-toplevel` 的輸出，遠端走 `hc exec --on`。
  - 清單裡的項目是 `key`（repo 自己的設定）或 `路徑: key`（submodule 的設定）。設定讀不到時，項目是 `UNREADABLE`（`(unreadable config)`）或 `路徑: (unreadable config)`：git 讀設定失敗、回答超過 `ANSWER_CAP`、掃描沒跑完或逾時、submodule 的工作樹指回上層、路徑被加了引號、gitlink 名稱是絕對路徑或 git 不會寫出的名稱（含 `.`、`..`、空的路徑段；Windows 主機上另有含 `:`、路徑段以 `.` 或空白結尾）、gitlink 可能在但看不到（例如資料夾進不去）、工作樹名稱以換行結尾、gitdir 從根算起超過 PATH_MAX、超過 16 層。設定與掃描在同一次呼叫裡，共用 `ANSWER_CAP` 與逾時：整個呼叫失敗、逾時或回答超過上限時，清單只有 `(unreadable config)`，不再列出設定裡的 key。這些都維持關閉，但使用者仍可以信任這個資料夾。
  - `pending_from(file, machine, top)`：同 `pending`，信任清單從 `file` 讀（測試用暫存檔，不碰真實的 app home）。
  - `is_trusted(machine, top)`：只讀信任清單，不跑 git。
  - `folder_id(machine, top)`：信任清單裡的 id，`<machine>:<top>`，本機的 machine 是空字串。
  - `trust_at(file, id)`：把 id 加進 `file`。信任只能由 Git 模式按下（yuzora-git 的 `trust`），其他 lane 只讀，不要呼叫它。
  - `programs_in(machine, top)`：不看信任清單，只列出 key（`pending` 在未信任時就是它）。
  - 本機的掃描結果會保留（`repo_programs_here`；`programs_in` 的本機路徑與 yuzora-git 的本機 repo 都經過它。遠端與 Windows 一律不保留：Windows 上檔案原地改寫、大小不變、時間設回去之後看起來和原本一樣，它的每個時間（包括 change time）都是程式可以設定的，所以 Windows 上也不看這些檔案，直接掃描）：再檢查同一個資料夾時，先看掃描讀過的檔案的 metadata，都沒變就直接回傳上次的答案，不跑 git。保留的 key 是呼叫端（`yuzora-core:git`，或 `yuzora-git:<git 執行檔>`）加頂層路徑，只存在這次執行的記憶體，最多 64 筆，滿了就全部清掉。
    - 只保留單純的 repo：掃描完整跑完（exit 0，有 `end`），記錄只有頂層自己的 `sub`、remote 檔案、`dirs`、`index`；index 裡沒有 gitlink（沒 checkout 的也不行，之後隨時可能 checkout，腳本以 `links` 記錄標出）；git dir 與 common dir 都沒有 `modules/`；`config` 與 `config.worktree` 的內容不含 `include`（不分大小寫；include 的檔案可能在任何地方）；index 自己就裝著所有 entry，沒有 split index（`link` extension，entry 在 `sharedindex.*`）或 sparse 資料夾（`sdir`，git 從 tree 展開）。後兩者要讀 index 以外的檔案，git 不會拿檔名去驗證那些檔案的內容，index 沒變它們也可能被換掉。split index 由 git 自己回答：頂層沒有 gitlink 時，掃描再跑一次 `rev-parse --show-object-format --shared-index-path`（git 照 status 的方式讀 index，有 shared index 就印出它的路徑），以 `index` 記錄交回；印出路徑、rev-parse 失敗，或 object format 不是 sha1／sha256（不認得這個選項的舊 git 會把它原樣印出）都不保留。另外自己再讀一次 index 檔案，照 git 的讀法（`create_from_disk`）：id 長度取 repo 的 object format，不兩種都試；路徑長度取自 flags（git 不看路徑後面那個 byte），`0xfff` 才往後找 NUL；extended flags 不論版本都多 2 byte；讀到結尾的 extension 裡有 `link` 或 `sdir` 就不保留。檔尾在 git 找 EOIE 的位置（checksum 前 32 byte）是 `EOIE` 時也不保留：git 用多個執行緒讀 index 時（`index.threads` 大於 1，或是 `true`／`0` 而 entry 超過 2 萬筆；repo 或使用者自己的設定都算，沒設定就只用一個）從 EOIE 記的位置讀 extension，那裡可以和 entry 結束的地方不同，實測可以把 `link` 包在 optional extension 裡藏起來。index 讀不懂時（版本 2–4 以外，或讀不到 checksum 剛好結束）也不保留。設定、index、gitfile 與 `commondir` 只讀一般檔案（不等待地開啟，再確認類型；FIFO 會一直等寫入的一方），而且有大小上限：設定 8 MiB、index 256 MiB、gitfile 與 `commondir` 1 MiB，超過就不保留。
    - 指紋：git dir 的 `HEAD`、`config.worktree`、`index`、`modules`（必須不存在），common dir 的 `config`、`objects/`、`refs/`（只看是不是同一個資料夾）、`remotes/`、`branches/`、`modules`（必須不存在）。每個檔案 lstat，是 symlink 就再 stat 它指向的檔案；比 mode、uid、dev、inode、大小、mtime、ctime（到 ns）。只在 kernel 自己維護 ctime 的檔案系統上保留（`statfs`：macOS 的 APFS、HFS+；Linux 的 ext2/3/4、XFS、Btrfs、tmpfs、ZFS、F2FS）：在這些檔案系統上程式無法設定 ctime，原地改寫後把 mtime 設回去也看得出來。git dir、common dir 與每個存在的指紋檔（symlink 看它指向的檔案）都要在這些檔案系統上。其他都不保留：FAT、exFAT 的 ctime 就是 mtime，程式可以設定（macOS 掛載的 msdos、exfat 實測：同大小原地改寫或用 `tar -xpf` 解壓蓋過去，指紋完全不變）；網路檔案系統（smbfs、NFS）的 ctime 可能來自另一端。`HEAD`、`objects/`、`refs/` 是 git 的 `is_git_directory` 的條件：任何一個壞掉，git 就往上層找別的 repo。命中前另外重讀頂層的 `.git`（資料夾或 gitfile）與 git dir 的 `commondir`，取真實路徑，必須仍是同一組 git dir 與 common dir：路上的 symlink 換了指向，這兩個檔案本身不會變。命中時的檢查不拿著快取的全域鎖：一個卡住的網路掛載只會讓那個資料夾的檢查等待。
    - 掃描前先取一次指紋，掃描後必須完全相同才保留：掃描當中換進來的 `.git`，裡面的檔案時間可能和原本一樣舊，只有 inode 等欄位分得出來。指紋檔案在掃描開始前 3 秒內（`RACY`）變動過也不保留：檔案系統時鐘粗的時候（FAT 是 2 秒），之後的修改可能得到相同的時間。
    - 使用者自己的設定（global、system、`GIT_CONFIG_GLOBAL`／`GIT_CONFIG_SYSTEM`／`GIT_CONFIG_NOSYSTEM`、`GIT_CONFIG_COUNT` 等環境變數，以及它們 include 的檔案）不在指紋裡：關卡只算 repo 自己的 scope（local、worktree），這些設定加不出這種 key；最多讓 git 讀不了這個 repo（例如 `safe.directory`），那時 git 在這裡的每個指令也都會失敗。`.gitattributes`、`info/attributes`、`.gitmodules` 掃描本來就不讀。app 的環境變數與 git 執行檔路徑在這次執行中不變；同一個路徑的 git 換了版本，要到下次啟動才重新掃描。macOS 的 ACL 也不在指紋裡：`chmod +a` 拒絕搜尋 `objects/` 之後 mode 與 inode 都不變（資料夾只看是不是同一個），git 的 `access(X_OK)` 失敗就改用外層的 repo；這要本機行程去改 ACL，和掃描當中先換掉再換回 `.git` 一樣不防。
    - 量測（macOS，2423 個檔案的 repo，10 次的中位數）：Files reload 的信任檢查 47.1 ms → 0.0 ms，行程 8 個 → 0 個（reload 另有 1 個 `rev-parse`，不變）；開 diff（`diffOpen`＋`diffView`）73.6 ms → 32.1 ms。有 gitlink 的 repo（例如 Yuzora 自己的 ghostty）不保留，數字不變。沒有 gitlink 的 repo，每次真的掃描時多一個 `rev-parse`（同一個 repo 約 11 ms）。
  - API 變動（#194 審查期間）：`repo_programs(run)` 拿掉了第一個參數 `gitmodules: bool`，submodule 一律從 index 掃，不再看 `.gitmodules`。`pending`、`programs_in`、`is_trusted` 的呼叫方式不變。自己傳 `run` 的呼叫端，要讓它能執行 `-c alias.…=!<script> yuzora-trust-scan`（本機與 `hc exec --on` 都可以；每次掃描只呼叫一次，設定也由這個腳本讀），並照原樣回傳 exit code 與 stdout（經 `whole`）；逾時要回傳 `Ok((-1, vec![]))`（當成讀不到），只有 git 根本跑不起來才回 `Err`。
  - 未信任時仍可以執行的指令（前面加 `READ_FLAGS`，也就是 `--no-optional-locks -c core.fsmonitor=false`）：只讀 refs、不碰 index 與工作樹的指令，例如 `rev-parse --show-toplevel`、`symbolic-ref`、`for-each-ref`（不帶 `:track`）、`config --get`。
  - 走訪 commit 的指令（`rev-list --left-right --count`、`for-each-ref` 的 `%(upstream:track)`、`log`、`merge-base`）：repo 有 promisor remote（partial clone）時，ref 可能指向 clone 裡沒有的 commit，git 會用 repo 自己的 ssh 指令去抓。這種 repo 要加 `--no-lazy-fetch`（git 2.44 起），更舊的 git 就不要走訪。範例是 L1 的 `yuzora-shell` `gitStatus`（`a_partial_clones_missing_commit_is_not_fetched`）。
  - 必須先信任的指令：`status`、`diff`、`diff-files`、`diff-index`、`ls-files -m`，任何寫入工作樹或 index 的指令，以及 `fetch`、`pull`、`push`。
- 寫入指令先拿 repo 層級的寫入鎖（等待中可取消）；讀取不拿。
- 取消與逾時：本機行程開在自己的 process group，取消或逾時就 `kill(-pid, SIGKILL)` 整棵樹。Windows 沒有 process group：git 先以暫停狀態啟動，放進自己的 job object 後才恢復執行，取消或逾時用 `TerminateJobObject` 結束 hook、ssh、credential helper 等所有子孫行程。job 允許明確要求脫離的程式（`CREATE_BREAKAWAY_FROM_JOB`）離開，否則那些程式會無法啟動；這和 unix 上行程可以離開 process group 對等。credential helper 為登入而開的瀏覽器在 job 裡，取消那次 fetch 時會一起被關掉。job 建不起來或放不進去時不執行 git（回傳錯誤），不會在取消只能結束 git 本身的情況下執行。讀取預設 60 秒，fetch／pull／push 等網路操作 600 秒；輸出上限 64 MiB。
- pathspec 超過 7600 bytes 改用 `--pathspec-from-file=- --pathspec-file-nul`（指令不支援時退回不帶路徑、讀全部）。
- 遠端 space：同一個執行器改走 `hc exec --json --on <machine> --cwd --timeout --env … [--stdin] -- git …`（`yuzora_core::remote`）。
- 輸出可以邊讀邊解析（`run_streaming`），log 用這條路。

### `yuzora:git` 以外的入口（L2c）

app 本體與 default mods 也會自己在使用者的資料夾跑 git，這些地方一樣遵守信任資料夾：未信任時只跑上面白名單裡只讀 refs 的指令，讀 index 或工作樹的指令要等 `pending` 為空。

實測（git 2.50，repo 自己設定 clean／smudge filter 與 fsmonitor hook，另有 stat 變動的檔案）：

- 會執行 repo 的程式：
  - `status`（即使帶 `--no-optional-locks -c core.fsmonitor=false`）、`diff HEAD --numstat`、單檔 `diff --no-ext-diff --no-textconv`：都會執行 clean filter。
  - `ls-files --others --ignored`：沒帶 `core.fsmonitor=false` 時執行 fsmonitor hook。
  - `worktree add`：執行 smudge。
  - `worktree remove`、`add -A`：執行 clean。
- 不會執行：`rev-parse`、`symbolic-ref`、`for-each-ref`、`show-ref`、`config --get-regexp`、`branch --show-current`、`worktree list`。

| 位置 | 指令 | 結論 |
|---|---|---|
| `app/defaults/mods/changes/main.js` | `rev-parse --show-toplevel` | 白名單 |
| 同上 | `status`、`rev-parse --verify HEAD`、`diff HEAD --numstat`、單檔 `diff`、Commit 的 `add -A`＋`commit` | 已改：先問 `yuzora:git` 是否還在等信任。未信任時清單位置顯示提示，後面的指令都不跑，Commit 停用。Commit 與單檔 diff 執行前再問一次（refresh 之後，repo 設定可能多了程式） |
| `app/defaults/mods/files/main.js` | `rev-parse --show-toplevel` | 白名單 |
| 同上 | `status --porcelain=v1`（檔案的 Git 標記） | 已改：未信任時不標記，清單下方顯示提示；`yuzora:git` 回答不了時也不標記，但不顯示提示 |
| `app/defaults/mods/worktree/main.js` | `rev-parse`（`repoOf`） | 白名單 |
| 同上 | `status --porcelain`、submodule 檢查（`rev-parse HEAD:<path>`、`cat-file -e`、`rev-list`）、`worktree list`＋`for-each-ref` 的 `committerdate`、`show-ref`、`worktree add`／`remove` | 已改：未信任時對話框顯示提示，新增、開啟、移除都不能執行。讀 commit 的指令在 partial clone 可能用 repo 的 ssh 指令去抓物件，所以開啟也一起擋。按 Enter 時再問一次，未信任就留在對話框顯示提示 |
| `app/defaults/mods/spaces/main.js` | `status --porcelain=v2 --branch` | 已改：改用 `yuzora:shell` 的 `gitStatus`（只讀 refs、config 與 commit，partial clone 帶 `--no-lazy-fetch`），不論是否信任都不再跑 status。分離的 HEAD 顯示成 `detached` |
| 同上 | `rev-parse --show-toplevel`、`--git-common-dir` | 白名單 |
| `app/defaults/mods/menu/main.js` | `rev-parse --show-toplevel`、`--git-common-dir` | 白名單 |
| `app/src/files.rs`（Linux 的 `watchFolder`） | `ls-files --others --ignored --exclude-standard --directory` | 已改：經 `app/src/yuzora/trust.rs` 的 `in_work_tree`，未信任時不讀。partial clone 也要先信任：sparse checkout 範圍外的 `.gitignore` 要從 index 讀，缺物件時會向 promisor remote 抓（實測 git 2.50） |
| 同上 | `rev-parse --path-format=absolute --git-dir --git-common-dir` | 白名單 |
| `app/src/window.rs` 的 `folder_info`（tab 的資料夾與分支） | `rev-parse --show-toplevel --abbrev-ref HEAD` | 白名單 |
| `app/crates/yuzora-shell/src/git.rs` 的 `status`（`gitStatus`，狀態列與 Spaces） | `symbolic-ref`、`config --get-regexp`、`for-each-ref`（partial clone 帶 `--no-lazy-fetch`） | 白名單，L1 的 `a_repos_own_filter_never_runs` 守住。同檔另外兩處是測試 |
| `app/defaults/AGENTS.md` 給使用者 mod 的範例 | `branch --show-current` | 白名單 |
| `app/vendor/gpui-kit/crates/shell/src/dependencies.rs` | mod 的 git 依賴（clone、fetch） | 不在使用者資料夾，在 `~/.gpui-shell/cache/dependencies` |
| `yuzora-git` 的 `exec.rs`、`yuzora-core` 的 `git_trust.rs` | | `yuzora:git` 的單一入口與信任掃描本身 |
| `src-tauri/` | | 凍結的移植來源，beta.2 不出貨 |

其他命中都是測試。`yuzora-statusbar`、`yuzora-modebar`、`yuzora-settings-general` 裡的 `"git"` 是模式名稱，不跑 git。

- 判斷方式：
  - mods 用 `yuzora:git` 的 `open(cwd, on)` 與 `status(id)`，`trust` 有值就是未信任。這和 Git 模式是同一份答案，遠端 machine 也適用。沒有待信任的項目時（已信任，或設定裡本來就沒有會執行的程式），`status` 會順便讀一次 status：有快取；本機另有 watcher，遠端 machine 沒有，靠 `changed()` 逾時輪詢（見〈已知限制〉）。
  - Rust 用 `yuzora_core::git_trust::pending(None, top)`，`top` 是帶 `READ_FLAGS` 的 `rev-parse --show-toplevel` 的輸出。讀不到、不是 UTF-8 或 `Err` 都當成未信任。
- 提示文字放在各 mod 自己的 `i18n/en.json`（`changes`、`files`、`worktree`），其他文字維持上游的英文。
- 測試：
  - `app/src/yuzora/trust.rs`：fixture 用自己的 HOME 建立，斷言未信任時沒有任何程式執行、信任後照舊（fixture 的程式真的會被 git 執行）。白名單指令同樣在這個 fixture 跑過。Linux 上另外從 `watchFolder` 端到端跑一次。
  - `app/scripts/yuzora-trust-mods.test.ts`：真實的 git，`yuzora:git` 的回答用 stub。也守住 default mods 跑 git 的位置數：每個 `exec("git", …)`，以及 mod 自己包住它的 helper（第一行就是 `exec("git", …)` 的函式，或 `name = (…) => exec("git", …)`，目前是 `changes` 與 `worktree` 的 `git`、`menu` 的 `checkout`）的呼叫數。上游同步在這些地方多了呼叫時會失敗，要重新判斷。守衛只讀各 mod 的 `main.js`（不讀 mod 裡其他 `.js` 檔，也不讀 `defaults/frame/`），helper 也只認上面兩種寫法（class method、block body 的 arrow function 不算）；目前 `exec("git"` 只出現在 changes、files、menu、spaces、worktree 的 `main.js`。經由更上層、本身會跑 git 的函式（例如 `worktree` 的 `submodules()`）多出的呼叫數不到，同步時仍要讀這幾個 mod 的 diff。
  - 拿掉任一處檢查，對應的測試都會失敗。
- 已知限制：
  - Linux 上未信任的 repo 不知道哪些資料夾被忽略，`watchFolder` 會連 `node_modules` 這類資料夾一起監看，超過 4096 個資料夾時改成定時重讀（原本就有的退路）。
  - Linux 上這個信任掃描和原本的 `ls-files` 在同一個位置（`watch_directories`；其他平台的 `watchFolder` 不跑 git）：`watchFolder` 啟動時同步執行（持有資料夾監看的 registry 鎖），之後每次重新掃描資料夾（新增或刪除資料夾這類事件）也在監看的執行緒裡再跑一次。多出 `rev-parse` 與掃描本身的 git 呼叫：20 萬筆 entry 的 index 約 125 ms（git 2.50.1、Apple Silicon 實測，`ls-files -s` 本身約 60 ms）。
  - 和引擎一樣以檢查當下為準：信任是對資料夾，信任之後 repo 設定再多出程式也照樣執行。
  - Spaces 清單：git 拒絕讀的 repo（例如擁有者不符）現在記成「不是 repo」，fallback 時不重問；切換資料夾或指令結束時仍會重問。

### Status（`status.rs`、`watch.rs`）

- `status --porcelain=v2 -z --branch --show-stash --untracked-files=no`，untracked 另走 `ls-files --others --exclude-standard`（與 IntelliJ 一樣分開，大量 untracked 不拖慢 tracked）；被忽略的目錄只用來過濾 watcher 事件。`+/-` 行數來自 `diff --numstat`（超過 3000 個變更就略過）；untracked 檔案直接數行（前 500 個），symlink 數它的目標字串（git 記錄的內容），不跟到目標檔案（可能在工作樹外）。全量刷新時這幾個讀取並行執行。status 與 untracked 清單超過輸出上限（64 MiB）時會被截斷，所以超過就回傳錯誤，不把只有前段的清單當成完整的存進快取（下次全部重讀）。
- 不是 UTF-8 的路徑（unix 的檔名可以是任意位元組）在 JS 只能顯示成把壞位元組換成 U+FFFD 的字串，把這個字串交回 git 會變成另一個名稱，可能剛好是另一個真的叫這個名字的檔案。status 記下這些路徑的顯示名稱（`StatusCache::lossy`），`repo_fn` 對 `status`、`trust` 以外的呼叫檢查參數：任何字串等於這種顯示名稱，或是它所在的資料夾（本身也含 U+FFFD），就拒絕（`path.lossy`，請使用者改用終端機）。上層的正常資料夾（例如 `src`）照常可以操作，git 會用真正的位元組。watcher 看到不是 UTF-8 的路徑時整個重讀；部分刷新的路徑碰到這種顯示名稱時也改成全量，否則只用這個名稱查 git，只會列出真的叫這個名字的檔案，另一個就從快取消失。
- watcher（notify，300 ms debounce）只在有 view 使用 repo 時運作。呼叫和等待（`changed()`、log 的 `logWait`）都算使用；超過 2 分鐘兩者都沒有，就停掉 watcher。下一次呼叫或等待會在背景執行緒重新啟動 watcher（遞迴監看可能要走過整個工作樹，不能放在 UI 執行緒），並把整個 repo 視為已變更。watcher 啟動失敗時改成輪詢（見〈pane 之間的約定〉的 `changed()`）。watcher 把事件分成 dirty scope：`.git/index` → 全部重讀；`.git` 內其他（refs、HEAD、MERGE_HEAD…）→ 全部＋refs；`.git` 雜訊（objects、logs、hooks、lock 檔…）與被忽略的目錄略過；工作目錄檔案 → 只重讀那些路徑。
- 部分刷新只對 dirty 路徑跑 status（`GIT_LITERAL_PATHSPECS=1`），A／D／R／C 的路徑一併重讀以免拆散 rename；路徑數過多（擁擠的目錄先折疊成目錄，超過 50 000 就全量）時退回全量。submodule 裡的檔案改成查它的 gitlink（git 把裡面的變更報在 gitlink 上）；gitlink 從 index 列出（`ls-files -s` 的 `160000`），沒寫在 `.gitmodules` 的也算，與全量刷新的其他讀取並行。這份清單失敗或超過輸出上限時，不存前段（後面的 gitlink 會漏掉，裡面的變更就查不到），而是記成不完整，部分刷新一律改成全量。折疊時去掉已被目錄涵蓋的路徑，只查每個路徑的上層目錄，不做兩兩比對（watcher 在 debounce 執行緒上同步呼叫，5 萬個路徑兩兩比對要 25 億次）。重讀前從快取拿掉這些路徑底下的項目時也一樣：每個項目用自己和上層目錄去查重讀的路徑集合（10 萬個項目、5000 個路徑原本要 18 秒）。
- `.git` 的操作狀態（rebase-merge／rebase-apply、MERGE_HEAD、CHERRY_PICK_HEAD、REVERT_HEAD、BISECT_LOG）直接讀檔，對應 IntelliJ 的 `GitRepositoryReader`。

### Log 與分支圖（`log.rs`、`graph.rs`、`graph_view.rs`）

- **兩階段**（IntelliJ `GitLogProvider`）：先讀最近 `logRecentCount`（預設 2000）筆完整 metadata（`%x01%x01`／`%x02%x02`／`%x03%x03` 控制字元分隔，邊讀邊解析），拓撲排序後立刻建圖；再用 `--date-order --format=%H%x00%P%x00%ct` 讀全部 commit 的 hash／parents／時間，換上完整的圖。全部 refs 指向的 commit 都已在第一段時跳過第二段。
- **詳情延遲載入**：可見列缺 metadata 時，背景以 `log --no-walk=unsorted --stdin` 每批 500 筆補齊；commit 詳情（`--name-status -M`）放 LRU（1000）。
- **刷新**（VcsLogJoiner 的作法）：refs 變了只讀最近 1000 筆，接回既有的圖；有 parent 不在圖裡、ref 指到圖外或舊 head 消失時接不上，才全部重讀。
- **永久圖**：hash 存成連續 bytes、parents／children 用 CSR int 陣列；「唯一 parent 就在下一列」的 commit 不存邊（bitset），大多數線性歷史因此幾乎不佔 parent 空間。
- **layout index**：依 IntelliJ `GraphLayoutBuilder` 的 DFS，目前分支、main／master、其他本地、遠端、tag 依序先排，重要分支固定在左側；顏色由 layout index 決定，同一條分支的顏色穩定。色票是 tokens 的 `git_graph`（8 條 lane 與 HEAD 點，深色與淺色各一組，在 hover、selected 列上也至少 3:1），log pane 傳給 `GraphCell`。
- **可見圖與列印元素**：依篩選得到可見列；每列的列印元素（節點欄、經過的邊、長邊箭頭，LONG_EDGE 30 列）在需要時才算，放 LRU（400）。元素排序移植 IntelliJ 的 `GraphElementComparatorByLayoutIndex`。
- **繪製**：`GraphCell` 是 HostModule 元件（GPUI `canvas`），直接讀 Rust 端的列印元素畫線段、箭頭與節點（HEAD 實心＋外框、merge 實心、一般空心）。
- **篩選**：branch、user、date、path、text（regex、區分大小寫）、hash、range、不含 merge。branch／path／range 篩選保留分支圖（`--parents` 改寫後的 parents）；user／text／date／hash／no-merges 顯示平面清單。篩選的 `--max-count` 分段 5 → 100 → 2000 → 全部（與 IntelliJ 一樣先給一屏）。branch／hash／range 是放在選項前面的 revision，開頭是 `-` 的一律拒絕，否則 git 會把它當成選項（例如 `--output=<file>` 會把 log 寫進那個檔案）；compare 的兩端也一樣。
- **持久化 index 的接縫**：圖只依賴 `RawCommit {hash, parents, time}`，`LogSession` 的第二階段來源可以換成讀 index；篩選目前全交給 git。

### Diff 與部分暫存（`diff.rs`、`diffview.rs`）

- 行 diff 在 Rust 計算：線性空間 Myers（middle snake），編輯數超過 `max(20000 + 10·√(n+m), 20000)`（IntelliJ `MyersLCS` 的門檻）就退回 Patience（唯一行錨點＋區段內有限 Myers）。單側超過 16 MiB 或 100 萬行不比較並告知（每一行在 diff 裡的成本遠大於它的位元組）。Myers 的對角線陣列依編輯預算配置（`budget / 2 + 2`，放棄前最多走這麼遠），不隨檔案大小變大：兩個 16 MiB、每行一個位元組的檔案原本要 512 MiB，現在約 1.2 MiB。Patience 用工作清單而不是遞迴：錨點一個接一個留在前一段裡時，遞迴深度等於行數，2 MiB 的 worker thread 會 stack overflow，整個 app 中止。這樣逐段找錨點是平方時間，所以掃描過的行數累計超過兩側總行數的 32 倍後，剩下的每段都當成一整塊變更。
- 內容：`git cat-file --filters <rev>:<path>`（套用 textconv 以外的 filter，與 IntelliJ 的 `git.read.content.with=FILTERS` 相同）、index 用 `:<path>`、工作目錄直接讀檔（symlink 讀目標，unix 上保留原始位元組，不轉成 UTF-8）。遠端也一樣：`cat` 會讀穿 symlink，所以先在那台機器上用 `test -L`／`readlink`／`test -x` 判斷（`Repo::kind`），新檔部分暫存時的 mode（`120000`、`100755`）也照它。
- 檢視：並排或合併、字詞層級高亮（tokens：單字、空白段、單一符號）、空白策略 Exact／Trim／Ignore（對應 IntelliJ `ComparisonPolicy`）、context 摺疊。
- **部分暫存不動工作目錄**：把「index 版本＋選中的 hunk 或行」組成新內容，`git hash-object -w --stdin --path` 寫 blob，再 `update-index --add --cacheinfo <mode>,<oid>,<path>`。取消暫存同理（HEAD 版本＋反向選取）；捨棄 hunk 則改寫工作目錄檔案（寫入與刪除都不穿過工作樹裡的 symlink 資料夾：路徑上層有 symlink 就拒絕，與 git 的 `beyond a symbolic link` 相同，免得寫到 repo 外面；merge 寫回結果也一樣，本機與遠端都檢查。遠端用 `${p%/*}` 取上層：`$(dirname …)` 會刪掉名稱結尾的換行，`link⏎` 這種 symlink 資料夾就躲過檢查）。整檔情況（新檔、刪除、全選）直接走 `add`／`reset`／`rm --cached`；逐行勾選了每個 hunk 的每一行也算全選（否則取消暫存一個新檔，會留下一個空檔在 index）。只有 mode 改變（例如 `chmod +x`）時，以 `git diff --raw` 讀兩邊的 mode（照 git 的判斷，含 `core.fileMode`），pane 顯示 mode 變更，暫存、取消暫存、捨棄都整檔處理，mode 一起帶上。已暫存的 rename 比較 HEAD 裡舊路徑的 mode 與 index 裡新路徑的 mode（`--no-renames` 會把它看成新增）。部分暫存是用開啟時讀到的內容組出來的，所以先確認工作目錄的檔案沒有被其他編輯器改過（捨棄也是），改過就要求重新載入，免得暫存檔案裡已經不存在的行。CRLF 與無結尾換行會保留。
- 空白不是 Exact 時不允許暫存（畫面上的 hunk 與實際內容不一致）。

### Merge 與衝突（`merge.rs`）

- 讀 index 的 stage 1／2／3（base／ours／theirs）做 diff3 分塊：equal、ours、theirs、same、conflict。
- 「自動解決簡單衝突」：對 conflict 塊再做 token 層級三方合併，兩側改到不重疊的地方時給出 auto 解答（對應 IntelliJ 的 magic resolve）。塊裡有不是 UTF-8 的行時不提供 auto（token 合併以文字進行，那些位元組會變成 U+FFFD）；塊以外的行照原本的位元組寫回。
- 結果寫回檔案後 `git add`；整檔 accept yours／theirs 用 `checkout --ours/--theirs`（刪除衝突用 `git rm`）；先確認每個路徑仍有 unmerged stage，已在外面解決就要求重新載入，否則不存在的一側會被當成刪除。symlink 的衝突以 `120000` 寫回 index，結果的每個位元組都是目標路徑（結尾的換行也是），不另外去掉。
- 任一側或工作目錄的檔案超過 16 MiB 或 100 萬行（與 diff 相同的上限）時不做逐塊合併（超過 16 MiB 時只讀得到開頭，合併那段會丟掉檔尾）：view 回傳 `tooLarge`、沒有分塊，`result`／`conflictResolve` 拒絕，merge pane 只提供整檔接受。
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

attach 的量測（v0.1.0 E1）：`app/scripts/git-bench/run-attach-bench.sh <yuzora-git 測試執行檔> <repo> [次數]` 跑 `lib.rs` 的 `perf_attach_to_a_repository`（ignored）：`open`、`status`、`messages` 各自的毫秒與 git 行程數，每次從關閉的 repo 開始（`cold`），緊接著再來一次（`again`＝回到看過的 repo）。分兩輪：PATH 前面放記錄起訖的 `git` 替身（算行程數；每個呼叫多約 15 ms 是替身自己的兩次 perl）和真的 git（毫秒）。`YUZORA_GIT_PERF_NO_WATCH=1` 不啟動檔案監看，用來分出 `open` 裡監看啟動的時間。

結果（v0.1.0 E1，macOS Apple Silicon，真的 git，3 輪 × 5 次、各輪前後交錯，中位數與範圍；`open` 約 10 ms 不變，已含在總數裡）：

| 一次 attach | 之前 | 之後 | 行程數（替身）之前 → 之後 |
| --- | --- | --- | --- |
| 1,100 commits／4,600 檔的 repo，從關閉開始（`cold`） | 215 ms（181–233） | 162 ms（140–195） | 12 → 11 |
| 同一個 repo，緊接著再來一次（`again`＝回到看過的 repo） | 139 ms（120–154） | 116 ms（100–139） | 6 → 5 |
| 5 個檔案的 repo，`cold` | 76 ms（71–132） | 43 ms（40–93） | 10 → 9 |
| 5 個檔案的 repo，`again` | 40 ms（37–54） | 19 ms（18–25） | 4 → 3 |

少掉的時間對得上單一指令的量測：`messages` 84 → 60 ms（拿掉 `log -n 300` 約 16 ms、`config` 與 `log -1` 並行約 12 ms），`status` 的 `cold` 112 → 89 ms（第二批 `numstat` 約 20–26 ms 與第一批重疊）。`status` 剩下的 90 ms 是兩個走整棵樹的 `ls-files --others`（單獨 50–60 ms，並行互搶 I/O 時各約 90 ms）與信任掃描，這次都沒動（信任關卡、ignored dirs 的列舉時機不在範圍）。

## A1–A12 完成度

對照 [research/intellij-git-research.md](research/intellij-git-research.md) 的 A 段。標記：✅ 完成　🟡 部分　⏳ 未做（後續）　⛔ 不做（範圍外）。

### A1. Clone、init、環境、多 root

- ⏳ Clone（含 shallow／unshallow）、init：beta.2 由 Workspace（L1）開資料夾，Git 模式不提供 clone／init。
- 🟡 Git 執行檔：設定頁可指定路徑（`executable`），每次呼叫時讀取，已開啟的 repo 也立刻換用；版本顯示在 repo pane 的「身分與簽章」；沒有自動下載。
- ✅ Repo 偵測：右側模式依焦點 pane 的資料夾（否則 space 的資料夾）找所在的 repo，跟著焦點切換；本機與遠端 space（`hc exec --on`）都可以。同一個 Space 有多個 repo 時，可以從右側模式頂端的選擇器替這個 Space 釘選其中一個（beta.3，見〈同一個 Space 的多個 repo〉）。
- ⛔ Multi-root 同步分支操作：一次看一個 repo。
- 🟡 Submodules：列出狀態（路徑照原樣，含空白）、`submodule update`；子模組本身當成另一個 repo 開啟。
- 🟡 Safe mode（未受信任專案）：Git 有自己的信任資料夾（見〈執行器〉）：repo 本地設定會自動執行程式時，Git 的所有操作先等使用者信任；自動執行的讀取另外關掉 `core.fsmonitor`，簽章檢查不用 repo 本地指定的程式，blame 不跑 textconv。⏳ 整個 app 共用的信任模型（終端機、其他模式）屬於殼層。

### A2. Status、Changes view、Partial commit

- ✅ 已暫存／變更／衝突三段，untracked 併在「變更」以 U 標示；數量、`+/-` 行數、全部暫存／取消。
- ⛔ Changelists：採 staging area 模式（IntelliJ 啟用 staging area 時同樣停用 changelists）。
- ✅ 部分暫存：hunk 或行，暫存、取消暫存、捨棄，不動工作目錄（`hash-object` 加 `update-index`）。
- 🟡 Ignored：被忽略的目錄不觸發刷新；沒有編輯 `.gitignore`／`info/exclude` 的 UI。
- ✅ 非 modal 的 commit 區（右側模式底部）。

### A3. Commit 選項

- ✅ amend（最後一筆）、sign-off、略過 hooks、cleanup（設定 `commitCleanup`）、作者；日期由 API 支援（`date`），右側模式未開欄位。
- ⏳ amend 指定 commit（`--allow-empty` 後 squash）：可以用互動式 rebase 的 fixup 達成。
- 🟡 GPG：與 IntelliJ 一樣改 git config（`commit.gpgSign`、`user.signingKey`、`gpg.format`），commit 詳情顯示 `%G?` 簽章狀態；沒有內建 pinentry。
- ✅ Hooks 由 git 執行，可逐次略過，預設值在設定頁。
- ✅ 訊息：歷史（每個 repo，數量可設定）、`commit.template`（絕對路徑照 git 的判斷：`/…`、`~`，以及 Windows 的 `C:/…`、`C:\…`、`\\server\…`）、amend 時帶入上一則訊息。⏳ `fixup!`／`squash!` 補全、Co-Authored-By 補全、長度檢查。
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
- ✅ Patches：建立（commits、已暫存、未暫存；複製或存檔）、套用（`git apply`：check、3way、只進 index、reverse）。`git diff` 不支援 `--pathspec-from-file`，選取的路徑太長時分批產生再串起來（依路徑排序），不會退回整個 repo。patch 超過 64 MiB（git 的輸出會被截斷）時拒絕，不回傳只有前段的 patch。存檔時目的檔是 symlink 就拒絕（開檔加 `O_NOFOLLOW`），不會覆寫它指向的檔案；不是一般檔案（例如 FIFO，開來寫會一直等讀的一方）也拒絕：開檔另加 `O_NONBLOCK`，開檔後再確認一次。

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

- **右側模式沒開過時沒有徽章**（實機確認：重啟後停在工作區模式，shared 裡沒有任何 git key；切到 Git 模式一次後才出現，之後切走仍會持續更新）：herdr 只在 stack 子節點第一次顯示時建立 mod，`yuzora.badge.git`／`yuzora.status.git` 要等 Git 模式開過一次才開始更新（之後切到別的模式仍會持續）。需要 L1／殼層在啟動時建立右側模式各模式，或由狀態列自己訂閱。
- **三欄合併的欄寬**：欄寬用字元數估算，視窗 1280 寬時 Theirs 欄會被裁掉一部分（可橫向捲動）；要改成依可用寬度分配。
- **popup／dialog 的截圖**：herdr CLI 沒辦法帶 props 開 overlay，分支 popup 與 push 對話框的實機截圖是開在分頁裡（`tab.new --mod`）。
- **信任只管 Git**：repo 本地設定會自動執行程式時，Git 的所有操作都先等信任，但終端機與其他模式不受影響。狀態列（L1）與檔案樹（L5）的 git 呼叫要改用 `yuzora_core::git_trust`（#194 合併後）。整個 app 的信任模型要由殼層（L1）提供，Git 再改用它。
- **遠端 repo 沒有 watcher**：靠 `changed()` 約 25 秒逾時輪詢一次 status。
- **`status::full` 的未追蹤或 ignored 列舉失敗時，兩個 `numstat` 已經跑完再被丟棄**（v0.1.0 E1 r1 的 nit）：`numstat` 在 `status` 讀完時就出發，與兩個走整棵樹的列舉並行，行程出發後沒有取消的辦法；列舉失敗是少見的路徑，代價是多 2 個行程，結果與以前相同（錯誤照舊傳回）。要避免就得等列舉結束才出發，會拿掉這次省下的時間。
- **回訪後第一次 status 可能是離開前的快取**（v0.1.0 E1 r2，這個改動之前也一樣，沒有放大）：本機是離開超過 `repo::IDLE`（120 s）後 watcher 重啟的時間（`used()` 的 `watch::start` 回來才 `invalidate_refs()`；閒置時約 10 ms，機器忙時 0.4–6 s），遠端沒有 watcher，最多 25 s（`changed()` 逾時輪詢）。這段時間內視圖已是 `confirmed`，stage、unstage 作用在 git 當下的狀態（可還原），`commitChecks` 的警告可能依據舊快取，會刪東西的兩條路（確認框的 Yes、右鍵選單的 Discard）先強制重讀所以不受影響。修法在引擎：`used()` 決定重啟 watcher 時同步 `invalidate_refs()`，遠端回訪的第一次讀取 `force`。
- **輸入框裡的字與畫面上的 repo 在換畫面的那一幀可能對不上**：`show` 換畫面時同步把新 repo 的草稿放進輸入框，下一幀才畫出來；這一幀裡剛好打下的字會加進新 repo 的草稿。⌘Enter 不受影響（交給最後畫出的 frame），只有這一個字元的歸屬。
- **一個 repo 本次執行的第一次 `open` 要等檔案監看啟動，機器忙的時候可以到好幾秒**（macOS，v0.1.0 E1）：`repo::open` 在登錄新 repo 時同步呼叫 `watch::start`（FSEvents 的 `watch()`）。其他 lane 同時在編譯、load average 12–15 時量到：5 個檔案的 repo 0.4–1.5 s，4,600 個檔案的 repo 1–6 s，不啟動監看（`YUZORA_GIT_PERF_NO_WATCH=1`）時整個 `open` 10–18 ms；load 約 6 時有沒有監看都是 9–12 ms。所以忙碌時，每個專案本次執行第一次顯示在 Git 模式，「讀取中」會多這麼久（之後同一個 repo 的 `open` 是登錄表命中，約 11 ms）。是 CPU 負載還是 fseventsd 被別的建置拖慢，沒有分開驗證。修法的形狀（沒做，要碰 `watch`、`open` 與一批假設 `open` 回來就已監看的測試）：`open` 登錄後立刻回傳，監看在 `used()` 重啟監看用的那條執行緒啟動，啟動完 `invalidate_refs()`（那一條路已經存在）。
- **持久化 log index**：未做；圖只依賴 `RawCommit`，第二階段來源可以換成 index（見〈Log〉）。
- **記憶體內編輯**只有 reword。
- **diff 的跟隨請求不分 pane**（Codex 對 #208 的 P2，延後）：先關掉 `diffFollowPane`、在同一個 Space 開出好幾個 diff pane、再打開它時，同一個 Space 的 `yuzora.git.diff.<space id>` 會讓每個 diff pane 都換檔，`showDiff` 卻只 focus 其中一個。mod pane 不知道自己的 pane id（herdr 只把 `props().pane` 給 pane 標頭 mod，`run()` 回傳 `null`），要改上游 API 或另做 opener 與 pane 的 key 對應。
- **同一個分頁裡有好幾個 log pane**：登記以分頁為鍵，每個分頁只有一格，`pane.focus` 要 pane id，開啟方用「該分頁的第一個、登記相符的 log pane」。把分支圖與另一個 log 分割在同一個分頁、又要重用其中一個時可能聚焦到旁邊那個；同一個分頁裡有兩個不同 repo 的分支圖時，只有最後登記的那個 repo 找得到；把分支圖 pane 拖到另一個分頁後，pane 只在第一次畫出來時登記，新分頁沒有它的格子，按分支圖會多開一個（預設每個圖、歷史、比較各自一個分頁，不會遇到）。
- **登記只在圖畫出來之後才有**：升級後第一次啟動，舊版開著的分支圖分頁在畫出來一次之前沒有登記，這段時間按分支圖會多開一個；遠端 machine 的 Space 刪除後它的格子不會被清掉（一格約一百位元組）。
- **mod 的 `init` 先於快照發布**：切換 Space 時 App 在 `sync()` 裡先建立新顯示的 pane（跑 `init`），最後才 `publish()`，所以 `init` 裡的 `spaces()`、`focused()` 是切換前的內容。從「新增 pane」選單開的 log（props 沒有 repo）要到第一次畫出來才讀目前 Space 的資料夾，merge、rebase 也延到第一次畫出來才讀自己 Space 的請求；diff pane 的 `init` 仍會讀一次（可能是切換前那個 Space 的請求），第一次畫出來時 `wanted()` 與 `spec` 不同就改成自己 Space 的，只多一次被取消的載入；其他 mod 若在 `init` 讀 Space 相關的快照有同樣的問題（未查）。這一點是讀 `src/window.rs` 的 `sync()`／`publish()` 與 `src/mods.rs` 的快照得到的，沒有在真 app 重現。
- **擋下 helper 之後才問**：tracked `.gitmodules`、使用者自己的 `insteadOf` 指到 helper 時，掃描事先看不到。第一次 submodule update 或 fetch 會失敗（git 的 `transport '…' not allowed`），之後右側模式才出現信任提示。這筆記錄只留在記憶體，重開 app 後要再失敗一次才會再問。
- **pull 帶 submodule 時的 `file`**：repo 設了 `submodule.recurse` 時，pull 會接著跑 submodule update。這一步的 `file` 照 fetch 的規則（URL 由使用者給）放行；git 自己的預設對 submodule 不放行。
- **branch 很多時掃描讀不到**：信任掃描列出所有 `branch.*.remote` 與 `remote.*` 設定，tracked branch 約 20 萬個以上時，回答超過 `ANSWER_CAP`（8 MiB），當成讀不到（fail closed），資料夾要先信任。
- **斷掉的 link 也要先信任**：`.git/modules` 底下（包括 submodule 的 git dir 裡面）只要有 kernel 跟不過去的 symlink，信任掃描就當成讀不到（fail closed）。shell 拿不到 errno，分不出「目標不存在」和 git 自己解得開的寫法，所以舊式 symlink HEAD、指到不存在路徑的 hook 這類無害的 link 也會讓資料夾要先信任。
- 列出的 ⏳ 項目。

### #194 合併後的 follow-up（已處理）

#194 合併時延後的項目，都在 follow-up PR 修正，每項先寫會失敗的測試：

- Codex 對 `9e0c88b0` 的 6 則與對 `74f3cb96` 的 3 則 P2：
  - `git_trust`：會執行 remote helper 的 remote URL（`ext::` 等）與改寫成它的 `insteadOf` 列入信任條件。拆在 URL 與 insteadOf 兩邊、`.git/remotes`／`.git/branches`、`extensions.partialClone` 這幾種寫法當時沒有涵蓋，見下一節。
  - log：資料夾信任前不 lazy fetch（`GIT_NO_LAZY_FETCH=1`）；因此讀不到時，log pane 提供「信任並抓取」（Codex P2）。status 與 git 2.44 以前的版本當時沒有涵蓋，見下一節。
  - untracked 行數不跟 symlink。
  - accept 前確認衝突還在。
  - 工作樹路徑含換行時分開查詢 `rev-parse`。
  - submodule 路徑含空白。
  - 遠端寫檔的上層檢查改用 `${p%/*}`。
  - Save Patch 不寫穿 symlink。
  - Windows 的絕對路徑 `commit.template`。
- 獨立 reviewer 對 `74f3cb96` 的意見：
  - should-fix：`:` 只在 Windows 主機上算讀不到。
  - nit：`gone` 不再每個沒有 `.git` 的 gitlink 開一個 subshell（2000 個：2.33 秒 → 0.26 秒）。
  - nit：測試的 SHA-1 改在 Rust 算，不需要 perl。
  - 說明更正：0600 的資料夾是刻意保守；PATH_MAX 看的是 gitdir。
- diff 的記憶體上限：`V` 依編輯預算配置，行數上限 100 萬（見〈Diff〉）。
- 淺色主題（yuzora-light）：L1 清單的 5 處：
  - log 圖的 `git_graph` tokens。
  - ref chip 的字改用 `foreground`。
  - 信任按鈕改用 `accent_foreground` 與透明度 hover。
  - selected／hover 上的紅綠改用 `file_status.deleted`／`diff.added`。
  - rebase 不再疊透明度，設定頁停用列至少 0.75。
  - `tokens.json` 與 `tokens.rs` 屬於 L1，由總指揮授權修改。
  - 隔離實機截圖（yuzora-light）：`evidence/l2-light-graph.png`（分支圖與 selected 列）、`l2-light-refs.png`（ref chip）、`l2-light-trust.png`（信任按鈕）。
- 未在 Windows 實機驗證：MSYS 遠端上結尾是 `.` 或空白的名稱，目前依 Win32 的規則當成讀不到（模擬 `uname`／`OS` 測試）。

### #207 合併後的 follow-up

獨立 reviewer 對 #207 的 should-fix 與 nit，以及對 #208 指出的既有問題。每項先寫會失敗的測試：

- App 會中止：Patience 改用工作清單（24,000 行、146 KB 的輸入原本在 2 MiB 的 worker thread 上 stack overflow），並限制總掃描量（見〈Diff〉）。merge 的逐塊比較走同一條路。
- `ext::` 信任檢查的四種繞過（使用者全域設了 `protocol.ext.allow=always` 時）：URL 拆在 `remote.*.url` 與 `url.*.insteadOf` 兩邊、`.git/remotes/<name>`、`.git/branches/<name>`、`extensions.partialClone`。測試先證明 git 真的會執行指令，再確認每種都要等信任。
- 右側模式自動 status 不 lazy fetch：未信任時和 log 一樣經 `trust::own_read`；兩者都另設空的 `GIT_ALLOW_PROTOCOL`，git 2.44 以前也擋得住。（第三輪起由結構性保證涵蓋所有指令，partial clone 一開始就等信任，見〈執行器〉。）
- 工作樹名稱以 CR 結尾時開到隔壁資料夾：`rev-parse` 的輸出只用 `\n` 切（`lines()` 會連 CR 一起去掉）。
- 「信任並抓取」只在 git 的錯誤訊息裡有確實缺少的物件時出現；bad range、regex 錯誤顯示原文。`initLog` 重設上一次信任失敗的訊息。（第三輪起 partial clone 一開始就等信任，這個按鈕連同判斷一起移除。）
- Save Patch 的目的檔是 FIFO 時不再卡住。
- props 沒帶 `at` 的 diff pane 不再被更早留下的 `yuzora.git.diff` 請求蓋過。

### #210 審查的 follow-up

獨立 reviewer 對 #210 的 blocker 與 should-fix。每項先寫會失敗的測試：

- remote helper 的其他寫法（blocker）：remote 的名稱本身是 helper URL（`remote.<URL>.fetch`、`.skipDefaultUpdate`、`.promisor`）、`branch.*.remote`／`pushRemote`、`remote.pushDefault`、`remotes.<group>`、`submodule.*.url`。`ext::` 與一般 helper（`zz::`：git 的預設 policy，使用者裝了 `git-remote-zz`）都測，測試先證明 git 真的會叫起 helper，修正前掃描都回傳空清單。
- 執行時再擋一層（`trust::reach`，見〈執行器〉）：tracked `.gitmodules` 掃描不到，也包括 `--recursive` 途中才遇到的巢狀 `.gitmodules`。另外實測未 checkout 的 submodule（`.git/modules/<name>`）在 manual fetch 時，git 2.50 會照預設 policy 叫起它設定的 helper，這個 git dir 也不在掃描範圍內。測試透過 `ops::fetch` 與 `ops::submodule_update` 確認信任前不跑、信任後會跑。（第四輪更正：`reach` 只擋得住 helper，同一個 git dir 的 filter、`core.sshCommand`、`gitProxy` 仍會執行；第四輪改成掃描這些 git dir，見下方。）
- 右側模式自動讀的 commit 訊息（`messages`：`log -1`、`log -n 300`）在未信任時經 `own_read`，partial clone 缺的 commit 不會被抓。（v0.1.0 起 `messages` 不再讀 `log -n 300`，也不回 `authors`：沒有 mod 用這份作者清單。）

第三輪（reviewer 複審 `c9cb897e`），改成結構性的修法，不再逐一列舉寫法：

- blocker：`<scheme>://` 而 scheme 不是 git 自己處理的，git 一樣交給 `git-remote-<scheme>`（預設 policy 把未知協定當成 `user`，會放行）。commit 詳情與 commit diff 在未信任的 partial clone 裡 lazy fetch 時會叫起它，`ext://x/run` 還會執行 repo 自帶的 `ext:/x/run`。
- 結構性保證（見〈執行器〉）：資料夾信任前，`exec::run_streaming` 對每個指令加上空的 `GIT_ALLOW_PROTOCOL` 與 `GIT_NO_LAZY_FETCH=1`，本機與 `hc exec` 都一樣。測試直接呼叫 `log::details` 與 `DiffSession::load`（繞過關卡），`zz://` 與 `ext://` 都不會叫起 helper；信任後兩條路都會叫起，證明測試有效。結構測試在本機與 `hc exec` 上檢查 read、write 指令的環境變數。
- 掃描照 `transport_get` 認得 `<scheme>://`（url、remote 名稱、`insteadOf`、`extensions.partialClone`），讓右側模式事先問。
- partial clone 一開始就列入待信任項目，log pane 的「信任並抓取」與它的判斷（`LAZY_FETCH`、`lazyFetch`）移除。
- 擋下的 helper 由 `exec` 統一記錄（`trust::heard`），`reach` 只負責列出可用的傳輸方式。

第四輪（reviewer 複審 `5cedb3ce`）：

- blocker：未 checkout 的 submodule 的 git dir（`.git/modules/<name>`）裡的 filter、`core.sshCommand`、`core.gitProxy` 在信任前會執行：submodule update 重用它時跑 smudge filter，fetch 的 on-demand 遞迴與 update 用它的 ssh 指令或 proxy。第三輪的文件與 PR 說這個 git dir 已由 `reach` 擋住，那是錯的：`reach` 只限制傳輸方式。修法兩個都做：
  - 掃描讀 `modules/` 底下每個 git dir（巢狀的也算；git dir 與 common dir 的都讀：實測 git 2.50 把 linked worktree 的 submodule git dir 放在 `.git/worktrees/<id>/modules`），以 `.git/modules/<name>: key` 列出；已 checkout 的 submodule 照原本的 gitlink 名稱只列一次（這些 git dir 最後才讀，略過已經讀過的）。這段只讓 `git_trust` 更嚴格。
  - 使用者自己按的 fetch、pull 在信任前加 `--no-recurse-submodules`。
  - submodule update 不另外要求信任：它會重用的 git dir 都在掃描範圍內（第五輪更正：第四輪只認得有 `HEAD` 檔、`objects/`、`refs/` 的資料夾，這句話當時並不成立），掃描有列出項目時 `repo_fn` 已經拒絕；沒有 git dir 的 submodule 是新 clone，只有 clone 寫入的設定。改成一律要求信任的話，掃描沒有項目的資料夾不會出現信任提示，submodule update 就再也跑不了。
  - 測試先證明 git 真的會執行（信任後 `ops::submodule_update`／`ops::fetch`／pull 會跑），修正前掃描回傳空清單、fetch 會跑 ssh 指令：smudge、`core.sshCommand`、`core.gitProxy`、fetch 與 pull 遇到未 checkout 的 submodule、巢狀與 linked worktree 的 git dir。
- nit：只有 `remote.<name>.partialCloneFilter`（沒有 `promisor`）也算 partial clone，照 git 的 `promisor_remote_config`。
- nit：`log.rs` 的註解還寫著 log pane 會提供信任；改成實際行為：回傳 `untrusted folder`，由右側模式的信任提示處理。

第五輪（reviewer 複審 `55a0a94d`）：

- blocker：掃描判斷 git dir 的方式和 git 不同，未 checkout 的 submodule git dir 可以藏起來：`HEAD` 寫成 `head`（macOS、Windows 不分大小寫，git 照樣開得到）、只有 `HEAD` 和 `commondir`（objects、refs、config 在 commondir 指向的地方）、`HEAD` 是指向只存在於 packed-refs 的 ref 的 symlink。三種 `submodule update` 都會重用並執行它的 smudge、`core.sshCommand`、`core.gitProxy`，掃描卻回傳空清單。改成結構性的做法：
  - 不再用檔名列舉 git dir。`find -L` 列出 `modules/` 底下每個資料夾，用檔案系統本身判斷（`[ -e ]`／`[ -L ]`，大小寫、symlink 都照 git 開檔的方式）有沒有 `HEAD`，以及 `commondir` 或 `objects/`＋`refs/`。git 的 `is_git_directory` 只接受同時有這些的資料夾（`objects/`、`refs/` 在 common dir，沒有 `commondir` 時就是資料夾本身），所以這個條件涵蓋 git 會接受的全部。
  - 符合條件的資料夾都以 `GIT_DIR` 交給 git：`rev-parse` 判斷是不是 git dir，`config` 讀設定（commondir、include、worktree 設定都由 git 自己處理），remote 檔案從 git 回報的 common dir 讀。git 不接受的當成讀不到（fail closed）。`GIT_WORK_TREE` 指向資料夾本身：submodule 的 `core.worktree` 指到已刪除的路徑時，git 會在 chdir 失敗，掃描不能因此略過或誤判。
  - `logs/`、`refs/remotes/origin/` 這類有 `HEAD` 的資料夾沒有 `objects/` 也沒有 `commondir`，不會叫 git。30 個 submodule git dir、8 千多個資料夾的合成例子，掃描約 0.75 秒。
  - 已 checkout 的 submodule 照舊用 gitlink 名稱只列一次（兩邊都以 git 回報的實際 git dir 路徑去重）。
- #211 的 Files（Linux 資料夾監看）只看共用的 `pending_from`，不含 yuzora-git 自己加的 partial clone key。實測 sparse partial clone 裡 `ls-files --others --ignored --exclude-standard` 會為了範圍外的 `.gitignore` 向 promisor remote 抓物件，所以不只是顯示差異。partial clone 的判斷移到共用掃描（`names_program`：`extensions.partialClone`、`promisor` 為 true、`partialCloneFilter`），yuzora-git 自己的 `partial_clone_keys` 移除。`kept_out`（git 擋下的 helper）只出現在連 remote 的指令；Files 的 `ls-files` 不連 remote（partial clone 已先等信任），這部分只是顯示差異。
- `push.recurseSubmodules=on-demand`：實測 git 2.50 不會進入未 checkout 的 git dir（submodule 的子程序以 `GIT_DIR=.git` 在空資料夾執行，直接失敗），只推已 checkout 的 submodule，而它們在掃描範圍內。（第六輪更正：這個結論錯了。實測只看了子程序，push 檢查 submodule 的 commit 時會在自己的 process 裡打開 `modules/<name>`，見第六輪。）
- 測試：三種藏法在 yuzora-git 端到端驗證（信任後 `ops::submodule_update` 真的會執行；大小寫敏感的檔案系統上 git 也開不到 `head`，兩邊一致），core 另測 git 不接受的資料夾與 `core.worktree` 不存在；Files 的 partial clone 測試先證明信任後真的會抓。

第六輪（reviewer 複審 `4ee825ea`）：

- blocker：`.git/modules/<name>` 本身是 gitfile（`gitdir: ../evil.git`）時，掃描只列資料夾，看不到它指向的 git dir。git 在自己的 process 裡打開 submodule 時（`repo_submodule_init` → `resolve_gitdir_gently`）會跟著 gitfile 走：repo 設了 `push.recurseSubmodules=check`／`on-demand` 或 `submodule.recurse` 時，push 檢查 submodule 的 commit（`find_unpushed_submodules`）就會打開它。若它是缺那個 commit 的 partial clone，git 2.44 以前（不認得 `GIT_NO_LAZY_FETCH`）會 lazy fetch，執行它的 `remote.origin.uploadpack`；`reach` 放行 file 傳輸。第五輪「push 不會進入未 checkout 的 git dir」的說法因此是錯的。兩個修正都做：
  - (a) 掃描：`modules/` 底下開頭是 `gitdir: ` 的一般檔案（`find -L -type f`，指向檔案的 symlink 也算）也交給 git（`GIT_DIR=<檔案>`，git 會跟著 gitfile 走），讀它指向的 git dir 與那個 git dir 自己的 `modules/`；git 不接受的當成讀不到。git 只看檔案開頭、不讀超過 1 MiB 的 gitfile（實測 512 KiB 接受、1.5 MiB 拒絕），掃描先用 grep 篩 2 MiB 以下的檔案，再比對第一行。（第七輪更正：指向 gitfile 的 symlink 有兩種基準，`GIT_DIR=<link>` 和 git 自己打開 submodule 時讀到的不一定是同一個 git dir，見第七輪。）
  - 其他讓 git 從 submodule 名稱解析出 git dir 的方式：`modules/<name>` 是資料夾（第五輪）、gitfile 或指向兩者的 symlink（這一輪）、git 2.54 起 `extensions.submodulePathConfig` 的 `submodule.<name>.gitdir` 可以指到任何地方。最後一種不另外解析路徑，這個 key 本身就要等信任（在 repo 自己的設定與每個 submodule git dir 的設定裡都算）。push 的檢查、`diff.submodule=log` 這類在同一個 process 裡打開 submodule 的讀取都經過這三種，所以掃描涵蓋它們，不必逐一處理。
  - (b) 未信任時 push 加 `--recurse-submodules=no`，和 fetch、pull 一致。（第七輪更正：這一輪只改了 branch push，tag push 沒改，見第七輪。）
- 測試：core 測 gitfile、指向 gitfile 的 symlink、gitfile 指向的 git dir 自己的 `modules/`、git 不接受的 gitfile（讀不到）、第一行不是 `gitdir: ` 的檔案（不算）與 `submodule.x.gitdir`；yuzora-git 端到端照 reviewer 的重現：gitfile 指向缺 commit 的 partial clone，用一個拿掉 `GIT_NO_LAZY_FETCH` 的 git（`GIT_EXEC_PATH` 換成包裝過的 git）模擬 2.44 以前，未信任時 push 不會執行 `uploadpack`；把 remote 退回後信任再 push，確實會執行，證明測試有效。
- 30 個 submodule git dir、8 千多個資料夾的合成例子，掃描約 1 秒；這個 repo（ghostty 的 git dir）約 0.12 秒。

第七輪（reviewer 複審 `d4486750`）：

- blocker：`.git/modules/<name>` 是指向 gitfile 的 symlink、gitfile 裡寫的是相對路徑時，掃描用 `GIT_DIR=<link>`，git 以 link 所在的資料夾為基準解讀這個路徑；git 在自己的 process 裡打開 submodule 時（`repo_init` 先 `real_pathdup`，再 `resolve_gitdir_gently`），以 gitfile 本身所在的資料夾為基準。兩邊得到不同的 git dir：掃描讀到乾淨的那個，push 打開的卻是缺 commit 的 partial clone。另外第六輪的 (b) 只改了 branch push，`tag_push`（單一 tag，以及 pushAll 的 `push --tags`）未信任時仍會檢查 submodule。git 2.44 以前，這兩種 push 都會執行 `uploadpack`。兩個修正都做：
  - (a) 掃描先取實際路徑（`real`：逐層 `readlink` 解開最後一段的 symlink，最多 40 層，再以 `cd -P`／`pwd -P` 取所在資料夾），用實際路徑交給 git，基準和 `repo_init` 一致。路徑經過 symlink 時，link 基準也照舊讀一次，兩個 git dir 都算（以 git 回報的實際 git dir 去重）。取不到實際路徑（沒有 `readlink`、超過 40 層）當成讀不到。資料夾候選不必取：資料夾裡沒有相對路徑要解讀（`commondir` 也照實際路徑解析），兩種基準是同一個資料夾。
  - (b) `ops.rs` 每個 push 都改用 `push_args`，未信任時加 `--recurse-submodules=no`。共 5 處：`push`、`tag_push`（單一 tag 與 `--tags`）、刪除遠端 branch（`branchDelete`）、刪除遠端 tag（`tagDelete`）。只刪除的 push 沒有要推的 commit，git 不檢查 submodule（實測 `push.recurseSubmodules=check` 擋下 branch push，刪除照常成功），所以後兩處沒有行為差別，只是寫法一致。
- 測試：core 新增「symlink 指向更深一層的 gitfile」，兩種基準各放一個 key，兩個都要列出，link 基準那個 git dir 自己的 `modules/` 也要列出。yuzora-git 端到端照 reviewer 的重現：`.git/modules/s` 是 symlink，link 基準是乾淨的 bare repo，實際基準是缺 commit 的 partial clone；用舊版 git 模擬，未信任時 branch push、單一 tag push、push 全部 tag 都不執行 `uploadpack`，信任後三種都會執行，證明測試有效。每次 push 前把 remote 退回，並刪掉 partial clone 抓進來的 pack，否則只有第一次會 lazy fetch，信任後的對照組會失效。

第八輪（reviewer 複審 `59fc4849`）：

- blocker：kernel 跟不過去、git 卻打得開的 symlink，掃描既不讀也不記成讀不到（fail open）。git 打開 submodule 前用自己的 `strbuf_realpath` 取實際路徑：`x/..` 不管 `x` 是不是資料夾都在字面上去掉，結尾多一個 `/` 的檔案照樣接受，跟 symlink 的層數也比 macOS kernel（32）多。kernel 在這些地方回 ENOTDIR 或 ELOOP，`find -L` 不把它當資料夾也不當檔案，`[ -d "$3/modules" ]` 也直接略過。reviewer 的 C1–C5（link 指向檔案結尾加 `/`、`x/../` 經過一般檔案、指到資料夾也一樣、`.git/modules` 本身這樣寫、33 層 link 鏈）都讓 git 在自己的 process 裡打開了 submodule（`diff --submodule=log` 印出 `> two`）。
- 修法：不再列舉寫法。`modules/` 底下（同一個 `modules()`，所以巢狀與 linked worktree 的 `modules/` 也一樣）`find -L -type l` 列出的就是 kernel 跟不過去的 link，一律當成讀不到；`.git/modules` 存在但不是 kernel 打得開的資料夾，也當成讀不到。GNU find（Linux）遇到 ENOTDIR、ELOOP 的 link 會報錯結束（exit 1），只有目標不存在的才列成 `-type l`，這時整個 `modules/` 當成讀不到，一樣 fail closed（ubuntu 24.04 的 findutils 4.9.0 實測）。git 沒有指令能對任意路徑做 `real_pathdup`，沒辦法以 git 的解析為準，所以 fail closed。kernel 跟得過去的 link 照舊：kernel 與 git 都打得開時，兩者解析到同一個地方。
  - 連帶效果：只存在 packed-refs 的 symlink HEAD 對 kernel 也是斷掉的 link，第五輪的兩個測試多了一筆 `…/HEAD` 讀不到（git dir 的設定照樣讀到）。見〈已知限制〉。
- 測試：core 的 `a_link_the_kernel_cant_follow_shuts_the_gate` 每種寫法各建一個 repo，先證明 git 在自己的 process 裡打開了 submodule，再檢查掃描：C1–C5 都是讀不到，指到資料夾、結尾加 `/` 的 link（C7）與 32 層鏈照常讀到 key。Linux 上 C1–C3 記成整個 `.git/modules` 讀不到；Linux kernel 可以跟 40 層，33 層鏈照常讀到 key。修正前 C1–C5 都是 `Ok([])`。

### #211 審查的 follow-up

#211 的 reviewer 提出三項（F1–F3），報告原文已遺失，依交接檔的摘要處理。

- F1：Changes 的 Commit 與單檔 diff、Worktree 對話框的 Enter 用的是 refresh 或對話框開啟時的信任答案。之後 repo 設定若多了程式（例如 `git lfs install --local` 加上 clean／smudge filter），仍會照跑。現在三處在執行 git 前都再問一次 `yuzora:git`。測試先建一個沒有程式的 repo，讀過答案後把答案改成「等信任」，三處都不再執行 git；Worktree 也不會關掉對話框。拿掉任一處重新判斷，測試都會失敗。
- F2：計數守衛只數 `exec("git", …)`，透過 mod 既有的 `git()` helper 新增的呼叫數不到（reviewer 在 Worktree 的 Escape 加了 `status`，測試照樣通過）。現在連 helper 的呼叫數一起守住，那個 mutant 會讓測試失敗；守不到的範圍寫在上面〈測試〉。
- F3：git.md 的「判斷方式」與「已知限制」各有一句與實作不符，依實作逐句核對後改正：「已信任的資料夾」改成「沒有待信任的項目時」，並寫明遠端 machine 沒有 watcher；信任掃描除了啟動時，Linux 每次重新掃描資料夾也會再跑，20 萬筆的時間重新實測。

## 互不相依的讀取一起發出（beta.3 效能）

在另一台機器上，每個 git 或檔案讀取都是一次 `hc exec` 往返（WSL 上還要啟動 `wsl.exe`）。下面三處原本循序、彼此不相依的讀取，現在用 `thread::scope` 同時發出，每個呼叫仍各自走 `exec::run`（trust guard、逾時、輸出上限、取消都不變），結果依原本的順序判斷（同一個錯誤先被回報）：

- `DiffSession::load`：index 項目、HEAD／commit 那一側、工作樹、mode 比對一起發出。**index 那一側例外**：等 index 項目回來之後，用它的 object id 讀（`cat-file --filters --path=<path> <oid>`），不用 `:path`，這樣 session 裡的內容和之後 `check_index` 檢查的 id 是同一份快照（另一個 git client 在兩次讀取之間 `git add` 時，不會拿到舊 id 配新內容而蓋掉對方 stage 的東西）。unstaged 遠端 5 輪 → 2 輪。unstaged 在 index 沒有該檔時不要左側，這時就算 `cat-file` 那一路出錯也不算（原本根本不會跑）。
- `log::read_refs`：`for-each-ref`、`symbolic-ref`、`rev-parse`。3 輪 → 1 輪。`rev-parse`／`symbolic-ref` 非零（unborn）仍不是錯誤。
- `status::full` 的 `operation()`（遠端要 `ls .git`，rebase 中還有逐檔 `cat`）搬進第一輪的平行區，少一輪。
- （v0.1.0 E1）`status::full` 的兩個 `diff --numstat` 在 `status` 讀完、知道變更檔數沒有超過 `NUMSTAT_LIMIT` 的那一刻就出發，與兩個走整棵樹的 `ls-files --others` 並行，不再等第一輪全部結束；超過上限時不出發，和以前一樣略過。`ops::messages` 的 `config commit.template` 與 `log -1` 並行，並拿掉沒人用的 `log -n 300`（3 個行程串行變 2 個並行）。

`exec::spawn_scoped` 讓測試用的 `TEST_ENV`（thread-local）也傳給這些執行緒。測試用記錄開始／結束的 `git` 替身（`repo::testing::timed_git`）證明同時在跑的數量，並證明任何一路失敗都不會被當成答案。

## 右側模式列的版面與 commit 詳情的上下比例（beta.3）

- **右側模式檔案列**（`yuzora-sidebar-git` 的 `fileRow`）：檔名 `flex_shrink(1).min_w(0).max_w(140).truncate()`，資料夾 `flex_1.min_w(0).truncate()`（沒有 basis，先縮），按鈕 `flex_none` 貼在列的右緣；`+N`、`−M` 可以縮（`flex_shrink(1).min_w(0).truncate()`，排在檔名之後），6 位數以上寫成 `150k`（`count()`，上限 `999k+`），所以最窄的 180px 也放得下，比 180 更窄的自訂 layout 數字會被截斷但按鈕不會被推走。所以任何右側模式寬度（預設 248、最小 180）下，名稱用 `…` 截斷，數字與按鈕完整可見；`clickRow` 從右邊用 `ACTION` 算 slot 的算法不變（按鈕仍是 22px 寬、貼右緣）。section 標頭的箭頭與分支列的 `⎇`、`▾` 也補 `flex_none`。`fileRow(it, th, probe)` 的 `probe` 只有測試給：`probe(slot)` 回傳各部分的 `on_size_change`，`probe(slot, "move")` 回傳 `on_mouse_move`（帶著該部分的位置）；按鈕的 slot 帶標籤（`action:↺`），測試才分得出順序。`sidebar_git_row_buttons_are_where_clickrow_counts_them` 用真的 `fileRow` 加上和 list 相同的 padding 與 `downX`，點每顆按鈕的中心與左右邊緣內 2px，斷言 `+` → stage、`↺` → discard、`−` → unstage、數字與名稱 → 開 diff（virtual list 在測試視窗不排版，所以不經過它）。
- **commit 詳情不能捲動的原因**：`detail-info` 原本是 `flex_none().max_h(…).overflow_y_scrollbar()`。`overflow_y_scrollbar` 搭配 `max_h` 不會捲（同樣的 `h` 或 `overflow_y_scroll`＋`max_h` 都會）。現在是 `relative` 外框裡一個 `overflow_y_scroll` 的區域（有 id `detail-scroll`），上面蓋 `Scrollbar.vertical("detail-scroll")`；沒拖過時高度跟內容走、最多 300（並排）／110（上下），並且不超過 `infoLimit()`，矮的 pane 裡分隔線不會被推出畫面。
- **訊息完整顯示**：不再 `slice(0, 80)`。為了避免一則極端長的訊息建出數萬個元素，上限 `BODY_LINES = 2000` 行，超過時資訊區最後一行顯示「還有 N 行未顯示」。
- **訊息以段落為單位（beta.4）**：連續的非空行合併成一個文字元素（行與行以 `\n` 相連，文字自己換行），每段後面的空行以 `pb(8 × 空行數)` 表示；訊息開頭的空行才另外用 `h(8 × n)` 的空白。原本每行一個 div，2000 行要在 JS 裡建 2000 個元素，約 12ms，而 mod 的 render 時間上限是 50ms（慢機器、CI 會被 interrupt：run 41886892064、job 126797852443）；改後 2000 行約 2–5ms，這是空行少的訊息；空行與文字行交替是最壞情況（2000 行變 1000 段，約 1000 個 div），約 10–16ms（審查量測，CI 設定）。高度與每行一個 div 完全相同（`git_log_commit_message_paragraphs_lay_out_like_a_div_per_line` 並排量測；經 `detailPanel` 的高度由 `git_log_commit_message_paragraphs_in_the_panel_take_the_height_of_their_lines` 守護）。這是在 gpui 測試用的等寬假字型（NoopTextSystem）下量的；真實字型的等價來自 GPUI 先依 `\n` 把文字切成行、各行各自排版與換行的程式路徑（`text_system.rs` 的 `shape_text`），不是字型實測。上限 2000 行與「還有 N 行未顯示」不變。
- **上下比例**：資訊區與「變更檔案」標題之間有 7px 的分隔線（`detail-split`，游標是上下調整）。按下後 pane 的根節點接手 `on_mouse_move`／`on_mouse_up`，所以放開時不必在分隔線上；拖到 pane 內任何位置都跟著。資訊區最小 48px、檔案區最小 72px（標題 24px、分隔線 7px 另計）；面板縮小時，存下的高度也照最大值夾住。雙擊分隔線回到預設（內容高度、有上限）。高度存在 pane 的狀態裡（`infoH`），同一個 pane 換選 commit 會保留；不跨 pane、不跨重啟（每個 pane 大小不同，存一個絕對高度沒有意義，要存也該存比例）。
- 指標離開 pane（`on_hover` 變成 false）時拖曳就結束：mod 的滑鼠事件沒有「按著哪個鍵」，離開後的放開也收不到，所以不讓狀態留著。要拖得更遠就得在 pane 內重新按下。
- 未改：compare 畫面（`renderCompare`）的 `cmp-files` 仍是 `h(150)`。
- 測試（`app/src/mods/render_tests.rs`，真的把 mod 放進視窗排版、送滑鼠與滾輪事件）：`sidebar_git_row_keeps_counts_and_buttons_clear_of_the_name`、`git_log_commit_message_scrolls_and_the_files_stay`、`git_log_detail_divider_drags_and_keeps_minimums`、`git_log_commit_message_is_shown_whole_up_to_a_cap`、`git_log_commit_message_paragraphs_lay_out_like_a_div_per_line`。
- beta.4 評估：gpui-base 其實有 `v_resizable`／`resizable_panel`（`size_range`、`on_resize`），拖曳由 gpui 處理；之後可評估是否取代自製的分隔線。

## 並排 diff 的左右比例與橫向捲動（v0.1.0）

- **原本為什麼不是各半**：兩側各有一個最小寬度（號碼 44 ＋ 該側最長一行的字元數 × 7 ＋ 24，`sides()`），只要某一側有長行，那一側就撐到那個寬度，另一側被擠成一小條（新增檔案的空側 68px、一側長行時另一側 103px）；全形字被當成一格，中文行超過估值就畫過分隔線。目前的 hunk 另外有 2px 左邊框，那一列的分隔線比其他列偏 2px。`sides()` 與這條路徑已刪除。
- **比例**：左側佔寬度的比例 `ratio`，預設 0.5，限制在 0.15–0.85。兩側都是同一列裡的 flex 份額（`w(0).flex_grow(ratio × 100)`、`flex_grow((1 − ratio) × 100)`，中間 1px 分隔線不算），所以 pane 或對話框變寬變窄時兩側立刻等比例變，不經過量測再重畫；新增、刪除整檔（一側全空）也各半。不另存像素寬度。
- **持久化**：存在共用狀態 `yuzora.git.diff.split`（數字，寫進 `shared.json`，重啟後還在；所有 diff 共用同一個比例）。不用 `localStorage`：它依「mod 名稱＋props 的 hash」分檔，而 diff 每次開啟的 props 都不同（路徑、`at`），每次都會從頭開始。拖曳中的比例只在 pane 的狀態裡（`drag.ratio`），放開（或指標離開 pane）才寫入，不在每次滑鼠移動時 `setShared`（會讓所有 mod 重繪、寫檔）。不是數字、超出範圍的值：非數字當 0.5，超出範圍夾到 0.15／0.85。
- **分隔線**：疊在列表上的一條 5px 抓取區，用同樣的 flex 份額排版，所以在每一幀都剛好在各列分隔線的位置（resize 時也不落後一幀）。按下記起點，pane 根節點接手 `on_mouse_move`／`on_mouse_up`（同 Git Log 詳情的分隔線）；指標離開 pane 時拖曳結束並保留目前的比例。雙擊回到預設（寫入 `null`）。抓取區 5px 寬（分隔線 1px，兩側各 2px）：蓋住左側文字區右緣 2px 與右側行號區左緣 2px，其餘的行號與文字不蓋。
- **長行**：每一側的文字區是 `overflow_hidden` 的 flex row，文字是 `flex_none`，所以長行不會畫到另一側，捲動時也不會蓋到自己的行號。兩側共用一個橫向位移（`scrollX`，作用在文字的 `ml(-scrollX)`），用橫向滾輪／觸控板滑動（橫向分量大於縱向才算；縱向滑動帶一點橫向不會讓行漂移；同一幀內來的多個橫向滾輪事件各自從「目前儲存的位移」起算、累加，不是從上一幀畫出的位移起算，否則同一幀的兩個事件只有最後一個生效〔審查 r2 的 C〕）或列表下方的橫向捲軸（thumb 可拖；只在需要捲動時出現）。儲存的位移（`scrollX`）每一幀都夾進目前的可捲範圍：滾輪或 thumb 把它推過尾端、或 pane 變窄、分隔線移動讓範圍變小，都不會留下一個之後突然生效的位移（Codex 在 PR #250 的留言：兩側都放得下時滾輪存的 400px，pane 縮到 700px 後文字在沒有任何事件下橫移 112.5px）。列表與它的縱向捲軸在需要橫向捲動時停在橫向捲軸開始的地方（捲軸原本蓋在最後一列的下緣 8px，thumb 會吃掉最後一列行號的點擊）。
- **能捲多遠**：範圍 ＝ 最寬那一行「真正畫出來的寬度」加 12px（`REST`），減去該側可用的寬度。真正畫出來的寬度是把那一行放進和列相同的盒子量出來的：`line()` 這一個函式建出「縮排 6px ＋ 文字（有字詞高亮時每一段各是一個文字元素）」的盒子，列（`cell()`）用它畫，量測（`probes()`）也用它排版，放在 body 看不見的地方（`h(0)`、`overflow_hidden`），用 `on_size_change` 取寬度。所以分段、字型、字級、每個文字元素的寬度被進位到整數 px，都和畫的一樣；沒有另一套「估」的算法會跟畫的漂移（審查 r2 的 A：整行當一段量，每段的進位算不到，360 個字元差 24px、1000 個差 88px）。量到的寬度以 script 單位存（`unzoom()`），縮放或字型改變時重量。
  - **怎麼找最寬那行**：不排版所有行，而且每一步的工作量都有上限；唯一隨列數長的是排完最後一列後的原生排序（每側一次，見步驟 1；release 下 60 萬列約 65 ms，CI 設定下 20 萬列約 35 到 40 ms），它自己佔一份，不和別的工作同一份。第四輪之前，排名在 `show()` 一次走完所有列：CI 設定〔依賴 opt-level 0〕下 10 萬列的 `show()` 就要 224 到 331 ms（測試的計數 Proxy 再加上去，超過事件的 500 ms），pane 停在 loading，`App (build · test)` 因此是紅的；release 下是 60 萬列整檔 context 336 到 346 ms，依斜率約 90 萬列才用完 500 ms（外推，沒有量到）。
    1. **排名**（`scan()`）每次最多處理 `SLICE`（200 萬單位）的列：一列算 `STEP`（100），讀一行另算 `TOLL`（200）＋ 字元數 ＋ 每個非 ASCII 字元 `SLOW`（25）。載入（`show()`）**不排任何列**：它是一個 task，被引擎中斷會連「跟著檔案變動」的迴圈一起結束，pane 就停在 Loading。列還沒排完時（第一份也是），每一幀在看不見的地方放一個 id 每次嘗試都不同的小元素（`rank-<已排的列數>-<嘗試次數>`），它的 `on_size_change` 在事件裡（預算 500 ms，不是 render 的 50 ms）**先 `notify`** 再排下一份。排名為兩側的每一行算「最多能佔幾格」（`bound()`，有高亮時每多一段再加半格；兩側相同的未變更列只讀一次，上界兩側共用），把（上界，列號）編成一個數字放進 `Float64Array`，排完最後一列後，兩側各做一次原生排序，每側算 `ORDER`（6）單位一行，且只在這一份還有剩的時候做：兩側的列很多時，後一側等下一份（量過：20 萬列整檔 context 最長一份 122–127 ms → 86–87 ms；小 diff 兩側都在第一份做完：第一幀的元素排它，再一幀排版）；render 不碰它。
    2. **排版**（`batch()`）排名完成後，每一幀從上界最大的開始取幾行放進看不見的盒子排版：第一幀每側 1 行，之後每幀 4 倍，一批最多 `FRAME`（32768，每行算字元數＋每段 16＋64）。一行單獨就超過 `FRAME` 的自己一批，而且一幀只讓一側排這種批（兩側各一條 20 萬字元的行，原本第一幀排了 465,683 個字元），最長 `LAYOUT`（26 萬字元）、最多 `TEXTS`（4096 段），再長的不排版、以上界計。所以一幀排版的量最多是 `FRAME`，或一條不超過 `LAYOUT` 的行，另加第一幀（以及縮放、字型改變後）一把 65536 個 W 的尺。**沒有「總共最多排多少」的上限**，排到停止條件成立為止（`MEASURE`〔26 萬〕的總量上限在第四輪拿掉：用完就停在上界，1,500 行各 150 個希伯來字母，範圍是 2,570px，要捲的是 498px）。
    3. **停止條件**（`settled()`）：還沒排版的行裡最大的上界，不超過已量到的最大寬度加 1 個視窗像素。停了就不再排任何行（probes 從樹上拿掉）。一般的程式碼 diff（ASCII 行的上界就是真實寬度）排名一份就完成、排版第一幀只排 1 行；格數表只決定順序與要排幾行，不決定範圍：表把某種文字高估了，就多排幾行（5 行各 150 個阿拉伯字母：上界 3 倍，2 幀排完那 5 行；每幀的行數 1、4、16、64……4 倍成長，受 `FRAME` 限制）。
  - **被引擎中斷時**（第五輪）：引擎對一個事件給 500 ms、對一幀給 50 ms，超過就中斷，而且是不能 `catch` 的錯誤（`finally` 也不跑）；中斷之後什麼都不會再叫 pane 重畫：事件到不了它尾端的 `cx.notify()`，失敗的 render 不會被再建（引擎留著上一個好的畫面加一條錯誤橫幅，直到 view 被 invalidate）。所以一份排名或一批排版被中斷，pane 就停在當時的範圍。作法有四個部分：**(1) 標記。** 一份排名開始時在 `fit.trying` 記下（meter、開始的列、份量），回來時清掉；一批排版在 `probes()` 建出來時記在該側的 `trying`，`render()` 的 `finally` 回來時清掉；整個 render 也有 `framing` 旗標。下一次開始時標記還在，就是上一次被中斷了。**(2) 退讓。** 被中斷的那一份，下一份是「它做完的量」的一半（至少 1 單位，也就是一列）；被中斷的那一批，下一批的成本上限（`g.frame`）是它成本的一半。一份連一列都沒做完就被中斷（`fit.at` 沒動），先照原樣再試一次（`fit.retry`），再被中斷才算那一列自己就超過預算，記在 `fit.hurt`，之後以上界（每單位 3 格）計、不讀；一批只有一行還被中斷，同樣先再試一次（`alone`），再被中斷那行才跳過、以上界計（`blind`），和太長不排版的行一樣。一次中斷不是對那一列的判決：機器停一下（GC、系統忙）會中斷當時正在做的任何事，而每個 diff 的第一個排版幀一定是單行批次（5 行各 150 個希伯來字母，第一幀被中斷一次，範圍曾是 2,569px，實際 497.5px，而且不會恢復）。一幀裡兩側都有批次時，成本較高的那一側才是原因（成本相同則兩側都是），另一側的批次照原樣再要一次（它的 probe id 仍要換，見 (4)）。**份量會長回來。** 一份（一批）沒有被中斷，下一份是兩倍，上限是「被中斷的那一份（那一批的幀上限）的一半」：被中斷過的大小不會再要，所以機器真的只做得了那麼多時，中斷的次數只有幾次（大小一路對半，到做得了的量為止），不會鋸齒式地每隔幾份就中斷一次；機器只是停了一下時，份量在幾份之內回到上限的一半，不會一份只做幾列（一次很早的中斷曾讓 2 萬列排了 3,662 份，一份一幀）。每中斷一次，上限再減半，所以同一個 diff 的排名中斷很多次時份數會增加，不會增加到無限。所以最後一定有最小單位，不會無限重試，範圍也不會低於畫出來的任何一行。**(3) 叫下一幀。** 事件在工作之前先 `notify`（`again()`）；render 不能 `notify`，所以事件同時設一個 `LATE`（120 ms）的計時器，時間到了還沒有新事件、而還有東西沒量完（或上一幀沒回來），就 `notify`，上一幀被中斷（`framing`、`trying`）的話每 `LATE` 再叫一次，最多連續 `RETRIES`（40）次。**(4) 每次嘗試的 probe id 不同**（`rank-<列數>-<嘗試次數>`、`gauge-…-<被中斷次數>`）：同一個 id 同樣大小的元素引擎不會再回報，被中斷的那份就沒有人再叫。量完（`settled`、排完名）之後沒有標記、沒有 probe、計時器醒來也沒事做，所以不再建任何一幀。`scan()` 被中斷在任何地方都不掉列：一列要寫的東西寫在「再寫一次結果相同」的位置（鍵寫在 `l.n`／`r.n` 指到的格子，不是 `l.n++`），計數（`l.n`、`r.n`、最後是 `fit.at`）在同一段沒有呼叫、沒有分支的連續敘述裡一起設（引擎只在呼叫與分支的地方看時鐘）；原本是先 `fit.at++` 再寫，中斷在中間那一列就永遠不在排名裡。被中斷的重讀（`few` 多了一個重複值）只會讓門檻偏高，多讀幾行，上界不會變低。測試用的中斷是真的中斷：測試探針（`DIFF_PROBE`）在指定的量以上讓 script 一直跑下去，直到引擎自己的時鐘中斷它，所以不依賴機器的快慢。常設的上限（超過就一律中斷）：`cut_event`（一個事件的排名單位）、`cut_render`（一幀的批次成本）。只中斷一次（像機器停了一下）：`stall_slice`（第一份做到這麼多單位時）、`cut_first`（第一個有批次要排版的幀）、`cut_big`（第一個批次的行數多過這個數的幀）、`cut_last`（第一個什麼都不用再找的幀）。
  - **收斂中的範圍，以及拖著 thumb 的時候範圍改變**：排名還沒完成時，範圍是「已排名的行裡上界最大的那行」（`remain()` 回 `top`）。這只在一份（`SLICE`，約 3,000 到 7,000 列，看行長與兩側是否都有行）排不完的 diff 才看得到：捲軸隨排名變長，只變長，不會超過整份排完的上界；排完名之後是「沒排版的行的最大上界」，之後每排好一批就縮向真實寬度。這個範圍在排名中**不是**整份 diff 的上界（還沒排名的列沒有比 O(n) 更便宜的上界可用），所以 100 萬列的 diff 在最後幾份排完之前，能捲的比最終的短；沒有任何行會因此被卡在範圍外超過排名的時間（100 萬列約 150 到 300 份，每幀一份；一次中斷之後上限減半，最多約兩倍）。**同一個 diff 被重讀時**（存檔、stage、切空白或 context、repo 的任何 index／ref 寫入）排名要重來，而它的範圍在排完前只是「已排名的行」，常常是 0：位移若照這個範圍夾，使用者正看著的橫向位置會被夾成 0 而且不會回來（2 萬列、最寬的行在最後一列的 diff：位移 13,264px，重讀後的 5 幀是 0，之後也是 0）。所以 `show()` 把上一幀兩側的寬度（`shown`，量好之後是真實寬度）留成下限（`floor`），排名完成前每側的範圍至少是它（`later()`）：位移與 thumb 不動；排完之後換成新 diff 的範圍（新的上界，之後隨排版縮向真實寬度），位移到那時才被夾進去，所以「新內容的範圍確定比舊位置小」才會把位移夾小。拖著 thumb 重讀時 thumb 也留在指標下面，不會有範圍為 0、thumb 消失的幾幀。上界高過真實的文字（阿拉伯文、希伯來文、泰文，上界 3 倍）會先高估再縮，縮的次數等於排版的幀數（1,500 行各 150 個希伯來字母約 13 幀，3,000 行約 24 幀；測試 `yuzora_diff_many_lines_in_a_script_the_table_does_not_list_scroll_to_where_the_widest_ends`）。滾輪的位移照常累加（從 `scrollX` 起算）、每幀夾進當時的範圍。**拖著 thumb 時範圍改變**（排名或排版收斂、pane 變窄、分隔線移動）：thumb 留在指標下面，位移跟著 thumb 走（`drag.at`＝thumb 的位置，位移＝範圍 × 位置／軌道），不是位移不動、thumb 跑到別處（測試 `yuzora_diff_a_held_thumb_stays_under_the_pointer_when_the_range_changes`）。
  - **上界要是真的上界，怎麼保證**：停止條件信任上界，所以上界不能低於畫出來的寬度。(1) 每個字元的格數取自量測表並留餘裕，bun 測試對表裡每個字元斷言 `bound ≥ 量到的格數`；(2) 每個文字元素最多多進位 1 個視窗像素，換成格數是 ≤ 0.5 格（zoom 最小 0.5〔`ZOOM_MIN`〕時一格 3.45px，1px 是 0.29 格），所以每多一段加半格，第一段的進位由停止條件的 1 個視窗像素容差吸收；(3) 縮排 6px 另外加；(4) 一格的寬度由 65536 個 W 的尺量出，尺的進位誤差是每格 1/65536px，只會讓上界偏高。bun 的隨機測試（400 次 × 0.5 到 3 的七種 zoom，一半的行分段）用同一份 `batch()`／`settled()`／`measured()`／`span()`，搭配「照量測表與進位排版」的假視窗，斷言找到的範圍不短於最寬那行真實寬度超過 2 個視窗像素（停止時最多差這麼多：一行的上界在 1 個像素內追平已量到的最寬那行）。**仍會破的條件**：某個字元在使用者的字型裡比它所屬的類別的格數還寬（例如比 0.5em 還窄的字型裡的中日文、U+FDFD 這種單一字元極寬的字）；這時沒排版的那行可能比量到的最寬那行寬，範圍短那些字元的超出量（`REST` 的 12px 是它的餘裕）。
  - **仍會高估的情況**：(1) 超過 `LAYOUT`（26 萬字元）或 `TEXTS`（4096 段）的行不排版，範圍取它的上界（ASCII 為真實寬度，中日文 1.2 倍，表上沒有的文字最多 3 倍）；(2) 一行只讀前 `LINE`（100 萬）字元，其餘每單位當 3 格（超過 100 萬字元的行，範圍多出尾巴的 2 倍）；(3) 一行的非 ASCII 字元超過 `SORT`（6.5 萬）個就不分類（當 3 格）；(4) 排版的行最多差 1 個視窗像素就停。排名分份不讀就是不會高估：整份 diff 的每一行都讀過（讀取不再有全域預算，原本的 `LOOK` 400 萬字元用完後，沒讀的行當 3 格，20,000 行各約 100 個 ASCII 字元的資料檔要捲 1,618px，實際 221.5px）。已經讀過的最寬 4 行都比不上的行（每單位 3 格也比不上）不讀，只放進排序，它的鍵仍是上界。(5) 一行單獨排版兩次都被中斷才以上界計（一次中斷不算，見上）；排名時一列單獨被中斷兩次才以上界（每單位 3 格）計，這一列若又超過 `LAYOUT` 就不會再被排版，範圍多出 2 倍。
  - **被中斷時仍有的限制**：(1) 計時器（`LATE`）醒來時如果還沒有東西被中斷，只 `notify` 一次就結束；之後那一幀才被中斷的話，沒有人再叫下一幀，pane 停在最後一個好畫面加錯誤橫幅，直到使用者操作（讀碼推得出，沒有重現：測試 harness 每次 `flush_effects` 都會畫髒的視窗，notify 之後的幀永遠在計時器之前；PR #250 審查 r5 的 U1）。(2) `RETRIES`（40）用完後同樣停住：連續 40 幀都被中斷，表示這台機器畫不出這個 pane。(3) 中斷的原因只知道是「這一幀超時」，不知道是哪一行：兩側成本不同時只算成本較高的那一側，但成本是排版的字元數與段數，不是時間；成本相同時兩側都算。(4) 份量的上限（`top`）只會減半、不會回升：同一個 diff 的排名中斷很多次，份量會越來越小（每次中斷減半；重讀會換一個新的，從頭再來）。
  - **載入的時間**（第四輪的設計：`show()` 排第一份；第五輪起 `show()` 不排任何列，第一份在第一幀的事件裡，一份的成本不變，所以下面 `show()` 的數字就是第一份的數字，合併〔unified〕的 `show()` 仍走過所有列）（`yuzora_diff_how_long_taking_a_diff_in_takes`，`#[ignore]`，手動跑：`YUZORA_DIFF_ROWS=100000,400000 cargo test --release --bin herdr-app yuzora_diff_how_long -- --ignored --nocapture`；`show()` 與每一份排名用 mod 自己的時鐘量，列是 plain rows、不經計數的 Proxy；機器 load 3 到 5，每個案例各跑 3 次，下面是範圍）。**CI 設定**（`--config 'profile.dev.package."*".opt-level=0'`，引擎也是 opt-level 0）：`show()` 修改前 → 修改後，新增整檔 10 萬列 224–259 → 48 ms、20 萬列 350–363 → 63–64 ms；整檔 context 10 萬列 297–314 → 53–60 ms、20 萬列 477–482 → 53–56 ms；左右都改 10 萬列 298–331 → 55–77 ms、20 萬列 476–499 → 55–57 ms。修改前 20 萬列的 context 與左右都改已貼著 500 ms；修改後 `show()` 與列數無關（約 50 ms，這部分是引擎要先熱起來）。排名每一份最長：10 萬列 56–62 ms（左右都改有一次量到 165 ms，是 load 最低的那一輪，沒有重現，原因不明）、20 萬列整檔 context 86–87 ms、左右都改 61–62 ms、新增整檔 49–51 ms（排序拆份前，20 萬列 context 是 122–127 ms、左右都改 95–97 ms）。離 500 ms 約 6 倍（165 ms 那一次只有 3 倍）。**release**（整份 `--release`）：`show()` 修改前 → 修改後，新增整檔 10 萬列 58–61 → 12–13 ms、40 萬列 149–151 → 11–13 ms、60 萬列 205–213 → 12 ms；整檔 context 10 萬列 79–83 → 13–15 ms、40 萬列 234–238 → 14 ms、60 萬列 336–346 → 14–15 ms；左右都改 10 萬列 78–82 → 14–15 ms、40 萬列 235–246 → 14–18 ms。修改前依這個斜率（context 每千列約 0.5 ms，新增整檔約 0.3 ms），`show()` 在約 90 萬列（context）、150 萬列以上（新增整檔）才會用完 500 ms；這是外推，沒有量到，機器忙的時候早得多（同一個案例在 load 約 20 時量到新增整檔 60 萬列 422 ms）。所以 CI 的紅是 opt-level 0 的引擎造成的，release 離上限很遠。修改後排名每一份最長（排序拆份後）：10 萬列 19–24 ms、40 萬列 38–51 ms、60 萬列 65–68 ms（拆份前 60 萬列 context 121–126 ms）；這一份是一次原生排序，成本隨該側的行數成長，100 萬列約 110 ms（外推）。**沒修的上限**：60 萬列的左右都改，以及任何 100 萬列的 diff，修改前後 `show()` 都沒被呼叫到：測試 harness 把 view 轉成 JS 的值（複製、轉成 host 值、再轉成 JS 物件）自己就超過事件的 500 ms（100 萬列光是 `view.clone()` 加 `to_host` 就量到 150–520 ms）。真正的 app 這一步的成本沒量，要修只能在 Rust 側分頁回傳列（`diffView`），不是這個 mod 能做的。合併（unified）沒有分份，release 下 `show()` 10 萬列 8–10 ms、40 萬列 35–39 ms，CI 設定下 10 萬列 52 ms、20 萬列 102 ms，與列數成正比。收斂（要幾幀）：一般程式碼與整檔 ASCII context，排名一份就完成、1 幀排版；排名要的份數 ≈ 列數 ÷ 3,000 到 7,000（10 萬列新增整檔 15 份、context 17 份、左右都改 30 份）；排版的幀數見上一條。每幀 render 在 CI 設定下沒有超過 50 ms（20 萬列畫得出來是測試）。
  - **CI 上的收斂**（第五輪，取代第四輪的推算；run 42809949935）：兩個紅燈都不是「慢機器本來就慢」。(1) `…a_line_in_a_thousand_pieces…` 是測試算錯：字型一個字母 6.9px、每一段向上進位，32 個字母是 ceil(220.8) = 221px，不是 224（7px 一字）；差的 3,000px 是 1,000 段各差 3px，產品的範圍 220,462.5px 本來就等於畫出來的寬度（1000 × 221 + 6 + 12 − 555.5）。(2) `…taking_a_diff_in…` 的 403 幀：不是第四輪診斷的「render 被中斷、隱藏批次沒有退讓」。在 `show()`、`ranked()` 前後記下開始與結束，本機在 60 個忙碌迴圈下重現：最後一個 `begin ranked` 沒有 `end`，是事件預算把第三份排名中斷了（CI 上 6 萬列那一例最長一份已是 422 ms，20 萬列的一份約 1.7 倍），被中斷的事件到不了尾端的 `cx.notify()`，沒有任何東西再畫它，`pending` 永遠是 1。更慢的機器（90 個忙碌迴圈）連 `show()` 本身都被中斷，pane 停在 Loading。作法見上一條。**怎麼驗證收斂不看機器速度**：退讓用單位數，不用毫秒。測試用 `cut_event`／`cut_render`／`cut_last` 三個測試用 prop 讓 script 自旋到引擎自己的時鐘中斷它，再數 `script_renders()` 與批次數（`batches()`），沒有任何斷言比較毫秒。慢機器：用 SIGSTOP／SIGCONT 把測試行程每 23 ms 凍結 20 ms（只凍結該行程，引擎預算用的時鐘一起慢，是均勻變慢，不是超額排程），整組 `yuzora_diff`（48 個）耗時 379.8 s，不凍結是 73.7 s，約 5.2 倍（印出的每份排名毫秒數比也約 5.3 倍），全過；修正前同一個倍率紅過一個（拿幀數相等當斷言），已改成批次數。CI 設定（`--config 'profile.dev.package."*".opt-level=0'`）的 `mods::`（149 個，4 執行緒）重複 6 次全過：無負載 64.5、67.5、74.7 s；旁邊 12、24、48 個忙碌迴圈時 113.3、265.1、362.7 s（約 1.7、4.0、5.4 倍）。整個 `cargo test`（CI 設定、4 執行緒）跑了兩次，第二次 604 個全過；第一次有 4 個 `files::tests`（檔案監看，等 FSEvents 最多 10 秒，不是這個 PR 加的）逾時，單獨跑 21 個全過，沒有再重現。
  - **每個 UTF-16 單位最多幾格**（取自 `app/scripts/fixtures/yuzora-git-diff-font-widths.json`，用 CoreText 在 JetBrains Mono 11.5px 量的，含系統 fallback；重量：`swift scripts/yuzora-git-diff-font-widths.swift assets/fonts/JetBrainsMono-Variable.ttf 11.5`）：ASCII 1；拉丁、希臘、斯拉夫、標點、箭頭、框線 1.25（量到 1）；中日文、全形、韓文 2（量到 1.67，韓文 1.44）；surrogate pair 的每個單位 1.5（emoji 量到 2.17，一對算 3）；其餘（含阿拉伯文、希伯來文、泰文、U+2600–27BF 與 ✅ 一類）3。這張表只用來排順序和判斷何時停，不是寬度。
  - 修之前：PR #250 的第一版在 render 內對含非 ASCII 的行逐字掃描並先寫快取再算，約 40 萬字元就超過 50 ms 的 render 上限，pane 停在 loading（審查 F1）；第二版取上界最大的 4 行排版，範圍取「量到的最大值」與「沒排版的行的最大上界」較大者，字元表一錯就高估或低估（審查 F2、r2 的 B），而且量測把有高亮的整行當一段排，真正的列每段各排一次（r2 的 A）；滾輪以上一幀畫出的位移為起點，同一幀的兩個滾輪只有最後一個生效（r2 的 C）。
- **縮放（zoom）**：script 寫的每個長度都會被 app 乘上 zoom（`gpui_shell::set_pixel_scale`），但 `on_size_change` 回報的尺寸、滑鼠位置、滾輪量都是視窗 px。pane 存的與算的一律是 script 的單位，換算只有 `unzoom()` 一處，在收到的地方做：隱藏排版的文字寬與一格寬回報時就換（字型大小也是 script 單位，所以縮放時它們不變）；列區的寬度存成回報的視窗 px、讀的時候才換（`rowsWidth` getter，因為縮放改變時那個盒子的視窗 px 沒變、不會重新回報，回報時換算再存會一直停在舊縮放）；滾輪量、拖曳的位移、按下的位置（`downX`）在事件裡換。所以縮放在 diff 開著時改變，下一幀的範圍、thumb、位移夾限都是新縮放的。修之前（PR #250 第二輪實機）：zoom 1.2 捲到最右整片空白，0.8 行尾到不了、thumb 停在軌道 80%，滾輪與 thumb 多動（或少動）zoom 倍，行號的挑行命中範圍用 44px 而不是 44 × zoom。分隔線的拖曳與抓取區本來就對（都是視窗 px 對視窗 px，抓取區是排版出來的），一併由測試守住。unified 沒有量測出來的長度，只有按下的位置要換算。
  - 測試（改縮放的測試在自己的行程裡跑：`alone()`，zoom 是整個行程共用的一個值）：`yuzora_diff_the_sideways_scroll_ends_where_the_longest_line_does_at_every_zoom`（0.8、1、1.2、1.5、2，排版的行與沒排版的行：捲到底行尾離邊緣的距離與 zoom 1 的一樣乘上 zoom、thumb 的長度與位置和實際可捲距離一致、到底時貼齊軌道末端）、`yuzora_diff_a_zoom_set_while_it_is_open_holds_from_the_next_frame`、`yuzora_diff_the_pointer_and_the_wheel_move_what_they_touch_at_every_zoom`（滾輪、thumb 拖曳、分隔線拖曳與抓取區、並排與 unified 的行號點擊）。
- **目前 hunk 的標記**：原本是列的 2px 左邊框（會讓那一列的內容整個右移 2px），改成蓋在列左緣的 2px 色條（絕對定位）。並排才這樣改，合併（unified）維持邊框。
- **合併（unified）**：行為不變（`overflow_x_scrollbar` ＋ 以最長行字元數估的最小寬度，同樣有「全形字估成一格」的問題，沒有改）；只是「最長行的字元數」原本每個 render 掃過所有列，改成在 `show()` 算一次。這一段沒有分份：是一個很便宜的迴圈（release 下 40 萬列約 35 到 40 ms），成本與列數成正比，上限是 Rust 側的 `MAX_LINES`（每側 100 萬行），約 100 到 200 ms；沒有量過 CI 設定以外的慢機器。
- **鍵盤與滑鼠**：行號點擊（挑行）、點列選 hunk、hunk 導覽鍵不變；`clickRow` 的命中座標改用最後一幀實際的比例（`lay.ratio`）。
- 測試（`app/src/mods/lifecycle_tests.rs`，真的把 mod 放進視窗排版、送滑鼠與滾輪事件；列表的列不回報大小，所以把第一列在列表下面再排一次來量）：`yuzora_diff_sides_share_the_width_evenly_whatever_the_lines`（各半，含新增／刪除／長行／全形字／左右不同的未變更列，1200 與 700px）、`yuzora_diff_a_press_on_a_sides_numbers_picks_that_sides_line`、`yuzora_diff_a_click_on_a_line_number_picks_it_through_the_list`（走列表自己的點擊，不是 harness 的 root mouse down）、`yuzora_diff_the_divider_drags_the_share_and_a_double_press_puts_half_back`、`yuzora_diff_the_drag_ends_where_the_pointer_leaves_the_pane`、`yuzora_diff_opens_with_the_share_last_kept`、`yuzora_diff_a_long_line_stays_on_its_side`、`yuzora_diff_long_lines_scroll_sideways_to_their_end`、`yuzora_diff_the_bar_under_the_lines_drags_them`、`yuzora_diff_the_bar_under_the_lines_has_room_of_its_own`、`yuzora_diff_the_kept_sideways_offset_stays_within_the_room_to_scroll`、`yuzora_diff_unified_has_no_divider_and_picks_by_its_numbers`；大 diff 與真字型（審查 F1、F2）：`yuzora_diff_long_lines_with_wide_characters_open_and_keep_drawing`（單行 100 萬與 400 萬字元、400 萬個全形字、100 行各 6 萬個全形字、3000 行混合、10 萬行短行、1 萬行整檔 context、本 repo 三個當新增檔會壞的檔案）、`yuzora_diff_a_frame_does_not_go_through_the_diffs_rows`（render 內不走過所有列：列是會計數的 Proxy，並排與合併各 3000 列，載入時走過、每一幀沒有）、`yuzora_diff_an_ordinary_diff_scrolls_to_where_its_longest_line_ends`（一般 diff 在打開的那一幀就量完，捲到最長那行的尾端）、`yuzora_diff_a_whole_file_in_context_is_measured_in_one_round`（1 萬行整檔 context，兩側同一行只讀一次，一輪排版就量完：排版的批數不超過 2）、`yuzora_diff_a_slice_that_fails_says_so_and_the_next_diff_works`（一份排名丟錯：錯誤顯示在 pane，下一個 diff 沒有錯誤地打開）、`yuzora_diff_the_longest_lines_tail_scrolls_into_view_in_the_font_that_draws_it`（`MeasuredFont`：視窗的文字系統照上述量測表排字，審查的反例：4 行 100 個漢字加 1 行 180 個 ASCII、4 行 170 個 ASCII 加 40 個 ✅ 的表格行、韓文、100 個 emoji）。
- 測試，PR #250 審查 r5 延後項的修正（`v010-git-diff-fu`；都數幀、像素、份數，沒有毫秒）：`yuzora_diff_a_diff_of_many_slices_read_again_keeps_its_offset_in_every_frame`（20,000 列、最寬的行在最後；滾到最右後重讀，每一幀畫的位移都不低於原來的，也不超過那一幀的範圍，最後 thumb 在原處）、`yuzora_diff_a_held_thumb_does_not_take_the_lines_to_the_start_when_the_diff_is_read_again`（按住 thumb 時重讀）、`yuzora_diff_a_narrower_diff_read_again_brings_the_offset_within_its_range_once_it_is_known`（重讀成較窄的 diff：舊位移撐到範圍確定，之後夾到新範圍的尾端）、`yuzora_diff_a_frame_cut_off_once_does_not_leave_its_line_at_its_upper_bound`（`cut_first`）、`yuzora_diff_a_frame_is_cut_by_the_costlier_side_and_the_other_sides_line_is_still_laid_out`、`yuzora_diff_a_row_cut_off_alone_once_is_read_again_and_not_taken_at_its_upper_bound`（`stall_slice: 1`）、`yuzora_diff_a_slice_cut_off_once_grows_back_but_not_to_the_size_that_was_cut`（`stall_slice: 3000`：20,001 列不超過 40 份、沒有一份超過被中斷的那一份的一半）、`yuzora_diff_a_batch_cut_off_once_grows_back_and_the_lines_are_laid_out_in_a_few_frames`（`cut_big`）；合併（unified）：`yuzora_diff_in_unified_a_selection_takes_no_room_from_how_far_the_lines_scroll`（量窗口畫出來的東西：列背景是最寬的 quad，起點說明捲了多遠、寬度說明盒子多寬，長行結尾的高亮字詞說明行尾在哪裡，各選取方式之後與沒選取時相同；把底色改成佔位置的子元素、加邊框或 padding 都會紅）。bun：`a slice that was cut off once grows back…`（用單位模擬常設上限與只停一次的機器）、`a batch cut off alone is asked for once more…`（blame、再試一次、frame 回升的簿記）。被中斷的測試用的上限因為「單獨一行被中斷要兩次才算」而改了：`…a_line_the_engine_cuts_off_even_alone_is_passed_over_at_its_bound` 的 `cuts` 由 4 到 6 改成 8 到 10（每行兩次）。
  - 第三輪的測試（審查 r2 的要求）：`yuzora_diff_a_line_cut_into_pieces_is_scrolled_to_its_end_as_it_is_drawn`（有字詞高亮、分很多段的行：360 個字元 40 段、1000 個字元 200 段、40 個值有變的 JSON 行；最後一個字元的右緣看得到、右邊的空白等於 `REST` ±1px；量測改回整行一段會紅）、`yuzora_diff_the_range_ends_where_the_widest_line_is_drawn_whatever_the_lines_are_cut_into`（範圍在畫出來最寬的那行結束，不論行分成幾段：最寬的是第一列，其餘的字元更多、段更多或兩者皆是，行尾留下的空白都一樣）、`yuzora_diff_the_lines_are_measured_in_the_font_the_rows_are_drawn_in`（視窗的文字系統讓不同字型家族的寬度不同〔比 JetBrains Mono 寬六分之一〕，行尾仍在範圍的盡頭；量測換成另一個字型家族會紅）、`yuzora_diff_text_in_a_script_the_table_does_not_list_scrolls_to_where_its_widest_line_ends`（5 行各 150 個阿拉伯字母：以前要捲 498px、空間 2570px、捲到底左側整片空白）、`yuzora_diff_sideways_wheels_that_come_before_the_next_frame_all_count`（同一個 `update` 內送兩個橫向滾輪事件）、`yuzora_diff_a_frame_costs_the_same_whatever_the_number_of_the_diffs_rows`（數的不是時間，數在做事的地方：每次讀列與列的陣列、載入的掃描被要求走過多少、這一幀建出的文字與字元數、視窗排版的字元數；2000 列與 6 萬列的最寬行相同，每一幀的成本相同，也沒有任何掃描；載入與排名的那幾份才走過列，見第四輪）、`yuzora_diff_a_diff_read_again_while_its_lines_are_being_laid_out_is_measured_again`（排版進行中又讀了一次 diff：回報的尺寸才算數，不當作上一次量測的；測試讓前幾批的尺寸遺失、停在飛行中）、`yuzora_diff_a_zoom_set_while_it_is_open_measures_the_lines_again`（縮放改變後重量：40 段的行在 zoom 2 量到的比 zoom 1 畫的短 20px）。
  - 第四輪的測試（審查 r3 的要求）：`yuzora_diff_taking_a_diff_in_goes_through_none_of_its_rows_and_the_frames_after_it_through_a_slice_at_a_time`（6 萬與 20 萬列，`show()` 一列都不排、每一份最多排的列數相同、份加起來補齊所有列、中間的幀讀不到任何列、畫了至少 4 次；20 萬列在 CI 設定下畫得出來也是「每幀一個 O(n) 迴圈」類突變的偵測：render 只有 50 ms；第四輪時這個測試要求 `show()` 排「一份」）、`yuzora_diff_while_the_rows_are_ranked_the_range_grows_to_what_the_whole_diff_needs`（排名中的範圍只變長、不短於已排名的最寬行、不超過整份排完的）、`yuzora_diff_many_lines_in_a_script_the_table_does_not_list_scroll_to_where_the_widest_ends`（1,000／1,500／3,000 行各至多 150 個希伯來字母：要捲 497.5px、捲到底右邊 12px；3,000 行 20 到 45 個阿拉伯字母的字幕檔放得下，不出現捲軸）、`yuzora_diff_a_data_file_of_thousands_of_lines_scrolls_to_its_widest_line`（2 萬行刪除的資料檔，90 到 110 字元：要捲 221.5px，原本 1,618px）、`yuzora_diff_a_line_longer_than_a_frame_is_laid_out_alone_and_scrolled_to_its_end`（4 萬個希伯來字母的單行，超過 `FRAME`，單獨排版後範圍收斂）、`yuzora_diff_a_frame_lays_out_a_frames_worth_and_a_line_longer_than_that_alone`（`MeasuredFont` 計數：第 201 列的兩條 20 萬字元的行，每一幀排版不超過 20 萬＋尺；原本第一幀 465,683）、`yuzora_diff_a_held_thumb_stays_under_the_pointer_when_the_range_changes`（按住 thumb 拖，pane 變窄，thumb 仍在指標下，繼續拖仍跟著）。審查 r3 的 N1 缺口：`yuzora_diff_the_right_sides_line_cut_into_pieces_is_scrolled_to_its_end`（右側的分段進位）、`yuzora_diff_a_font_set_while_it_is_open_measures_the_lines_again`（字型改變重量）、`yuzora_diff_the_widest_line_in_the_middle_of_a_batch_is_found`、`yuzora_diff_a_diff_read_again_after_it_settled_keeps_its_offset_and_measures_the_next`、`yuzora_diff_a_zoom_set_and_set_back_measures_the_lines_again`。排序拆份：bun 的 `putting a side's lines in order is work of the slice…`（40 萬列：沒有一份讓兩側都排序；小 diff 在第一份就完成；兩側的結果與一次排完相同）。一條有很多段、每段 32 個字母的行：第四輪是 4,000 段 × 2 個字母（那一幀在探針的處理器下要 46 ms）、再改成 1,000 段（預期值算錯，每段 221px 不是 224px，差 3,000px）；第五輪改成 250 段（`…a_line_in_two_hundred_and_fifty_pieces…`，那一幀約 3 ms，離 50 ms 有 15 倍；一行單獨被中斷時沒有更小的批可退，只能以上界計）。另有 `#[ignore]` 的計時 `yuzora_diff_how_long_taking_a_diff_in_takes`（手動跑，見上）。**偵測不到的**：在 render 裡對「列的副本」做一個不經 `ranks`／`bound`／Proxy 的 O(n) 迴圈，計數看不到（內建物件被凍結，不能計量陣列方法）；擋它的是 20 萬列要在 render 的 50 ms 內畫出來：突變 `frame-index-loop`（每幀對列做一個索引迴圈）在 CI 設定下讓那個 20 萬列的測試變紅；沒有量過 release 下要多少列才抓得到，所以這一類只有 CI 設定下的測試守著。
  - `app/scripts/yuzora-git-diff-layout.test.ts`（不需要視窗的部分，23 個）：比例範圍；`bound()` 不低於量測表的每個字元、ASCII 一格一字元、讀取成本從 meter 扣；一行超過 `LINE` 的尾巴當最壞；排版順序依上界、每行都在裡面；分段的行每多一段加半格；上界是 4 分之一格的整數（它是排序鍵的一部分）；兩側相同的未變更列只讀一次；排名分份（每一份不超過預算、結果與一次排完相同、不讀的列仍各算一份的 `STEP`）與每一份讀取量與 diff 大小無關、計數看得到每一種讀法；`batch()`／`settled()`／`measured()`／`span()`（和 mod 用同一份）的停止條件、每幀的量、超過一幀的行自己一批、超過 `LAYOUT`／`TEXTS` 的行跳過；排版到最寬的找到為止（沒有總量上限）；400 次固定亂數（0.5 到 3 的七種 zoom、一半的行分段）的「可捲範圍不短於最寬那行的真實寬度超過 2 個視窗像素」；第五輪：一份排名在它計量的每一個地方被中斷、再接著排，都不掉列、每行的上界仍不低於字型畫出來的格數（`a slice cut off anywhere…`），被中斷的一份的下一份是它做完的一半、一列都沒做完就被中斷的那一列以上界計（`a slice that was cut off is followed by…`）。gpui 測試視窗預設的文字系統是等寬假字型（每字元 6.9px，不分全形），真字型的寬度靠量測表；實機仍要看。

## diff 的選取與複製（v0.1.0）

使用者回報 diff 沒辦法複製。原因：diff 的每一行是普通 `div` 文字，不在視窗的文字選取（`TextSelection`）裡，JS 端也沒有複製入口。

- **選取是以「列」為單位**（不是字元）：點一列的文字區選那一列，Shift＋點延伸到那一列（從選取開始的那一列算起）。並排時選取屬於按下去的那一側（左或右），只複製那一側；合併（unified）複製畫面上看到的行（刪除行取左側、新增行取右側）。行號區不開始選取：在可暫存的 diff 裡行號是「挑行」，沒有挑行的 diff（commit、比較、忽略空白）裡行號也選取該列。分隔線的抓取區與橫向捲軸的 thumb 先吃掉按下事件（`stop_propagation`），不會到列：按在分隔線的抓取區（分隔線 1px，兩側各多 2px）只拖動、不選取；抓取區外一點的列點擊只選取、不開始拖動。橫向捲軸的軌道（thumb 以外）沒有處理器，按下會穿到列表的盒子，由 `pressed()` 擋下，不開始選取也不捲動。按住左鍵從文字區拖過去也選取，見「拖曳選取」。
- **標示不佔版面**：選取的列在該側文字區蓋一層 accent 的 `44` alpha（`shade()`），絕對定位（`absolute`，上下左右釘在 cell 的文字區），在 cell 的流程之外、也不在 `line()` 裡。#250 的範圍量測用 `line()` 排版（probe 與畫出來的列走同一個函式），所以範圍等於畫出來的寬度；標示若佔了寬度（邊框、padding、行號區加寬、流程內的元素）而 probe 不知道，長行的行尾就會被切掉。測試 `…_a_selection_takes_no_room_from_the_lines_and_the_range_they_scroll` 在選取（點、Shift、右鍵、捲動前後、兩側）下量範圍、最寬行行尾後的空白、兩側寬度，都要和沒選取時一樣。
- **資料形狀**：`this.mark = {side: "left" | "right" | "unified", anchor, head} | null`，`anchor`／`head` 是 `view.rows` 的索引（不是畫面上的列：清單是虛擬化的，捲走再捲回來選取還在），`head` 可以在 `anchor` 之上。diff 的列進來的地方只有 `show(view)`（`load`：換檔、檔案被改過、暫存或捨棄之後都會重新載入；`relayout`：換並排／合併或空白／context），它清掉選取：列換了，索引就不再指同一列。排名（`ranked()`）與排版（`measured()`）是之後在幀裡一份份做的，不碰選取：選取在排名進行中做、排名完成後仍在；排名進行中 diff 又被重讀，舊選取不會留到新的列上（Shift＋點從無到有開始）。拖曳中的狀態是 `this.sweep`（起點列、側、Shift、列表上緣、按下點與指標位置、`drag`：是否已離開按下點、`began`：是否已開始選取），不進 `mark`；`show()` 與 `follow()` 一併清掉它與它的計時器，所以拖曳中 diff 被重讀，拖曳就結束、後面的移動不選任何列。
- **複製的內容**：`copyRuns(rows, mark)` 把選取算成「某一側的行區間」`[side, start, end)`（0 起算、半開，相鄰的併成一段）：沒有該側行的列（新增行旁邊的空白）與「N unchanged lines」折疊列不算在內，折疊把區間切成兩段。文字由 Rust 的 `diffText(id, runs)` 從 session 保存的原始內容取（`DiffSession::lines_text`）：不含行號、不含 `+`／`-`，只去掉行尾（`\n` 或 `\r\n`），段與行之間用 `\n`，所以 tab、縮排、行尾空白、全形字、空行都照原文。不從 `rows` 取文字，因為 `rows` 的文字是顯示用的（tab 已換成 4 個空格）。被捲出畫面的列也包含在內。
- **⌘C（Windows／Linux 是 Ctrl+C）**：diff pane 有焦點、有選取時複製選取的行，並 `stop_propagation`；沒有選取時什麼都不做，也不攔鍵。理由：「目前 hunk」是 ↑↓／n／p 的導覽位置，不是使用者看得見的選取，用一個習慣性的 ⌘C 去蓋掉剪貼簿太突兀；要複製 hunk 走右鍵選單。按鍵送得到 mod 的依據（讀碼）：`ShellRoot` 的 `Copy` 動作在視窗沒有文字選取時 `cx.propagate()`，之後才輪到 mod 的 `on_key_down`；herdr 的 `terminal.copy` 只綁在 Terminal 的 key context。視窗裡別處（例如某個 TextView）有文字選取時，`Copy` 會先複製那段並攔下，不會到 mod；但在 diff 裡按下滑鼠左鍵會先清掉視窗的文字選取。
- **右鍵選單**：清單的 `on_item_secondary_click(key, event)` 取得按到的列與（列內的）x。那一列不在目前選取裡就取代選取（在裡面就保留）。選單是這個 mod 自己的 popup（`openOverlay({mod: "yuzora-git-diff", kind: "popup", at: "pointer", props: {menu}})`），同一個 class 在 `props().menu` 時畫選單，做法同 Git Log。三項：複製（選取的行數）、複製 hunk（按到那一列所在的 hunk 的新版內容，也就是右側範圍；只刪除的 hunk 沒有，反灰）、複製為 patch（選取碰到的每個 hunk 的 unified patch；沒有碰到 hunk 反灰）。摺疊列沒有選單。
- **patch**：`diffPatch(id, hunks, whitespace)` → `DiffSession::patch`，由左右文字與 hunk 在 Rust 算出，格式是 `git apply` 接受的 unified diff（`diff --git`、新增／刪除檔的 `new file mode`／`deleted file mode`、rename 的 `rename from/to`，名稱有引號或控制字元時照 git 的方式加引號）。每個 hunk 最多前後各 3 行 context，而且只取兩邊相同的行（相鄰的 hunk 之間不夠 3 行就少一點）；彼此相鄰且中間不超過 6 行的 hunk 合成一個 `@@`；新側的行號只算 patch 裡的 hunk，所以只選第二個 hunk 的 patch 也能套用在舊檔上。檔尾沒有換行的行照 git 的寫法加 `\ No newline at end of file`；沒有換行的最後一行後面還有新增行時，改寫成 `-行`＋`+行`（git 自己也這樣寫）。空白模式不是精確時，hunk 的編號是那個檢視的編號（`diffPatch` 帶 `whitespace` 重算同一份），context 取舊側的文字，所以 patch 套得上舊檔，但套完後不見得與新檔在空白上逐字相同。`git apply --check` 對 rename、新檔、刪除、無結尾換行、CRLF、空白模式都驗過。
- **複製的回饋**：用 `notify`（toast，1.5 秒）說「Copied N lines」／「Copied the hunk」／「Copied the patch」。剪貼簿只在 `clip()` 一個地方寫。
- **拖曳選取**（使用者：「git diff 沒辦法複製」之後補上；#250 時因為 JS 拿不到捲動位置而沒做）：在一列的文字區按下、拖過去，選取從按下的那一列到指標下的那一列（上下都可以），並排時屬於按下的那一側，放開後還在，⌘C 複製。
  - **指標下是哪一列**：`rowAt(y) = floor((unzoom(y) + unzoom(scroll.offset())) / ROW)`，`y` 是視窗 px、從列表上緣算（按下事件的 `position.y − local_position.y` 就是列表上緣）。`scroll.offset()` 是這次加的上游方法（捲動量，視窗 px，頂端為 0，是上一幀畫出的）。
  - **上游改動（20 行，3 個檔，每處都有 `// yuzora(git-copy): …` 註記，只讀不寫）**：`app/vendor/gpui-kit/crates/shell/src/engine/quickjs/entity_api.rs` +10（`__virtual_scroll_offset`，回 `-offset.y`）、`engine/quickjs/mod.rs` +4（`virtualScrollHandle` 的 `offset: () => __virtual_scroll_offset(handle)` 兩行；滑鼠移動事件的 payload 加 `dragging: event.dragging()`，左鍵是否按著，兩行）、`typings.rs` +6（`VirtualListScrollHandle.offset(): number` 與 `MouseMoveEvent.dragging: boolean`，各三行）。同步上游時這三處要留著；測試 `yuzora_diff_the_list_says_how_far_it_is_scrolled_in_window_pixels` 在 zoom 1 與 1.5 量 `offset()`，`yuzora_diff_a_move_with_no_button_down_ends_a_drag_across_rows` 與 `…_ends_the_dividers_drag_and_keeps_its_share` 量 `dragging`（把 `event.dragging()` 改成 `true` 它們就紅）。`dragging` 由各平台層填，讀碼（`vendor/gpui-pre-*`，不是在各平台的真實視窗跑過）：macOS 的 `NSLeftMouseDragged` 等帶按下的鍵、`NSMouseMoved`（沒有按鈕才有的事件）是 none；Windows 取 `WM_MOUSEMOVE` 的 `MK_LBUTTON` 旗標；X11 取 `button_mask`；Wayland 取 `state.button_pressed`。沒有按鈕卻在拖曳中到達 pane 的 none 移動，找過所有非測試的建構處（`MouseMoveEvent {`）：檔案拖放合成的移動是 `Some(Left)`；測試用的 `simulate_mouse_move` 與 div.rs 的 tooltip 測試是 `cfg(test)`；剩下兩個是 Wayland 指標「進入」視窗時合成的一個移動（拖曳是在這個視窗裡開始的，進入不會發生）與 Windows 非客戶區的移動（指標已離開 pane）。每幀之後 gpui 只重算 hit test（`self.mouse_hit_test = …`），不合成移動。
  - **不動上游的路線都實際跑過，全部走不通**（實測，暫存實驗，沒進 repo）：(A) 由列表畫出的第一列（`range.start`）推算：捲走 10px 以上，列表就從第 0 列開始畫，按在離頂端 10px 處實際是第 1 列，差一列。(B) 按下後拖到別的列再放開：`on_item_click` 不會來，⌘C 什麼都沒複製；拖出去再回來才有（選到起點那一列）。(C) 捲動 handle 只有 `__handle`、`scroll_to_item`、`scroll_to_bottom`、`release`，`offset` 是 `undefined`。(D) 用 `scroll_to_item(0, "top")` 把列表貼齊列再量：捲走 13px 時按下選到第 1 列，貼齊後同一點選到第 0 列，貼齊會把內容移到指標底下。(E) `on_click`／`on_mouse_down`／`on_mouse_move`／`on_hover`／`on_mouse_up` 在列表的 item renderer 裡全被拒絕（"cannot be registered from a virtual list's item renderer…"）。從 Yuzora 這邊接（`yuzora` crate 或 host function）也不行：`gpui_shell::scope` 是 private（`error[E0603]: module scope is private`），`lib.rs` 註明 `entities`／`value` 不可達。
  - **按下、移動、放開**（一次按下有三個狀態，`this.sweep` 說它在哪個）：
    - **down**：按鈕按著，指標離按下點還不到 `SLOP`（4px），也沒有離開起點列。這是還沒放開的點擊：沒有計時器、列表不捲、什麼都不選；放開就是列表回報的那個點擊（`on_item_click`→`clickRow`）。以前一按下就啟動 50ms 的計時器，在列表最下面一列（或捲過之後最上面一列）按住不動超過 50ms，列表就在不動的指標底下捲走，放開時指標下已經是別的列，列表不回報點擊，Shift＋點也延伸過頭；並排時按在橫向捲軸的軌道上也一樣（審查 P1）。
    - **drag**：指標離按下點 4px 以上，或在別的列上（再近也是：放開時不在按下的那一列，列表不會回報點擊）。計時器每 50ms（`TICK`）看一次指標，邊緣捲動與選取延伸都在這個狀態（`sweepTo()`、`beat()`、`extend()`）。
    - **over**（`this.sweep` 是 null）：`endSweep()` 清掉計時器，選取保留。放開、指標離開 pane、按鍵送到 pane、diff 被重讀（`show()`／`follow()`）、**沒按左鍵的指標移動**、**pane 失去鍵盤**都會到這裡。
    - 不進入任何狀態的按下：摺疊列、可挑行的行號區（各有自己的點擊）、列表以下的橫向捲軸軌道（`pressed()` 比對 `listHeight()`：盒子的高度，並排有橫向捲軸時減去 `BAR`）。分隔線的抓取區與橫向捲軸的 thumb 先吃掉按下事件（`stop_propagation`），到不了列的盒子。unified 的橫向捲軸是原生的，`pressed()` 不知道它在哪，按在它上面就是按在後面那一列（測試 `…_unified_a_button_held_still_at_the_bottom_where_the_bar_is_scrolls_nothing` 只量列表不捲、只選那一列）。
  - **結束拖曳的條件，以及為什麼不能只靠放開**：pane 根節點的 `on_mouse_move`／`on_mouse_up`／`on_hover` 每一幀都掛著，用 `this.drag`／`this.sweep` 判斷要不要做事：按下與放開都不要求重畫，只有選取改變才 `notify`。app 層的全域快捷鍵（`⌘1`–`⌘9`、`⌘T`、`⌘D`、`⌘=`、`ctrl-tab`、F2 等，`keys.macos.json` 的 `global` 區）在按著按鈕時按下，按鍵由 herdr 根節點的 `ShellAction` 處理掉，不會送到 pane（`app/src/window.rs`、`vendor/gpui-pre/src/window.rs` 的 key 派送，讀碼）；任何鍵盤輸入之後、指標下一次移動之前，gpui 把所有 hitbox 都當成沒被指著（`last_input_modality`，讀碼），pane 根節點的 `on_mouse_up` 要求 `is_hovered()`，所以放開收不到。審查 P2-1 指出的後果：放開之後沒按按鈕移動指標，選取還在改；計時器照跑，換到別的分頁也在背景每 50ms 一次。所以不靠按鍵、也不靠放開：(1) **滑鼠移動事件帶 `dragging`**（左鍵按著），一個 `dragging` 為 false 的移動就是按鈕已經放開，結束列的拖曳，也放開分隔線與橫向捲軸 thumb 的拖曳（`drop()`，同一個既有問題，一併修）。(2) **計時器每次響先看鍵盤是不是被帶走了**：拖曳開始（進入 drag）時記下 pane 有沒有鍵盤（`s.keys = handle.is_focused()`，按下時 pane 根節點取得焦點：`on_mouse_down`→`handle.focus()`，⌘C 能到 pane 靠的就是它），之後每一拍「開始時有、現在沒有」就結束（顯示或開啟別的 pane 的快捷鍵會把鍵盤帶走）。不是「現在沒有」就結束：真的 app 裡拖曳進行中 `is_focused()` 是不是一直是真的沒有驗過，用「有過才算失去」就不可能讓一個沒有被帶走鍵盤的拖曳在第一拍就死（測試 `…_a_drag_that_began_without_the_keyboard_is_not_ended_for_the_want_of_it`）。(3) 按鍵**送到** pane（⌘C 複製目前選到的）仍結束拖曳。三者各有測試（`…_a_move_with_no_button_down_ends_a_drag_across_rows`、`…_ends_the_dividers_drag_and_keeps_its_share`、`…_a_drag_ends_when_the_pane_no_longer_has_the_keyboard`、`…_a_key_pressed_during_a_drag_copies_what_is_selected_and_ends_it`）。
  - **開始與延伸**：進入 drag 之後，指標第一次落在起點列以外的列才開始選取（`began`；`press()`：Shift 時從現有選取延伸，並且和點擊一樣把那一列的 hunk 當成目前的 hunk），之後 `head` 跟著指標下的那一列；回到起點列就是那一列。沒離開起點列就放開（不論按了多久，只要指標沒動 4px），仍是原來的點擊（`on_item_click`→`clickRow`）；在起點列裡移動超過 4px 又放開也是點擊：選那一列。按著按鈕、指標不動而用滾輪把列表捲走，不會開始選取（要等指標動）。排名中（`ranked()`）照做，拖曳不呼叫 `again()`、不碰 `fit`，也不被排名打斷（測試用 `hold` 讓排名停在第一份之後）。
  - **邊緣捲動**（只在 drag 狀態；「列表」是列表本身，並排有橫向捲軸時不含它下面的 `BAR`，`listHeight()`）：指標在列表上緣或下緣 20px 內（或在列表之外，例如標題列）時，每 50ms（`beat()`，`cx.timer.after`）把列表捲 `1 + floor(深度 / 20)` 列、最多 8 列（`MOST`；深度是指標進到邊緣帶多少 px；標題列裡的指標比列表邊緣的快一倍，測試量這個），選取跟著；已經在盡頭就不再要求。**實際的速度上限**：往下，指標最多到 pane 的下緣（再往外拖曳就結束），所以深度小於 20，每 tick 1 列，也就是每秒 20 列（並排有橫向捲軸時，指標在那 8px 的軌道上算「在列表之外」，深度 20 到 28，每 tick 2 列、每秒 40 列）；往上，指標在標題列裡（28px，加上列表上方多出的提示列）深度 20 到 48，每 tick 最多 3 列、每秒 60 列。`MOST = 8`（每秒 160 列）只有列表上方有更高的帶子（捨棄確認、訊息）時才到得了。所以 2 萬列的 diff 從頂端拖到底約 1,000 秒；要快就用捲軸或滾輪。**以上是測試的時鐘量到的**（每 tick 的列數：下緣 1.00、並排橫向捲軸軌道 2.00、標題列 y=14 為 2.00、視窗頂端 3.00；暫時的量測測試，沒進 repo）。**macOS 的真實視窗另有一個沒量的加成**（推論）：gpui 的 macOS 平台在按著按鈕拖曳時，每 16ms 重送最後一個拖曳移動（`vendor/gpui-pre-macos/src/window.rs` 的 `synthetic_drag`，直到按鈕實際放開，讀碼），每一次重送也進 `sweepTo()`→`extend()`，所以除了每秒 20 拍的計時器，每秒另有約 62 次檢查。一次 `scroll_to_item` 要等下一幀才套用，同一幀裡的檢查算出同一個目標列，所以邊緣捲動的實際速率大約介於計時器的速度與「每幀一次」之間，下緣是 20 到約 80 列／秒（60Hz 螢幕；更高更新率的螢幕上限更高）。這個範圍是算術，沒有在 app 裡量過。測試平台沒有這個重送，所以測試量不到。這是 #266 就有的行為，這次沒有改；要讓各平台一致，做法是 `sweepTo()` 只更新位置與選取、捲動只留給計時器（見報告「給總指揮決定」）。`scroll_to_item` 只把那一列「帶進視野」，不是貼齊上緣（`virtual_list.rs` 的 `scroll_to_deferred_item`，讀碼，測試也量到），所以往下指的是「最後一列再往下 step 列」，往上指的是「第一列再往上 step 列」。滾輪或捲軸移動列表時，下一個 50ms 內 `head` 也會跟上。放開、指標離開 pane、diff 被重讀（`show()`／`follow()`）都停止計時，之後不再畫幀（測試量 `script_renders`）。
  - **限制**：(1) 指標離開 pane（包含往下拖出 pane 的下緣）拖曳就結束、選取保留：離開之後這個 pane 的根節點收不到移動與放開（`is_hovered()`，讀碼，測試 `…_a_drag_across_rows_ends_where_the_pointer_leaves_the_pane`）。所以邊緣捲動靠的是 pane 內靠邊的 20px，不是拖到 pane 外面。(2) 列表上緣取的是按下那一刻的視窗位置；拖曳中標題列下面多出一條提示（例如 stage 失敗的訊息）會讓它偏掉，直到下一次按下（推論，沒測）。(3) unified 的橫向捲軸是原生的，不知道軌道在哪：按在它上面就是按在後面那一列（只量到列表不捲，沒量到拖曳；測試視窗裡它沒有畫出來，所以沒量到它自己吃不吃按下事件）。(4) **放開沒被 pane 看到，而且指標不動**：`dragging` 要有移動事件才說得出按鈕放開了，計時器只能看鍵盤。指標停在列表邊緣、放開被全域快捷鍵擋掉、而快捷鍵沒有把鍵盤帶走（例如只改縮放的 `⌘=`，推論）時，列表會繼續捲到盡頭，直到指標動一下（移動就結束）或下一次點擊。(5) 計時器「pane 失去鍵盤就停」靠 `handle.is_focused()` 從真變假：測試用另一個 `FocusHandle` 模擬鍵盤被帶走；真的 app 裡 `⌘T`、`⌘1`–`⌘9` 是不是把鍵盤交給另一個 `FocusHandle`、拖曳中 `is_focused()` 是不是一直為真（在 pane 裡與在 diff modal 裡），都沒有在 app 裡驗過。(6) 按著按鈕、指標不動時用滾輪把列表捲走，不會開始選取，要等指標動（放開也不會有點擊：按下的那一列已經不在指標下面，列表不回報）。(7) 沒有實機操作過；Windows 與 Linux 都沒跑。
- **沒做：字元級選取**。讀碼評估（沒有實作）：Yuzora 在 `yuzora-git` 自己註冊一個原生 `DiffText` 元件（仿 `GraphCell`，0 行上游，約 450 行）、每個 cell 一個 `TextSelection` participant 是可行的，但 anchor／cursor 那一列捲出視窗時整個選取會消失、中間捲出去的列不會被複製、空白行與第一行的縮排複製後會遺失（`TextSelection` 的複製會濾掉純空白項、ShellRoot 的 `Copy` 會 `trim`）。
- 測試：`app/src/mods/lifecycle_tests.rs`（真的把 mod 放進視窗，用滑鼠、Shift、右鍵與按鍵；剪貼簿是測試視窗自己的，不是系統的）：`yuzora_diff_cmd_c_copies_the_lines_selected_as_the_file_has_them`、`…_a_selection_copies_only_the_side_it_began_on`、`…_unified_copies_the_lines_as_they_are_shown`、`…_a_selection_stays_when_scrolled_away_and_back`、`…_the_selected_rows_are_marked_on_their_side`、`…_cmd_c_with_nothing_selected_copies_nothing`、`…_the_divider_and_the_pick_numbers_do_not_select`、`…_the_selection_goes_with_the_rows_it_named`、`…_the_menu_copies_the_selection_the_hunk_and_a_patch`、`…_a_right_click_keeps_the_selection_it_is_in_and_replaces_the_one_it_is_not_in`、`…_the_patch_is_asked_for_every_hunk_the_selection_touches_and_a_fold_has_no_menu`；#250 之後接上的：`…_a_selection_takes_no_room_from_the_lines_and_the_range_they_scroll`、`…_dragging_the_divider_and_selecting_rows_leave_each_other_as_they_are`、`…_scrolling_sideways_and_selecting_rows_leave_each_other_as_they_are`、`…_a_selection_made_while_the_rows_are_ranked_stays_through_it_and_goes_when_the_diff_is_read_again`（探針的 `hold` 讓排名停在第一份之後，F12 繼續、F11 重讀）、`…_a_click_and_a_right_click_select_on_the_side_they_are_on_at_every_zoom`（縮放 0.8 到 2，單獨一個行程）；拖曳選取（本節「拖曳選取」）：`…_dragging_across_rows_selects_from_the_row_pressed_to_the_row_under_the_pointer`（並排的左右側、往上、回到起點列、Shift、unified）、`…_a_press_that_stays_in_its_row_is_a_click_and_the_numbers_the_fold_and_the_divider_start_no_selection`、`…_dragging_selects_the_rows_under_the_pointer_at_every_zoom_with_the_list_scrolled`（縮放 0.8／1／1.5／2，捲走 0 與 37，單獨一個行程）、`…_the_list_says_how_far_it_is_scrolled_in_window_pixels`（`offset()`，zoom 1 與 1.5，單獨一個行程）、`…_a_pointer_held_at_the_edge_scrolls_the_list_and_the_selection_goes_on`、`…_a_drag_while_the_rows_are_ranked_neither_ends_nor_stops_the_ranking`、`…_a_diff_read_again_during_a_drag_ends_it`、`…_a_drag_across_rows_ends_where_the_pointer_leaves_the_pane`、`…_a_drag_makes_the_hunk_it_began_in_the_current_one`、`…_a_key_pressed_during_a_drag_copies_what_is_selected_and_ends_it`；審查 r1 之後：`…_a_button_held_still_on_the_last_row_in_view_is_a_click`、`…_a_shift_click_held_still_on_the_last_row_in_view_reaches_that_row_and_no_further`、`…_a_button_held_still_on_the_first_row_in_view_of_a_scrolled_list_is_a_click`、`…_a_button_held_on_the_bars_track_selects_nothing_and_scrolls_nothing`、`…_unified_a_button_held_still_at_the_bottom_where_the_bar_is_scrolls_nothing`、`…_a_drag_begins_a_few_pixels_from_where_the_button_went_down`、`…_a_drag_begins_when_the_pointer_is_over_another_row_however_near`、`…_a_pointer_held_at_the_bottom_edge_of_a_list_with_a_bar_under_it_scrolls_to_the_last_row`（以上測 down／drag 兩個狀態：按住不動超過 50ms 的按下仍是點擊，測試在按下與放開之間推進測試的時鐘，既有的測試都沒有）、`…_a_move_with_no_button_down_ends_a_drag_across_rows`、`…_a_move_with_no_button_down_ends_the_dividers_drag_and_keeps_its_share`、`…_a_drag_ends_when_the_pane_no_longer_has_the_keyboard`（`focus_elsewhere()`：把鍵盤交給視窗裡另一個 `FocusHandle`，pane 沒有收到任何鍵）。拖曳中按到送得到 pane 的鍵會結束拖曳，所以這些測試用探針每幀寫的筆記讀選取（`mark`）與捲動量（`now`），並用按著按鈕的滾輪（`wheel_held`，指標用 `drag` 移過去，因為沒按按鈕的移動會結束拖曳；不是 F12／F11）讓 `hold` 的排名繼續、讓 `reload` 的 diff 重讀；`clickRow` 筆記數點擊有沒有被列表回報。**機器忙時偶爾丟點擊的原因（審查 r1 要求查，實測）**：按下讓視窗重畫，引擎只給畫列表各列的那次 JS 呼叫 50ms 的牆鐘時間（`Budgets`，沒有可調的旋鈕），debug build 在忙碌的機器上畫 66 列（1300px 高的視窗）會被切斷，那一幀列表一列都沒畫（`render_range` 回空），各列的 element state（`pending_mouse_down`）跟著被丟掉，放開時就沒有點擊。證據是兩組量測，各自只證明自己那一段：（甲）在 `render_range` 回空處與 gpui 的點擊判斷處暫時加診斷輸出（沒進 repo），只跑現在這一版 mod，1,800 個點擊（3 次，每次 600 個，16 個忙碌迴圈加上本來約 25 的 load）丟了 5 個，每個都在按下（或按下前的移動）那一幀有一次空的列表繪製；按下之後的幀（放開、`settle()`）空繪製了 5 次，一個點擊都沒丟。（乙）同一個測試二進位換三版 `main.js`（拖曳之前的 `563cb8be`、審查時的 head `e55967e3`、這一版）輪流各跑 3 輪、每輪 600 個點擊，丟的點擊依序是 5、2、2 個（共 5,400 個）：**偵測不到新版比較常丟**。同一個腳本裡舊版（甲版）自己兩次各 300 個就丟了 0 個與 9 個，機器負載的起伏比三版的差距大，所以只能說沒有差別，不能排名；先前量到的「新版約 6 倍」（3/384 對 19/232）是兩組不在同一段負載下的量測。release build 畫幾十列是否遠低於 50ms 沒有量（推論）。**判斷：這是引擎對每次 JS 繪製呼叫的預算，早於這個 PR，任何用 virtual list 的 mod 都一樣，這次沒有動。** 還在用裸 `click()` 的測試：全檔 58 處，大多在 `mount`／`mount_diff` 的 600px 高視窗（約 29 列）；在 `zoomed`（1300px）視窗裡點擊的只有 `…_a_click_and_a_right_click_select_on_the_side_they_are_on_at_every_zoom`，它只有 2 列，繪製很便宜。只有縮放那個測試與「選取在排名中」那個測試用 `click_arrives()`。所以測試框架的處理是：`click_arrives()` 先 `hover` 再點，沒被回報就重送，但每個視窗最多 3 次（`Resends`），第 4 次就讓測試失敗，訊息說是機器太忙還是點擊真的丟了；縮放那個測試與「選取在排名中」那個測試共用它（以前縮放測試自己重按四次，不計數）；排名進度（`rows_ranked_in_all`）讀探針每幀寫的 `at` 筆記，也就是 `fit.at`：被引擎中斷的那一份不會留下筆記，而 `hold` 之後不會再開始下一份，用「份的筆記」算會把它做過的列算成 0（`…_a_first_slice_the_engine_cut_off_counts_the_rows_it_went_through_while_the_ranking_is_held`，`stall_slice: 3000`。機器忙時舊的計數 20 次裡有 2 次紅；訊息帶診斷資訊的版本量到 `cut=1`、`fit.at=6199`、d1 已畫 14 幀，也就是第一份做了 6,199 列才被引擎中斷。CI 上同一句 `0 of 20000 rows are ranked, and the ranking is held` 是推論同一個原因，沒有看到 CI 裡面）；`DiffPane` 的視窗開在它的大小（`add_window` 開的是顯示器大小再 resize，pane 根節點的 hover 回呼在第一幀跑時，diff 會在那一幀進來並以 1920×1080 量）；`app/scripts/yuzora-git-diff-copy.test.ts`（選取算出哪些行、哪些 hunk、`press`／`covers`）；`crates/yuzora-git/src/diffview.rs` 的 `copied_lines_are_the_files_own_text`（tab、行尾空白、全形字、空行、CRLF、無結尾換行、非 UTF-8）與 `a_patch_*`（用真的 repo 跑 `git apply --check` 並套用）。
