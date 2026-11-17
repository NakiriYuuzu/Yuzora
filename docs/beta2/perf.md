# L8 效能監控（資源用量）

lane L8 把 beta.1 狀態列的 CPU／記憶體 chip 與「資源用量」popover 移植到 beta.2，放在**視窗最右下角**：有狀態列的 layout 在狀態列最右端（`yuzora` 在指令面板快捷鍵的右邊），沒有狀態列的在視窗底部一個 26px 高的 block 右端（見「放置」）。點 chip 往上展開明細，右緣對齊。beta.1 的程式（`src-tauri/src/perf_service.rs`、`src/state/perfStore.ts`、`src/workbench/PerfBridge.tsx`、`ResourceUsagePopover.tsx`）是移植來源，已從 repo 移除（見 tag `v0.1.2`）。架構規則見 [architecture.md](architecture.md)，lane 流程見 [lanes.md](lanes.md)。

## 範圍與檔案

| 路徑 | 內容 |
| --- | --- |
| `app/crates/yuzora-perf/` | `yuzora:perf`：取樣 worker、行程樹聚合與分類、找 herdr-core、滾動視窗 |
| `app/defaults/mods/yuzora-perf/` | 右下角的 chip 與 popover；`i18n/en.json`；Lucide 圖示（`cpu`、`memory-stick`、`app-window`、`wrench`、`chevron-up`） |
| `app/crates/yuzora-perf/examples/cost.rs` | 從外部對執行中的 app 量測單次取樣成本 |

自己目錄以外的改動（總指揮特准）：

| 檔案 | 改動 |
| --- | --- |
| `app/Cargo.toml` | yuzora crate 區塊加 `yuzora-perf = { path = "crates/yuzora-perf" }` |
| `app/src/yuzora/mod.rs` | `host_modules()` 加 `yuzora_perf::host_module(),`；同檔測試的模組數 7 → 8（加模組的必然結果） |
| `app/defaults/layouts/yuzora.json` | `status` 由 block 改成 row：`yuzora-statusbar`（佔滿）＋`yuzora-perf`（150px），`resizable: false` |
| `app/src/yuzora/dock.rs` | `place_perf`：載入時把 chip 放進其他 layout（見「放置」）；`strip` 的還原；`ensure` 依 `layout.set` 之後 layout 檔的大小放 chip；`edit`：app 自己改結構時改在使用者的樹上，再放一次 chip |
| `app/src/mods/render_tests.rs` | 一行 `mod yuzora_perf_place_tests;`，註記 `// yuzora(perf-place):` 在同一行；測試在 `render_tests/yuzora_perf_place_tests.rs` |
| `app/src/window.rs`（非測試） | 3 行，各行同一行註記 `yuzora(perf-place)`：`sync_folders` 把第一次看到的 mod 停靠在邊上時，`dock(&mut self.chrome, …)` 改成 `dock::edit(&mut self.chrome, …, \|t\| dock(t, …))`（改 1 行；`dock()` 改在使用者的樹上做，不再把 chip 包住的根再包一層）；`save` 寫檔後的 `self.file_chrome = edited` 改成 `dock::as_read(&edited, …)`（改 1 行；存檔後 app 手上的檔案樹和載入一樣依大小放 chip）；`layout.set` 個人分支的 `dock::ensure` 移到 `fill_ids` 之前（dock 的那一行，原本的 `yuzora(dock)` 註記之後加上原因；`strip` 清掉保留 id 的節點在同一次寫檔就拿到產生的 id） |
| `app/src/window.rs`（`mod tests`） | 4 行：`edit_heavy`、`open_heavy`、`load_recovery_survives_switches`、`new_vm_keeps_popup` 數 mod 複本時加上 `&& c.inst.name != "yuzora-perf"`，各行同一行註記 `// yuzora(perf-place):`。這些測試的 layout 現在也有 chip，它的複本不算在內 |
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

