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
