# Changelog

這裡只記錄使用者可以直接感受到的改變，不包含 commit、檔案名稱或內部實作細節。

## [0.0.9] - 2026-09-10

### 新增

- 全新 Space／Agent 工作面，集中查看多台主機的終端機、Agent 狀態與 Attention；Files／Git 側欄可獨立收合。
- Windows 原生 HERDR 與可選擇啟用的 WSL；本機、WSL、SSH 工作區的終端機、檔案與 Git 在各自選定的主機執行。
- 各主機可選用 Yuzora 管理、已安裝或自訂 HERDR，並提供相容性檢查、更新與診斷入口。
- 遠端工作區支援檔案編輯、安全儲存、Git／worktree、Browser 與 SQLite；純 SFTP 可直接編輯及手動上傳／下載。
- Markdown 文件編輯與原始碼切換、檔案分頁釘選及重啟恢復、設定搜尋、主題調整、資源用量與可保存的 Log 開關。
- 終端機支援多行整段貼上、選取文字後自動複製，以及 Option／Alt+V 將剪貼簿圖片存到終端機所在主機並貼入路徑；自動複製可在設定關閉。
- 官網加入可直接操作的網頁 Demo，體驗範例終端機、檔案編輯、Git 差異、資料庫與外觀設定；中英文官網與 Demo 一同部署至 GitHub Pages。

### 改善

- 隨附 HERDR 升級至 0.9.0，支援連回相容的既有 server；關閉 Yuzora 保留正在執行的 HERDR、Agent 與 WSL。
- 更新品牌圖示與主題配色，調整 Space 角色動畫、側欄、設定頁、資料夾選擇器與未儲存確認視窗。
- Git 差異視窗提供清楚的新增／刪除標記、並排閱讀及一致的視窗操作；HERDR Session 選擇器顯示主機與狀態。
- 編輯器與 Git 差異的捲軸配合深淺主題；提交紀錄與分支圖入口改為清楚的按鈕。
- 資料庫連線側欄可拖曳或以鍵盤調整寬度，長連線名稱不再擠出編輯與移除按鈕。
- 加快 Git 狀態、分支列表、工作區差異與歷史版本差異的內容載入。
- 改善多個 HERDR 終端機分頁之間的切換流暢度，保留背景終端機畫面與連線。
- 降低多終端機的重複畫面更新、Git 掃描與背景工具執行緒成本，釋放不再使用的工作區、訂閱及連線資源。

### 修正

- 修正 WSL 路徑轉換、切換主機後套用過期結果、取消資料夾選擇後反覆彈窗，以及非 Git 資料夾無法載入 Files 的問題。
- 修正工作區信任確認被背景查詢失效；WSL 檔案可依所屬發行版在 Windows 檔案總管顯示。
- 修正 Windows 終端機啟動目錄顯示 PowerShell provider 前綴，以及空 Session 無法建立第一個終端機的問題。
- 修正快速輸入與多行貼上的邊界、切換終端機後套用過期圖片結果，以及長時間隱藏的終端機再次顯示時畫面不完整的問題。
- 修正 Windows Browser 開啟時的鎖定問題、遠端檔案重連與資源釋放、SQLite 路徑選擇及資料庫頁面間距。

### 功能調整與已知限制

- 從 v0.0.9 起，macOS App 僅支援 Apple Silicon（M 系列）；不再提供 Intel／universal 安裝包。遠端 Intel macOS Host 保留支援。
- 終端機統一由 HERDR 提供；Agent 由使用者在終端機手動啟動。移除獨立本機／SSH 終端機、shell profiles 與新增 Agent 表單。
- 移除 LSP 語言伺服器功能；保留語法編輯、搜尋及 Markdown／SVG／圖片檢視。Browser 保留網址導覽、歷史與遠端連接埠轉送；開發伺服器需自行在終端機啟動。
- 既有 WSL／SSH 主機保留原工具路徑，升級後需在 HERDR 設定檢查並更新所選來源；不會自動停止不相容的既有 server 或搬移執行中的程序。
- HERDR／Browser 分頁的釘選僅保留於本次應用程式工作階段。剪貼簿圖片辨識取決於終端機內 Agent 對圖片路徑的支援。
- 網頁 Demo 使用範例資料，重新整理後重設檔案與查詢結果；不連接本機或遠端主機。
- Windows Authenticode 尚未啟用，安裝或首次開啟時仍可能出現 SmartScreen 提示。
- macOS 安裝檔未經 Apple Developer ID 簽章或公證，首次開啟可能被 Gatekeeper 提示或阻擋；請從官方 GitHub Release 下載，確認來源後依 macOS「隱私權與安全性」的「仍要打開」流程開啟。正式版自動更新仍驗證 Tauri updater 簽章。

