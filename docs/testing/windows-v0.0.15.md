# Windows v0.0.15 候選版驗收清單

狀態：待使用者實機測試。請使用 release PR **最新 head** 的 `yuzora-release-candidate-windows-x86-64` artifact，內含 NSIS `setup.exe` 與 MSI；每次修正後更換候選檔與 SHA。候選版停用 updater，不驗證正式 OTA。

## 測試資料與回報

- Windows 版本／build、CPU 架構、螢幕縮放比例、WebView2 版本：
- 安裝器類型（NSIS／MSI）、檔名、SHA-256：
- Release PR URL、CI run ID、candidate head SHA／tree SHA：
- 升級前 Yuzora 版本、HERDR client／既有 server 版本：
- WSL distro／版本、工作區（本機／`/mnt/c`／`/mnt/d`／`/home`／SSH）：
- 每項填：通過／失敗／不適用（原因）；失敗附步驟、時間、截圖或錄影與錯誤文字。

SHA-256：`Get-FileHash -Algorithm SHA256 "安裝器完整路徑"`。從本機 shell 執行 `wsl --list --verbose` 可列出 distro。

HTML 資料位於 `fixtures/windows-candidate/`；請將該目錄獨立複製到各測試位置並開啟為工作區。Git 操作使用可丟棄的測試 clone；fetch／checkout／建立分支都要从**底部狀態列分支按鈕**進入。完成安裝前按安裝程式提示處理正在使用的 HERDR，避免中斷需保留的工作。

## 優先阻擋案例

| ID | 操作 | 通過條件 | 結果 |
|---|---|---|---|
| I1 | NSIS 從 0.0.14 升級後啟動；另於測試機／VM 驗證 MSI | 版本為 0.0.15，現有設定與工作區保留，能開啟本機 HERDR；記錄 SmartScreen 提示 | 待測 |
| H1 | HERDR 設定／診斷確認隨附 0.9.1，連線既有 0.9.0 Session | 相容性通過，能輸入、接收輸出、resize；App 不擅自停止舊 server | 待測 |
| H2 | 以 0.9.1 建立隔離 Session，執行一般 shell 與常用 Agent/TUI | 啟動、輸入、輸出、捲動正常；重啟 App 可重新連回 | 待測 |
| R1 | 輸出 400 行，反覆調整視窗、字級、側欄及分割 pane | ROW_400 與 prompt 留在預期末行，順序不亂、沒有瞬間縮至極窄畫面或跳到頂端 | 待測 |
| R2 | 一般 shell 與常用 Agent/TUI 各持續使用至少 10 分鐘；交替捲動、分割與切頁 | 無非預期閃爍、收縮、游標／捲動跳動；若發生需錄影並記錄同時進行的操作 | 待測 |
| F1 | Terminal 是目前頁籤時，Alt+Tab 切離並返回；在左右 pane 分別操作 | 直接輸入到原作用中 pane，不需再點擊，不輸入到另一個 Session | 待測 |
| F2 | 新建 Terminal，分別完成命名及取消命名；建立期間切到其他檔案或搜尋輸入欄 | 留在新 Terminal 時自動可輸入；已切頁／其他輸入欄／對話框時不搶焦點 | 待測 |
| W1 | WSL `/mnt/c` 新建中文與空白檔名，輸入、存檔、重新開啟，連續修改存檔至少 10 次 | 無「已寫入卻報錯」；內容完整且 dirty 標記正確消失 | 待測 |
| W2 | `/mnt/d` 重做 W1；`/home` 作對照；有第二個 distro 時也重做 | 各位置結果一致；不存在 D 槽記不適用 | 待測 |
| W3 | Yuzora 保留未存檔修改，再由另一工具改同檔並返回；另測不可寫入目錄／遠端斷線存檔 | 真衝突／失敗清楚提示且保留 dirty；不丟失較新的編輯內容；恢復後能成功存檔 | 待測 |
| G1 | 底部 Git 視窗連續 fetch、切換既有分支、建立新分支，再切回 | 每次只執行一次，分支清單及狀態刷新正確；用 `git branch --show-current`、`git status --short` 核對真值 | 待測 |
| G2 | fetch／切分支進行中切工作區；另測無效 remote 或會覆蓋本地修改的 checkout | 舊錯誤不出現在新工作區；真實失敗保留提示；恢復後可重試；刷新失敗不能把已成功寫入說成失敗 | 待測 |

PowerShell 400 行：

```powershell
1..400 | ForEach-Object { "ROW_{0:D3}" -f $_ }
```

