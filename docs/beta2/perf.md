# L8 效能監控（資源用量）

lane L8 把 beta.1 狀態列的 CPU／記憶體 chip 與「資源用量」popover 移植到 beta.2，放在**視窗最右下角**、狀態列的最右端：`yuzora` layout 由自己的 block 放（指令面板快捷鍵的右邊），其他有狀態列的 layout 由視窗畫在狀態列的右端（`main` 在「⌘⇧P commands」右邊，`win98` 在工作列右端），沒有狀態列的 layout 不顯示（見「放置」）。點 chip 往上展開明細，右緣對齊。beta.1 的程式（`src-tauri/src/perf_service.rs`、`src/state/perfStore.ts`、`src/workbench/PerfBridge.tsx`、`ResourceUsagePopover.tsx`）是移植來源，已從 repo 移除（見 tag `v0.1.2`）。架構規則見 [architecture.md](architecture.md)，lane 流程見 [lanes.md](lanes.md)。

## 範圍與檔案

| 路徑 | 內容 |
| --- | --- |
| `app/crates/yuzora-perf/` | `yuzora:perf`：取樣 worker、行程樹聚合與分類、找 herdr-core、滾動視窗 |
| `app/defaults/mods/yuzora-perf/` | 右下角的 chip 與 popover；`i18n/en.json`；Lucide 圖示（`cpu`、`memory-stick`、`app-window`、`wrench`、`chevron-up`） |
| `app/src/yuzora/perf_chip.rs` | 哪個 block 是狀態列（`bar`）、chip 的 copy（`slot`、`at`）、在狀態列的右端畫它（`beside`）；規則的單元測試 |
| `app/src/window/yuzora_perf_chip_tests.rs` | chip 在視窗裡的測試：畫在狀態列右端、狀態列自己的內容在它左邊、沒有狀態列或自己放了 chip 時沒有、跟著狀態列的修改／隱藏／改大小、點 chip 開明細、win98 工作列的樣子、layout 檔與 `.state` 和沒有 chip 時相同 |
| `app/crates/yuzora-perf/examples/cost.rs` | 從外部對執行中的 app 量測單次取樣成本 |

自己目錄以外的改動（總指揮特准）：

| 檔案 | 改動 |
| --- | --- |
| `app/Cargo.toml` | yuzora crate 區塊加 `yuzora-perf = { path = "crates/yuzora-perf" }` |
| `app/src/yuzora/mod.rs` | `host_modules()` 加 `yuzora_perf::host_module(),`；同檔測試的模組數 7 → 8（加模組的必然結果）；chip 再加一行 `pub mod perf_chip;` |
| `app/src/window.rs`（上游） | chip：4 行，每行都以 `// yuzora(L8):` 註記。`visible()` 與 `owned_mods()` 各加 1 行 `mods.extend`／`out.extend(perf_chip::slot(&self.chrome, &self.file_chrome))`；`render_block` 的 `Content::Mod` 改 1 行，把 `render_slot(&n.id)` 與 chip 的 copy（`perf_chip::at` 找到時）交給 `perf_chip::beside`；測試區加 1 行 `include!("window/yuzora_perf_chip_tests.rs")` |
| `app/defaults/layouts/yuzora.json` | `status` 由 block 改成 row：`yuzora-statusbar`（佔滿）＋`yuzora-perf`（150px），`resizable: false` |
| `app/Cargo.lock`、`app/THIRD-PARTY-NOTICES` | 新增 workspace package `yuzora-perf`；沒有新的第三方套件（sysinfo 0.37.2 已由 gpui-fps 帶入） |

## 量測語意

beta.2 沒有 WebView，分成三類：

| 類別 | 內容 |
| --- | --- |
| **App 本體** | herdr-app 這個 process |
| **herdr-core** | 這個 app 的 `HC_SESSION` 對應的 `hc` server、它的 holders，以及它們底下的所有行程（終端機程式與 agents） |
| **Yuzora 工具** | app 的其他子孫：git、ssh、資料庫輔助程式、`hc connect`（app 與 core 的連線）、mod 的 `exec`。分類不出來的 process 也歸這類 |

