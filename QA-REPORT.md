# Yuzora 上線前唯讀 QA 驗收報告

> 狀態：原始唯讀驗收、使用者授權的 remediation 與 release review 回歸均已完成。QA-001～QA-033 共 33 個 findings，其中 32 個實際缺陷已修復；QA-019 經 upstream API 證明為 false positive，並已修正誤導性的 provenance 註解與加入 contract。所有外部簽章、平台、review 與權限條件仍誠實列為 BLOCKED。

## 1. 執行摘要

- 測試開始：2026-08-26 00:52:49 +08:00（Asia/Taipei）
- 測試環境：macOS 26.6.1（25G76）、Apple arm64、Bun 1.3.14、rustc/cargo 1.96.0
- 測試來源：原始 QA baseline `d97fb7a5724669394abeb11360eef7330e332d06`；latest remote PR head／本輪 remediation baseline `38a445aad3155011b4c13b1c5bfcef1509cfa9d2` 加本報告所列 15-file patch
- 產品版本：`0.0.9-beta.1`（`package.json` 與 `src-tauri/tauri.conf.json` 一致）
- 原始驗收結束：2026-08-26 09:48:24 +08:00（01:20:36 曾因 QA-009 暫停；經使用者明確要求後續測完成）
- Post-review 修復回歸與本機候選驗收結束：2026-08-26 23:32:41 +08:00
- QA-028～QA-033 follow-up remediation完整本機驗證結束：2026-08-27 09:35:28 +08:00
- 整體結論：**NO-GO**
- 功能總數／通過／失敗／阻塞／未測試：**96／79／0／17／0**
- Repository 可處理問題：**32／32 個實際缺陷已修復；QA-019 的 invalid-ref 前提已證偽。QA-003 的 release workflow contract PASS，但實際簽章候選仍受外部憑證／新 exact-head workflow run 阻擋。**
- 上線阻擋問題摘要：
  - QA-003（P1 release gate）：Stable／Beta macOS workflow 已改為 fail-closed Developer ID signing、notarization、strict `codesign`、`spctl` 與 app／DMG stapler validation；本機沒有 Apple credentials，尚未取得實際 signed／notarized candidate，因此仍不可發布。
  - GitHub secret inventory 只有 `TAURI_SIGNING_PRIVATE_KEY` 與其 password；六項 Apple secrets 仍缺。`main` 未啟用 branch protection，repository rulesets 為空；release-sensitive patch 尚待 maintainer review 與明確 merge 授權。
  - Windows／SmartScreen／WSL2、真實 PostgreSQL／MSSQL、updater download／install 與 OS vault 等外部候選環境仍受阻；不得推定正常。
  - SFTP browse 已實機通過，但 mkdir／rename／delete／upload 等破壞性或寫入流程未在正式資料上執行；backend unsafe-leaf guard 已由回歸測試驗證。

### 修復驗證摘要

- 上一個13-file remediation隔離 build：`/private/tmp/yuzora-ops.LSCoT2/cargo-target/release/bundle/macos/Yuzora.app`；DMG SHA-256 `97ad88de36c798473abdbe36bd1dedea4888c9efde4e52927ae32c641230d680`，`hdiutil verify` PASS；bundle `0.0.9-beta.1`／`dev.yuuzu.yuzora`／arm64，按 local candidate contract 為 ad-hoc、無 TeamIdentifier。本輪15-file patch的新 exact-head candidate尚待 push／CI，不把舊包當成最終 acceptance。
- Frontend：typecheck PASS；lint PASS（0 errors，49 既有 warnings）；180 files／2,463 tests PASS；production build PASS（3,105.77 kB chunk warning保留）。
- Rust：`cargo check --locked --all-targets`、fmt、exact clippy baseline PASS；891 unit tests PASS、1 ignored；DB integration 1 PASS、1 ignored。
- Release：version／notes／Beta／Stable preflight PASS；release 7 files／45 tests、actionlint v1.7.7與YAML parse PASS；UI targeted 5 files／80 tests PASS；先前 `bun run tauri:build` 在 repository 外 code 0。
- Computer Use：僅用 bundled `@oai/sky` 操作 exact `/private/tmp/yuzora-ops.LSCoT2/.../Yuzora.app`。新增回歸包含 `>` Space／Agent 搜尋與 keyboard flow、Logs date-only／datetime-local／RFC3339／非法日曆時間、50-row paging／expanded-state isolation、accent persistence／restore；Git menu、Preview、SSH smoke 亦 PASS。no-snapshot session、large inventory cap 與 execution-origin refresh 以 deterministic tests 覆蓋，未偽稱實機重現。

### 模型與委派紀錄

- 呼叫主機可用模型：`gpt-5.6-sol`、`gpt-5.6-terra`、`gpt-5.6-luna`、`gpt-5.5`、`gpt-5.4`、`gpt-5.4-mini`、`gpt-5.3-codex-spark`、`@cf/zai-org/glm-5.2`、`claude-opus-4-6-thinking`、`claude-sonnet-4-6`、`grok-composer-2.5-fast`、`gemini-3-flash`、`gemini-3.1-flash-image`、`gemini-3.1-flash-lite`、`gemini-pro-agent`、`gemini-3.1-pro-low`、`gemini-3-flash-agent`、`gemini-3.5-flash-extra-low`、`gemini-3.5-flash-low`、`gemini-3.6-flash-high`、`gemini-3.7-flash-high`、`gpt-5.6-luna-fast`、`gpt-5.6-sol-fast`、`gpt-5.6-terra-fast`、`gpt-oss-120b-medium`、`grok-3-mini`、`grok-3-mini-fast`、`grok-4.20-0309-non-reasoning`、`grok-4.20-0309-reasoning`、`grok-4.20-multi-agent-0309`、`grok-4.3`、`grok-4.5`、`grok-4.6`、`grok-build-0.1`、`stealth/ox-alpha`。
- 已委派：2 個 `gpt-5.6-luna`／max effort 獨立全專案盤點；兩者均完成。其一僅做 source inventory；其二執行 automated checks，但錯誤地在原 repository 寫入 ignored build artifacts，見 QA-009。
- 已委派：2 個 `gemini-3.7-flash-high`／high effort；兩次均被代理 runtime 以 `Requests ending with a model turn are not supported.` 拒絕，沒有取得盤點結果，列為 BLOCKED。
- Computer Use：僅使用 Codex 內建 `@oai/sky` 操作 `/private/tmp/.../Yuzora.app`；沒有執行任何 Orca 指令。

## 2. 功能覆蓋表

