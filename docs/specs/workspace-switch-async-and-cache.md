# Workspace 切換背景化與 per-workspace 快取

- 日期：2026-07-25
- 狀態：draft（待核准後實作；尚未開 GitHub issue）
- 症狀：切換專案（workspace）時整個 UI 一致卡頓；切回曾開過的專案時所有內容（git、檔案樹、tabs）全部重抓、從空白開始。
- 目標一句話：**切換的每一步都在背景非同步並行執行；切換過的 workspace 保留紀錄，切回立即顯示、背景刷新（stale-while-revalidate）。**

## 1. 根因分析

### R1（主因）：熱路徑 Tauri command 全是同步 fn，在 main thread 執行

Tauri 2 官方文件（v2.tauri.app/develop/calling-rust §Async Commands）：*"Commands without the `async` keyword run on the main thread"*。本 repo 的 `perf_service.rs:203-210` 註解也已自行驗證過同一事實（同步 command 走 Blocking execution context，macOS/wry 下 inline 跑在主執行緒，作者實測 8.2–10.4ms）。

全 app 116 個 command 中 **64 個是同步 fn**，而 workspace 切換熱路徑上的幾乎全部都是：

| command | 位置 | 阻塞內容 | 量級 |
|---|---|---|---|
| `git_detect` | `git_service.rs:253` | 2 個 git 子行程（`--version`、`rev-parse`）＋ .git watcher 建立 | 數十 ms 起 |
| `git_status_cmd` | `git_service.rs:286` | `git status --porcelain=v2 --untracked-files=all` 子行程 | 大 repo 百 ms 級；watcher 事件驅動、高頻 |
| `git_branches` | `git_service.rs:1024` | git 子行程 | 數十 ms |
| `git_log_page` | `git_log.rs:778` | 無 query 1 個子行程（200 筆/頁）；**帶 query 3 個子行程、其中 2 趟全量 log** | 百 ms～秒級 |
| `git_log_authors` | `git_log.rs:808` | 全量掃 `git log --all` 歷史 | 正比 repo 歷史 |
| `git_commit_detail` | `git_log.rs:800` | 固定 3 個 git 子行程 | 百 ms 級 |
| `list_dir` | `fs_service.rs:52` | 單層 `read_dir`＋排序（lazy 展開，成本可控） | cold cache 大目錄數十 ms |
| `open_file` | `fs_service.rs:213` | 整檔讀（上限 50MB）＋全檔逐 byte line-ending 掃描 | 大檔數百 ms |
| `save_file` / `read_file_base64` | `fs_service.rs:218/354` | 整檔寫／讀＋base64 | 檔案大小線性 |
| `start_watch` | `watcher.rs:42` | notify 遞迴掛載；macOS FSEvents 便宜，**Linux inotify 走訪整棵樹** | 平台差異大 |
| `lsp_send` | `lsp_service.rs:763` | 寫 server stdin；**pipe buffer 滿時硬卡 main thread**（每次鍵入都走） | 隱藏最深 |
| `lsp_start` / `lsp_detect_server` | `lsp_service.rs:735/717` | spawn server 子行程／全 PATH stat | 數十 ms |

疊加機制：JS 端 `invoke` 看似 async，但這些 command 在 Rust main thread 上**排隊序列執行**。切換一次 = `open_workspace` + `list_dir` + `git_detect`（含 2 子行程）+ `git_status` + `git_branches` + `start_watch` +（LogTab 掛載時）`git_log_page` + `git_log_authors` 全部凍住 event loop，且每個 `run_git`（`git_service.rs:131-189`）是 10ms 粒度 `try_wait` 輪詢——單一 git 呼叫的阻塞下限就是 ~10ms。

### R2：切換即清空、無任何 per-workspace 快取