## [0.0.9-beta.3] - 2026-09-09

### 新增

- Windows 支援原生 HERDR；可在設定中啟用 WSL，再為資料夾選取 Linux 發行版。每個工作區的終端機、檔案與 Git 在所選主機執行。
- HERDR 設定可管理各主機的隨附、已安裝或自訂版本，檢查相容性、更新工具並複製診斷；錯誤提供直接修復主機的入口。
- 共用新增資料夾入口支援本地、SSH 完整工作區及純 SFTP，並以主機標示區分近期資料夾與文件。
- 新版工作面以 Space／Agent 側欄集中呈現多台主機的 Agent 與 Attention，Files／Git 工具側欄可獨立收合；遠端工作區提供編輯、安全儲存、Git／worktree、Browser 與 SQLite。
- SFTP 支援直接編輯與手動上傳／下載，儲存前確認遠端版本。
- Markdown 提供文件編輯與原始碼切換；不適合文件模式編輯的內容保留安全閱讀與原始碼操作。
- 工作分頁支援釘選；檔案分頁的釘選狀態可在重啟後恢復。
- 設定新增搜尋、主題與漸層調整，資源用量可查看 App、WebView 與受管工具分類。
- 可關閉 Yuzora Log 記錄並保存設定；既有記錄仍可查閱與匯出。

### 改善

- 升級官方 HERDR 至 0.9.0／protocol 22，支援連回已執行的 0.9.0 server；主機工具安裝於使用者版本目錄，保留既有 runtime。
- 斷線保留未儲存內容，重連重新確認檔案狀態；關閉 Yuzora 保留 HERDR、Agent 與 WSL。
- 側欄與 Session 選單只呈現仍在執行的 Sessions；外部建立的 Space／Agent 可直接開啟終端機，不再要求先綁定檔案資料夾。
- Git 檔案可直接開啟大型並排差異檢視，並以狀態色與文字區分修改、未追蹤、已暫存及刪除。
- 調整窄視窗的側欄、頂列與資料夾選擇器；終端機預設使用 JetBrains Mono。

### 功能調整

- 終端機統一由 HERDR 提供，移除獨立本機／SSH 終端機與 shell profiles。Agent 改由使用者在終端機手動啟動，移除新增 Agent 表單；既有 Agent 狀態與 Inspector 保留。
- 移除 LSP 智慧編輯、語言伺服器下載與相關設定；保留語法編輯、搜尋及 Markdown／SVG／圖片檢視。
- Preview 收斂為 Browser，保留網址導覽、歷史、重新整理與遠端連接埠轉送；開發伺服器改由使用者在 HERDR 終端機啟動，不再提供靜態 Preview server 或 Dev Server 管理。

### 修正

- 修正近期遠端資料夾在未連線時無法開啟，以及切換主機後收到舊目錄結果的問題。
- 修正 Linux 讀取檔案誤觸發外部修改，以及遠端終端機快速輸入漏字的問題。
- 修正取消資料夾選擇後，背景 Session 更新反覆開啟選擇器或擅自切換資料夾的問題。
- 修正手動輸入 Windows 路徑的 WSL 轉換，以及切換主機／發行版後套用過期瀏覽結果的問題；其他發行版的 WSL 路徑會明確拒絕。
- 終端分頁的關閉按鈕現在會關閉對應 HERDR tab；操作失敗時保留分頁並顯示錯誤，不會只隱藏畫面。
- 修正 HERDR 不相容或快照讀取失敗時仍顯示「已載入快照／尚無 Space」；現在顯示 client／server 版本、protocol、選用路徑與恢復說明，主機與 Session 使用可讀名稱。
- 更新主機工具時保留原本的 managed HERDR 選擇，並在事件訂閱建立後重新讀取快照，避免遺漏連線期間的變更。