| 功能／畫面 | 測試情境 | 結果 | 備註與證據 |
|---|---|---|---|
| 01 模型可用性清單 | 列出主機全部模型 | PASS | 已完整列於執行摘要。 |
| 02 Luna x2 盤點 | 兩份 max-effort inventory | PASS | 兩者完成；一份 source inventory、一份 checks/report。 |
| 03 Gemini x2 盤點 | 兩份 high-effort inventory | BLOCKED | 代理 runtime 兩次同一 400 錯誤，未產生結果。 |
| 04 測試前 Git 基線 | `git status --short` | PASS | 無輸出；分支與 HEAD 已鎖定。 |
| 05 版本與 release contracts | version、beta、stable updater contract | PASS | 三個 check 均 exit 0。 |
| 06 README source build | `bun run tauri:build` | PASS | local no-updater／no-sign build code 0；不再要求 production updater private key。 |
| 07 Beta Tauri build | no-updater／no-sign isolated build | PASS | exit 0，約 1m05s。 |
| 08 DMG metadata／完整性 | version、identifier、SHA-256、verify | PASS | 最新 post-review DMG VALID；hash `97ad88de…30d680`。 |
| 09 macOS code signing | strict codesign／Gatekeeper | BLOCKED | protected workflow 已 fail-closed；本機 build 按設計 unsigned，無 Apple credentials／實際 release run，不能宣稱 Gatekeeper PASS。 |
| 10 Windows installer／SmartScreen | Windows candidate 實機驗收 | BLOCKED | 無 Windows candidate／主機。 |
| 11 README 版本資訊 | 英／繁中 badge 對照 | PASS | 兩份 badge 與 packaged README 均為 `0.0.9-beta.1`。 |
| 12 Frontend 靜態檢查 | typecheck、lint | PASS | exit 0；lint 0 errors／49 既有 warnings。 |
| 13 Frontend tests | Vitest | PASS | 179 files／2,452 tests。 |
| 14 Frontend production build | Vite build | PASS | exit 0；最新 bundle 3,105.68 kB，保留 chunk warning。 |
| 15 Rust checks/tests | check、fmt、clippy baseline、tests | PASS | 891 passed／1 ignored；DB integration 1 passed／1 ignored；baseline 251 diagnostics。 |
| 16 Packaged app 啟動／重啟 | 最新 `.app` 冷啟、關閉、再開 | PASS | 首次可操作 state 約 1.69s。 |
| 17 Graceful close | native close、process exit | PASS | 關閉後 packaged binary 無 process。 |
| 18 Workspace session restore | mode、README tab、drawer、設定 | PASS | 重開後恢復 ADE、README 與 drawer closed。 |
| 19 Workspace rail | 收合／恢復、重複操作 | PASS | live UI 驗證。 |
| 20 Space switching | yuzora／privacy-filter 往返 | PASS | tabs／panes 保留，未重現 session 遺失。 |
| 21 Agent focus | 選 Agent 自動切 owning Space | PASS | live HERDR 驗證。 |
| 22 HERDR terminal pages／panes | tab、雙 pane、切 Space 恢復 | PASS | 既有 page／pane state 保留。 |
| 23 HERDR pane menu | 右鍵開啟、Escape 取消 | PASS | 未執行 rename／split／close。 |
| 24 Agent Inspector | read-only detail／切換／鍵盤 | PASS | production Inspect button 可開啟 metadata、text／ANSI output；Escape 關閉後焦點回到 trigger。 |
| 25 HERDR mutations | Start Agent、新 terminal、rename、split、close | BLOCKED | 會改變外部 HERDR session／process。 |
| 26 Files tree | 載入、展開、開 README | PASS | 完整 tree 與檔案內容可讀。 |
| 27 Editor／marksman | CodeMirror、status、LSP readiness | PASS | README 正常；marksman Ready。 |
| 28 Markdown Preview | render、images、table、link、close | PASS | split preview 完整渲染並可關閉。 |
| 29 Editor tabs／split menu | add、reorder、context close、cancel | PASS | Browser add、context close／cancel、右側 split／close、Alt+Arrow reorder／boundary／restore 均正常。 |
| 30 Editor write/save/conflict | dirty、save、reconcile、錯誤檔案 | BLOCKED | 禁止修改產品／repo 檔案。 |
| 31 File mutations | create、rename、move、delete | BLOCKED | 禁止 repository 寫入。 |
| 32 Command Palette | `⌘K`、`>`、上下／Enter、Escape | PASS | keyboard selection／execution 正常。 |
| 33 Workspace search | no-result、case toggle、cancel | PASS | 顯示「沒有符合的結果」。 |
| 34 Symbol pickers | document／workspace symbols | PASS | Document headings／filter／reveal／reset；workspace empty／cross-file reveal／no-result／⌘K close 均正常。 |
| 35 TS／Python／Rust LSP | detect／start／install | BLOCKED | UI 顯示 integrity mismatch／未安裝；未授權系統安裝。 |
| 36 Terminal Drawer | shortcut、空狀態、重複開關 | PASS | `Ctrl+\``、disabled controls 正常。 |
| 37 Local PTY | create、cwd、mode retention、idle close | PASS | prompt cwd=yuzora；關閉回空狀態。 |
| 38 Terminal profile/settings | profile menu、Escape、字級還原 | PASS | System default；原值 12px 已還原。 |
| 39 Terminal I/O | copy/paste、split、process exit/error | PASS | type／paste／copy、2-pane limit／resize、command-not-found、exit 7／0 cleanup 均正常。 |
| 40 Git status／log | branch、untracked、history | PASS | 只顯示 `QA-REPORT.md`；log 正常。 |
| 41 Git commit detail | hash、author、parents、files | PASS | HEAD detail 與 disabled reset control 正常。 |
| 42 Git diff/filter/branch menus | local／commit diff、filters、cancel | PASS | User／Date Radix menus 均可 Escape 關閉且焦點回 trigger；其餘 diff／filter／branch 唯讀流程維持 PASS。未執行 Git mutation。 |
| 43 Git mutations | stage、commit、discard、fetch/pull/push 等 | BLOCKED | 明確禁止 Git 寫入／外部改動。 |
| 44 Database offline/empty | saved profiles、object tree、recent SQL | PASS | profiles 保持 offline，空狀態明確；app 重啟後 SQLite／PostgreSQL 均未自動連線、recent queries 為空。 |
| 45 DB connection forms | SQLite／PostgreSQL／MSSQL、cancel | PASS | 空欄 disable；三種表單與 Escape 正常。 |
| 46 SQLite invalid path | missing-parent path／Test connection | PASS | 顯示「SQLite 檔案不存在」且未建立檔案。 |
| 47 PostgreSQL transport guard | verify-full、trust-cert、plaintext | PASS | 兩個 downgrade 均顯示明確 modal；Escape 撤銷。 |
| 48 DB vault/profile mutation | save、credential remove、forget | BLOCKED | 會修改 OS vault／saved profiles。 |
| 49 Live SQLite query console | SQL、paging、cancel、recovery | PASS | 以既有 saved profile 指向新建 `/private/tmp/yuzora-db-layout-repro-20260711.sqlite` fixture（1,205 rows；未新增 profile／vault）驗證 schema／columns／view、object-tree auto query、selection／cursor／all-statements、兩 result tabs、空字串／Unicode／NULL、500／500／205 paging、前後頁、100-row client sort、recent queries、missing-table error 與後續 recovery；100,000,000-row recursive CTE 在 5,820 ms 顯示 `interrupted`／已取消，controls 恢復。重啟後可重新連線載入 schema，profile context menu 可 Escape 取消，再以「中斷連線」安全回到 offline。全程只執行 SELECT。 |
| 50 Live PostgreSQL／MSSQL | real protocol／TLS／DML | BLOCKED | 無本機 listener／fixture；不連外部 saved DB。 |
| 51 SSH／SFTP offline state | saved hosts、SFTP／SSH empty | PASS | 未連線提示與 saved list 正常。 |
| 52 SSH new-host form | auth、port 0／65535／65536、cancel | PASS | Password／Key file 均為正確 AX radio；selected state與私鑰欄位一致。 |
| 53 Live SSH／TOFU／shell | connect、fingerprint、reconnect | PASS | accept 0.527s、shell marker成功、known-host reconnect 0.528s；reject 0.468s 即時回 Error／Reconnect，無 60s 卡住。 |
| 54 SFTP operations | browse、transfer、mkdir、rename、delete | BLOCKED | TOFU 後 SFTP root／子目錄 browse PASS；upload／mkdir／rename／delete 等寫入或破壞流程未執行，不推定正常。 |
| 55 SFTP remove validation | backend unsafe remote path guard | PASS | guard 在 session／I/O 前執行；focused SSH/SFTP Rust 41 tests PASS。 |
| 56 Preview empty／occupied port | Start、port 3000 collision | PASS | 正確提供 connect existing／alternate／redetect。 |
| 57 Preview localhost iframe | connect existing、404 content | PASS | 連既有 Remotion port；未啟動專案命令。 |
| 58 Preview navigation UI | valid URL、reload、responsive | PASS | path navigation與 mobile frame 正常。 |
| 59 Preview invalid schemes | file／javascript scheme | PASS | 兩者皆拒絕並顯示可存取 `Preview URLs must use http:// or https://` 提示。 |
| 60 Preview start/stop | alternate port、trust、process cleanup | BLOCKED | 強制停止；未啟動 configured `bunx serve`。 |
| 61 External native preview／history | external HTTP(S)、back/forward | PASS | `example.com` child webview 切 Space 後不再出現在另一 workspace；返回無 orphan，明確關 tab 後 webview 亦銷毀。 |
| 62 Settings sections／keyboard | 10 sections、Escape／close | PASS | 所有 section 可開啟。 |
| 63 Theme／language persistence | Light/English、restart、restore | PASS | persisted；已還原 Auto／Follow system。 |
| 64 Accent colors | blue／violet click | PASS | blue／violet 立即套用、互斥 selected、關閉／冷啟後保存；最終已還原 lime。 |
| 65 Editor settings persistence | font 14/minimap on、restart、restore | PASS | persisted；已還原 13/off。 |
| 66 Terminal/Herdr/Git/Safety settings | diagnostics、toggle/restore | PASS | 原值已還原；未撤銷 trust。 |
| 67 Logs visual query | load／500 rows／3 runs | PASS | screenshot 顯示結果與 actions。 |
| 68 Logs filters／copy／export | filters、sanitize/raw、ZIP | PASS | 原流程維持 PASS；invalid since 顯示可存取 ISO error，有效未來時間得 0 rows，清除後恢復。 |
| 69 Logs accessibility | tree／Tab focus | PASS | 500 rows 以 AX `content list` 呈現，每頁 50 筆、10 頁；Previous／Next 正常。 |
| 70 English i18n | LSP、Logs、About | PASS | 固定字串與 LSP integrity diagnostics 均為 English；LSP／Logs full AX 無硬編碼漢字。 |
| 71 Updater | retry、download、install、relaunch | BLOCKED | Beta 明示無 OTA；check 只回 Couldn't check。 |
| 72 LSP install/re-detect | vtsls、pyright、rust-analyzer | BLOCKED | 需系統安裝／catalog remediation 權限。 |
| 73 OS vault/redaction | real secrets、raw logs | BLOCKED | 無隔離憑證；不傳入 sensitive data。 |
| 74 Windows/WSL HERDR | native provider／WSL2／named pipe | BLOCKED | 無 Windows／WSL2 candidate。 |
| 75 Docker/service E2E | DB fixtures／compose | BLOCKED | Docker socket 不存在。 |
| 76 Performance/soak | 長時間、memory、resource caps | PASS | macOS packaged app 進行 30m01s bounded soak（09:16:50–09:46:51），13 輪 ADE／Files／Git／Database／SSH、Space、Settings、Terminal Drawer 往返及 4 次 SQLite connect/disconnect；同一 PID 全程存活且 UI／AX 可操作。RSS 約 104,480–115,072 KB，起點 112,384 KB→終點 111,776 KB；open files 79→77、threads 34→34，未見持續成長。仍有 3,078.11 kB build chunk warning，且本結果不涵蓋多小時／Windows soak。 |
| 77 Native window controls | minimize、fullscreen、resize persistence | PASS | minimize／restore 正常；原生 full screen button 進入後內容仍可操作，`Ctrl+⌘+F` 正常退出；視窗由 `1463×769` 調為 `1388×768`，關閉／重啟後為 `1389×768` 且 HERDR 正常重連，最後已還原 `1463×769`。 |
| 78 測試後 tracked Git 狀態 | `git status --short`／diff | PASS | 僅含使用者明確授權的修復檔與 `QA-REPORT.md`；無 staged changes。 |
| 79 QA write-scope compliance | repository 內實際寫入範圍 | PASS | QA-009 歷史事件保留；本輪修復後的 Cargo／Tauri build 全在 `/private/tmp`，未再寫 repository build outputs。 |
| 80 HERDR no-snapshot Session recovery | selected stopped/error session、其他 running session | PASS | deterministic component test 顯示 unavailable state 仍保留 Sessions tablist並可切換；live default session 無安全方式製造此狀態，未偽稱實機。 |
| 81 Command Palette 大型 inventory 搜尋 | 64+ Spaces、128+ Agents、label／exact ID | PASS | 完整集合先比對 label／ID／Space，再套 64／128 cap；Palette targeted 15/15。實機 `>privacy` 與 `>π` keyboard flow PASS。 |
| 82 HERDR execution origin reconciliation | event set／clear、inventory refresh | PASS | `snapshot` 與 `baseSnapshot` 同步 patch；相鄰 HERDR store 41/41 PASS。 |
| 83 checkout action provenance | 查證 pinned SHA 是否存在 | PASS | upstream API 回傳 `11d5960…` 與 `backport fixes to releases-v4 (#2524)`；原 P1 invalid-ref 前提不成立，contract 鎖定 reviewed commit。 |
| 84 no-checkout `gh release` repository | 無 `.git` runner cwd | PASS | 未設定 repo 時重現 fatal；四個 no-checkout jobs 均設定 `GH_REPO: ${{ github.repository }}`，contract 保護。 |
| 85 REST Release state normalization | existing draft／published release | PASS | 驗證 `draft`／`prerelease` 是 boolean，再正規化為 `isDraft`／`isPrerelease`；state＋contract tests PASS。 |
| 86 resumed-draft release notes | Stable／Beta publish 前 notes | PASS | publish jobs 以 guard base64 notes 覆寫 draft body並逐字讀回比對；contract tests PASS，真實 publish 因 merge gate 未執行。 |
| 87 Logs pagination component contract | Previous／Next primitive | PASS | 改用 shadcn `Button`；latest packaged UI 的 10-page paging、disabled state 與視覺 smoke PASS。 |
| 88 Logs expanded-row identity | 跨頁相同 timestamp/source/event | PASS | key 含 absolute result index；component regression與 packaged page 1→2→1 state isolation PASS。 |
| 89 Logs supported timestamp formats | date-only、datetime-local、RFC3339、非法日期／時間 | PASS | packaged UI 三種有效格式均套用；`2026-02-30`、`24:00` 顯示可存取錯誤；targeted tests PASS。 |
| 90 Accent persisted-key validation | own key、`constructor`／`toString` | PASS | own-property regression 拒絕 inherited keys；violet 關閉／重啟保存，最終 lime／Follow system 已還原並再重啟確認。 |
| 91 HERDR read-only paste lifecycle | observer paste、Take Control、async clipboard race | PASS | observer intent不讀取／不保留 clipboard；initial-open control race仍可交付合法 setup paste；Terminal targeted 31/31 PASS。 |
| 92 Logs expanded-state replacement | new query、run filter switch、colliding records | PASS | successful query與每次 run filter切換清空 expanded state；deterministic collision regressions PASS。 |
| 93 HERDR Attention component contract | shared shadcn Button、disabled state | PASS | Attention action改由既有 `Button variant="ghost"` 組合；`data-slot=button` regression PASS。 |
| 94 Partial draft recovery | same-SHA draft、partial upload、notes／channel drift | PASS | local artifacts在 draft mutation前驗證；雙平台重建、notes讀回比對、versioned／alias `--clobber`、無 skipped bypass；release 45/45 PASS。 |
| 95 Logs Copy current page | page 2、sanitized／raw、100 rows | PASS | 兩種模式只複製 `pageRows` 的 event_50～event_99，notice為 50 rows；Logs adjacent 33/33 PASS。 |
| 96 macOS watcher batch ordering | root prelude、late `b.txt`、5s deadline | PASS | CI flake已 deterministic重現；測試逐批等待目標事件，exact targeted PASS；新 exact-head CI仍待 patch push後確認。 |

## 3. 問題清單

### QA-001 — README 版本徽章落後目前產品版本

- **Bug ID**：QA-001
- **嚴重度**：P3
- **修復狀態**：**FIXED／PASS** — 英／繁中 README badge 與 packaged editor 均顯示 `0.0.9-beta.1`。
- **問題標題**：英文與繁中 README 的版本徽章仍顯示 0.0.8
- **受影響功能**：下載／版本辨識／發布文件
- **前置條件**：檢出 `release/v0.0.9-beta.1` @ `d97fb7a…`
- **完整重現步驟**：
  1. 讀取 `package.json` 或 `src-tauri/tauri.conf.json` 的 version。
  2. 開啟 `README.md` 與 `README.zh-TW.md`。
  3. 比對頂部 Version badge。
- **預期結果**：README 顯示目前 release candidate `0.0.9-beta.1`，或採不會手動落後的動態來源。
- **實際結果**：兩份 README 的 badge 都硬編碼為 `0.0.8`。
- **重現率**：100%（2/2 README）
- **錯誤訊息、日誌或畫面證據**：`README.md:15`、`README.zh-TW.md:15`；產品版本證據為 `package.json:4` 與 `src-tauri/tauri.conf.json:4`。
- **對上線的影響**：不阻擋執行，但會讓候選版／下載頁的版本資訊混淆，降低發布可信度。
- **已知暫時解法**：以 GitHub Release asset/tag 與 app「關於與更新」頁顯示版本為準。

### QA-002 — README 所示 installer build 命令在 Beta 分支必然以 code 1 結束

- **Bug ID**：QA-002
- **嚴重度**：P2
- **修復狀態**：**FIXED／PASS** — `bun run tauri:build` 改走明示 no-updater／no-sign 的本機 installer path；repository 外完整 build code 0。
- **問題標題**：`bun run tauri:build` 產生 bundle 後因缺 updater private key 失敗
- **受影響功能**：從原始碼建置、Beta candidate 本機驗證、貢獻者開發流程
- **前置條件**：目前 `0.0.9-beta.1` 設定含 updater public key、`bundle.createUpdaterArtifacts=true`，環境未注入 production private key。
- **完整重現步驟**：
  1. 由目前 HEAD 建立乾淨副本並安裝／提供既有 dependencies。
  2. 依 `README.md:138-143` 執行 `bun run tauri:build`。
  3. 等待 frontend、Rust release 與 bundle 完成。