- `gitStore.detect()`（`src/state/gitStore.ts:160-178`）進場先把 `status/branches` 清成 null，再 waterfall 重抓。
- `FileTree`（`src/workbench/FileTree.tsx`）的 roots/children/展開狀態全是元件 local `useState`，換 workspace 或 `treeRevision` bump 即全滅。
- `workspaceSession`（`src/state/workspaceSession.ts`）只存**單一**（最後）workspace 的 tabs，且切換時**只寫不讀**（`SessionRestoreBridge.tsx:28-36` 只在冷啟 restore）——A→B→A 不會還原 A 的 tabs。
- `documentRegistry.clearAll()`（`workspaceActions.ts:32`）丟掉所有 buffer；LSP client 全部 disconnect（`LspBridge.tsx:16-22`）。
- **唯一的反例（也是本計畫的設計先例）**：terminal 完全不清——`terminalStore` 用 per-workspace 分桶（`layouts: Record<workspace, …>`），PTY 跨切換存活，`TerminalDrawer.tsx:980-998` 用 `hidden/inert` 藏非當前 workspace 的 pane。`workbenchLayoutStore.terminalWorkspaceRatios` 亦同。

### R3：Waterfall 與 debounce 疊加

- `openWorkspaceAtPathWithOutcome`（`src/lib/workspaceActions.ts:19-44`）：`openWorkspace` → `clearAll` → `setWorkspace` → **await** `allowWorkspaceAssetScope` → `startWatch`，嚴格序列。
- `gitStore.detect`：await `gitDetect` 完成後才 `Promise.all([refresh(), loadBranches()])`，而 `refresh` 還吃 300ms trailing debounce（`gitStore.ts:75,194`）——首載也被 debounce 拖慢。
- Rust 端依賴：`git_status_cmd`/`git_branches` 從 `GitServiceState` 讀 root（由 `git_detect` 寫入），前端無法先發。

### R4（次要但相關）：watcher 生命週期與全樹 remount

- 切換時**不先停舊 watcher**、新 watcher fire-and-forget（`workspaceActions.ts:40`；Rust 端 `watcher.rs:43-54` 靠新 handle 覆寫舊的才 drop）→ gap 內舊 workspace 事件可打進新 workspace 的 listener（`fs:external-change` 的三個 listener 都沒做 workspace 比對）。
- `fs:external-change` 一來 `ExternalChangeBridge` 就 `refreshTree()` → `treeRevision` bump → `FileTree` 以 `key={treeRevision}` **整樹 remount**：展開狀態全丟＋重新 listDir。`dist/`、`target/` 等 build 產物未被 watcher 過濾（只濾 `.git`/`node_modules`，`watcher.rs:8-11`），開發中會頻繁觸發。

## 2. 目標與非目標

**目標**
1. 切換 workspace 期間 host main thread 無可感知阻塞（輕 command 往返 p95 < 50ms）。
2. 所有切換觸發的請求並行發出，無人為 waterfall／debounce 首載懲罰。
3. 切換過的 workspace 保留：git status/branches 快照、檔案樹（含展開狀態）、editor tabs；切回 <100ms 內顯示上次內容，背景 revalidate。
4. 高頻日常操作（開檔、存檔、LSP send、git status 刷新）同步移出 main thread。

**非目標**
- 不做多 workspace 同時 watch（維持單一 active watcher；非活躍 workspace 的快取靠切回時 revalidate）。
- 不快取檔案內容 buffer（tabs 恢復後由 async 化的 `open_file` 背景重載；unsavedGuard 已保證切換時無 dirty buffer）。
- 不動 shutdown/exit handler（`command_inventory_tests` 守護區，見 §5）。
- git log 帶 query 的全量掃描優化、SFTP local pane 等附帶發現（§7）另案。

## 3. 方案設計

### Phase 1 — Rust：熱路徑 command 移出 main thread（止血，收益最大）

按 codebase 既有三種慣例改造（樣板皆已存在，照抄）：

