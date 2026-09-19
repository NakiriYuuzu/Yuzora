# Windows v0.0.15 候選版驗收清單

狀態：2026-09-20，一般欄位、Browser 表單切回與游標保留，以及重啟後點頁籤已有安裝版定向通過。3736d25 實機確認首次命名焦點仍失敗，追加修補等待選單退出動畫完成才開始建立 Terminal；新候選安裝驗收待完成。完整矩陣尚未完成。請使用 release PR **最新 head** 的 `yuzora-release-candidate-windows-x86-64` artifact，內含 NSIS `setup.exe` 與 MSI；每次修正後更換候選檔與 SHA。候選版停用 updater，不驗證正式 OTA。

## 3736d25 實機結果與選單退出動畫修補

CI `35459830057` 首次 Windows HERDR 契約測試出現 `pane.scroll authoritative range mismatch`，相同 SHA 重跑失敗 job 後通過；原因尚未確認，未放寬斷言。Windows artifact `10590310306`；NSIS SHA-256 `b8ba87a091295b9f648b8a49019b71a8bafcea07c3125da8f46abb0b4199ddf9` 在 Windows 核對後安裝。仍保留三個已重新比對雜湊相同的使用中 runtime 檔案。

- 重啟後點既有 Terminal 直接收到 `#tab3736`，工作列切回直接收到 `#return3736`；取消命名後直接收到 `#cancel3736`。
- 從加號建立 Terminal，不點欄位直接輸入 `qa3736a` 未出現；按一次 Tab 後 `x` 可取代預設名稱。上一版 close-autofocus guard 不足以修復實機，不計通過。
- 使用 Codex 內建瀏覽器載入真實 TabBar／DropdownMenu／TextInputDialogHost 與產品 CSS，確認 input 取得焦點後，退出動畫中的 menu 又取得焦點，卸載後落到 body。舊 jsdom 測試未涵蓋 CSS 動畫。
- 修補將建立操作移至 menu close-autofocus 完成後；這段期間若 Session／Space 改變則取消。動畫生命週期回歸在旧碼失敗；修補後含快速／延遲建立及工作區切換共 54 項相關測試通過。相同瀏覽器流程可直接輸入並取代預設名稱，Windows 新安裝包仍待驗證。

8a2f91c 後續亦已完成 `/mnt/c` 中文／空白新檔十次儲存與重新開啟；隔離 `/home` repo 的不可寫入失敗保留 dirty、恢复權限後存檔成功；Git 真實 checkout 衝突、fetch 錯誤跨工作區隔離與恢復；0／1／9／10 頁籤、pin 與雙分割數字鍵。詳細證據保留於分版本驗收報告，不等同完整矩陣通過。

## 8a2f91c 安裝版結果與命名首次焦點修補

CI `35454735412` 成功，Windows artifact `10587768685`；Windows 核對 NSIS SHA-256 `c4d18174c256f6aa18aff783de040d2e3fa9159579d76a3eb80f2c0e71b70f27` 後安裝。

- Browser textarea `ab` 切回後接續 `cd` 得到 `abcd`；游標左移兩格再切回加 `x` 得到 `abxcd`。文字 input 接續 `xy`，Browser 可見時主介面篩選欄接續 `qa save`，彼此不搶焦點。重啟後點 qa958 頁籤直接輸入 `#tab8a`，工作列切回直接輸入 `#return8a`。
- `/mnt/c/Users/Yuuzu/qa0919/qa save.txt` 十次增量儲存，dirty 每次消失且沒有錯誤，`cat` 核對為 `1234567890`；中文新檔、不可寫入與斷線仍待測。
- 選取修改過的假表單 `#form-card`，輸出移除 password／input 值與 textarea 內容。超大 `#large-element` 的尺寸為 800×55677，複製結果有 `[HTML truncated]`、樣式與來源；保存後核對為 29,263 字元／53,832 UTF-8 bytes，App 可繼續操作。Esc 後一般按鈕恢復互動；不同焦點與導覽競態尚未全測。
- 最新安裝版 ping／top 抽樣自 01:39:23 至 02:00:10，至少 20 分 47 秒；字級 12→28→10→12、分割 50/50→35/65→65/35→50/50，未見擠字或錯行。右 pane 切回後直接 `h` 開啟 top help，Esc 可回復。背景 RDP 滾輪未可靠送達，不計本輪捲動通過；抽樣不能排除每一個瞬時閃爍。
- Git 底部入口 fetch、main→qa-git→新建 qa-git-8a→main，清單與 `git branch`／`git reflog`／`git status -sb` 一致。同名分支的真實錯誤有提示，重新讀取後可恢復；無效 remote、寫入中切工作區與刷新失敗仍待測。
- 命名完成及取消後直接輸入 Terminal 通過；從加號新增時，命名框首次不能輸入重現兩次。真實 TabBar／DropdownMenu／TextInputDialogHost 回歸確認，選單延遲卸載後焦點落到加號 trigger。修補僅在命名請求仍開啟時取消選單的 close autofocus；涵蓋建立回應早於／晚於選單卸載，以及一般 Esc 關閉仍返回 trigger。相關 47 tests 通過，安裝版重測仍待完成。