- **預期結果**：README 標示的「Build installers from source」命令應成功退出；若 Beta 需不同命令，README 應明確提供。
- **實際結果**：成功產生 `Yuzora.app`、`Yuzora_0.0.9-beta.1_aarch64.dmg`、`.app.tar.gz` 後，顯示 `A public key has been found, but no private key` 並 code 1。
- **重現率**：100%（1/1 隔離乾淨 build）
- **錯誤訊息、日誌或畫面證據**：`/private/tmp/yuzora-qa-build.mtp9lh/.qa-artifacts/tauri-build.log`；最後錯誤為 `A public key has been found, but no private key. Make sure to set TAURI_SIGNING_PRIVATE_KEY environment variable.`
- **對上線的影響**：CI 的 Beta lane 有獨立 no-updater/no-sign command，故不直接證明發布 workflow 失敗；但公開來源建置指引目前不可直接成功，也容易誤導使用者索取 production signing secret。
- **已知暫時解法**：依 `.github/workflows/release.yml:285-293` 產生 `scripts/release-msi-build-config.ts <version> --no-updater` config，並以 `bun tauri build --ci --no-sign --config <config>` 建置 Beta。

### QA-003 — macOS 候選 bundle 無法通過 code-sign 與 Gatekeeper 驗證

- **Bug ID**：QA-003
- **嚴重度**：P1
- **修復狀態**：**WORKFLOW FIXED／RELEASE VERIFICATION BLOCKED** — macOS Stable／Beta 皆強制 Developer ID signing、notarization、strict `codesign`、`spctl` 與 app／DMG stapler gate；本機無 Apple credentials，尚無實際 signed candidate 可驗。
- **問題標題**：Beta `Yuzora.app` 只有 ad-hoc linker signature，系統驗證失敗
- **受影響功能**：macOS 下載、安裝、首次啟動與供應鏈信任
- **前置條件**：依 `.github/workflows/release.yml:285-293` 的 Beta no-sign/no-updater lane 成功建置 macOS candidate。
- **完整重現步驟**：
  1. 執行 Beta release-candidate build。
  2. 對產生的 `Yuzora.app` 執行 `codesign -dv --verbose=4`。
  3. 執行 `codesign --verify --deep --strict --verbose=4 <app>`。
  4. 執行 `spctl -a -vv -t exec <app>`。
- **預期結果**：上線候選應具有效 Developer ID 簽章並通過 strict code-sign／Gatekeeper assessment；正式散佈還應完成 notarization。
- **實際結果**：signature 顯示 `adhoc,linker-signed`、`TeamIdentifier=not set`、`Sealed Resources=none`；兩個驗證命令皆 code 1，錯誤為 `code has no resources but signature indicates they must be present`。
- **重現率**：100%（1/1 最新 Beta candidate）
- **錯誤訊息、日誌或畫面證據**：packaged app `/private/tmp/yuzora-qa-build.mtp9lh/cargo-target/release/bundle/macos/Yuzora.app`；`docs/operations.md:110-114` 亦明確記錄 macOS code signing／notarization 尚未啟用。
- **對上線的影響**：**上線阻擋**。一般使用者從網路取得 app 後會遭 Gatekeeper 信任／啟動阻礙，且無可驗證的發行者 identity。
- **已知暫時解法**：開發者可在未帶 quarantine 的本機 build 直接執行；要求使用者繞過 Gatekeeper 不視為可接受的上線方案，本次也不執行繞過。

### QA-004 — 外觀設定的主題色控制可點擊但完全沒有作用

- **Bug ID**：QA-004
- **嚴重度**：P2
- **修復狀態**：**FIXED／PASS** — accent tokens 立即套用、保存並跨冷啟恢復；blue／violet 實機通過，最終還原 lime。
- **問題標題**：主題色 radio control 未停用、無說明，但選擇 blue／violet 後仍固定為 lime
- **受影響功能**：設定 → 外觀 → 主題色、外觀自訂與基本可用性
- **前置條件**：啟動最新 packaged app，開啟設定 → 外觀；目前 accent 為 lime。
- **完整重現步驟**：
  1. 點擊 `blue` 主題色。
  2. 觀察 radio selected state 與 app accent。
  3. 再點擊 `violet` 重測。
- **預期結果**：選取的主題色立即套用並成為 selected；若功能尚未提供，控制應停用並說明。
- **實際結果**：兩次點擊後皆無任何變化或提示，`lime` 維持 selected，`blue`／`violet` 維持 unselected；控制在 Accessibility tree 中仍是可操作 radio button。
- **重現率**：100%（2/2 不同選項）
- **錯誤訊息、日誌或畫面證據**：Computer Use 前後 Accessibility tree：`lime Value: 1`、`blue Value: 0`、`violet Value: 0`；source inventory 亦指出 `src/app/workbench/SettingsDialog.tsx:141-155` 是未持久化 placeholder。
- **對上線的影響**：不阻擋核心工作，但形成明確的失效控制與錯誤 affordance，使用者無法判斷功能尚未完成。
- **已知暫時解法**：保持預設 lime；UI 內沒有可用替代方式。

### QA-005 — 切換 English 後多個設定區段仍混用繁體中文

- **Bug ID**：QA-005
- **嚴重度**：P2
- **修復狀態**：**FIXED／PASS** — LSP／Logs 固定字串與 runtime integrity diagnostic 均依 locale 呈現；English full AX 復驗通過。
- **問題標題**：English locale 未完整覆蓋 LSP 與 Logs 設定內容
- **受影響功能**：設定、國際化、LSP、Logs
- **前置條件**：設定 → Appearance → Language 選擇 English。
- **完整重現步驟**：
  1. 將 Language 切成 English，確認 settings chrome 已變為 English。
  2. 開啟 LSP section。
  3. 開啟 Logs section。
- **預期結果**：所有可見產品字串使用 English（技術名詞與使用者資料除外）。
- **實際結果**：LSP 仍顯示「儲存時自動格式化」「伺服器設定範圍」「此工作區」「全域」「未安裝」「就緒」「最近啟動記錄」等；Logs 仍顯示「篩選」「文字搜尋」「動作」「結果」「載入中」等，與 English chrome 混用。
- **重現率**：100%（2/2 檢查區段）
- **錯誤訊息、日誌或畫面證據**：Computer Use Accessibility tree 在 English locale 下同時出現 `Settings`／`Language servers` 與上述繁中 label。
- **對上線的影響**：英文使用者會看到不一致且部分不可理解的設定流程；屬面向支援平台的完成度落差。
- **已知暫時解法**：切回 Follow system／繁體中文；沒有完整 English 的 UI 內解法。

### QA-006 — Logs 結果載入後內容區從 Accessibility tree 消失

- **Bug ID**：QA-006
- **嚴重度**：P2
- **修復狀態**：**FIXED／PASS** — Logs results 使用可存取 content list，500 筆資料每頁 50 筆、10 頁，Previous／Next 實機通過。
- **問題標題**：Logs 的 filters、actions 與 500-row 結果在載入完成後無可辨識的輔助技術節點
- **受影響功能**：Settings → Logs、鍵盤操作、螢幕閱讀器可用性
- **前置條件**：開啟 Logs，環境有 500 筆 log／3 runs。
- **完整重現步驟**：
  1. 開啟 Settings → Logs。
  2. 載入期間取得 Accessibility tree，可看到 filters、Copy、Export bundle、sanitize 與「載入中」。
  3. 等候結果完成並重新讀取 Accessibility tree。
  4. 由 sidebar 連續按 Tab 進入內容區。
- **預期結果**：載入完成後 filters、actions、結果列保留可存取名稱／角色／狀態；Tab 焦點可被辨識。
- **實際結果**：視覺畫面仍完整顯示 `500 / 500 rows · 3 runs` 與所有控制，但 Accessibility tree 只剩 settings sidebar 和空 content container；Tab 進入內容區後連續多個焦點沒有任何可辨識節點。
- **重現率**：100%（1/1 載入，10 個後續 Tab position）
- **錯誤訊息、日誌或畫面證據**：Computer Use screenshot 顯示 Logs 結果；同時 full Accessibility tree 在 content container 後直接進入 dialog splitters，且 focused element 消失。
- **對上線的影響**：依賴鍵盤或輔助技術的使用者無法可靠操作 log 查詢、匯出與結果閱讀；基本可用性驗收失敗。
- **已知暫時解法**：滑鼠仍可操作視覺控制；這不構成可及性修復。

### QA-007 — SFTP 遠端刪除缺少 backend path safety validation

- **Bug ID**：QA-007
- **嚴重度**：P2（release safety blocker；若 renderer 視為不可信，應升級 P1 security review）
- **修復狀態**：**FIXED／PASS** — `sftp_remove` 在取得 session／執行遠端 I/O 前先跑 unsafe-leaf guard；SSH/SFTP focused Rust 41 tests PASS。
- **問題標題**：`sftp_remove` 未套用 mkdir／rename／download 已使用的 unsafe remote leaf guard
- **受影響功能**：SSH／SFTP 遠端刪除、renderer-to-backend IPC safety boundary
- **前置條件**：任一已建立的 SFTP connection；可直接呼叫 renderer IPC。
- **完整重現步驟**：
  1. 對照 `sftp_mkdir`、`sftp_rename`、download 的 path validation。
  2. 追蹤 renderer `sftpRemove(connectionId, path, recursive)` 到 Rust `sftp_remove`。
  3. 傳入 guard 明確會拒絕的 path，例如 `/home/u/../.ssh/config`。
- **預期結果**：backend 在任何 remote delete 前呼叫 `reject_unsafe_remote_leaf(path)` 並拒絕 `..`、反斜線、empty segment、trailing slash 等。
- **實際結果**：`sftp_remove` 將 `path.to_string()` 直接傳給 `remove_dir`／`remove_file`，沒有 guard；renderer/store 也原樣傳遞 path。
- **重現率**：100% source-level（1/1 call path）；未在真實 server 執行刪除。
- **錯誤訊息、日誌或畫面證據**：`src-tauri/src/ssh_service.rs:1397-1425,1535-1539`；guard 定義 `:959-975`；guard tests `:3170-3183`；`src/lib/ipc.ts:612-614`；`src/state/sftpStore.ts:189-195`；`src/app/panels/SshPanel.tsx:771-781`。
- **對上線的影響**：破壞性遠端操作缺少既有 backend trust-boundary validation；crafted／compromised renderer 可要求刪除超出預期 leaf 的 remote path。公開上線前需安全審查。
- **已知暫時解法**：不要使用 SFTP delete；UI confirmation 不能替代 backend validation。

### QA-008 — Preview 拒絕危險 URL scheme 時沒有錯誤提示

- **Bug ID**：QA-008
- **嚴重度**：P2
- **修復狀態**：**FIXED／PASS** — `file:`／`javascript:` 均拒絕並顯示可存取 HTTP(S)-only 提示，輸入保留供使用者修正。
- **問題標題**：file／javascript URL 被安全阻擋，但 URL 欄保留無效值且內容停在舊頁面，沒有 feedback
- **受影響功能**：Preview URL bar、錯誤處理、使用者狀態判讀
- **前置條件**：Preview 已連接 `http://localhost:3000`。
- **完整重現步驟**：
  1. 在 Preview URL bar 輸入 `file:///etc/passwd` 並按 Enter。
  2. 觀察 URL bar、preview document 與錯誤提示。
  3. 以 `javascript:alert(1)` 重測。
  4. 輸入有效 `http://localhost:3000/qa-preview-valid` 作對照。
- **預期結果**：危險 scheme 不導航，並顯示可存取的明確錯誤；URL bar 應回復實際頁面或標示 invalid。
- **實際結果**：兩個危險 scheme 都未執行（安全 allowlist 有效），但 URL bar 保留輸入值、preview 仍顯示舊 localhost document，沒有 toast、inline error 或 AX announcement；有效 HTTP URL 可正常導航。
- **重現率**：100%（2/2 invalid scheme；1/1 valid control）
- **錯誤訊息、日誌或畫面證據**：Computer Use full Accessibility tree 同時顯示 URL bar `file:///etc/passwd`／`javascript:alert(1)`，但 iframe URL 仍為先前 localhost path，無錯誤節點。
- **對上線的影響**：安全邊界本身生效，但錯誤處理不符合驗收標準，使用者可能誤認 preview 已導航或內容對應目前 URL。
- **已知暫時解法**：重新輸入有效 `http://`／`https://` URL。

### QA-009 — QA 委派檢查更新了 repository 內 ignored build artifacts

- **Bug ID**：QA-009
- **嚴重度**：P1（QA process blocker；非產品碼 defect）
- **修復狀態**：**HISTORICAL INCIDENT／REMEDIATION PASS** — 不刪除或還原既有 ignored artifacts；其後所有 Cargo／Tauri build 均使用 repository 外 source／target，事件未再發生。
- **問題標題**：自動檢查未全數在隔離副本執行，違反本輪 repository 唯一可寫 `QA-REPORT.md` 的限制
- **受影響功能**：QA 證據鏈、工作區完整性、剩餘驗收覆蓋
- **前置條件**：原 repository 起始乾淨；`dist/` 與 `src-tauri/target/` 已存在且被 Git ignore。
- **完整重現步驟**：
  1. 委派 agent 在 repository root 執行 `bun run build`。
  2. 在 `src-tauri` 執行 `cargo check --locked --all-targets` 與 `cargo test --locked`。
  3. 檢查執行位置與輸出目錄。