- **只量本機**。SSH 遠端 Space 的終端機跑在遠端機器上，不計入；popover 的範圍說明寫明這點。
- herdr-core 是**整個 core**：同一個 core 的終端機都算，包括同時連在這個 core 的其他 client（例如用預設 session 的官方 Herdr app）開的終端機。core 不分 client，這裡也不分。
- **CPU** 沿用 beta.1：單一 process 是相對一顆核心的百分比（可超過 100），總量是各 process 的算術和，不除以核心數。
- **記憶體**是 RSS，十進位 MB（÷ 1,000,000）。
- **不變式**：三類加總＝總量（記憶體、CPU、process 數），每個 pid 只算一次、只屬於一類。`tree.rs` 的測試全部檢查這點。

### herdr-core 的辨識

調查結果（hc 0.1.13，macOS）：

- **hc server 在樹裡的位置不固定**。app 只有一個子行程 `hc connect`（`app/src/hc.rs` 的 `Core::connect`），沒有 core 時由它啟動 `hc serve`。結構是 `hc serve` → `hc hold`（holder）→ 終端機程式。實機觀察到兩種情況：
  - server 由這個 app 的 `hc connect` 啟動：`app → hc connect → hc serve → hc hold → 終端機`，herdr-core 整棵在 app 底下。
  - server 早就在跑（別的 client、CLI、官方 Herdr、或 app 重啟過，原本的 `hc connect` 已結束）：`hc serve` 的 parent 是 launchd（pid 1），不在 app 底下。
  所以用樹的位置分類會出錯：前者會把所有終端機算成工具，後者會漏掉整個 core。
- 一台電腦可能同時有好幾個 `hc serve`（每個 `HC_SESSION` 一個，例如各 lane 的隔離 core），所以**也不能用名稱猜**。
- **辨識方式**：用 app 的 `hc`（`yuzora_core::remote::hc_program()`，與 app 連 core 時同一個執行檔、同一組環境變數）執行 `hc server status --json`，取 `pid`（server）與 `holders[].pid`。它回答的就是 app 連的那個 server。
- herdr-core 這一類＝從 server 與每個 holder 往下走整棵樹。`hc server restart` 後舊的 holder 會留在 launchd 底下慢慢結束，不在新 server 底下，但 status 仍列出它，所以從 holders 走也會算到。
- **快取**：答案會保留，只在這些情況重問：server 的 pid 不在了，或同一個 pid 的啟動時間變了（pid 被重用）；答案超過 60 秒；上次問失敗後過了 10 秒。正常情況每 60 秒只多一次 `hc` 呼叫。
- **只在已連線時問**：`hc server status` 在沒有 core 時會**自動啟動一個**。所以只有 app 底下有活著的 `hc`／`herdr-core` 子行程（`hc connect`）時才問；否則 herdr-core 記為 0，並標示「not connected to herdr-core」。這也讓 `herdr-app check` 與 `cargo test`（render test 會 headless 執行每個 mod）永遠不會碰到使用者真正的 core。
- **邊界情況**：
  - 從 herdr 終端機裡啟動的 Yuzora：app 在 core 的樹裡面。core 先走，但不進入 app；app 和它的子孫仍算 App 本體與 Yuzora 工具。
  - server 在 app 底下（由這個 app 的 `hc connect` 啟動）：仍算 herdr-core，不算工具；`hc connect` 本身算工具。
  - 找不到 server（`hc` 失敗、未連線）：herdr-core 記為 0，chip 顯示 ⚠，popover 寫出原因。

### 取樣

- `yuzora:perf` 有一條 worker thread（`yuzora-perf`）擁有持久的 sysinfo `System`。mod 的 `tick()` 只把請求交給它並 await 結果，**UI thread 不碰行程表**。
- CPU 是與上一次 refresh 的差值，所以啟動後第一個樣本、以及第一次看到的 process，CPU 都是 0。
- macOS 只 refresh 要量的樹：用 `proc_listallpids`／`proc_listchildpids` 從 app、server、holders 往下列出 pid，sysinfo 只 refresh 這些（整表 refresh 會讀每個 process 的 argv 與環境變數，beta.1 實測 650 個 process 約 10 ms）。列不出來時退回整表。其他平台整表 refresh。
- 視窗在前景時每 2 秒取樣一次；不在前景時記為 `skipped_no_focus`，不取樣。
- 距上一次 poll 不到 1.5 秒、或上一個樣本還在跑時，`tick()` 只讀不取樣。layout 裡放了兩份 mod 也只會每 2 秒取樣一次。
- **滾動視窗**（移植 perfStore）：記錄最近 30 次 poll 的結果（`ok`／`failed`／`empty`／`skipped_no_focus`）。`empty` 是找不到 app 自己的 pid（沒有資料，不代表 0）。有 `failed` 或 `empty` 時 chip 顯示 ⚠，popover 顯示次數與最後一次錯誤；視窗內沒有失敗後錯誤訊息就清掉，恢復後一分鐘內警示自動消失。失敗或空樣本不會蓋掉最後一個好樣本。
- 取樣成本：每次取樣的耗時也記在同一個 30 筆視窗，popover 的註腳顯示中位數與 p95。