WSL shell 400 行：

```bash
for i in $(seq 1 400); do printf 'ROW_%03d\n' "$i"; done
```

## 頁籤與輸入

| ID | 操作 | 通過條件 | 結果 |
|---|---|---|---|
| K1 | 0／1／9／超過 9 個頁籤按 Ctrl+1～9；Ctrl+Tab／Ctrl+Shift+Tab 循環 | 數字鍵選第 N 個；9 是第 9 個，不存在則不動作；循環方向正確 | 待測 |
| K2 | 釘選頁籤、改順序、雙分割區；在編輯器／Terminal／Browser 文件內重做 | 依作用中分割區的可見順序，包含釘選；Browser 有焦點也可切頁 | 待測 |
| K3 | 設定自訂快捷鍵，重啟；於快捷鍵錄入、對話框、Microsoft Pinyin 組字時按快捷鍵 | 設定可保存；錄入／對話框／IME 不切頁，組字不重複或遺失 | 待測 |
| K4 | 嘗試切到離線／啟用失敗的 HERDR 頁籤，再立刻選另一頁 | 失敗回復不覆蓋最新選擇；正常頁籤仍可操作 | 待測 |

## HTML Browser 與元素複製

| ID | 操作 | 通過條件 | 結果 |
|---|---|---|---|
| B1 | 檔案樹、檔案頁籤、編輯器右鍵開啟「在此預覽」；測 `.html` 與 `.htm` | 三個入口皆使用既有 Browser；非 HTML 不出現此入口 | 待測 |
| B2 | 開啟 `預覽 測試.html` 和子目錄 HTM | 中文／空白路徑正確；根路徑 CSS、相對 JS module/import、SVG 都載入 | 待測 |
| B3 | 將同份 fixture 放至本機、WSL `/mnt/c`、`/home`；有 SSH 環境時加測 | 內容取自正確主機與工作區，不誤讀本機同名路徑 | 待測 |
| B4 | 改文字／CSS，先不存再存；快速連續存檔；按手動重整 | 未存檔不更新；成功存檔後顯示新內容；不自動存 dirty 檔；手動重整可用 | 待測 |
| B5 | 關閉 Browser 再由右鍵重開；切工作區再返回；遠端斷線、重連再重開 | 不出現持續的 resource expired；新工作區不顯示舊檔，重連後可讀取 | 待測 |
| B6 | 正常資源可用後按 fixture「越界資源」按鈕；上一層放假 outside.txt | 越界請求被拒絕，不能取得上一層檔案內容 | 待測 |
| B7 | 遠端啟動使用者管理的 localhost 服務並在 Browser 開啟；前進／返回／重整 | 仍走 Host tunnel，HTTP／常用 WebSocket 互動正常 | 待測 |
| E1 | 啟用選取元素，移動到 card 後點擊，貼到文字編輯器 | 外框正確、複製成功提示；內容含來源、selector、文字、HTML、尺寸與樣式 | 待測 |
| E2 | 選同源 iframe 按鈕、open shadow DOM 按鈕；另用可嵌入的跨來源 iframe | 可選同源及 open shadow 子元素；跨來源只選 iframe 自身 | 待測 |
| E3 | 選取模式按 Esc（toolbar 焦點／Browser 焦點各一次）；選取中導覽或切工作區 | 選取取消，不誤複製；新頁不套用舊元素結果 | 待測 |
| E4 | 先修改假表單值，選 form-card；展開並選超大元素 | 複製內容不含密碼、即時表單值與 textarea 內容；超大內容受限且 App 可繼續使用 | 待測 |

## 安裝後回歸與結束條件

| ID | 操作 | 通過條件 | 結果 |
|---|---|---|---|
| S1 | 開既有 repo 與非 Git 資料夾；檔案編輯／搜尋／Markdown／SVG | 正常開啟，無因版本更新丟設定／資料 | 待測 |
| S2 | 有環境時 smoke test SSH/SFTP 與常用 Database | 連線、讀取、編輯／查詢及取消正常 | 待測 |
| S3 | 完成測試後關閉 App，再檢查原 HERDR／Agent | 釋放 App 自己的連線，原 HERDR／Agent 保留，重開可連回 | 待測 |

合併前需回報每項結果；不適用要寫原因，失敗要附重現證據。回報到 release PR 時附上述 candidate SHA、tree SHA、run ID 與 installer hash。**測試通過與授權 merge 請分別明確表達**；本清單不是預先核准 merge 或發布。