- **預期結果**：所有會產生 output 的 checks 使用 `/private/tmp` 隔離 source／target；repository 內只有 `QA-REPORT.md` 可被寫入。
- **實際結果**：`bun run build` 更新 repository `dist/`；Cargo commands 更新 repository `src-tauri/target/`。兩者被 ignore，因此一般 `git status --short` 不顯示；委派 agent 明確回報實際 workdir／output。`dist` directory mtime 為 `2026-08-26 01:09:25 +0800`。
- **重現率**：100%（本輪該委派的 1/1 frontend build 與 1/1 Cargo check group）
- **錯誤訊息、日誌或畫面證據**：`git check-ignore -v` 顯示 `.gitignore:17:dist/` 與 `src-tauri/.gitignore:3:/target/`；事件當下 tracked status 只有 `?? QA-REPORT.md`。使用者其後明確授權恢復測試與修復，目前精確工作區狀態列於第 5 節。
- **對上線的影響**：事件當下依使用者明示流程立即停止，當時有 10 項 NOT TESTED、17 項 BLOCKED；後續經使用者明確要求恢復測試與修復後，所有可測功能已補測，最終為 0 項 NOT TESTED、17 項 BLOCKED。此事件不再是產品上線阻擋，但仍保留為 QA 證據鏈與隔離執行規範的歷史缺口。
- **已知暫時解法**：無。本輪禁止刪除或還原，故 artifacts 保留原狀；下一輪應以全新隔離副本與外部 target dir 重跑。

### QA-010 — Agent Inspector 已實作但沒有任何 production UI 入口

- **Bug ID**：QA-010
- **嚴重度**：P2
- **修復狀態**：**FIXED／PASS** — Agents 列表新增可發現 Inspector action；metadata、text／ANSI output、Refresh、Escape 與 focus return 實機通過。
- **問題標題**：Agent Inspector 元件與測試存在，但使用者無法從 Agent 清單或任何可發現控制開啟
- **受影響功能**：ADE → Agents、Agent 狀態／來源／output 唯讀檢視、鍵盤可用性
- **前置條件**：最新 packaged app 已連接 running Herdr session，Agent 清單至少有一個可聚焦 Agent（本次為 `π - privacy-filter`）。
- **完整重現步驟**：
  1. 開啟 ADE，確認 Agent row 可見。
  2. 單擊 Agent row，觀察右側內容與 dialogs。
  3. 右鍵 Agent row，檢查所有 context-menu actions，再以 Escape 取消。
  4. 檢查畫面與 Accessibility tree 中可發現的 Agent controls。
  5. 唯讀追蹤 `HerdrAgentInspector` 的 production import／render caller。
- **預期結果**：應有可發現且可由鍵盤操作的入口，開啟唯讀 Agent Inspector，顯示 status、Space／tab／pane、cwd、revision、labels 與可重新整理的 output。
- **實際結果**：單擊只聚焦 Agent 所屬 Space／terminal pane；右鍵選單只有重新命名、清除名稱、分割、縮放、交換與關閉 pane。畫面與 Accessibility tree 沒有 Inspector 控制。`HerdrAgentInspector.tsx` 只被自身單元測試 import，production 無 import／render caller。
- **重現率**：100%（1/1 live Agent；click／right-click／source caller 三條路徑一致）
- **錯誤訊息、日誌或畫面證據**：Computer Use full Accessibility tree 的 Agent 區只有 Agent row、啟動 Agent、新增 terminal；右鍵 menu 只有 pane actions。`src/app/workbench/HerdrAgentInspector.tsx:35-248` 定義完整 dialog；`src/app/workbench/HerdrAgentInspector.test.tsx:23,155-239` 測試元件；repository 全域 literal search 除 definition／tests／dialog-size key／locale 外沒有 production import；code graph inbound callers 為 0。
- **對上線的影響**：產品中已實作的唯讀診斷能力完全不可達，使用者無法檢視 Agent 詳細狀態與 output；屬明確功能缺失，但不直接破壞既有 terminal 操作。
- **已知暫時解法**：可直接查看對應 terminal pane 的可見內容；無法替代 Inspector 的 metadata、read source／format／line count 與 refresh 功能。

### QA-011 — Git 使用者／日期篩選選單無法以鍵盤或可存取 Cancel 關閉

- **Bug ID**：QA-011
- **嚴重度**：P3
- **修復狀態**：**FIXED／PASS** — User／Date filters 改用 Radix DropdownMenu；Escape 關閉且焦點回到各自 trigger。
- **問題標題**：Git 紀錄的使用者與日期 filter menu 忽略 Escape 與 Accessibility Cancel
- **受影響功能**：Git → 紀錄 → 使用者篩選／日期篩選、鍵盤操作與取消流程
- **前置條件**：最新 packaged app 已開啟任一 workspace 的 Git → 紀錄。
- **完整重現步驟**：
  1. 開啟「日期篩選」menu。
  2. 按 Escape；再按一次 Escape 重測。
  3. 對 menu 執行 Accessibility tree 明示的 `Cancel` secondary action。
  4. 觀察 menu 是否關閉；改選「全部」作控制組。
  5. 以「使用者篩選」重複開啟與 Escape。
- **預期結果**：Escape 或 Accessibility Cancel 應關閉 menu，不改變目前篩選；焦點回到觸發按鈕。
- **實際結果**：日期 menu 在兩次 Escape 與一次 AX Cancel 後都持續顯示；使用者 menu 也在 Escape 後持續顯示。只有選擇一個項目（本次選「全部」）才關閉。
- **重現率**：100%（日期 Escape 2/2、日期 AX Cancel 1/1、使用者 Escape 1/1）
- **錯誤訊息、日誌或畫面證據**：Computer Use full Accessibility tree 在每次取消後仍只有 `menu Secondary Actions: Cancel` 與相同 options；選擇 `全部` 後才回到 Git 紀錄畫面。branch picker overlay 的 Escape 可正常關閉，證明 app keyboard dispatch 並非全面失效。
- **對上線的影響**：不阻擋主要 Git 唯讀工作，但破壞標準取消操作，鍵盤使用者必須重新選擇一個值才能離開 menu。
- **已知暫時解法**：點選目前已選的篩選值（例如「全部」）以關閉 menu。

### QA-012 — External Preview 切換 Space 後跨 workspace 殘留並成為 orphan webview

- **Bug ID**：QA-012
- **嚴重度**：P1
- **修復狀態**：**FIXED／PASS** — native close queue 與 workspace／tab lifecycle 已串接；Space switch 與 explicit tab close 後均無 child webview 殘留。
- **問題標題**：External native child webview 未隨 owning Space／mode 隱藏或銷毀，覆蓋其他 workspace 且失去關閉入口
- **受影響功能**：Preview external HTTP(S)、Space switching、ADE／Files／terminal、workspace isolation、native child-webview lifecycle
- **前置條件**：Yuzora Space 的 Preview 已連接 localhost，並成功導航到 external HTTPS（本次為 `https://example.com/`）。
- **完整重現步驟**：
  1. 在 Yuzora Space → Files 開啟 Preview，連接既有 localhost server。
  2. 在 URL bar 導航到 `https://example.com/`，確認 native child webview 正常顯示。
  3. 點擊左側 `privacy-filter` Space。
  4. 觀察 privacy-filter 的 ADE／terminal 與 Accessibility tree。
  5. 返回 Yuzora，再切換 ADE／Files，觀察 tabs 與 child webview；重複 Space 往返一次。
- **預期結果**：離開 owning Space／Preview tab 時 child webview 應隱藏；返回時只在 Preview tab active 時恢復。若 tab 被關閉，native webview 必須銷毀。
- **實際結果**：Example Domain child webview 在 privacy-filter Space 持續覆蓋幾乎整個 terminal／editor；AX tree 同時存在 `privacy-filter`、兩個 `Terminal input` 與 `HTML content Description: Example Domain`。返回 Yuzora 後 active mode 為 ADE／Files，tab bar 已沒有 Preview tab，但 external webview 仍顯示，成為 UI 無法關閉的 orphan。
- **重現率**：100%（2/2 Space 往返）
- **錯誤訊息、日誌或畫面證據**：Computer Use screenshots 兩次顯示 privacy-filter chrome／terminal 背景被 Example Domain 白色 webview 覆蓋；full Accessibility tree 同時列出另一 Space 的 Agent／Terminal 節點與 `Example Domain` child content。返回 Yuzora 的 Files tab 後只有 `Yuzora`／`README.md` tabs，仍另列 `Example Domain` scroll area。
- **對上線的影響**：**上線阻擋**。任意 external preview 可跨 Space 遮蔽並攔截另一 workspace 的操作，破壞 workspace 隔離；tab 消失後一般使用者沒有關閉路徑，只能重啟 app。
- **已知暫時解法**：在切換 Space 前避免 external Preview；若已 orphan，關閉並重新啟動 Yuzora。

### QA-013 — Logs since／until 接受無效日期但靜默忽略

- **Bug ID**：QA-013
- **嚴重度**：P3
- **修復狀態**：**FIXED／PASS** — invalid since／until 顯示 locale-aware inline／AX error 並標記 invalid；有效 ISO filter control 正常。
- **問題標題**：Logs 時間篩選輸入 `not-a-date` 後無 validation feedback，查詢退化為未設定 bound
- **受影響功能**：Settings → Logs → since／until filters、錯誤輸入處理與查詢可信度
- **前置條件**：Logs 已載入可見結果，所有其他 filters 清空。
- **完整重現步驟**：
  1. 在 since 輸入有效未來時間 `2030-01-01T00:00:00+08:00`，確認結果變為 0。
  2. 將 since 改成 `not-a-date`。
  3. 按 Tab 離開欄位，觀察 validation、結果數與提示。
- **預期結果**：拒絕非 ISO timestamp，顯示 inline／可存取錯誤且不執行含糊查詢；或自動清空並明確通知。
- **實際結果**：欄位保留 `not-a-date`，沒有錯誤樣式、訊息或 AX announcement；結果立即恢復 500 rows，等同悄悄忽略 since bound。
- **重現率**：100%（1/1 invalid input；有效未來時間 control 1/1 正確為 0 rows）
- **錯誤訊息、日誌或畫面證據**：Computer Use screenshot 同時顯示 since=`not-a-date` 與 `500 / 500 rows`，Tab blur 後仍相同；Accessibility tree 沒有 error 節點。
- **對上線的影響**：不阻擋 logs 基本瀏覽，但使用者可能誤信查詢已套用時間範圍，導致診斷 bundle／Copy 包含超出預期的資料。
- **已知暫時解法**：手動輸入完整 ISO 8601 timestamp，並以結果時間戳交叉檢查。

### QA-014 — SSH 驗證方式按鈕的 AX 名稱與實際動作互換

- **Bug ID**：QA-014
- **嚴重度**：P2
- **修復狀態**：**FIXED／PASS** — Password／Key file 使用 RadioGroup contract，AX name、selected state與表單內容一致。
- **問題標題**：新增 SSH 主機時，Accessibility tree 的「金鑰檔案」與「密碼」按鈕會執行相反驗證方式
- **受影響功能**：SSH host setup、screen reader／Voice Control、鍵盤與基本可用性
- **前置條件**：開啟 SSH → 新增主機；使用 macOS Accessibility action 操作驗證方式按鈕。
- **完整重現步驟**：
  1. 開啟「新增 SSH 主機」對話框。
  2. 讀取 Accessibility tree，確認兩個 control 分別標示「驗證方式 金鑰檔案」與「驗證方式 密碼」。
  3. 啟用 AX 標示為「驗證方式 密碼」的按鈕。
  4. 觀察「私鑰路徑」欄位出現，視覺上「金鑰檔案」成為 active。
  5. 再啟用 AX 標示為「驗證方式 金鑰檔案」的按鈕，觀察私鑰欄位消失、視覺上切回密碼。
- **預期結果**：每個可存取名稱應與視覺文字及實際 `authKind` 動作一致。
- **實際結果**：AX 名稱和觸發動作互換；「密碼」觸發 key auth，「金鑰檔案」觸發 password auth。視覺標籤／active style 本身正確。
- **重現率**：100%（2/2 方向）
- **錯誤訊息、日誌或畫面證據**：Codex Computer Use full AX tree 在私鑰欄位出現前後均列出兩個相反命名的 button；以 fresh state 的 element action 重現。Screenshot 顯示 AX「密碼」action 後右側「金鑰檔案」呈 active 且私鑰路徑顯示。
- **對上線的影響**：視覺滑鼠使用者仍可操作，但 screen reader／Voice Control 使用者會被引導至相反驗證方式，可能無法建立 SSH 連線；屬主要 accessibility failure。
- **已知暫時解法**：視覺確認 active style，或在輔助工具中選擇文字相反的 control；此 workaround 不可發現且不可靠。