### 已知限制

- 此候選版完整平台與 Agent 驗收尚未完成，尚不可標示為完整替代版；正式發布前必須完成候選安裝包驗證。
- Beta 僅供手動下載，不提供 OTA，也不會取代 Stable Latest。HERDR／Browser 分頁的釘選狀態僅在本次應用程式工作階段保留。
- 舊 Windows 工作區需重新綁定 WSL2。舊 session 資料保留，執行中程序不會跨環境搬移；Agent 原生還原依官方整合支援。
- 不相容的既有 HERDR server 不會自動停止或重啟，請先保存工作再處理版本遷移。
- 已設定的 WSL／SSH 主機保留原 binary 路徑；升級後請從「新增資料夾」選取該主機，確認使用 Yuzora 隨附 HERDR，按「更新主機工具」套用新版 client。此操作不會自動停止既有 server。
- macOS Beta 沒有 Developer ID 發行者身分、notarization 或 Gatekeeper 信任，首次開啟時可能被警告或阻擋；只應從 Yuzora 官方 GitHub Pre-release 下載。
- Windows Authenticode 尚未啟用，首次開啟時仍可能出現 SmartScreen 提示。

## [0.0.9-beta.2] - 2026-08-28

### 改善

- macOS Beta 改為不需 Apple Developer Program 憑證的 unsigned 安裝檔；Stable 版仍保留 Developer ID 簽章與 notarization 要求。

### 已知限制

- macOS Beta 沒有 Developer ID 發行者身分、notarization 或 Gatekeeper 信任，首次開啟時可能被警告或阻擋；只應從 Yuzora 官方 GitHub Pre-release 下載。
- Windows Authenticode 尚未啟用，首次開啟時仍可能出現 SmartScreen 提示。

## [0.0.9-beta.1] - 2026-08-24

### 新增

- ADE 可從 HERDR 公告的 Agent catalog 在所選 Space 直接啟動 Agent，支援明確選擇已驗證的 bypass-permissions 旗標；建立失敗時只清理由本次操作新建的 tab。
- 命令面板可依 blocked、done、working、unknown、idle 的優先順序搜尋並跳至 HERDR Agents，也可快速切換 Spaces。
- Terminal 複製／貼上同時支援快捷鍵與系統 Clipboard event；當 Tauri clipboard service 暫時不可用時會改用 WebView clipboard fallback，尚未取得控制權的 HERDR terminal 不會把貼上內容送入 server。

### 改善

- Windows 版只會連線 Windows-native HERDR；Yuzora 會如實顯示 HERDR snapshot 與事件回報的所有 Agent 身分與狀態，並依 Windows `PATHEXT` 診斷 `.exe`、`.cmd`、`.bat`、`.com` Agent 啟動器，但實際啟動仍由 HERDR 驗證。
- New Agent 遇到新 pane 尚在初始化時，只會在 terminal、tab、Space 身分維持一致且前景程序仍是 shell 時短暫重試；若 pane 已被其他程序接管則會安全停止並回收本次新建 tab。
- HERDR 沒有任何 Space、連線失敗、停止或不可用時，ADE 會提供建立 Space 與開啟本機資料夾的明確入口；本機 Terminal 沒有 workspace 時會開啟資料夾選擇器，不再靜默無反應。
- 建立第一個 Space 僅要求 `workspace.create`，並在後續 snapshot 驗證成功才回報完成；終端連線、split resize 與接管控制都會依 HERDR capabilities 安全停用。
- Native Preview child webview 的開啟、尺寸、可見性與關閉操作會依最新工作區、overlay 與 focus 狀態序列化，避免 preview 蓋住應用程式 dialog 或回到過期位置。