此輪未停止原有 pi 或其他 Session。同版本 NSIS 仍只保留三個先前已核驗相同的 locked runtime 檔案，不計乾淨升級／MSI 通過。

## e89e916 安裝版結果與 Browser 焦點追加修補

候選 `e89e9161f8674929f91662de89e9dc6b13ccf8b2`、CI `35449597731` 全數成功，Windows artifact `10586674246`；Windows 端核對 NSIS SHA-256 `231be11c7bfa74e288c50ae8d7ffaac5e23ff65dc095dc62909752a131722681` 後安裝。

- 一般檔案篩選欄 `focus` 切至 Chrome 再返回，直接接續成 `focus.txt`；重啟後點既有 Terminal 頁籤直接收到 `#e89tab`，工作列返回直接收到 `#return89`。
- 命名兩輪切回保留文字與中間游標：`qa89` → `qa89def` → `qa89xdef`，完成命名後 Terminal 直接收到 `#named89`。
- WSL `/home` 中文／空白 HTML 的 CSS、相對 JS module/import、SVG 載入；主頁／子目錄 HTM 連結及 Back／Forward 直接顯示，無需手動重新整理。
- ping／top 抽樣觀察自 9 月 19 日 23:43:46 至 9 月 20 日 00:06:39，至少 22 分 53 秒。字級 12→27→26→10→30→14→12 未見擠字／錯行。抽樣不能排除所有瞬時閃爍；本候選的捲動操作曾誤碰 splitter，不能算乾淨的 scroll 通過證據。已恢復 12px 與 50/50 分割。
- 原有 Session 持續執行；同版本 NSIS 安裝仍只保留三個先前已核驗相同雜湊的 locked runtime 檔案，不能算乾淨升級或 MSI 通過。

Browser 表單另有可重複失敗：textarea 先輸入 `ab`，工作列 Chrome → Yuzora 後 `cd` 不出現，點欄位再輸入則成功；文字 input 先加 `x`，切回後 `y` 也不出現。主 WebView 的欄位恢復不涵蓋原生 child WebView。追加修補在 `WM_ACTIVATE` 失活前記住取得鍵盤焦點的 HWND，啟用後只恢復仍可見且仍屬於目前前景視窗的子視窗，不重新選取 DOM 欄位。Windows 原生測試涵蓋恢復、隱藏及銷毀目標；是否修復 WebView2 表單仍以新安裝版重跑為準。

## HTML 原生導覽追加修補

a186b41 的 WSL `/home` 隔離 fixture 已確認中文／空白 HTML、根 CSS、相對 JS module/import、SVG 與同源 iframe 正常載入。但點子目錄 HTM 連結後 Browser 空白，手動重新整理才恢復；返回主頁亦重現。原因是文件路徑改變時，資源解析 hook 短暫回傳 null，Panel 關閉已完成導覽的 native child；後續原生導覽同步又抑制重開。檔案 URL 改以工作區資源識別維持 lease，路徑／query／hash 分別映射；工作區、capability、Host generation 與 reloadNonce 仍使舊結果失效。真實 Panel／hook／store 的回歸先確認錯誤呼叫 previewClose，修補後 Browser 相關 68 tests 通過。

## a186b41 安裝後追加發現

已核對並安裝 a186b41（CI 35445637065；NSIS SHA-256 `52c9fd364aa6c7ba18634c26101a0178ac511bda263d293a112fe3602e2ceaf2`）。重啟後首次點既有 Terminal、再次點已作用中頁籤，均可直接輸入。一般篩選欄切回可接續文字，第二輪保留中間游標；取消命名後也可直接輸入 Terminal。既有 ping／top 在 App 更新前後持續執行。

命名對話框切回仍有阻擋問題：連續輸入 `def` 最後僅剩 `f`，再次點欄位後恢復正常。Windows WebView 的 GotFocus 被當成外層視窗啟用，完成 setFocus 後的延遲事件會再次要求焦點；命名欄每次 onFocus 又全選內容。追加修補只由 Windows HWND 啟用事件要求 native focus，合併重複啟用通知，命名初始值僅首次 focus 全選。三個回歸案例先失敗，相關 84 項測試通過；修補仍須新候選版重測。