### QA-015 — 拒絕未知 SSH host key 後 UI 卡在連線中直到 60 秒 timeout

- **Bug ID**：QA-015
- **嚴重度**：P2
- **修復狀態**：**FIXED／PASS** — reject response 會即時喚醒 pending challenge；localhost 實機 0.468s 進入 Error／Reconnect。
- **問題標題**：TOFU prompt 點「拒絕」不會立即結束連線，約一分鐘後才回復為錯誤
- **受影響功能**：SSH host-key verification、取消／返回流程、reconnect
- **前置條件**：以尚未 pin 的 host key 連線；本次使用只監聽 `127.0.0.1:48222` 的隔離 sshd、throwaway host/client Ed25519 keys、獨立 app HOME／bundle identifier。
- **完整重現步驟**：
  1. 從 SSH host list 連線到未知 host key。
  2. 核對 endpoint、algorithm 與 SHA-256 fingerprint。
  3. 點「拒絕」。
  4. 立即觀察 host badge、SSH panel 與可用 controls；持續觀察至 backend challenge deadline。
- **預期結果**：拒絕後應立即中止 handshake，host／panel 回到明確錯誤或 idle 狀態，並允許立刻重試。
- **實際結果**：dialog 關閉後 host badge 與 panel 持續顯示「連線中」，沒有取消或 retry control；直到約 60 秒後才顯示「使用者拒絕或逾時未確認主機金鑰，連線已中止」與「重新連線」。
- **重現率**：100%（1/1；完整觀察至 timeout）
- **錯誤訊息、日誌或畫面證據**：Computer Use AX state 在拒絕後、+1.5s、+10s 與約 +40s 仍為「連線中」，最後才出現上述錯誤。隔離 sshd verbose log 只記錄 TCP connection，沒有 public-key authentication；isolated `~/.yuzora/known_hosts.json` 不存在，證實 fail-closed。
- **對上線的影響**：安全邊界正確，但使用者在正常拒絕／誤點情境會被鎖住約一分鐘且無法立即重試，容易誤認 app hang；自動化與大量主機管理也會被長 timeout 阻塞。
- **已知暫時解法**：等待約 60 秒直到「重新連線」出現；或重啟 app。

### QA-016 — SSH TOFU event 欄位命名錯誤，任何新 host key 都無法接受

- **Bug ID**：QA-016
- **嚴重度**：P1
- **修復狀態**：**FIXED／PASS** — TOFU event 使用 camelCase `challengeId`；accept 0.527s、shell、known-host reconnect 0.528s、SFTP root／subdir browse 均通過。
- **問題標題**：`ssh://host-key-prompt` 送出 `challenge_id`，frontend 讀取 `challengeId`，首次 SSH／SFTP 連線無法完成
- **受影響功能**：SSH／SFTP first connection、TOFU accept／reject、reconnect、host-key pin persistence
- **前置條件**：連線至 isolated known-hosts store 中尚未 pin 的主機；本次為 `127.0.0.1:48222`、Ed25519 fixture，顯示的 SHA-256 fingerprint 已用 `ssh-keygen -lf -E sha256` 獨立核對一致。
- **完整重現步驟**：
  1. 在獨立 bundle identifier／HOME 的 Yuzora 建立 key-auth localhost host。
  2. 點 host 發起首次連線，等候 TOFU dialog。
  3. 核對 endpoint、algorithm、fingerprint。
  4. 點「信任主機金鑰」。
- **預期結果**：frontend 應把 prompt 的 challenge identifier 傳回 `ssh_host_key_respond`，backend durable-pin 後才進入 public-key authentication。
- **實際結果**：dialog 不關閉並 inline 顯示 `invalid args challengeId for command ssh_host_key_respond: command ssh_host_key_respond missing required key challengeId`；沒有 known-host pin、沒有 public-key authentication，無法建立 session。拒絕路徑吞掉同一 IPC error，因此退化為 QA-015 的 60 秒 timeout。
- **重現率**：100%（accept 1/1；reject side effect 1/1）
- **錯誤訊息、日誌或畫面證據**：Computer Use AX tree 直接顯示完整 Tauri invoke error；isolated sshd log 在 UI accept 後仍沒有 auth 記錄；isolated `~/.yuzora/known_hosts.json` 不存在。唯讀 source：Rust `SshHostKeyPrompt` 是 enum struct variant，只標註 `#[serde(rename_all = "camelCase", tag = "kind")]`，variant field `challenge_id` 未套用 camelCase；frontend `SshHostKeyHost` 讀 `current.challengeId`，`ipc.ts` 再傳 `{ challengeId, ... }`，undefined key 被 invoke 移除。
- **對上線的影響**：**上線阻擋**。所有尚未出現在 Yuzora 自有 known-hosts store 的 SSH／SFTP 主機都無法首次連線；主要功能在正常新使用者環境不可用。手工 pre-pin 才能繞過，但不屬產品支援流程。
- **已知暫時解法**：無 UI 暫時解法。不得要求一般使用者手工修改 `~/.yuzora/known_hosts.json`；本次 QA 亦未繞過。

### QA-017 — HERDR selected Session 無 snapshot 時無法切換其他 Session

- **Bug ID**：QA-017
- **嚴重度**：P1
- **修復狀態**：**FIXED／AUTOMATED PASS** — unavailable shell 保留 Sessions tablist；targeted regression PASS。
- **問題標題**：selected stopped／error Session 無 snapshot 時提前 return，Session switcher 消失
- **受影響功能**：ADE → named Sessions、HERDR unavailable recovery、Open Local Folder escape
- **前置條件**：inventory 至少兩個 named Sessions；selected Session stopped 或連線 error，且尚無 snapshot；另一 Session running。
- **完整重現步驟**：1. 選取 stopped／error Session。2. 使其在 snapshot 前進入 unavailable state。3. 觀察 ADE sidebar。4. 嘗試切回 running Session。
- **預期結果**：unavailable message 與 Open Local Folder 可見，同時保留 Sessions tablist供切換。
- **實際結果**：原實作在 render tablist 前 return，只剩 Open Local Folder，running Session 不可達。
- **重現率**：100% deterministic component test。
- **錯誤訊息、日誌或畫面證據**：PR #83 discussion `r3852466712`；`herdrNavContent.test.tsx` 覆蓋 no-snapshot stopped Session switching；HERDR targeted suite 37/37。
- **對上線的影響**：主要 recovery path 被鎖死，使用者可能誤認全部 HERDR runtime 不可用。
- **已知暫時解法**：修復前只能重啟 app 或改用 Open Local Folder；無可靠 UI Session recovery。

### QA-018 — Command Palette 先 cap 再搜尋使後段 Space／Agent 永久不可達

- **Bug ID**：QA-018
- **嚴重度**：P2
- **修復狀態**：**FIXED／PASS** — 完整 inventory 先依 label／ID／Space 比對，再套 64／128 顯示上限。
- **問題標題**：大型 HERDR snapshot 中第 65 個 Space／第 129 個 Agent 無法被搜尋
- **受影響功能**：Command Palette、HERDR Space／Agent jump
- **前置條件**：snapshot 超過 64 Spaces 或 128 Agents，目標位於 cap 後方。
- **完整重現步驟**：1. 建立 65+ Spaces 或 129+ Agents fixture。2. 開啟 `⌘K`。3. 輸入目標 exact label 或 ID。4. 檢查結果。
- **預期結果**：完整集合先搜尋，符合項目才受顯示 cap 限制。
- **實際結果**：原實作先 `slice`，後段項目在 filtering 前已被丟棄，exact query 也無結果。
- **重現率**：100%（合成 65th／129th fixtures）。
- **錯誤訊息、日誌或畫面證據**：PR discussion `r3852466723`；Palette 15/15、combined targeted 37/37；packaged UI `>privacy`、`>π` 與 Enter／Escape PASS。
- **對上線的影響**：大型真實 Session 的搜尋／跳轉功能不完整，但不影響前段 inventory。
- **已知暫時解法**：修復前只能從其他 UI 導航或降低 inventory 數量。

### QA-019 — checkout pinned SHA 被誤報為不存在

- **Bug ID**：QA-019
- **嚴重度**：P1（review 報告；最終判定 **NOT A DEFECT**）
- **修復狀態**：**DISPROVED／CONTRACT HARDENED** — 保留有效 SHA，修正錯誤的 `v4.2.2` 註解，contract 鎖定已審查 commit。
- **問題標題**：review 認為 `actions/checkout@11d5960…` 無法解析
- **受影響功能**：Release guard／build／metadata checkout provenance
- **前置條件**：檢查 `.github/workflows/release.yml` 的三個 checkout ref。
- **完整重現步驟**：1. 以 GitHub upstream API 查 `repos/actions/checkout/git/commits/11d5960…`。2. 比對 workflow ref與 commit message。3. 以 mutation 改成全零 SHA執行 contract。
- **預期結果**：有效 reviewed commit 可解析；非法或未審查 SHA fail closed。
- **實際結果**：API 回傳有效 commit `11d5960a326750d5838078e36cf38b85af677262`，訊息為 `backport fixes to releases-v4 (#2524)`；問題只在 provenance 註解錯標 `v4.2.2`。
- **重現率**：誤報前提 0%；API 查證與 mutation contract 100%。
- **錯誤訊息、日誌或畫面證據**：PR discussion `r3852466727`；GitHub upstream API live response；checkout mutation test正確 fail closed。
- **對上線的影響**：沒有 invalid-ref blocker；錯誤註解可能誤導供應鏈 review，已修正並鎖定。
- **已知暫時解法**：不需要 workaround；以 full SHA 與 upstream API為準。

### QA-020 — executionOrigin 未寫回 baseSnapshot，inventory refresh 會回退

- **Bug ID**：QA-020
- **嚴重度**：P2
- **修復狀態**：**FIXED／AUTOMATED PASS** — event set／clear 同步 patch `snapshot` 與 `baseSnapshot`。
- **問題標題**：Agent execution origin badge 在 worktree inventory refresh 後恢復舊值
- **受影響功能**：HERDR Agent／terminal origin、WSL badge、event reconciliation
- **前置條件**：已有 snapshot／baseSnapshot；收到含或清除 `executionOrigin` 的 `agent_status_changed`，之後執行 inventory refresh。
- **完整重現步驟**：1. 注入 origin event。2. 確認 projected snapshot 更新。3. 觸發 worktree inventory refresh。4. 觀察 origin。
- **預期結果**：event origin 在 refresh 後仍維持最新值，clear 亦不復活。
- **實際結果**：原實作只更新 projected snapshot；refresh 從 baseSnapshot 重建後回到舊 origin。
- **重現率**：100% deterministic store race test。
- **錯誤訊息、日誌或畫面證據**：PR discussion `r3852614105`；HERDR store adjacent 41/41 PASS。
- **對上線的影響**：diagnostic／origin badge可能陳舊，影響使用者判斷 Agent 實際 runtime。
- **已知暫時解法**：修復前等待下一個完整 snapshot；無即時可靠 UI workaround。

### QA-021 — no-checkout release jobs 無法推斷 repository

- **Bug ID**：QA-021
- **嚴重度**：P1
- **修復狀態**：**FIXED／PASS** — 四個 no-checkout `gh release` jobs 均設定 job-level `GH_REPO`。
- **問題標題**：fresh runner 無 `.git` 時 draft assembly／metadata upload／publish 失敗
- **受影響功能**：Stable／Beta Release 建立、metadata upload、Publish
- **前置條件**：GitHub Actions no-checkout job，在沒有 repository cwd 的 runner 執行 `gh release`。
- **完整重現步驟**：1. 進入無 `.git` 的 `/private/tmp`。2. 只設 `GITHUB_REPOSITORY` 執行 `gh release view`。3. 再設 `GH_REPO=NakiriYuuzu/Yuzora` 重試。
- **預期結果**：job 明確指定 repository，不依賴 checkout cwd。
- **實際結果**：原流程回 `fatal: not a git repository`；加入 `GH_REPO` 後查詢成功。
- **重現率**：100%（無 GH_REPO fail；有 GH_REPO pass）。
- **錯誤訊息、日誌或畫面證據**：PR discussion `r3852614111`；release contract列舉並守護四個 jobs。
- **對上線的影響**：**發布阻擋**；新 release 在 assemble-draft 前即失敗。
- **已知暫時解法**：手動加 `--repo`；workflow 已採一致的 job-level `GH_REPO`。

### QA-022 — GitHub REST Release flags 未轉成 state machine shape