### 修正

- 修正外觀主題色無法即時套用與保存、English 設定頁仍混入繁體中文，以及 Logs 大量結果缺少可存取分頁與無效日期提示的問題。
- 修正 Agent Inspector 缺少可發現入口與鍵盤關閉後未回焦、Git 使用者／日期篩選選單無法以 Escape 正常取消的問題。
- 修正 Preview 拒絕危險 URL scheme 時沒有錯誤提示，以及 external child webview 在切換 Space 或關閉分頁後可能殘留的問題。
- 修正 SSH 驗證方式的可存取名稱互換、首次連線 host key 無法接受或拒絕後長時間卡住，以及 SFTP 刪除缺少遠端路徑安全檢查的問題。

### 已知限制

- 此為 Beta GitHub Pre-release，僅供手動下載測試；不會成為 Latest Release、不提供 OTA、不會更新 stable `latest.json` 或產品頁固定下載連結。
- Windows-native HERDR 透過 `wsl.exe` 啟動的互動式 Linux shell，不保證能讓 HERDR 看見其隱藏的 Linux descendant Agent process；Yuzora 不會以自行推測的 Agent 身分取代 HERDR 回報。
- 正式發布的 macOS 安裝檔會由 fail-closed workflow 完成 Developer ID 簽章與 notarization；Windows Authenticode 尚未啟用，首次開啟時仍可能出現 SmartScreen 提示。

## [0.0.8] - 2026-08-15

### 新增

- 以 HERDR runtime 取代原本的 AgentZone：工作區側欄現在可直接查看 Spaces、具名 Sessions、Attention 與 Agents，並把指定 HERDR terminal 以可和檔案並存的頁面開啟。
- HERDR terminal 頁面支援建立、聚焦、重新命名、移動與關閉分頁，以及 pane 的分割、縮放、交換、關閉和配置調整；預設為觀察模式，需要時可明確接管輸入控制。
- 新增唯讀 Agent Inspector，可查看 agent 的狀態與輸出，而不會在檢視時改變 HERDR runtime。
- 新增工作區信任確認；執行指令、Git 寫入、預覽與遠端檔案操作會在 native backend 再次檢查授權、路徑與資源限制。
- Git 分支選擇器新增 Local、Remote 與 Tags 分類、搜尋與狀態標記，支援 detached checkout、從 tag 建立 branch，以及大量變更的多選與批次操作。
- App 內的一般 Dialog 可依各自用途調整大小並保留偏好；Alert 類確認視窗仍維持精簡尺寸。

### 改善

- Git、SSH／SFTP、資料庫、LSP 與預覽等耗時操作加強取消、逾時、輸入大小與並行數限制，降低大型工作區或異常遠端回應拖慢介面的風險。
- Git revision 與檔案操作改用更嚴格的 OID、literal path 與 repository 邊界檢查，避免 branch、路徑或 symlink 被誤當成命令選項或越過工作區範圍。
- LSP 下載改由內建完整性 catalog 驗證來源、內容與 provenance；找不到可信 artifact 時會拒絕安裝，而不是繼續使用未驗證內容。
- Markdown 預覽改為與來源文件相鄰的連結頁面，並改善分頁拖曳排序、檔案拖放、圖片／SVG 預覽與工作區切換後的狀態恢復。
- Git 變更清單、diff、log 與 branch 操作改善大量資料下的載入、選取、虛擬化與 stale response 防護。
- 更新 Tauri、React、Vite、CodeMirror、資料庫／SSH 函式庫與其他開發相依套件。
- 重構產品網站與中英文導覽素材，套用目前的 Yuzora Logo，並改以更精簡的 ADE × HERDR 故事、light/dark theme、響應式功能展示與新版 Git／Terminal 影片介紹目前工作面。
- Windows 上的 HERDR public API 與事件訂閱改用 HERDR 0.8.0 的 named-pipe transport；當 server、protocol 與 schema 相容時，可使用 snapshot、runtime mutation 與即時事件，不再因作業系統固定停用。
- macOS 與 Windows 安裝檔內附經 SHA-256 固定的 Yuzora-managed HERDR 0.8.0 相容 binary；預設優先使用 PATH 全域安裝版，偵測不到時會自動退回內附版本。Windows 內附官方 protocol-19 preview package 與其 ConPTY runtime。