- `layout` 只有 `px` 與 `weight` 兩種大小，沒有「依內容寬度」。所以 `status` 改成 row，`yuzora-perf` 固定 150px、chip 靠右；左邊空白與狀態列同為 `chrome` 背景、同一條 `divider` 上緣線，看起來是同一條狀態列。
- row 設 `resizable: false`、不設 `divider`，兩個 block 之間沒有分隔線也沒有拖拉把手。
- 不需要改 L1 的 `yuzora-statusbar/main.js`。
- 其他 layout（`main`、`terminals`、`win98`、使用者自己的）由 `app/src/yuzora/dock.rs` 的 `place_perf` 在載入時放進去。和 dock 一樣只在畫面上，存檔時 `strip` 拿掉，layout 檔不會出現 `yuzora-dock-*`；dock 設成 off 也照放。layout 裡任何地方（block 或 float）已經有 `yuzora-perf` 時不動：`yuzora.json` 與它的複本（包括改過而另存的 `yuzora-edited`）。
  - 狀態列：放 tab 的 block 最近的上層 column，最後一個子節點是 32px 以下的 mod block。那個 block 原地變成 row，原本的內容移到子節點 `yuzora-dock-body`，右邊是 `yuzora-dock-perf`（150px）。id、大小、外觀留在原節點，所以 `.state` 對它的點選（隱藏、改高度）仍是整條狀態列的。`main` 走這條，形狀和 `yuzora.json` 的 `status` 相同。
  - 沒有狀態列：根是 column 時在最後加一個 26px 高的 block `yuzora-dock-perf`（`terminals`；`win98` 在 taskbar 下方，taskbar 40px 不算狀態列）。根不是 column 時，根原地變成 column（id 改成 `yuzora-dock-root`）：根原本的內容帶著根的 id 成為第一個子節點，下面是 chip。`.state` 以 id 記點選的欄位（隱藏、背景、字級）與根上的 floats 跟著 id 走，所以之後不管什麼包住這個節點，畫面和 app 手上的檔案樹都在同一個節點上看到它們；`titlebar`、`traffic_lights`、`theme`、dock 的 `yuzora` 設定這些只有根才有的欄位，以及大小、padding、邊框，留在根上。`layout.select`、終端機 pane、mod 複本照舊用原本的 id 找到內容。
  - 位置只看 layout 檔裡的大小，不看點選。載入或檔案被改時，看剛讀到的檔。shipped layout 的 `layout.set` 不改檔（送來的大小都是點選），`ensure` 看原本的檔（`file_chrome`）。個人 layout 的 `layout.set` 把送來的樹寫成檔，`ensure` 與 `as_read` 都看送來的樹。存檔時 `file_chrome` 也是寫出去的樹重新放一次（`as_read`），不是畫面上的樹。app 自己改結構（`sync_folders` 把第一次看到的 mod 停靠在邊上）時，`dock::edit` 把改動做在使用者的樹上，再依檔案的大小放一次 chip。所以載入、檔案被改、`layout.set`、存檔或停靠之後，畫面上的樹和 `file_chrome` 只差點選，重啟後 chip 在同一個位置，`.state` 不會記到沒有人做的點選。
  - `ensure` 只在送來的樹就是重新放置的樹加上點選時照收；否則換成重新放置的樹（例如在舊狀態列下面再加一條 32px 以下 mod block，chip 當下就移到新的那條）。重新放置時點選不會丟：每個節點的隱藏、在同一個方向（row 裡是寬、column 裡是高）上的大小、stack 顯示的子節點、float（dock 的抽屜）的邊、對齊與隱藏，都從送來的樹帶過去。
  - `strip` 不刪任何不是保留 id 的節點。chip 的包裝不管被 mod 或 `dock()` 移到哪裡（例如 `yuzora-dock-root` 被包進新的 row），都在原地還原；只拿掉沒有子節點的保留節點（chip、模式列、模式、使用者自己取保留 id 的 block）。`yuzora-dock-body`、`yuzora-dock-root` 與其他有子節點的保留節點還原不了時留下，只清掉 id。
  - mod 用 `layout.set` 改了狀態列顯示的內容（`yuzora-dock-body`），例如加 padding：狀態列在檔裡成了包著那個內容的 row，不再是狀態列，chip 離開它，視窗底部多一條 26px 的 chip 列（根不是 column 時根被包成 column）。內容留成狀態列的子節點，在同一次寫檔就拿到產生的 id。改的是根不是 column 時帶著根 id 的子節點：包住它的 column 留在檔裡，帶著只有根才有的欄位與大小，在同一次寫檔拿到產生的 id；畫面和原本一樣，chip 在底部。mod 把背景、字級或 floats 設在 `yuzora-dock-root` 本身而不是那個子節點上時也一樣，那個 column 留在檔裡。這些情況 mod 的修改都不會不見，重啟後和畫面一樣，代價是使用者的檔多一層。
  - 使用者自己在檔裡寫的保留 id：沒有子節點的保留節點（例如自己取名 `yuzora-dock-perf` 的 block）照舊被拿掉，和 `main` 一樣。有子節點的、或取名 `yuzora-dock-body` 的，內容全部留下，id 被清空（`main` 會連同底下的節點一起拿掉）。根取名 `yuzora-dock-root` 時內容也全部留下，但 `main` 保留這個名字，這裡 id 被清空。清空的 id 在 `layout.set` 寫檔時當下換成產生的 id；一般存檔先寫成 `"id": ""`，下次載入時 app 補上產生的 id。
  - win98：`fitLayout` 先用點選把 taskbar 縮成 28px（zoom 改變時是 21.538px 這類值），再把 `layout()`（含 chip）送回 `layout.set`。shipped 的 `win98` 檔仍是 40px，chip 留在 taskbar 下方，結構沒變，所以被接受。win98 的複本則會把縮小後的高度寫進自己的檔，taskbar 從此算狀態列，chip 當下就移到 taskbar 右端（150px 寬），重啟後也一樣。