### 取樣成本（實測）

2026-11-03，Apple Silicon Mac（macOS 27），系統約 570–600 個行程，隔離 core。`cost` example 為 release build，從外部對 app 的 pid 每 2 秒取樣 30 次（不含第一次 prime）；app 內數字取自 popover 註腳（debug build，最近 30 次）。

| 終端機數 | herdr-core 行程數 | 外部 median | 外部 p95 | 外部 max | app 內 median／p95 |
| --- | --- | --- | --- | --- | --- |
| 8 | 10 | 0.57 ms | 7.38 ms | 16.23 ms | 0.6 ms／0.7 ms（27 次，暖機後） |
| 24 | 26 | 1.29 ms | 2.06 ms | 13.47 ms | 1.7 ms／9.6 ms |

- max 都是每 60 秒重問一次 `hc server status` 的那個樣本（起一個行程，約 13–16 ms）；其餘樣本只 refresh 量測的樹。
- 全部在 worker thread，UI thread 只送請求、等結果。

## UI

- chip：`{cpu}% · {mem}MB`，取樣有問題（失敗、空樣本、找不到 herdr-core）時加 ⚠。tooltip 是各類明細與取樣統計。
- popover（gpui-base `Popover`，`anchor: bottom_right`）：內容畫在視窗最上層並夾在視窗內，不會被 26px 的 block 裁切。內容依序是標題與關閉、範圍說明、總記憶體與 CPU、各類記憶體摘要、「記憶體／CPU」排序切換（gpui-base `ToggleGroup`＋`Toggle`）、各類列（行程數、說明、數值、占比、占比條）、註腳（量測方式、取樣頻率、herdr-core 的 pid、取樣統計、取樣耗時、錯誤）。
- 配色用 `yuzora-dark` theme 色名：底 `popover`、框 `border`、分隔 `divider`、摘要列 `raised`、選取 `selected`、次要文字 `muted`；⚠ 與錯誤用 `tokens().status.blocked.color`（琥珀）。數值用 JetBrains Mono（app 內建）。
- 文字只有英文（`i18n/en.json`）。

### 放置