### 修正

- 修正開發模式重新載入時，Tauri event listener 清理可能拋出錯誤並略過 backend unlisten，造成殘留 listener 的問題。
- 修正 HERDR terminal 串流在第一次 flush 後可能停止更新，以及 snapshot／capability 暫時失敗後無法恢復或隱藏錯誤的問題。
- 修正 Git diff、log、branch checkout／cherry-pick 與工作區切換時，較舊的非同步結果可能覆蓋目前 repository 狀態的競態問題。
- 修正資料庫查詢取消、SSH／SFTP 傳輸、askpass 與預覽資源生命週期中的多個清理、逾時與錯誤回報問題。
- 修正提早釋放分頁結果後，下一次 MSSQL 查詢可能因 helper 控制訊息被中斷而失敗，以及無資料列的 stored procedure 回傳被誤判為串流結尾的問題。
- 修正 macOS 因系統拒絕有限的 process memory rlimit，導致 PostgreSQL／MSSQL query helper 無法啟動的問題；改以短週期 resident-memory watchdog 保留隔離與上限保護。
- 修正 Windows 工作區信任與路徑能力使用不穩定 metadata identity API，導致無法以 stable Rust 建置的問題；改由已開啟的檔案 handle 取得穩定 identity。
- 修正 HERDR／Markdown pseudo pages 被誤存成一般檔案分頁，導致重新啟動後嘗試以檔案方式還原的問題。
- 修正 Windows canonical path 的 `\\?\` 前綴出現在 HERDR 診斷與工作區信任介面；顯示路徑會正規化，但 backend 仍使用原始 canonical identity 執行授權與撤銷。

### 已知限制

- Yuzora 桌面版目前只發佈 macOS 與 Windows 安裝檔；Linux 僅作為 CI／測試 host，不是支援的桌面發佈平台。
- HERDR 功能取決於實際選用 binary 的版本、protocol 與 schema；不相容或缺少必要 method 時會停用對應操作並顯示原因。
- macOS 上以 ⌘Q 結束應用程式時不會提示未儲存的變更；請改以關閉視窗的方式離開。
- macOS 與 Windows 安裝檔目前尚未完成作業系統簽章，首次開啟時可能出現 Gatekeeper 或 SmartScreen 提示。

## [0.0.7] - 2026-07-26

### 改善

- 其餘所有背景操作（開啟工作區、檔案清單讀取、日誌記錄、語言伺服器設定存取、Agent 執行環境偵測、搜尋啟動等）移出主執行緒，消除最後一批可能造成介面瞬間卡頓的來源。加上 0.0.6 的工作區切換改善，現在除了必須在主執行緒執行的原生預覽視窗操作外，所有背景工作都不會佔用介面執行緒。

### 已知限制

- macOS 上以 ⌘Q 結束應用程式時**不會**提示未儲存的變更。此路徑受上游框架限制，應用程式收不到任何可攔截的事件；請改以關閉視窗的方式離開。
- macOS 與 Windows 安裝檔目前尚未完成作業系統簽章，首次開啟時可能出現安全提示。

## [0.0.6] - 2026-07-26

### 新增

- 關閉視窗時若有未儲存的變更，會先詢問要儲存、不儲存或取消；選擇儲存但寫入失敗時不會關閉，內容不會遺失。
- AgentZone 找不到可用的 JavaScript 執行環境時，改為在啟動 Agent 之前顯示安裝指引，逐一列出 Bun、Deno 與 Node.js 的偵測結果與安裝連結，不再只給一句錯誤訊息。
- Logs 匯出可選擇去識別化，移除完整路徑、主機與 IP、帳號、SSH 指紋與憑證後再交出，方便附在公開的問題回報。
- 診斷記錄可依「每次啟動」分組檢視與篩選，匯出時另附各次啟動的摘要（版本、平台、起訖時間、效能峰值），便於回報時對齊當下狀況。

### 改善

- 切換工作區時介面不再卡頓：切換過程中視窗維持可操作，不會出現整體凍結。
- 切回開過的工作區時，git 面板不再從空白開始——會先顯示離開時的內容，背景更新完成後再換成最新狀態。
- 檔案樹的展開狀態與捲動位置會跨工作區切換保留，切回去不必重新一層層展開。
- 切回工作區會恢復先前開啟的編輯器分頁。
- 首次開啟工作區時各面板並行載入，不再逐一等待。
- 終端機在高速輸出（例如 `yes`、`find /`）下大幅減少送往介面的事件量，同時輸入的回顯延遲明顯降低；輸出量超過上限時會顯示明確的截斷提示而非靜默丟棄。
- 狀態列的系統佔用改為顯示「應用程式本體與其管理的背景程序」的總量，滑鼠停留可看到本體與子程序的分解。
- 終端機分頁新增總數上限，達到上限時顯示明確訊息並指出要到哪裡關閉，而非靜默失敗。
- Agent 首次啟動需要下載外部 adapter 時，不再被誤判為連線逾時。
- 內建與社群的 ACP adapter 改為釘選版本，避免每次啟動抓到不同版本造成行為不一致。

### 修正

- Windows 上路徑含空白的執行檔（例如安裝在 `Program Files` 的工具）不再因為引號處理而啟動失敗。
- Agent 連線失敗後不會再誤關仍在使用中的工作階段；重試改為逐次拉長間隔，不再無限重試。
- 修正診斷記錄匯出遇到非 ASCII 內容（中文、箭頭、表情符號）時會中斷並清空目標檔案的問題。
- 修正 Agent 啟動失敗時可能把環境變數的值當成執行環境名稱寫進記錄的問題。

### 已知限制

- macOS 上以 ⌘Q 結束應用程式時**不會**提示未儲存的變更。此路徑受上游框架限制，應用程式收不到任何可攔截的事件；請改以關閉視窗的方式離開。
- macOS 與 Windows 安裝檔目前尚未完成作業系統簽章，首次開啟時可能出現安全提示。
- Windows 強制中止或已脫離應用程式管理的外部子程序，仍可能需要作業系統自行完成回收。

## [0.0.5] - 2026-07-23

### 新增

- Terminal 字體大小可在設定中調整，已開啟的本機與 SSH 終端機會立即套用並保留設定。

### 改善

- 隱藏的 Terminal 會限制暫存輸出並在重新顯示時補上內容，降低多個背景終端機持續輸出造成的介面卡頓。
- Terminal 內容區不再顯示右鍵選單，讓滑鼠操作可交給互動式 TUI；Terminal 分頁的重新命名與關閉選單仍然保留。
- 只有實際進入 Agent 功能時才準備外部 ACP 程序，單純開啟或切換工作區不再自動啟動。

### 修正

- 關閉應用程式時會主動結束 Terminal、Agent、LSP、Git、SSH 與預覽服務的背景程序，降低 Windows 關閉後仍有程序殘留的情況。
- Windows 的背景程序與 WSL profile 探測不再彈出額外的命令列視窗。

### 已知限制

- macOS 與 Windows 安裝檔目前尚未完成作業系統簽章，首次開啟時可能出現安全提示。
- Windows 強制中止或已脫離應用程式管理的外部子程序，仍可能需要作業系統自行完成回收。

## [0.0.4] - 2026-07-23

### 新增

- Windows 可在 Terminal 設定中選擇命令提示字元、Windows PowerShell、PowerShell 7、WSL 預設環境或已安裝的 WSL 發行版。
- 新增 Terminal profile 下拉選單；建立終端機時可以臨時改用其他 profile，不會改變預設設定。
- 自訂 Terminal profile 可分別設定執行檔、每行一個啟動參數，以及原生 Windows 或 WSL 工作目錄策略。

### 改善

- 改善 Windows 中文輸入法在 Terminal 中遺失、重複送出或組字位置不正確的問題。
- Windows 可切換游標跟隨或固定 TUI 輸入框模式，讓全螢幕終端程式中的中文組字位置保持可見。
- 從 WSL 網路路徑開啟工作區時，會使用相符的發行版與 Linux 工作目錄啟動 Terminal。

### 已知限制

- macOS 與 Windows 安裝檔目前尚未完成作業系統簽章，首次開啟時可能出現安全提示。
- Windows 中文輸入法候選視窗仍可能受輸入法與 WebView2 版本影響；遇到定位不穩定時，可在 Terminal 設定改用 TUI 輸入框。

## [0.0.3] - 2026-07-22

### 新增

- AI agent 對話介面全面改版：工具活動聚合為可展開的步驟鏈，訊息支援表格、巢狀清單等完整 Markdown 格式，程式碼區塊可一鍵複製。
- Pi 內建執行環境隨應用程式出貨，不需另外安裝社群轉接器；設定中可一鍵切換回社群版本，既有對話在兩種環境都能續聊。
- Agent 的提問（選項、確認、文字輸入）以互動表單呈現，可以直接作答，不再只能取消或卡住。
- 子代理（sub-agent）呼叫有專屬卡片，顯示代理類型與任務內容；Claude 子代理的工具過程以縮排嵌套呈現。
- 對話中即時顯示 context 用量與累計費用；輸入框上方彙總本回合的檔案變更行數。
- 模型與思考深度選單改為可搜尋，執行期間也能切換；agent 執行中可直接送出下一句補充指示。
- Agent 對話支援 Pi 內建指令（/compact、/session、/name 等）。
- 「關於與更新」現在會列出目前版本及可用更新帶來的主要改變。

### 改善

- 應用程式內、GitHub 下載頁面與更新通知會顯示一致的版本說明。
- Terminal 的輸入法組字視窗會跟隨實際輸入位置。
- Changelog 改用白話整理功能、改善與已知限制，不再列出內部開發細節。

### 已知限制

- macOS 與 Windows 安裝檔目前尚未完成作業系統簽章，首次開啟時可能出現安全提示。
- Windows 上 AI agent 功能尚有已知問題，將於後續版本處理。

## [0.0.2] - 2026-07-16

### 新增

- 可直接在設定中檢查、下載並安裝新版本。
- 「關於與更新」會顯示應用程式實際執行的版本。

### 改善

- 下載更新時會顯示進度；失敗後可直接重試，不必重新檢查版本。
- 安裝前會提醒儲存尚未完成的文件，並在使用者確認後才重新啟動。
- Windows 自動更新統一使用 MSI 安裝格式，降低不同安裝方式混用造成的問題。
- 更新入口只顯示在設定中的「關於與更新」，不會打斷日常工作。

### 已知限制

- macOS 與 Windows 安裝檔目前尚未完成作業系統簽章，首次開啟時可能出現安全提示。

## [0.0.1] - 2026-07-13

### 新增

- 在同一個工作區中使用 AI agent、程式碼編輯器、Terminal、Git、SSH 與資料庫工具。
- 支援程式碼診斷、補全、格式化，以及 Markdown 即時預覽。
- 可保存最近使用的工作區與 agent 對話，重新開啟後繼續工作。
- 可查看 Git 變更、提交紀錄與分支，並在執行具風險的操作前取得提醒。
- 提供本機網站預覽、開發伺服器偵測與應用程式 Logs。

### 修正

- 改善從 Finder 或 Dock 開啟時，AI agent 無法取得登入資訊的問題。
- 改善 Windows 工作區與語言工具的路徑相容性。
- SSH 連線失敗時會留下可供排查的紀錄。

### 已知限制

- macOS 與 Windows 安裝檔目前尚未完成作業系統簽章，首次開啟時可能出現安全提示。
