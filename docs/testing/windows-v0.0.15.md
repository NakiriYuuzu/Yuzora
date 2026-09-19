# Windows v0.0.15 候選版驗收清單

狀態：2026-09-19 接續驗證仍有失敗，完整矩陣尚未完成。請使用 release PR **最新 head** 的 `yuzora-release-candidate-windows-x86-64` artifact，內含 NSIS `setup.exe` 與 MSI；每次修正後更換候選檔與 SHA。候選版停用 updater，不驗證正式 OTA。

## 2026-09-19 修補與證據界線

以下修補新增於本次候選迭代，已安裝的 `c6fcb16` 候選不包含它們；完整候選驗收仍未通過。

- **R1 字級**：在 Windows App 背景操作中，以 `seq 1 400` 建立基準，`12 → 14 → 26 → 10` 在 10px 重現整行擠至左側。將正式 HERDR 元件與固定 frame 放入 Windows WebView2 153，也能重現；不經 HERDR 的直接 xterm 字級更新同樣失敗。26px 的五字元實際寬約 114px、格線預期約 78px；10px 字距約 −9.59px。原因是隱藏量測節點同步回傳前一字級的寬度，錯誤值被快取。Bun 的 xterm 6.0.0 補丁在字型改變時重建小型量測子樹，不重建 visible terminal 或 Session。正式補丁在 Windows WebView2 的完整元件重跑九次字級循環後，字寬與底部 marker 全數通過。這仍不等同於新安裝包的完整 R1／R2 驗收。
- **F1 返回 App**：native activation 即使 `document.hasFocus()` 為 true，也會先檢查原生視窗仍作用中，再要求 main WebView 取得鍵盤焦點。DOM focus 事件不重複觸發 native focus，保留切頁／其他輸入／對話框的取消條件。這個分支已有回歸測試，修補後的 Windows 原生視窗仍需換候選驗證。
- **F2 命名／信任對話框**：命名輸入框仍在卸載時等待，不把它誤判為使用者選中的其他欄位；信任提示允許或取消後，依提示開啟前的頁面身分恢復焦點。切頁、改用其他輸入欄，以及信任期間換頁都有保護測試。修補後的 Windows 安裝包仍待測。
- **B8 原生層級**：未儲存確認 store 納入 Browser overlay gate；新增已開啟／開啟中兩種情境，均驗證隱藏以及取消後恢復。原版的兩個案例先失敗，修補後通過。安裝包仍需重測原生對話框可見性。

檢查：`bun install --frozen-lockfile`、238 files／2,867 tests、typecheck、build 通過；lint 0 errors／52 warnings，與本輪修補前相同。未改 Rust／Host，本輪沒有重跑其完整測試。永久瀏覽器回歸入口：[`fixtures/xterm-font-regression.html`](../../fixtures/xterm-font-regression.html)，檢查實際 regular／bold／italic／bold italic／中文字寬與底部 marker；Windows WebView2 與 Codex Browser 均通過，移除補丁的同一 fixture 在 Windows 26px 確實失敗。必須由 Vite 或靜態 bundle 載入，不使用 jsdom 判斷視覺結果。補丁維護說明：[`patches/README.md`](../../patches/README.md)。

## 2026-09-19 Windows App 接續驗證

候選 head `c6fcb16aa58cf9dd8fce2d3879108a9a5a7deb0b`、CI `35344294526` 仍未通過驗收，完整矩陣尚未完成。本次透過 Windows App 背景操作重新確認：從 Windows 工作列切離並返回後，Terminal 必須再次點擊才可輸入；新開 WSL 工作區、完成信任對話框後也有相同焦點問題。前段字級切換的渲染失敗仍待修復與受控重測。

另新增 B8：Browser 為作用中頁籤時，關閉另一個 dirty 檔案，原生 Browser 會蓋住儲存確認對話框；Esc 可取消並保留修改，先切至該檔案再關閉可避開此問題。

有限檢查已確認 Git popup fetch／建立分支／切回 main 的一輪操作、WSL `/mnt/c` 空白檔名的兩次儲存、檔案樹／編輯器 HTML 預覽、成功儲存後更新及基本元素上下文複製可運作。這些結果不代表相應整列案例全數通過。背景 RDP 的快速輸入、直接修飾鍵與中文輸入不可靠，未送達的快捷鍵不判定為產品缺陷。

詳細結果保存於本次執行輸出 `output/windows-acceptance-2026-09-19/report.md` 與 `results.json`（非版本控制檔案）；下方尚未完整覆蓋的矩陣維持待測。未合併 PR #107 或發布版本。

## 2026-09-18 首次回報與重測

先前提供的候選來源為 `4a601ecce337f7ce227378b48adb4526f9b5187a`、CI run `35306428894`；使用者尚未回報實際安裝器 hash 與系統版本。以下只記錄使用者確認的範圍，不代表完整細項已驗收。

| 類別 | 使用者結果 | 下一步 |
|---|---|---|
| Terminal 渲染 | 一般操作 OK，但切換文字大小時異常嚴重 | 阻擋；新候選重測 R1／R2 |
| Terminal 焦點 | 切回應用程式後仍不聚焦 HERDR | 阻擋；新候選重測 F1，另確認 F2 |
| HERDR 相容性 | 可以 | 記錄相容性通過；H1／H2 詳細環境與步驟待補 |
| WSL 存檔、Git、快捷鍵、HTML／元素複製 | 未測試 | W1～W3、G1～G2、K1～K4、B1～E4 維持待測 |

優先重測時，先輸出 400 行，反覆切換字級 `14 → 26 → 10 → 30 → 14`，在一般 shell／常用 TUI 及分割 pane 各測一輪。調整過程與完成後都需檢查末行／prompt，不能只檢查最終畫面。接著讓作用中 pane 可直接輸入，Alt+Tab 切離並返回，確認無需點擊即可繼續輸入；搜尋欄、命名對話框或 Browser 為目前輸入位置時，也需確認不搶焦點。

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
| B8 | Browser 作用中，關閉另一個 dirty 檔案；分別取消、儲存、捨棄，再測 Browser 尚在載入時開啟確認框 | 確認框不被 native Browser 蓋住；取消保留 dirty；關閉確認框後 Browser 正常恢復 | 舊候選失敗；修補後安裝包待測 |
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