- **Bug ID**：QA-022
- **嚴重度**：P1
- **修復狀態**：**FIXED／PASS** — boolean type validation後正規化為 `isDraft`／`isPrerelease`。
- **問題標題**：existing draft／published release 一律在 channel assertion 失敗
- **受影響功能**：Release resume、已發布版本 safe skip、Stable／Beta channel guard
- **前置條件**：同 tag GitHub Release 已存在，REST 回應使用 `draft`／`prerelease`。
- **完整重現步驟**：1. 將 raw REST object傳入 state resolver。2. 使用 `{draft:true,prerelease:true}` beta draft。3. 觀察 channel assertion。4. 正規化後重試。
- **預期結果**：beta draft解析為 `{isDraft:true,isPrerelease:true}`，並進入 same-SHA rebuild／repair path。
- **實際結果**：原 resolver讀到 undefined flags並報 channel mismatch；正規化與 partial-draft修復後回 `shouldBuild=true, shouldPublishExisting=true`。
- **重現率**：100% deterministic state test。
- **錯誤訊息、日誌或畫面證據**：PR discussion `r3852614119`；raw error `existing release prerelease state does not match beta channel`；release state＋contracts 18/18。
- **對上線的影響**：resume／safe-skip 路徑不可用，可能讓正式 release recovery 失敗。
- **已知暫時解法**：無安全手動繞過；必須在 guard 正規化並 fail closed。

### QA-023 — Logs pagination 未使用專案標準 shadcn Button

- **Bug ID**：QA-023
- **嚴重度**：P3
- **修復狀態**：**FIXED／PASS** — Previous／Next 改用 `@/components/ui/button`。
- **問題標題**：Logs 新增手刻 native pagination buttons，偏離 UI primitive contract
- **受影響功能**：Settings → Logs pagination、互動／視覺一致性
- **前置條件**：log query超過 50 rows。
- **完整重現步驟**：1. 載入 51+ logs。2. 檢查 Previous／Next component來源。3. 比對 AGENTS.md shadcn-first規則與實際 UI。
- **預期結果**：共用 shadcn Button variants／interaction behavior。
- **實際結果**：原實作為手刻 `<button>` class。
- **重現率**：100% source-level。
- **錯誤訊息、日誌或畫面證據**：PR discussion `r3863124932`；targeted Logs 22/22；packaged UI 500 rows／10 pages／disabled state PASS。
- **對上線的影響**：不直接阻擋功能，但造成設計系統與可用性行為漂移。
- **已知暫時解法**：原 native buttons 可操作；無需使用者 workaround。

### QA-024 — Logs expanded row key 在跨頁時碰撞

- **Bug ID**：QA-024
- **嚴重度**：P2
- **修復狀態**：**FIXED／PASS** — key 使用 absolute result index。
- **問題標題**：不同頁同 page offset／timestamp／source／event 的 row 共用 expanded state
- **受影響功能**：Settings → Logs metadata expansion、pagination state
- **前置條件**：兩頁同一相對 index有相同 timestamp、source與event。
- **完整重現步驟**：1. 建立碰撞 fixture。2. 第一頁展開 row。3. 切下一頁。4. 觀察相對位置相同的 row。
- **預期結果**：只有使用者點選的 absolute result row展開。
- **實際結果**：原 key末段使用 page-local index，每頁從 0 重算，未點選 row也展開。
- **重現率**：100% collision fixture。
- **錯誤訊息、日誌或畫面證據**：PR discussion `r3863124940`；Logs targeted 22/22；packaged page 1→2 無串頁、返回 page 1 原 row仍展開。
- **對上線的影響**：顯示錯誤 metadata，降低診斷可信度。
- **已知暫時解法**：修復前切頁前先手動收合；無法避免真實 key collision。

### QA-025 — Accent validator 接受 prototype inherited keys

- **Bug ID**：QA-025
- **嚴重度**：P2
- **修復狀態**：**FIXED／PASS** — 使用 own-property判定，拒絕 `constructor`／`toString`。
- **問題標題**：corrupt persisted accent 可讓 CSS variables 變成 undefined
- **受影響功能**：Appearance settings、startup persistence、accent CSS variables
- **前置條件**：persisted appearance accent值是 Object prototype inherited key。
- **完整重現步驟**：1. 將 stored accent設為 `constructor` 或 `toString`。2. 啟動 app。3. 載入 appearance settings。4. 觀察 accent variables。
- **預期結果**：只接受 `ACCENT_THEMES` 自有 keys，其餘 fallback lime。
- **實際結果**：原 `value in ACCENT_THEMES` 對 inherited key回 true，palette欄位為 undefined。
- **重現率**：100% storage regression fixture。
- **錯誤訊息、日誌或畫面證據**：PR discussion `r3863124947`；settings storage adjacent 70/70；packaged violet persistence與 lime restore PASS。
- **對上線的影響**：損壞或惡意 persisted data會讓每次啟動的主題色失效。
- **已知暫時解法**：修復前清除 appearance storage；會遺失設定，並非理想處理。

### QA-026 — Logs client validator 拒絕 backend 支援的時間格式

- **Bug ID**：QA-026
- **嚴重度**：P2
- **修復狀態**：**FIXED／PASS** — 對齊 date-only、datetime-local、RFC3339，並嚴格拒絕非法日曆／時間／offset。
- **問題標題**：有效 date-only／datetime-local filters 被 frontend擋下
- **受影響功能**：Settings → Logs since／until filters
- **前置條件**：輸入 Rust `parse_query_time` 支援的 `2026-01-02` 或 `2026-01-02T12:30`。
- **完整重現步驟**：1. 在 since輸入 date-only。2. 再輸入 datetime-local。3. 比對 RFC3339控制組。4. 測 `2026-02-30`、`24:00`、非法 offset。
- **預期結果**：三種 backend支援格式均查詢；非法日期時間顯示 inline／AX error。
- **實際結果**：原修復版 validator要求 seconds與timezone，使前兩種合法格式停止 query。
- **重現率**：100% targeted tests與 packaged UI。
- **錯誤訊息、日誌或畫面證據**：PR discussion `r3863124950`；Logs targeted 22/22；packaged date-only／datetime-local／RFC3339得到 0-row future result，invalid輸入顯示 ISO error。
- **對上線的影響**：既有合法 filter regression，使用者無法使用簡短時間格式。
- **已知暫時解法**：修復前改輸入完整 RFC3339。

### QA-027 — resumed draft 可能發布 stale／任意 release notes

- **Bug ID**：QA-027
- **嚴重度**：P2
- **修復狀態**：**FIXED／CONTRACT PASS** — Stable／Beta都重新 build／assemble，assembly與publish前同步 guard-verified notes並逐字比對。
- **問題標題**：assemble-draft 被略過時，非空 stale body 可直接通過 Publish gate
- **受影響功能**：Stable／Beta draft recovery、Release notes provenance、automated Publish
- **前置條件**：same-SHA draft已存在且 body非空；guard走 publish-existing path，assemble-draft略過。
- **完整重現步驟**：1. 建立同 SHA draft並放入任意非空 notes。2. 讓 guard恢復 draft。3. 進入 publish job。4. 比對 body與 CHANGELOG-derived notes。
- **預期結果**：Publish只能使用 guard驗證的 exact notes。
- **實際結果**：原 gate只驗證 body非空，stale／人工 notes可不經比對發布。
- **重現率**：100% workflow contract mutation。
- **錯誤訊息、日誌或畫面證據**：PR discussion `r3863124957`；release 7 files／45 tests、Stable／Beta contract commands PASS；另見 QA-031 的 partial-draft recovery contract。
- **對上線的影響**：Release內容與 reviewed CHANGELOG脫鉤，屬供應鏈／發布完整性風險。
- **已知暫時解法**：修復前只能人工核對 draft；不足以取代 automated fail-closed gate。

### QA-028 — HERDR observer paste 會在 Take Control 後意外執行

- **Bug ID**：QA-028
- **嚴重度**：P1
- **修復狀態**：**FIXED／TARGETED PASS** — setup buffering只存在於 initial connection；read-only paste intent立即丟棄，async clipboard完成時也不會跨 control boundary replay。
- **問題標題**：唯讀 terminal 拒絕的 clipboard text 被保留，取得 control 後可能連同換行送入 shell
- **受影響功能**：HERDR Terminal observe／control、Ctrl／Cmd+V、Take Control
- **前置條件**：HERDR connector以 observer模式開啟；clipboard包含文字，可能含 trailing newline。
- **完整重現步驟**：
  1. 在 observer terminal按 Ctrl／Cmd+V或送出 paste event。
  2. 確認當下 terminal不可寫。
  3. 點 Take Control，或讓 clipboard read promise在 control切換後才完成。
  4. 觀察 terminal input。
- **預期結果**：read-only時發起的 paste永遠丟棄；只有 initial connection最終以 control成功時，setup期間的 paste才可交付一次。
- **實際結果**：原實作把 rejected text留在 `pendingPaste`，control event的 `flushPendingPaste()` 隨後送入 terminal；async read也可能在取得 control後直接送出舊 intent。
- **重現率**：100% deterministic controller／component regressions。
- **錯誤訊息、日誌或畫面證據**：PR discussion `r3864436328`；新增 async race在修復前收到 `late observer paste` 1次而 FAIL；修復後 Terminal兩檔 31/31、合併 UI targeted 80/80 PASS。
- **對上線的影響**：可能在使用者只觀察 session時，於稍後取得控制權的瞬間執行非預期 shell內容，包含具副作用命令；屬 terminal safety blocker。
- **已知暫時解法**：修復前不要在 observer模式貼上，且取得 control前清空 clipboard；無可靠產品內 workaround。

### QA-029 — Logs expanded state 會跨 query／run filter 套到不同 record

- **Bug ID**：QA-029
- **嚴重度**：P2
- **修復狀態**：**FIXED／TARGETED PASS** — successful query replacement與 run filter切換時都清空 expanded map。
- **問題標題**：相同 timestamp／source／event與相對 index可讓舊 metadata expansion轉移到新 record
- **受影響功能**：Settings → Logs、run groups、query refresh、metadata expansion
- **前置條件**：兩個 run或前後兩次 query在同一 visible index有相同 timestamp、source與event，但 metadata不同。
- **完整重現步驟**：
  1. 載入 collision fixture並展開 run A record。
  2. 切到 run B filter；或執行會替換 rows的新 query。
  3. 檢查相同位置 record的 expansion與 metadata。
- **預期結果**：結果集合或 filter identity改變後，未被本次點選的 record保持收合。
- **實際結果**：原 expanded key仍以 `visibleRows` index構成，index重置後把舊 expanded state帶到不同 underlying record。
- **重現率**：100%（run switch與 query replacement各1個 deterministic fixture）。
- **錯誤訊息、日誌或畫面證據**：PR discussion `r3864436335`；`logsSection.test.tsx` 驗證 run A／B owner與 replacement owner不誤展開；Logs adjacent 33/33 PASS。
- **對上線的影響**：診斷畫面可能顯示使用者未選擇的 metadata，造成錯誤歸因。
- **已知暫時解法**：修復前每次切 filter／重查後手動收合；無法可靠避免 identity collision。

### QA-030 — HERDR Attention action 未使用 shared shadcn Button

- **Bug ID**：QA-030
- **嚴重度**：P1（review gate classification）
- **修復狀態**：**FIXED／TARGETED PASS** — Attention entries改用既有 `@/components/ui/button` 的 ghost variant並保留 row layout。
- **問題標題**：新 Attention actions以手刻 native button繞過 repository UI primitive contract
- **受影響功能**：ADE → HERDR Attention、disabled／hover／focus interaction consistency
- **前置條件**：HERDR snapshot含 attention item。
- **完整重現步驟**：1. 開啟 HERDR nav。2. 顯示 Attention item。3. 檢查 action component與 disabled state。4. 對照 AGENTS.md shadcn-first規則。
- **預期結果**：通用 button action由 shared `Button` 組合，domain row只提供必要 layout classes。
- **實際結果**：原新增項目直接使用手刻 `<button>` 與完整互動 classes。
- **重現率**：100% source-level。
- **錯誤訊息、日誌或畫面證據**：PR discussion `r3864436343`；regression確認 `data-slot="button"` 且 unavailable item仍 disabled；UI targeted 80/80 PASS。
- **對上線的影響**：主要是 review／設計系統 gate；若放行會讓 focus、disabled與後續 theme behavior持續漂移。
- **已知暫時解法**：原 native button可操作，但不符合 repository acceptance contract。

### QA-031 — Partial draft recovery 會永久略過缺失平台／assets