- 既有 app home 裡沒有 `yuzora-perf` 的舊 `yuzora` layout 也會在載入時得到 chip，不用重設 layout。

## 不做

- gpui-kit 的 `gpui-fps` HUD（`show_fps_monitor`）：不開啟。
- beta.1 的診斷 heartbeat 落盤（#40，`PerfBridge` 的 `logDiagnosticsSample`、stall telemetry、terminal queue metrics）：不移植。beta.2 沒有 WebView event loop 與 xterm queue，要量 UI 卡頓得另外設計。
- 逐行程明細、依 Space 歸屬：沿用 beta.1，只給分類。
- 遠端機器的用量。

## 驗證

```bash
cd app
cargo test --locked -p yuzora-perf          # 聚合、分類、不變式、hc server 辨識與快取、滾動視窗、模組宣告
cargo test --locked --bin herdr-app yuzora  # 模組數、layout 引用的 mod 都有出貨、每個 layout 剛好一個 chip；三個不變式在 38 種 layout × 3 種 dock 設定 × 7 種 layout.set（798 次）與 5 種停靠（570 次）上成立，I1、I2 另在亂數產生的樹上成立（3000 次，能讀成 layout 的才檢查）（yuzora::dock）
cargo test --locked --bin herdr-app yuzora_perf_place  # 出貨的 yuzora-perf 收到一筆正常樣本時畫出 chip（文字、CPU 圖示、展開箭頭），不是等待中或 ⚠；測試環境沒有字串檔，畫的是字串 key，看不到數字
scripts/yuzora/test-isolated -p herdr-app --bin herdr-app window::tests::  # 每個出貨 layout 只載入一個 chip、畫在右下角；layout.set 跨過狀態列高度、包住 layout()、改狀態列內容，與停靠新 mod 之後，檔案是使用者的、畫面與重啟一致、不丟也不多記點選
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
- chip 放在狀態列旁邊時，狀態列原本的 mod（`main` 的 `workspace-status`）在畫面上的 node id 是 `yuzora-dock-body`，layout 檔裡仍是原本的 id。
- 根不是 column、也沒有狀態列的個人 layout：`.state` 以根的 id 記的背景或字級點選，套用在根原本的內容上，不含下面的 chip。出貨的 layout 都不是這種。
- 個人不能關掉 chip（預設決定）。
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