- **`yuzora` layout**：`status` 是 row：`yuzora-statusbar`（佔滿）＋`yuzora-perf`（150px），chip 靠右。`layout` 只有 `px` 與 `weight` 兩種大小，沒有「依內容寬度」，所以 chip 的 block 固定寬度；左邊空白與狀態列同為 `chrome` 背景、同一條 `divider` 上緣線，看起來是同一條狀態列。row 設 `resizable: false`、不設 `divider`，兩個 block 之間沒有分隔線也沒有拖拉把手。不需要改 L1 的 `yuzora-statusbar/main.js`。
- **其他 layout 的狀態列**（`main`、`win98`、使用者自己做的、從連結安裝的）：視窗畫狀態列的 block 時，把狀態列自己的 mod 畫在剩下的寬度，右端畫一份 `yuzora-perf`，寬 150px（隨 zoom 縮放；和 `yuzora` 狀態列裡 chip 的 block 同寬），不多加任何一列。chip **不是 layout 的節點**：`layout()`、layout 檔、`.state`、`strip`／`ensure`、`layout.set`、dock、`remove_slot` 都看不到它，所以這些操作寫出的檔案與點選，和沒有 chip 時逐位元組相同。chip 畫在狀態列 block 的範圍裡：狀態列的 id、大小、`hidden`、點選都不變，狀態列隱藏或改大小時 chip 跟著它。
- **哪個 block 是狀態列**（`perf_chip::bar`，純函式）：從顯示中的 tab（`"tab": "active"` 的 block）往上，找最近的一個 column，它的最後一個子節點不在 tab 那條分支上，而且是 mod block、固定高度不超過 56px；不是的話再往外一層。高度看 layout 檔的，不看點選：使用者拖大狀態列，chip 不會因此消失。檔案裡的高度與乘上 zoom 畫出來的高度取較小的：win98 依 zoom 把工作列調成 28 個 Win98 像素，也就是 28 × scale ÷ zoom（大字型 Large Fonts 時 scale 是 2）。在 `win98` 本身這只是點選，檔案不變；在使用者複製的 win98 layout 裡會寫進檔案（zoom 0.5 時 56px，大字型再加 zoom 0.5 是 112px），畫出來則一律是 28 × scale，所以 chip 照樣在。結果：`main` 是 `status`（28px），`win98` 是 `taskbar`（檔案裡 40px），`terminals` 沒有（tab 是 column 的最後一個），`yuzora` 自己放了 chip。上限 56px 是 win98 大字型時工作列畫出來的高度：`main` 的狀態列改成 40px 的複本照樣算，dock 在底部的 panel（320px，zoom 0.5 時畫出來 160px）不算。縮小時一條畫出來不超過 56px 的 block 也算狀態列（例如 zoom 0.8 時 64px 的 block），比它外層的狀態列更靠近 tab 時，chip 會畫在它那裡。各種情況：
  - 在頂端的標題列、tab 列、細工具列：不是 column 的最後一個，不算。
  - 終端機 block（例如 28px 高的終端機）：不是狀態列，往外一層找。
  - 兩條細列上下相疊：下面那條（最後一個）。
  - 不包含 tab 的 column 裡的細列（側欄的 footer）：不算。
  - 狀態列隱藏：沒有 chip，也不會跑到上面那條；再顯示時回來。
  - column 的最後一個是 stack（stack 裡放的是可切換的內容）或 row（由幾個 block 組成的狀態列）：不算；整個 stack 包著 tab 與狀態列時，stack 顯示那一邊就照常找到。
  - 狀態列下面還有別的 block（例如一個終端機）：狀態列不是最後一個，不算；那個 block 移除後就有 chip。
- **什麼時候畫**：判斷在 `perf_chip::slot`，由 `window.rs` 的 `visible()` 與 `owned_mods()` 呼叫。這兩處把樹變成 mod 的 slot，`sync()` 在每次樹變動之後（載入、layout 檔被改、`layout.set`、mod 自動 dock、layout 裡的終端機結束而移除 slot、隱藏與顯示）都會跑它們，所以 chip 和每個 layout 裡的 mod 一樣，沒有哪條路徑會漏掉。成本是幾次走訪樹、不讀檔。copy 的 key 是 `<狀態列 id>#yuzora-perf`（上游替屬於某個節點的 mod slot 取名的方式，如 `<stack>#header`）；節點 id 是自由文字，layout 裡有節點正好叫這個名字時，chip 不畫，slot 留給那個節點。`render_block` 畫 mod block 時，這個 key 的 copy 存在、而且沒有節點叫這個名字（`perf_chip::at`），才把它畫在右端；那個節點自己的 copy 只畫在節點的位置。layout 自己放了 `yuzora-perf`（block 或 float，隱藏的、不在前面的 stack 子節點也算）就什麼都不加。
- **沒有狀態列的 layout**（`terminals`、使用者自己沒有狀態列的）：沒有 chip，也沒有 copy。`yuzora-perf.toggle` 這時回錯誤 `unknown command "yuzora-perf.toggle"`（mod 的指令只送給顯示中的 copy），狀態列隱藏時也一樣。要 chip 就在 layout 裡放 `yuzora-perf` block。
- **與 dock 開關無關**：layout 的 `"yuzora": {"dock": "off"}` 與設定頁的個人 dock 選擇都不影響 chip。
- **樣子**：chip 收到 `props().bar`（狀態列的 mod 名稱），畫成那條狀態列的樣子，看起來是同一條：`win98` 的工作列是它的灰 `#c0c0c0`、上緣兩條亮線（`#dfdfdf`、`#ffffff`）、黑字，以 Win98 像素計；其他狀態列照上游 `workspace-status` 的畫法：`chrome` 底、`border` 上緣線、11.8 的 `muted` 字。chip 在右端垂直置中，左邊留著 150px 裡剩下的空白。`yuzora` 狀態列裡的 chip（沒有 `bar`）照舊是 `yuzora-statusbar` 的樣子。
- **zoom**：狀態列與 chip 一起縮放。`win98` 的工作列在任何 zoom 都維持 Win98 像素的大小，所以放大時 chip 也維持大小、不會比工作列高；縮小時 chip 跟著視窗變小，才放得進 150px。
- **窄視窗**：和 `yuzora` 的狀態列一樣，chip 保持 150px，狀態列自己的項目先讓位；放不下的部分在 chip 的左緣被切掉，不會畫到 chip 底下。
- **代價**：沒有狀態列的 layout 沒有 chip；使用者只能用隱藏狀態列來關掉 chip；狀態列自己的項目少了 150px；chip 要知道 `win98` 工作列的畫法（顏色與 Win98 像素），`win98` 改樣子時這裡要跟著改；上游 `window.rs` 多 4 行（見上表）。
- chip 的 copy 和 layout 裡的 mod 一樣：點 chip 開明細（畫在視窗最上層，不會被狀態列裁切）、`yuzora-perf.toggle` 送得到（它是顯示中的 copy）、只在視窗位於前景時取樣。換 layout 時它和其他 layout mod 一起重新開始，第一個畫面就是上一次的樣本（取樣狀態在 `yuzora:perf`）。