- **Bug ID**：QA-031
- **嚴重度**：P1
- **修復狀態**：**FIXED／CONTRACT PASS** — same-SHA draft必須重建兩平台並修復同一 draft；local handoff在 draft mutation前驗證，notes／channel讀回核對，assets與Stable aliases用 `--clobber`，Publish無 skipped-build旁路。
- **問題標題**：draft建立後若 validation／upload失敗，後續 run會直接驗證不完整 draft並反覆失敗
- **受影響功能**：Stable／Beta automated Release、partial upload recovery、artifact provenance
- **前置條件**：`gh release create`成功，但本地 artifact validation、任一 versioned asset或Stable alias upload、或後續 gate失敗。
- **完整重現步驟**：
  1. 讓 release attempt建立 same-SHA draft後於 asset assembly中途失敗。
  2. 重新執行同一 source SHA的 workflow。
  3. 觀察 guard輸出的 `shouldBuild` 與 build／assemble jobs。
  4. 檢查 Publish前 asset inventory。
- **預期結果**：可預知的本地 handoff錯誤不先建立 draft；既有 same-SHA partial draft則重建 macOS／Windows、冪等覆寫完整 assets並重跑全部 gates。
- **實際結果**：原 state resolver對 matching draft回 `shouldBuild=false`，因此 build與assemble都 skipped；缺失 asset不會被補回，verification每次都在同一 incomplete draft失敗。
- **重現率**：100% state／workflow contract regression。
- **錯誤訊息、日誌或畫面證據**：PR discussion `r3864436349`；舊 state expectation在紅燈回傳 `shouldBuild=false`；修復後 release 7 files／45 tests、Stable／Beta contracts、actionlint與YAML parse全部 PASS。
- **對上線的影響**：**發布阻擋**；正常可恢復的網路／upload失敗會把該 immutable version卡在無法自動完成的 draft狀態。
- **已知暫時解法**：修復前只能經 maintainer授權清理 tag／draft或改新 version；兩者都不是安全的自動 recovery。

### QA-032 — Logs Copy 在第 2 頁仍複製全部查詢結果

- **Bug ID**：QA-032
- **嚴重度**：P2
- **修復狀態**：**FIXED／TARGETED PASS** — sanitized與raw Copy都改用 active `pageRows`，成功提示也使用當頁 count。
- **問題標題**：畫面只顯示50 rows，但 Copy可把最多500 rows／非預期raw資料放進 clipboard
- **受影響功能**：Settings → Logs pagination、Copy、sanitize/raw
- **前置條件**：query至少100 rows；切至Page 2。
- **完整重現步驟**：1. 載入100 rows。2. 切至Page 2。3. 分別在sanitize on／off點Copy。4. 解析clipboard events與count notice。
- **預期結果**：只複製目前顯示的 event_50～event_99共50 rows，提示也顯示50。
- **實際結果**：原實作序列化全部 `visibleRows`，Page 2仍複製100 rows；raw mode可能帶入使用者眼前未顯示的資料。
- **重現率**：100%（sanitized／raw各1次）。
- **錯誤訊息、日誌或畫面證據**：PR review body（reviewed head `38a445aad…`）；紅燈為實際100 rows、預期50；修復後 Logs adjacent 33/33 PASS。
- **對上線的影響**：clipboard內容與UI affordance不一致，並擴大未預期 raw log資料暴露範圍。
- **已知暫時解法**：修復前先用更窄 filter讓全部結果不超過單頁，或不要使用raw Copy。

### QA-033 — macOS FSEvents root prelude 使 watcher test 隨機失敗

- **Bug ID**：QA-033
- **嚴重度**：P1（release CI gate）
- **修復狀態**：**FIXED／LOCAL PASS；EXACT-HEAD CI PENDING** — test在5秒deadline內逐批等待 `b.txt`，不再假設第一個合法 FSEvents batch就是目標檔案。
- **問題標題**：watcher generation test把合法 watch-root batch誤判為功能失敗
- **受影響功能**：macOS Rust tests、PR required CI、release candidate gate
- **前置條件**：macOS FSEvents在寫入 `b.txt` 前先回報 watched temp root。
- **完整重現步驟**：
  1. 在 exact head `38a445aad…` 執行 macOS `cargo test --locked`。
  2. 讓 FSEvents先送 root event。
  3. 觀察 `stale_watch_build_finishing_late_does_not_override_newer_watcher` 的第一個 `recv_timeout`。
- **預期結果**：測試忽略無關但合法 batch，直到deadline內看到 `b.txt`；產品 watcher generation semantics不變。
- **實際結果**：原測試只讀第一批並立即 assert `b.txt`，CI收到 `got: ["/private/.../.tmpqEFRyh"]` 後失敗；整體為890 passed／1 failed／1 ignored。
- **重現率**：CI 1/1該 run；注入 root prelude後舊斷言 deterministic 100% FAIL。
- **錯誤訊息、日誌或畫面證據**：Actions run `33027444809`、job `98371995228`；修復後 exact targeted PASS，先前完整本機 Rust為891 passed／1 ignored。
- **對上線的影響**：產品 watcher未證實故障，但 required macOS CI為紅燈，PR不可進入候選／merge gate。
- **已知暫時解法**：單純 rerun可能碰巧通過但會保留不穩定 gate，不視為修復。

## 4. 未完成與受阻項目

| 未測試功能 | 阻礙原因 | 所需條件 | 殘餘風險 |
|---|---|---|---|
| 實際 signed／notarized macOS release candidate | 本機無 Apple Developer ID／notary credentials；protected workflow 尚未執行 | 受保護 secrets、tag／workflow run、下載其 app／DMG | workflow contract 已 fail-closed，但 Gatekeeper／stapling 仍未由真實 artifact 證明；這是目前 NO-GO 主因。 |
| Windows installer／SmartScreen／WSL2／native HERDR | 舊 head Windows candidate已有靜態驗證，但 post-review exact-head candidate尚待 CI；目前無 Windows 11／WSL2 主機 | 新 exact-head candidate、Windows 11、WSL2、測試 Herdr provider | Windows 安裝、SmartScreen、named pipe與 WSL descendant Agent互動行為未知。 |
| PostgreSQL／MSSQL 真實連線與 SQL console | 無隔離 local listeners／fixtures；不連 saved external DB | Docker 或本機 fixture、非正式資料、測試憑證 | TLS、vault、DML effect、paging、cancel、recovery 未實機證明。 |
| Docker DB integration | Docker socket `/Users/yuuzu/.docker/run/docker.sock` 不存在 | 可用 Docker daemon／compose fixture | ignored matrix以 `YUZORA_DATABASE_TEST_ENGINES=sqlite` 1/1 PASS；PostgreSQL／MSSQL跨 engine風險保留。 |
| SFTP 寫入／破壞性操作與 changed-key 流程 | SSH shell、TOFU、known-host reconnect、SFTP root／subdir browse 已 PASS；未執行 upload／mkdir／rename／delete 或替換 host key | 全隔離 remote filesystem、可棄置下載／上傳路徑、changed-key fixture | backend guard 有自動測試，但完整 UI mutation／overwrite／cleanup 仍不可推定正常。 |
| OS vault／真實 credentials／raw logs | 不建立或傳入 sensitive data | 隔離 keychain namespace 與測試 secrets | credential lifecycle／redaction 只具 source/test 證據。 |
| Updater download／install／relaunch | Beta contract 明示 no OTA，本機 candidate 無 updater signing key | signed Stable artifact、staging endpoint、throwaway install | workflow contract PASS；實際 download/install/relaunch 未測。 |
| TypeScript／Python／Rust LSP | runtime 顯示 launcher／digest integrity mismatch | 修正 catalog 或隔離安裝權限 | 只有 Markdown marksman runtime PASS。 |
| Preview managed dev server | port 3000 已被不相關服務占用；未啟動 configured `bunx serve` | 隔離 localhost server／可安全執行的 throwaway workspace command | managed start/stop／alternate-port process cleanup 未完成；external HTTPS／history／lifecycle 已 PASS。 |
| HERDR mutations／Agent start | 會改變外部 Herdr session/process | throwaway Herdr session／Space | create/rename/split/close/capability failure paths未測。 |
| Git／file／editor destructive or write flows | 本輪明確禁止 repository/Git 寫入 | throwaway clone 或專屬 fixture repo | stage/commit/discard/save/conflict/file mutations 均不得推定正常。 |
| Gemini 3.7 x2 inventory | 代理 runtime 兩次 400 | 可正常回傳 model-turn 的代理 runtime | 少兩份獨立 variance-reduction inventory。 |
| 多小時／Windows performance soak | 本輪只有 macOS 30m01s bounded soak | 長時間 telemetry、Windows candidate | 目前未見持續 RSS／FD／thread 成長，但不能外推到多小時或 Windows。 |

## 5. Git 工作區檢查

### 測試前狀態

- 命令：`git status --short`
- 結果：無輸出，工作區乾淨。
- 分支：`release/v0.0.9-beta.1`
- HEAD：`d97fb7a5724669394abeb11360eef7330e332d06`

### 測試後狀態

- 原始 QA 結束時間：2026-08-26 09:48:24 +08:00；當時 `git status --short` 只有 `?? QA-REPORT.md`。
- 使用者其後明確要求修復所有問題；第一批 remediation 已依授權形成 `f8e8d17c1cd4df34a71ba3d360ed0b3f19cbf2d5`、`183ff004a6772f7d1439c97d6c11bb1d2e380b47` 與 `bf82126649a0f2d44415caf7526f15a3eb1d5757` 並 push 至同一 release branch。
- `bf821266…` 後的 review與 exact-head CI共觸發 QA-017～QA-033；本輪追加修復狀態為 15 個 source／test／workflow／operations modifications 加 `QA-REPORT.md`，沒有 staged changes，全部可追溯至 findings或其必要回歸／文件同步。
- 最新本機核對時間：2026-08-27 09:35:28 +08:00；local／remote baseline均為 `38a445aad3155011b4c13b1c5bfcef1509cfa9d2`；`git diff --check` PASS；`git diff --cached --name-only` 無輸出；status只含本報告列出的15-file patch加 `QA-REPORT.md`。
- 最新 build source、Cargo target、app與 DMG位於 `/private/tmp/yuzora-ops.LSCoT2`；最新 build 未寫 repository `dist/` 或 `src-tauri/target/`。
- QA-009 早期 historical ignored-artifact event 保留原狀，沒有清理、刪除或還原；修復階段未再重現。
- `git check-ignore -v dist src-tauri/target` 證實兩者分別由 root `.gitignore:17` 與 `src-tauri/.gitignore:3` 排除。
- localhost sshd 已停止、兩個 clone app 已關閉；本輪新增的 `127.0.0.1:48222` known-host test record已移除，原有兩筆正式 fingerprint 內容保持不變。
- 最新 packaged Yuzora 已以 exact app path正常 Quit；matching process不存在，lime／Follow system已還原並跨重啟確認。

### Git 寫入確認

- 原始唯讀 QA 階段未執行 `git add`、`git commit` 或 `git push`；使用者後續明確要求修復並執行 `docs/operations.md` 後，才在授權範圍內提交／推送上述 remediation。
- 本次 post-review patch在本報告內容定稿時尚未 staged／committed／pushed；後續 operations只會明確 stage本報告列出的 16 個檔案。最終 commit SHA、push、exact-head CI與 candidates屬定稿後事件，以 PR #83／Issue #84留言為權威證據。
- 全程未執行 reset、restore、clean、stash、force push、branch rewrite、tag、merge或 Publish；沒有刪除、還原或隱藏其他 worktree changes。

## 驗收事件與證據紀錄