1. **`async fn` + `tauri::async_runtime::spawn_blocking`**（主要手法；樣板 `process_service.rs:640-679`、`logging.rs:554-560`、`agent_terminal.rs:330-385`）：先 `lock → clone 出 root/Arc → drop guard`，再 move 進 closure。**禁止跨 `.await` 持 `std::sync::MutexGuard`**。
2. **`#[tauri::command(async)]` 保持 sync body**（樣板 `perf_service.rs:211`）：適用整段持鎖、無法乾淨拆 clone 的 command——無 await 就無 guard-across-await 問題。
3. command 內 `std::thread::spawn` + generation 取消（樣板 `search_service.rs:185-201`）：已用於 search，不需再動。

改造清單（P0 → P1）：

| 批次 | commands | 手法 |
|---|---|---|
| P0 | `git_status_cmd`、`git_detect`、`git_branches`、`git_log_page`、`git_log_authors`、`git_commit_detail`，以及 `git_service.rs` 其餘全部（stage/unstage/discard/commit/checkout/fetch/pull/push/diff/rollback/…） | async fn + spawn_blocking（`repo_root()`／`with_requested_repo` 已是「clone root 後放鎖」形狀，`git_log.rs:770` 同）；`git_detect` 持兩個 State Mutex＋建 watcher，可先用 `(async)` 標記 |
| P1 | `open_file`、`save_file`、`read_file_base64`（零 State，無風險）；`lsp_send`、`lsp_start`、`lsp_detect_server`；`start_watch`；`fs_create_*`/`fs_rename`/`fs_delete` | async fn + spawn_blocking；lsp 系列 State 是 `Arc<LspManager>`，clone 後 move |
| 不動 | `open_workspace`、`allow_workspace_asset_scope`（µs 級）、`search_workspace`（已背景化）、`list_dir`（單層；若 profiling 顯示大目錄明顯再納入） | — |

注意事項：
- tokio features 缺 `rt`/`rt-multi-thread`（`Cargo.toml:56`）→ **一律用 `tauri::async_runtime::spawn_blocking`**，不可用 `tokio::task::spawn_blocking`。
- async command 的 `State<'_, T>` 需帶 lifetime 且回 `Result`（現有 command 已全是 `Result`）。
- `AppHandle`／`tauri::ipc::Channel` 皆 Send，可 move（`agent_terminal.rs`、`search_service.rs:194` 已驗證）。
- async 化後 git 讀寫可能真並發：寫操作有 git 自身 index.lock 互斥、前端 `runOp` busy 閘（`gitStore.ts:256-258`）已防 UI 併發寫、status 已設 `GIT_OPTIONAL_LOCKS=0`。驗收時加「stage/commit 與 watcher refresh 併發」的行為測試；若出現問題，Rust 端補 per-repo `tokio::sync::Mutex` 序列化寫操作（讀不序列化）。
- 完成後同步重產 `.github/clippy-baseline.json`（exact baseline，任何 warning 位移都會 fail CI）。

### Phase 2 — 前端：切換編排並行化

1. **新增 `git_bootstrap(path)` command**（async）：Rust 端一次完成 detect → status ‖ branches（兩個 blocking task join），回傳 `{ environment, status?, branches? }`。消除「detect 先行寫 State、status/branches 才能發」的結構性 waterfall 與兩趟 IPC。細粒度 command 保留給後續 refresh。`gitStore.detect` 改吃 bootstrap 結果一次 set。
2. **首載豁免 debounce**：`gitStore.refresh` 的 300ms trailing debounce 只該管 watcher 風暴，`detect`/bootstrap 後的首次載入直接執行（加 `immediate` 參數或由 bootstrap 直接回 status）。
3. **`openWorkspaceAtPathWithOutcome` 並行化**（`workspaceActions.ts`）：
   - 保留：`confirmDiscardingUnsaved` → `openWorkspace`（需要 canonical 當 key）→ `clearAll` → `setWorkspace`（UI 立即切換）。
   - 之後全部並行 fire-and-forget：`allowWorkspaceAssetScope`（失敗僅 warn，現已如此，不必再 await）、watcher 切換、git bootstrap（GitBridge 觸發）、MRU 記錄、logUserAction。