## 不做

- gpui-kit 的 `gpui-fps` HUD（`show_fps_monitor`）：不開啟。
- beta.1 的診斷 heartbeat 落盤（#40，`PerfBridge` 的 `logDiagnosticsSample`、stall telemetry、terminal queue metrics）：不移植。beta.2 沒有 WebView event loop 與 xterm queue，要量 UI 卡頓得另外設計。
- 逐行程明細、依 Space 歸屬：沿用 beta.1，只給分類。
- 遠端機器的用量。

## 驗證

```bash
cd app
cargo test --locked -p yuzora-perf          # 聚合、分類、不變式、hc server 辨識與快取、滾動視窗、模組宣告
cargo test --locked --bin herdr-app yuzora  # 模組數、layout 引用的 mod 都有出貨、狀態列的規則（yuzora::perf_chip::）
scripts/yuzora/test-isolated -p herdr-app -- window::tests::the_chip window::tests::a_layout_without_a_status_bar window::tests::a_click_on_the_chip window::tests::in_win98s  # chip 在視窗裡
scripts/yuzora/check-mods yuzora-perf
cargo run --release -p yuzora-perf --example cost -- <app pid> 30   # 在 app 的隔離環境變數下執行
```

實機以 lanes.md 的隔離方式執行（`env -i`、暫存 `YUZORA_APP_HOME`、`HC_SESSION=yuzora-l8-test`、XDG 目錄），結束時 `hc server stop`。

截圖在 `docs/beta2/evidence/`：

| 檔案 | 內容 |
| --- | --- |
| `l8-chip.png` | 右下角的 chip，在指令快捷鍵右邊，與狀態列無縫（merge `beta2/integration` 87249406 後用新安裝的 home 重拍；5 個終端機） |
| `l8-popover.png` | 展開的資源用量 popover（記憶體排序；herdr-core 26 個行程、工具 1 個） |
| `l8-popover-cpu.png` | CPU 排序（8 個終端機；只在隔離 home 的 mod 複本把預設排序改成 CPU 截圖，之後還原） |

popover 由 `herdr-app do yuzora-perf.toggle` 打開。排序切換與關閉按鈕的實際點擊沒有做 GUI 自動化驗證（需要 computer-use，未取得使用者同意）。

## 已知限制

- popover 約 640px 高（zoom 1.0）。視窗比這更矮時，popover 會貼齊視窗上緣，下半部可能蓋到 chip。
- 沒有狀態列的 layout 沒有 chip；chip 只能隨狀態列一起隱藏（見「放置」）。
- 一個行程被 re-parent 離開它的樹（Windows 的 re-parent、自己 daemon 化的輔助程式）就不會再被計入。

## Windows 效能（beta.3）

2026-11-04 在 GitHub `windows-2022` 實測（git 2.56，每項 20 次的中位數）。數字與探針在 `.yuzora-lanes/beta2-research/perf-a-notes.md`。