長時間觀察：95807f8 的 top／ping 已連續觀察至少 20 分 37 秒，期間字級 12→27→26→10→30→14→12、pane 比例 50/50→35/65→65/35→50/50，未見原先擠縮／錯行；捲動停留可維持並回到底部追蹤。a186b41 重新連線 22:13:49 至 22:25:40 的抽樣畫面亦正常。這是抽樣觀察，不能排除每一個短暫閃爍；完整矩陣與新焦點修補驗收仍未通過。

## 一般欄位與重啟後頁籤焦點

95807f8（CI 35439917925）已實際安裝，Windows 端 NSIS SHA-256 為 `b3ac9bece39cd7eecc00f7c7ce6555d8af7a4f41a15fa030191c536048aa9e2e`。工作列 Chrome → Yuzora 的單 pane 兩輪、左右 pane 各一輪均可直接輸入到正確 pane；命名／取消後直接輸入與 Browser 上確認框的取消／捨棄亦通過。原 Session 與 400 行歷史保留。

追加問題有受控重現：篩選欄或命名對話框切回後沒有接收文字；重啟後點已有 Terminal 頁籤，焦點留在頁籤按鈕。前者是欄位保護也跳過了 native WebView 鍵盤恢復；後者是已作用中的頁籤不觸發 pane 的 active／visible effect，啟用入口也未要求焦點。

追加修補保留一般欄位及選取範圍，只恢復 native 鍵盤接收權；仍核對視窗、工作區、頁面、欄位與取消世代。可見 native Browser 的舊 main-document 欄位不拿來搶焦點。HERDR 頁籤在 runtime 啟用成功且選擇仍有效後要求焦點，等候 pane 掛載；啟用失敗或較新的選擇不聚焦舊頁。

回歸先失敗後通過；相關 77 項測試及完整前端 238 files／2,880 tests、typecheck／build 通過，lint 0 errors／52 既有 warnings。本輪未改 Rust／Host。下一候選追加驗收：欄位與對話框保留游標／選取、已作用中頁籤重點、重啟後首次點頁籤、快速切頁／欄位、Browser 防搶焦點；R2 仍需一般 shell 與 TUI 各至少 10 分鐘。

## 2026-09-19 安裝版重測與 Windows 啟用事件

已安裝 `bb54e55`（CI `35435109518`、artifact `10581438716`），Windows 端核對 NSIS SHA-256 為 `377f38b614128e28505b4ee684da430990c404efe75237c8186bb47eb386fde2`。

- **R1 字級部分通過**：既有 WSL `qa0919` Session 的 400 行輸出，逐次在設定切換 `14 → 26 → 10 → 30 → 14`，每次關閉設定後字元與底部 prompt 均可讀，沒有擠成極窄一列。長時間 TUI／分割 stress 尚未完成。
- **F2 命名與取消通過**：新 Terminal 完成命名後直接輸入 `#named`，另一個新 Terminal 取消命名後直接輸入 `#cancel`，兩者均不需要再次點擊 Terminal。信任提示與分割／其他欄位案例仍待測。
- **B8 通過**：Browser 作用中時關閉非作用中的 dirty HTML，儲存確認框可見且可操作；取消後 Browser 恢復且保留修改，再次關閉／捨棄僅清除測試修改。
- **F1 仍失敗**：工作列切至 Chrome 再返回後，逐字輸入 `#return` 未出現；點擊 Terminal 後相同輸入成功。原生 Console 的暫時診斷確認：失焦事件會送達，但工作列返回缺少 WebView／DOM focus 事件；另一筆原生 focus=true 時，Tauri `isFocused()` 仍回傳 false，而 DOM 已有焦點。

F1 後續修補：Windows 透過頂層 HWND 的 `WM_ACTIVATE` 通知前端，與 WebView 的 GotFocus 分開；恢復前以 `GetForegroundWindow` 確認真正的前景視窗，其他平台沿用既有焦點查詢。切頁、對話框、其他輸入欄與切離視窗仍可取消恢復。新增「沒有 WebView focus 事件」及非同步啟用後切離的回歸案例，先失敗後通過；Windows CI 另執行真實 HWND subclass 的事件與釋放測試。新修補仍需下一份候選安裝包驗證。

本次 NSIS 同版本重新安裝遇到執行中的 `conpty.dll`、`conpty/x64/OpenConsole.exe`、`herdr.exe` 鎖定。逐一核對 SHA-256 與 `herdr-runtime.json` 完全相同後保留原檔，未停止現有 Session。這是本次已核驗相同檔案的處理，不代表不同 runtime 的升級可略過錯誤，也不能算無阻礙安裝通過。

啟用事件修補的本機檢查：238 files／2,869 tests、typecheck、build、Rust all-targets check、Rust tests（381 單元及 1 integration 通過，4 項 ignored）與 clippy 90 項既有 baseline 通過；lint 0 errors／52 warnings。完整矩陣仍未驗收。

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