- 2026-08-26 00:52:49 +08:00：取得測試前 Git 基線，工作區乾淨。
- 2026-08-26 00:52:49 +08:00：確認版本 `0.0.9-beta.1`、Tauri identifier `dev.yuuzu.yuzora`、production bundle active、updater artifact enabled。
- 2026-08-26：確認英／繁中 README version badge 均落後為 `0.0.8`，記錄 QA-001。
- 2026-08-26：一般 production Tauri build 於 3m11s 完成 binary／app／DMG／updater archive，最後因缺 updater private key code 1，記錄 QA-002。
- 2026-08-26：依 release workflow Beta lane 以 `--no-sign` 與 no-updater config 重建，code 0；產生 `.app` 與 DMG。
- 2026-08-26：DMG checksum 驗證通過；app strict code-sign／Gatekeeper 驗證均失敗，記錄 P1 QA-003。
- 2026-08-26：以 Codex 內建 Computer Use 操作最新 packaged app（不使用 Orca）：首屏、工作區 rail、HERDR Space／Agent、Files tree、README editor、marksman Ready、Markdown Preview、Terminal Drawer 與本機 idle PTY 均可操作；Space 來回切換後 terminal page／pane 仍在，未重現 session 遺失。
- 2026-08-26：Markdown Preview 可由 close control 關閉；Terminal Drawer 可由 `Ctrl+\`` 反覆開關，建立的 idle PTY 可關閉並回到「尚無終端機工作階段」空狀態；Profile menu 可開啟並以 Escape 取消。
- 2026-08-26：Command Palette 可顯示空查詢 suggestions，Escape 關閉後焦點返回 workbench。
- 2026-08-26：SSH／SFTP 未連線空狀態正確；「新增 SSH 主機」對話框空表單的新增按鈕停用，可切換密碼／金鑰驗證；Port `0` 與 `65536` 皆停用提交、`65535` 啟用提交；Escape 取消後未新增主機。未連接既有外部主機，未傳送憑證。
- 2026-08-26：Settings 全區段驗收；theme/language/editor settings 可跨 restart 保存且已還原；記錄 QA-004～QA-006。LSP 顯示 vtsls／pyright／rust-analyzer integrity mismatch，marksman Ready。
- 2026-08-26：Database 三 engine 表單、SQLite missing path、PostgreSQL trust-cert／plaintext confirmation 及取消通過；missing-parent path 測試後仍不存在。
- 2026-08-26：Git 僅讀檢查顯示 branch、`QA-REPORT.md` untracked、log 與 commit detail；沒有點擊任何 mutation action。
- 2026-08-26：Preview 正確處理 port 3000 collision、連接 existing localhost、404、reload、responsive 與 valid URL；危險 scheme 安全拒絕但沒有 feedback，記錄 QA-008。
- 2026-08-26：委派 automated checks 回報 frontend 2425 tests 全過、Rust 889 passed／1 ignored，並回報 SFTP remove validation gap，記錄 QA-007。
- 2026-08-26 01:20:36 +08:00：委派 agent 進一步確認上述 checks 在原 repository 更新 ignored `dist/`／`src-tauri/target/`；立即停止全部剩餘測試，記錄 QA-009，執行最終唯讀 Git 核對。
- 2026-08-26：使用者明確要求繼續逐功能實機測試；續測 baseline 為 `?? QA-REPORT.md`、同一 branch／HEAD。以 Codex 內建 Computer Use 重新連線最新 packaged app，從 ADE／README tab／Terminal Drawer closed 狀態開始。
- 2026-08-26：續測 Agent Inspector。實際 click 只聚焦 Agent，右鍵只有 pane actions且 Escape 可取消；畫面／Accessibility tree 無 Inspector 入口。唯讀 source 與 code graph 確認 production caller 為 0，記錄 QA-010。
- 2026-08-26：Document Symbol Picker 續測通過。marksman Ready 後列出 README headings；`Features` 篩選與 Enter 正確 reveal／選取 `## Features`；不存在名稱顯示「無符號」；Escape 取消且重開 query reset。
- 2026-08-26：Workspace Symbol Picker 續測通過。空 query／不存在 query 顯示「無符號」；`Features` 找到 README，`功能` 找到 `QA-REPORT.md` 並可 Enter 跨檔 reveal／選取 heading；picker 開啟時 `⌘K` 正確關閉且不疊加 dialog。
- 2026-08-26：Editor tabs／split 續測通過。新增 Browser/Preview、menu Escape、Preview context close、file tab split-to-right、two-group limit、close split 均正常；README tab 以 Alt+Left／Right 重排、邊界 no-op 且已還原原順序。
- 2026-08-26：Terminal I/O 續測通過。type／paste marker 正常；終端 output selection 可 ⌘C／⌘V 回貼並取消；右 split、最多兩 pane、50/50→60/40→50/50 resize 正常；不存在指令有 zsh feedback並恢復 prompt；exit 7 移除右 pane、exit 0 回空狀態，Drawer 已關閉。
- 2026-08-26：Native window controls 續測通過。minimize／restore 正常；原生全螢幕進入後 ADE／HERDR 內容仍完整可操作，`Ctrl+⌘+F` 正常退出；標準視窗由 `1463×769` 調成 `1388×768`，關閉並重新啟動後為 `1389×768`、HERDR 正常重連，最後還原 `1463×769`。
- 2026-08-26：以 repository 外 `/tmp/yuzora-qa-ssh.j1AfwC` 建立只監聽 `127.0.0.1:48222` 的 throwaway OpenSSH fixture；host/client Ed25519 keys、authorized key、SFTP root、獨立 HOME 與不同 bundle identifier 均隔離。CLI control 的 SSH command 與 SFTP initial directory/list 均 code 0；未操作正式 hosts／credentials／known-hosts。
- 2026-08-26：Yuzora TOFU 顯示 endpoint、`ssh-ed25519` 與 SHA-256 fingerprint 均正確；拒絕未送憑證／未 pin，但約 60 秒後才回錯，記錄 QA-015。接受直接顯示 Tauri invoke 缺 `challengeId`，無法建立任何 first-use SSH session，記錄 P1 QA-016；依規範未以手工 pre-pin 或 direct IPC 繞過，downstream shell／SFTP 維持 BLOCKED。
- 2026-08-26：Database restart／recovery 續測通過。重啟後 SQLite／PostgreSQL profiles 皆保持 offline，recent queries 為空；SQLite fixture 可重新連線載入 schema，已連線 profile context menu 可 Escape 取消，再以「中斷連線」回到 offline；未執行任何 SQL。
- 2026-08-26 09:16:50–09:46:51 +08:00：完成 30m01s macOS packaged-app bounded soak。以 Computer Use 執行 13 輪主要模式／兩個 Space／Settings／Terminal Drawer 往返，並做 4 次 SQLite connect/disconnect；同一 PID `55870` 全程存活、UI／AX 可操作。RSS 約 104,480–115,072 KB，起點 112,384 KB、終點 111,776 KB；open files 79→77、threads 34→34，無持續上升或 crash／hang。此結果不涵蓋多小時或 Windows endurance。
- 2026-08-26 09:48:24 +08:00：以 native close 關閉 packaged app並確認 process exit；最終 `git status --short` 只有 `?? QA-REPORT.md`，tracked／staged diff 均為空。正式 `~/.yuzora/known_hosts.json` 保持既有 7 月 7 日檔案，localhost port 48222 無 listener。全程未執行 add／commit／push。
- 2026-08-26：使用者明確要求修復所有已發現問題；開始 remediation。QA-001～016 的 repository 可處理範圍均加入實作與回歸測試，QA-009 僅保留歷史事件、不清理既有 ignored artifacts。
- 2026-08-26：release workflow 改為 macOS Stable／Beta 都要求 Developer ID certificate／identity 與 Apple notarization credentials；建置後必須通過 Developer ID Authority、TeamIdentifier、strict `codesign`、`spctl`、app／DMG `stapler validate`，並在成功或失敗時清理暫時 keychain。Windows lane 不再要求 Apple secrets。
- 2026-08-26（第一批 remediation）：frontend typecheck PASS；lint 0 errors；完整 Vitest 179 files／2,439 tests PASS。Rust check／fmt／exact-clippy baseline PASS；891 unit tests PASS、1 ignored；DB integration 1 PASS、1 ignored；release/build contract 19 tests PASS。
- 2026-08-26（第一批 remediation）：以 repository 外 `/private/tmp/yuzora-fixed-build.8SSbqE/source` 與 `cargo-target` 執行 `bun run tauri:build`，code 0；app／DMG version `0.0.9-beta.1`、identifier `dev.yuuzu.yuzora`，DMG SHA-256 `8ec5bc99302847147bc75104a40ef0e6d3d678071028d2b8ba4806d1f7e13f5c`，`hdiutil verify` PASS。
- 2026-08-26：QA-004 實機回歸：blue／violet 立即 selected、互斥、關閉設定與冷啟後保存；最終還原 lime／Follow system。
- 2026-08-26：QA-005／006／013 實機回歸：English LSP／Logs 固定字串與 integrity diagnostics 無硬編碼漢字；Logs 500 rows 以 AX content list 呈現、每頁 50 筆共 10 頁，Next／Previous 正常；invalid since 顯示可存取 ISO error，有效未來 ISO 得 0 rows後可清除恢復。
- 2026-08-26：QA-010／011 實機回歸：Agent Inspector production action可開啟 metadata 與 text／ANSI output；初次發現 Escape 未回焦點後補修，最新 build 已確認焦點回 `Inspect π - privacy-filter`。Git User／Date menus 均可 Escape 關閉且回焦點。
- 2026-08-26：QA-008／012 實機回歸：`file:`／`javascript:` 均顯示 HTTP(S)-only 提示；`https://example.com/` native child webview 切至另一 Space 後不再殘留，返回無 orphan，explicit tab close 後 child content亦消失。
- 2026-08-26：QA-014～016 實機回歸：Password／Key file AX radios與欄位一致；TOFU accept 約 0.527s、shell marker成功、known-host reconnect 約 0.528s；未記錄 `localhost` endpoint reject 約 0.468s 即時回 Error／Reconnect；SFTP root 與 `qa-subdir` browse PASS。
- 2026-08-26：SSH fixture 只監聽 `127.0.0.1:48222` 且使用 throwaway keys。測試後 sshd停止、clone apps關閉；測試產生的 localhost known-host record 已精確移除，既有兩筆正式 fingerprint 內容保留。
- 2026-08-26：本機 no-sign candidate 按設計仍為 ad-hoc linker signature；strict codesign／`spctl` exit 1，DMG `stapler validate` exit 65（無 ticket）。因此 workflow remediation 已完成，但在取得真實 signed／notarized release artifact 前整體維持 NO-GO。
- 2026-08-26 19:59:01 +08:00：以 bundled Computer Use 聚焦 exact packaged app 並送出 macOS Quit；程序與 listener 複查無 Yuzora／clone app／sshd 殘留。最終 `git diff --check` PASS、staging area 為空；工作區僅有本輪明確授權的 40 個 tracked 修復與 6 個 untracked 修復／報告檔。
- 2026-08-26：PR #83 三輪 review共 11 個 unresolved threads。獨立重現後記錄 QA-017～QA-027；其中 QA-019 的 invalid checkout ref前提被 upstream API證偽，其餘 10 項皆完成紅→綠最小修復。
- 2026-08-26：post-review targeted combined 7 files／77 tests PASS；HERDR store adjacent 41/41、Logs／settings adjacent 70/70、release state＋contracts 18/18；全專案 Vitest 179 files／2,452 tests、typecheck、lint（0 errors／49 warnings）、production build、Rust fmt／check／exact 251-diagnostic Clippy baseline、891 unit tests與 SQLite ignored matrix均 PASS。
- 2026-08-26：以 repository 外 `/private/tmp/yuzora-ops.LSCoT2/repo` 套用 13-file patch並執行 `bun run tauri:build`，code 0；DMG SHA-256 `97ad88de36c798473abdbe36bd1dedea4888c9efde4e52927ae32c641230d680`，`hdiutil verify` PASS；app為 `0.0.9-beta.1`／`dev.yuuzu.yuzora`／arm64／ad-hoc。
- 2026-08-26：bundled Computer Use exact-path回歸：Command Palette `>privacy`／`>π` 搜尋、Enter與Escape PASS；Logs date-only／datetime-local／RFC3339、非法日曆／時間、10-page navigation與expanded-state isolation PASS；violet persistence後已還原 lime並再重啟確認。
- 2026-08-26：同一 packaged app 的 Git User／Date Escape、Preview `javascript:`拒絕、`https://example.com` child webview／responsive／Space lifecycle、SSH radio方向鍵與Escape cancel smoke均 PASS。最終以 ⌘Q結束，exact process不存在；未使用 Orca。
- 2026-08-26 23:32:41 +08:00：post-review patch與 `QA-REPORT.md` 的 `git diff --check` PASS、staging area為空；GitHub live gate仍為六項 Apple secrets缺失、`main`未保護、rulesets空、maintainer review／merge授權未完成，因此結論維持 NO-GO。
- 2026-08-27：GitHub Actions恢復後，authoritative exact-head run `33027444809` 的 frontend、Linux／Windows Rust、database與兩個 candidate jobs均PASS；macOS Rust job `98371995228` 唯一失敗於 watcher test先收到合法 temp-root FSEvents batch，記錄QA-033。舊 outage-corrupted run `32985785981`不再作為測試結果。
- 2026-08-27：PR head `38a445aad…` 新一輪review記錄QA-028～QA-032。四個 inline threads與一個review-body finding均以deterministic red→green修復；同時補足 observer async clipboard race與「local artifacts先驗證、後建立draft」contract。
- 2026-08-27：初步 targeted回歸：UI 5 files／80 tests、release 7 files／45 tests、watcher exact test、version／notes／Beta／Stable contracts、actionlint v1.7.7、Ruby YAML parse與`git diff --check`全部PASS；完整 suite與新 exact-head CI仍依後續事件更新。
- 2026-08-27 09:35:28 +08:00：完整本機gate完成。Frontend typecheck、lint（0 errors／49既有warnings）、180 files／2,463 tests與production build PASS；Rust fmt、check、exact 251-diagnostic Clippy baseline、891 unit tests／1 ignored與SQLite integration 1 PASS／1 ignored；staging area仍為空。新 exact-head CI／candidate與bundled Computer Use驗收尚待PR branch更新後執行。