4. **watcher 生命週期收斂**：新增/改造為 `switch_watch(path)`（先 drop 舊 handle 再掛新），事件 payload 附 `workspacePath`；`fs:external-change` 的三個 listener（`ExternalChangeBridge.tsx:10-29`、`GitBridge.tsx:29-31`、`ExternalChangeResolver.tsx:119-140`）比對 live workspacePath 後才處理（比照 `LspBridge.tsx:29-33` 的防串場模式）。`git:state-changed` 同樣附 root。

### Phase 3 — Per-workspace 快取（stale-while-revalidate）

比照 `terminalStore` 的 per-workspace 分桶慣例：

1. **git 快照**：`gitStore` 增加模組級 `snapshots: Map<root, { environment, status, branches, at: number }>`（LRU 上限 8）。每次 `set` status/branches 時同步寫快照；`detect(root)` 進場時若有快照→**先 hydrate 顯示（標記 stale）**→ 背景 bootstrap 完成後覆蓋。UI 可在 stale 期間顯示細微 refreshing 指示（可選）。
2. **檔案樹狀態提升**：新建 `src/state/fileTreeStore.ts`，per-root 保存 `{ rootNodes, childrenByDir: Map<dir, FileNode[]>, expandedDirs: Set<string>, scrollTop }`。`FileTree`/`TreeNode` 改為受控（去掉 local useState）：
   - 切回：hydrate 即整樹（含展開）復原；背景並行 re-list root＋所有 expandedDirs（并發上限例如 8）後 diff-apply。
   - `fs:external-change` 改走**精準失效**：payload 路徑 → 對應目錄 re-list，不再 `treeRevision` 全樹 remount（同時修掉 R4 的展開狀態丟失）。`treeRevision` 機制保留給 context-menu 檔案操作的顯式刷新，但改為只失效受影響目錄。
3. **editor tabs per-workspace**：`workspaceSession` 升 v2：`Record<workspacePath, { tabs, activePath }>`（含 v1 讀取遷移）。`SessionRestoreBridge` 持續寫入對應 key；`openWorkspaceAtPathWithOutcome` 在 `setWorkspace` 後從 map 恢復 tabs（`open_file` 已 async 化，內容背景載入）。冷啟 restore 行為不變。
4. **順帶修**：`previewStore.reset()`（`previewStore.ts:373`）從「全清」改為只清離開的 workspace（它的 state 本身已按 workspace 分桶）。
5. 失效策略：活躍 workspace 由 watcher 維護常新；非活躍 workspace 無 watcher，切回時全面 revalidate（設計上不信任舊快照的正確性，只用它消除空白等待）。

### Phase 4 — 量測與驗收

- **host main thread 阻塞**：gui-acceptance 腳本在切換期間連續 invoke 輕 command（如 `get_log_level`）記錄往返延遲，改造前後對比；目標 p95 < 50ms（現況推估數百 ms～秒級）。
- **renderer 側**：#40 的 `stallTelemetry`（`src/workbench/stallTelemetry.ts`，long-task 計數＋event-loop lag）量切換後 3s 窗口，數字應顯著下降。
- **快取體感**：切回已開過的 workspace，檔案樹（含展開）與 git 面板 <100ms 顯示內容；冷開新 workspace 各面板獨立進場、UI 全程可互動。
- 回歸：`bun run test`、`cargo test --locked`、`cargo fmt --check`、clippy baseline 驗證；jsdom 測不到的（watcher、切換體感）走 `gui-acceptance` skill 實機驗收。
- 單元測試重點：gitStore 快照 hydrate/stale 覆蓋、fileTreeStore 精準失效、workspaceSession v1→v2 遷移、watcher 事件 workspace 過濾、Rust 端 git 讀寫併發行為。

