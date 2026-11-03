# Yuzora 更新說明

<!-- yuzora(L7): CHANGELOG.md 裡 Yuzora 每一筆的繁體中文版，版本、### New／### Fixed 標題與項目數都要一樣（App 依標題判斷 Fixed）。這裡沒有的版本，App 會改顯示 CHANGELOG.md 的英文。 -->
## [0.1.0-beta.2] - 2026-11-02

### New
- Yuzora 改以 herdr-app 為基底：預設 Yuzora 版面（左側 5 個模式的側欄、模式列、狀態列）與 yuzora-dark 主題。
- Yuzora 主題改用品牌的藍與青，並新增淺色版 yuzora-light；預設跟隨系統的淺色／深色模式。
- 不再傳送任何使用資料；意見回饋改開 GitHub Issues。
- 工作區側欄：Spaces／Agents 分頁、每台機器的 scope chip（LED 顯示連線狀態）、「需要注意」區，以及機器 → Space → worktree → Agent 的樹；Agent 狀態同時以顏色與形狀表示。
- 新增 Space 會先選資料夾（遠端機器輸入路徑並在那台機器上確認），Space 以資料夾命名、終端機從那裡開始；同一個資料夾已有 Space 時直接切換過去。
- 模式列顯示各模式的徽章數；tab 列每個 tab 帶 Space 徽章與狀態，「＋ 新增 pane」可把工具 pane、終端機、Agent 混合分割進同一個 tab。
- pane 標頭與外框：focus 時邊框變亮，Agent 需要回覆時邊框與標頭轉為琥珀色；狀態列顯示 pane／Space／Agent 數、需要回覆數、目前分支 ↑↓ 與 HERDR 版本。
- 歡迎、新功能、意見回饋畫面改為 Yuzora 品牌；介面字型改用系統字型。
- 與官方 Herdr 共用 herdr-core 時：版本不符的畫面改用 Yuzora 的名稱並說明 core 是共用的；core 重新啟動後若變成 Yuzora 不能用的版本（例如 Herdr 換成新一代），Yuzora 不會接上，並一直顯示原因（點一下開啟更新頁），恢復後自動重連。
- SSH / SFTP 模式：列出 ~/.ssh/config、Yuzora 自建主機與 herdr machines，顯示連線狀態與延遲；首次連線確認主機金鑰指紋，金鑰變更時警告。
- SFTP 雙欄傳輸：本機 ↔ 主機各自瀏覽，→／← 或拖放傳輸，佇列顯示進度與速度，可取消、重試並從中斷處續傳，可保留權限與時間；遠端檔案可在本機文字編輯器開啟，存檔時自動上傳。
- 主機快速動作：開啟 SFTP、新終端機、HERDR Session、已知主機管理。資料庫還不能經由 SSH tunnel 連線。
- 視窗右下角顯示這台電腦上 Yuzora 的 CPU 與記憶體用量（App 本體、herdr-core 的終端機與 Agent、Yuzora 啟動的工具），點擊展開各類明細；取樣出問題時顯示 ⚠。
- App 改由 Yuzora 的 GitHub Releases 自我更新：每小時檢查，更新清單的簽章與每個檔案的 SHA-256 都驗過才會下載，下次重新啟動時套用。beta 版預設走 beta 通道，可在 `yuzora/settings.json` 的 `update.channel` 改成 `stable`。
- 安裝檔：macOS 是 `Yuzora-macos-aarch64.dmg`（僅 Apple Silicon，ad-hoc 簽章，第一次開啟要在「隱私權與安全性」按「仍要打開」）、Windows 是免系統管理員權限的 `Yuzora-windows-x64-setup.exe`、Linux 是 tar.gz 加 `install`。命令列程式叫 `yuzora`。
- 設定改從 Yuzora 的「一般」頁開啟，並新增「Yuzora」分組列出各功能的設定頁；隱私頁說明 Yuzora 不收集也不傳送任何資料。
- 一般設定：語系（跟隨系統、English、繁體中文）、主題、字型與大小、縮放、終端機剪貼簿寫入。
- Agent 模板：在設定裡管理各台機器的 herdr-core 模板（表單編輯、存檔前檢查、移到垃圾桶），並能「從模板啟動」到指定的 Space 與 tab。
- MOD 管理：列出內建、已修改、複製與個人的 mods 與 layouts 及其錯誤；可複製、在資料夾中顯示、檢查、還原、切換 layout，並在顯示信任警告後從資料夾安裝。
- 資料庫：側欄「資料庫」模式支援 PostgreSQL、SQL Server、SQLite 連線（密碼存在系統鑰匙圈）、資料庫切換與 schema 樹；資料表 pane 有資料（虛擬捲動表格、排序、複製、編輯儲存格並在單一交易內寫回）、結構（欄位、索引、外鍵、DDL）與 SQL 分頁；SQL pane 可執行選取或整段、多個結果集、取消與歷史紀錄。
- Git（以 IntelliJ IDEA 為基準）：側欄 Git 模式可暫存、提交（amend、sign-off、略過 hooks、訊息歷史與範本）、推送；diff pane 可暫存或捨棄單一 hunk 或選取的行；提交紀錄與分支圖、篩選與右鍵操作；分支 popup（smart checkout、merge、rebase、比較）；三方合併解衝突；互動式 rebase；Annotate；stash、tag、remote、worktree、patch 管理；Git 設定頁。
- Git 會先問是否信任資料夾：信任前，Git 在那裡離線執行，不執行任何會啟動 repo 自己的設定指定的程式（filter、SSH 指令、remote helper）的 Git 指令，包括 Git 為還沒 checkout 的 submodule 留在 `.git/modules` 裡的設定；partial clone 也會先問，因為它讀取時會向 remote 抓缺少的物件（Linux 上 Files 面板的資料夾監看也會等信任）。
- 未信任的資料夾：在 Git 模式信任資料夾之前，Yuzora 不會在那裡執行任何可能啟動 repo 自己設定的程式（filter、fsmonitor hook）的 Git 指令。Changes 與 Files 面板、worktree 對話框會顯示資料夾還沒有信任，Spaces 清單只從 refs 讀取分支與 ↑↓；Linux 上監看這類資料夾時，也不再向 Git 查詢被忽略的資料夾。
- 同步 herdr-app 上游到 f6fe216（v0.1.12 之後）：可選用終端機字型連字（`settings.json` 的 `font_ligatures`，預設關閉）、快捷鍵說明再按一次就關閉，並修正 pane 程式的啟動尺寸、焦點框與控制鍵別名等問題。