| 項目 | 實測 |
| --- | --- |
| `git --version`／300 檔 repo 的 `git status` | 15 ms／20 ms |
| 同樣的呼叫經 `yuzora_core::remote::run` | 52 ms／52 ms（pipe 輪詢下限） |
| `git_trust` 信任掃描 | 256–306 ms |
| Job Object＋執行緒快照（每個本機 git） | 6 ms |
| PowerShell 列一個資料夾（冷啟動） | 227 ms |

已做：

- **資料夾 watcher**（`app/src/files.rs`）：macOS 與 Windows 的 watcher 遞迴監看整個資料夾，build 寫 `target/`、`node_modules/` 會一直喚醒側欄與 panel。現在位在**被 ignore 的資料夾裡**的事件，會先用被監看的資料夾自己的 `.gitignore` 與 `.git/info/exclude` 過濾掉（監看 `repo/src` 不會讀 `repo/.gitignore`）（只讀檔，不執行任何程式，所以不需要資料夾信任）。
  - 被 ignore 的**單一檔案本身**（`.env`、`build.log`）照常回報：編輯器、Markdown、圖片檢視器用 `watchFolder(檔案所在資料夾)` 監看，根目錄的 `.env` 被外部改了也要知道，檔案樹新增 `.env.local` 也要馬上出現。
  - `.gitignore` 本身、`.git/` 底下的事件不過濾；watcher 掉事件（rescan）不過濾；規則檔改變後下一批就重讀。
  - 規則的讀法比照 git：`.gitignore` 蓋過 `info/exclude`；符號連結的 `.gitignore` 不讀（git 2.32 之後也不讀）；符號連結的 `info/exclude` git 會讀、這裡跳過（少濾，方向安全）；大小寫不分（沒有讀 `core.ignorecase`，以平台預設為準；在 `core.ignorecase=false` 的 repo，規則 `build/` 也會濾掉實際資料夾 `Build/` 裡的事件，最多延後到 10 秒計時器）。
  - Linux 本來就用 git 修剪被忽略的資料夾，不變。
  - 限制：只讀被監看資料夾自己的規則，子資料夾的 `.gitignore` 與全域 ignore 不算（少濾，方向安全）。被 ignore 的資料夾裡的變動（例如在檔案樹展開 `node_modules/`）不會有 watcher 事件，靠檔案側欄原本的 10 秒計時器反映，與 Linux 修剪被忽略資料夾的行為一致。
- **檔案側欄的 10 秒計時器維持不變**：信任之後、linked worktree 的 git 操作（非 Linux 的 watcher 看不到 gitdir）、Git 失敗重試，watcher 都看不到，不能拉長。信任快取合併後它的成本就會降低；要拉長得先讓非 Linux 也監看 `.git` 檔案 `gitdir:` 指向的資料夾（beta.4 候選）。
- **`remote::run` 不再輪詢**（`yuzora-core/src/remote.rs`）：原本 Windows 讀 pipe 用 `PeekNamedPipe`＋`sleep(50 ms)`，等行程結束用 `try_wait`＋`sleep(10 ms)`，每次呼叫牆鐘有 ~52 ms 的下限。現在 Windows 的讀取緒直接阻塞在 `read`（資料一到就拿走），需要放手時（背景子行程握著 pipe）從外面用 `CancelIoEx` 取消，取消前先在鎖內確認 handle 還沒關；等行程結束不輪詢：Windows 用 `WaitForSingleObject`，Unix 用一條 `waitid(WNOWAIT)` 的緒（只觀察、不回收），`Child` 一直留在呼叫端，逾時才 `kill_tree(&mut child)`（Unix `killpg`；Windows `taskkill /PID /T /F`，再 `child.kill()`＝對 handle 的 `TerminateProcess`）。不用裸 pid 殺：行程被回收（Windows：最後一個 handle 關閉）後 pid 可能已分給別的行程，例如本 app 另一個 git。Unix 的讀取仍是 `poll`，只換掉 10 ms 的等待輪詢。逾時、輸出上限、背景子行程握 pipe 不卡答案的行為不變，Unix 測試在 app 的 CI 跑；Windows 的 `remote::` 測試目前只在 scratch probe 跑過（之後由 #229 的 Windows job 接手）。