## 4. 分期交付

| 里程碑 | 內容 | 預期效果 |
|---|---|---|
| M1 | Phase 1（Rust async 化 ~20+ commands）＋ clippy baseline 重產 | 卡頓主體消失：UI 不凍，只剩面板 loading |
| M2 | Phase 2（git_bootstrap、debounce 豁免、編排並行、watcher scoping） | 冷開 workspace 各面板同時進場、更快填滿 |
| M3 | Phase 3（git 快照、fileTreeStore、workspaceSession v2、preview 順帶修） | 切回零空白；展開狀態與 tabs 保留 |
| M4 | Phase 4（量測、gui-acceptance、LRU/邊界收尾） | 有數字的結案證據 |

M1 獨立可交付且風險最低，建議先行；M2/M3 依賴 M1（async 化後並行才有意義）。

## 5. 風險與護欄

- **`command_inventory_tests`**（`lib.rs:394-445`）：以 `include_str!` 對 `run()` 原始碼做字面比對，守 exit handler 內容與順序。**不改 exit handler、不在其中引入 `.await`、不動那些字面 token**（如 `…lsp_service::LspState>().0.stop_all()`）即安全；`generate_handler![]` 清單本身不因 async 化而變。
- **clippy exact baseline**：async 化的大量行號位移必然動 baseline → 實作 PR 內同步重產 JSON，CI 腳本只驗不重產。
- **git 併發**：見 Phase 1 注意事項；驗收含併發行為測試。
- **stale 快照誤導**：切回瞬間顯示的是舊資料；revalidate 通常 <1s 收斂。git 寫操作不因 stale 禁用（操作完成本身會刷新收斂）；如需保守可在 stale 期間 disable rollback/discard 這類破壞性操作（實作時裁量）。
- **記憶體**：快照 LRU 上限 8 個 workspace；單快照量級 = status entries + branches + 樹 children map，估百 KB 級；log 首頁暫不快取（LogTab 惰性載入 + async 化後不卡）。

## 6. 附帶發現（本計畫不修，另案追蹤）

1. `lsp_stop_workspace` command 前端無任何非測試呼叫點（`ipc.ts:365` wrapper 孤兒）——切換時實際走 `lspManager.stopWorkspace` 前端 disconnect；確認 Rust 端 command 是 dead code 還是該接上。
2. SFTP 面板本機側 cwd 在切換 workspace 後停留舊路徑（`SshPanel.tsx:537-549` 只在 `cwd === null` 時吃 workspacePath）。
3. `git_log_page` 帶 query 時 2 趟全量 log ＋ `git_log_authors` 全量掃歷史——大 repo 的後續優化題（`--max-count` 分頁化／authors 快取）。
4. watcher ignore 清單只有 `.git`/`node_modules`（`watcher.rs:8-11`），`dist`/`target`/`.venv` 等 build 產物會觸發事件風暴（M3 的精準失效可大幅緩解，但 ignore 擴充仍值得）。
5. `agent_detect_runtimes`、`agent_list` 等仍為 sync（低頻，暫不納入）。

## 7. 驗收清單（供 yuuzu-dev / implementer 對照）

- [ ] M1：P0/P1 清單內 command 全部移出 main thread；切換期間輕 command 往返 p95 < 50ms（gui-acceptance 量測留證）。
- [ ] M2：切換觸發的 IPC 無 waterfall（git 一趟 bootstrap；assetScope/watch/MRU 並行）；watcher 事件帶 root 且 listener 過濾；舊 watcher 先停後掛新。
- [ ] M3：A→B→A 切回時樹（含展開）、git 面板、tabs 立即復原並背景刷新；外部 fs 變更不再整樹 remount。
- [ ] 全回歸綠：vitest / cargo test / fmt / clippy baseline。
