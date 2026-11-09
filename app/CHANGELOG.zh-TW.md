# Yuzora 更新說明

<!-- yuzora(L7): CHANGELOG.md 裡 Yuzora 每一筆的繁體中文版，版本、### New／### Fixed 標題與項目數都要一樣（App 依標題判斷 Fixed）。這裡沒有的版本，App 會改顯示 CHANGELOG.md 的英文。 -->
## [0.1.0-beta.4] - 2026-11-11

### New
- **右側 dock。** 有側邊 panel 的 layout（main，以及預設的 yuzora）會在 panel 旁多一條 40px 的模式列，放 Files、Git、Database、SSH；沒有 panel 的 layout（terminals、win98、自訂）則在右緣有抽屜，點模式就在 window 裡開啟。切換 Space 時，開著的模式保持不動；換 layout 也不會再讓 Yuzora 的功能消失。
- **左側改用 Herdr 原生的 Spaces 與 Agents。** 預設的 yuzora layout 左側現在是 Herdr 原生的 Spaces 與 Agents 清單（左上角顯示「Herdr」），Files、Git、Database、SSH 移到右側 dock。在 Yuzora 設定的 Space 顏色會顯示在左側清單的頭像上。改過 `yuzora.json` 的使用者會保留舊版並命名為 `yuzora-edited`；沒改過的會自動換成新版。
- **每個 layout 可設定 dock，啟動時也可指定。** 設定 › Mods › Layouts 可讓每個 layout 把右側 dock 設為 Auto、Drawer 或 Off，優先於 layout 檔自己的設定。設定 › General › When Yuzora opens 可指定啟動時 dock 開在 Files、Git、Database 或 SSH，或維持收起。
- **`Cmd+Alt+B`（macOS）／`Ctrl+Alt+B` 開關右側 dock。** 沒改過的 beta.2、beta.3 `keys.json` 會自動更新成新的預設（包含 Herdr 的終端搜尋 Cmd/Ctrl+F）；改過的不會動，請自行加上 `"cmd-alt-b": "yuzora-modebar.toggle"`（或 `ctrl-alt-b`）。
- **同步官方 herdr-app v0.1.14。** herdr-core 連線錯誤會顯示原因並可重試、新增終端搜尋（Cmd/Ctrl+F）與捲動指令、pane 有捲軸、mod 可用 `pane.swap`，Add machine 也改為告知要執行的指令。herdr-core 的需求不變（0.1.11 以上、generation 1）。

### Fixed
- Git：開著的 repo 的 `.git` 不見、而且上一層路徑含 `:` 或 `;` 時，fetch、push、status 等任何 Git 指令都不會再跑到外層的 repo；其他情況由 `GIT_CEILING_DIRECTORIES` 擋住往上搜尋，Git hook 也不會多繼承 `GIT_DIR`。
- Git Log：極長的 commit 訊息在較慢的機器上，不會再被 render 時間上限中斷（改成每個段落一個文字元素）。

## [0.1.0-beta.3] - 2026-11-05

### New
- **整個 App 介面只剩英文，語言設定已移除。** 已存的中文設定或語系不會報錯，只是改成英文。
- **Git 側欄 repo 選擇器。** 一個 Space 有多個 repo 時，可以選要看哪一個（預設跟隨 focus，也可釘選；每個 Space 記住各自的選擇）。找 repo 不執行 Git，選中的 repo 照常經過信任關卡。指令：`yuzora-sidebar-git.pick-repo`。
- **Git Log 詳情可調整大小。** 詳情可以捲動，上下比例可以拖曳調整，雙擊恢復預設。詳情顯示完整的 commit 訊息（上限 2000 行）。
- **更簡潔的 tab 列與側欄。** 移除「新增 pane ▾」選單、兩個分割鈕與「下一個 layout」，並取消 `layout.switch` 的預設快捷鍵（⌘⇧L／Ctrl+Shift+L）。要開新增 pane 選單，請自行綁定 `yuzora-shell-tabs.new-pane`（是點，不是冒號）；`keys.json` 裡舊的預設綁定會被略過。

### Fixed
- Windows 上執行 Git、檔案與遠端指令時，不再連續跳出 console 視窗。
- tab 列恢復只列出目前 Space 的 tab（與 Herdr 相同），不再列出所有 Space 的 tab。
- 並排的 Git diff：左右各自依該側最長的一行決定寬度（新增檔案空白的左欄不再佔掉最長一行的寬度），點行號選行時也照實際的欄寬判斷是哪一側。
- 信任的資料夾：`trusted.json` 一次只讓一個寫入者修改（thread 與 process 都一樣，用 `trusted.json.lock` 上鎖），並整份取代，所以同時信任的資料夾不會再被蓋掉，同一時間讀清單的指令也不會把資料夾當成未信任。
- 未信任的資料夾：Changes 面板的 diff 與 Commit、worktree 對話框的 Enter，在執行 Git 前會再確認一次資料夾是否信任（面板上次讀取之後，設定可能多了程式）；Files 面板執行的 Git 不連任何 remote（包括本機的），所以 2.44 以前的 Git 也不會替 partial clone 去抓物件。
- 信任檢查：repo 設定和 submodule 掃描在同一個 shell 裡讀，每次檢查只要一次 git 呼叫（遠端一次 `hc exec`），回答裡沒有設定的 exit code 時當成讀不到；Changes 面板 refresh 自動開啟的 diff 不再多問一次。
- Files：macOS 與 Windows 上，根目錄 `.gitignore` 或 `.git/info/exclude` 忽略的資料夾（build 的 `target/`、`node_modules/`）裡的變動，不再喚醒 Files 側欄與面板。
- Files（Windows）：寫入被 ignore 的資料夾，不再被當成資料夾本身的變動回報（原本每批 build 都讓面板重載一次）；資料夾被建立、刪除或改名時仍會回報。
- 狀態列：Git 分支資訊在遠端機器上只要一次來回（原本三次），本機則兩項讀取並行。
- 遠端 Git 讀取：diff 中彼此不相依的讀取（index 項目、blob、種類、HEAD、modes）、log 的 refs 與 repo 的進行中操作狀態改成同時發出，不再循序（unstaged diff 5 → 2 輪、staged 4 → 2 輪、refs 3 → 1 輪）；diff 的 index 側改用該項目的 object id 讀取，內容與檢查過的項目是同一份。
- Windows：安裝程式詢問移除舊版全機安裝時，改為說明舊版資料會保留、但目前尚不會自動帶入連線與設定。
- Windows：SFTP「編輯」不再於 30 秒後關掉記事本；設定頁可把模板移到資源回收筒、在檔案總管中顯示檔案。
- Windows：編輯器、Markdown、圖片窗格會在檔案於磁碟上變更時重新載入；SFTP 本機面板顯示 `C:\…` 路徑。
- 在本機執行程式（Git、搜尋、腳本）時不再多等輪詢：`git --version` 這類呼叫在 Windows 上從約 52 ms 降到 29 ms，在 macOS 上從約 53 ms 降到 8 ms。
- Git diff pane：只有變動可能碰到自己的檔案、改名前的檔案、它們的上層資料夾或底下的路徑，或任何 `.gitattributes` 時才重新載入，不再因為 repo 裡任何變動就重讀；無法判斷變動的路徑時（git 寫了 index 或 refs、rescan）照舊重新載入。
- 信任檢查（本機，macOS 與 Linux）：單純 repo 的答案會保留，條件是 `HEAD`、`index`、`config`、`config.worktree`、`objects/`、`refs/`、`remotes/`、`branches/` 都沒變，`.git` 與 `commondir` 也仍指向同一組 dir；index 裡有 gitlink、有 `modules/` 資料夾、設定含 `include`、index 是 split 或 sparse（是否讀 shared index 由 git 自己回答）、檔案系統的 kernel 不自己維護 change time（只在 APFS、HFS+、ext2/3/4、XFS、Btrfs、tmpfs、ZFS、F2FS 上保留；FAT、exFAT 與網路檔案系統不保留），或檔案在掃描前 3 秒內或掃描當中變動過，就每次都掃描。未信任資料夾開 diff 從 74 ms 降到約 38 ms；Files reload 的檢查原本 47 ms、8 個行程，現在都是 0。
- Git 側欄：+／− 數字與按鈕不會再被過長的檔名蓋住，6 位數的行數會縮寫成 `150k`。
- app home 的 `herdr.d.ts` 原本把 mod 指令快捷鍵示範成冒號寫法，照著寫會讓整份 `keys.json` 失效，現在改成點。

## [0.1.0-beta.2] - 2026-11-04

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
- App 改由 Yuzora 的 GitHub Releases 自我更新：每小時檢查，更新清單的簽章與 SHA-256 都通過才會安裝，下次重新啟動時套用。beta 版只會更新到正式版（0.1.0 以後），不會更新到其他 beta，beta 請手動下載。
- 安裝檔：macOS 是 `Yuzora_<version>_aarch64.dmg`（僅 Apple Silicon，ad-hoc 簽章，第一次開啟要在「隱私權與安全性」按「強制打開」）、Windows 是免系統管理員權限的 `Yuzora_<version>_x64-setup.exe`、Linux 是 tar.gz 加 `install`。命令列程式叫 `yuzora`。
- 設定改從 Yuzora 的「一般」頁開啟，並新增「Yuzora」分組列出各功能的設定頁；隱私頁說明 Yuzora 不收集也不傳送任何資料。
- 一般設定：語系（跟隨系統、English、繁體中文）、主題、字型與大小、縮放、終端機剪貼簿寫入。
- Agent 模板：在設定裡管理各台機器的 herdr-core 模板（表單編輯、存檔前檢查、移到垃圾桶），並能「從模板啟動」到指定的 Space 與 tab。
- MOD 管理：列出內建、已修改、複製與個人的 mods 與 layouts 及其錯誤；可複製、在資料夾中顯示、檢查、還原、切換 layout，並在顯示信任警告後從資料夾安裝。
- 資料庫：側欄「資料庫」模式支援 PostgreSQL、SQL Server、SQLite 連線（密碼存在系統鑰匙圈）、資料庫切換與 schema 樹；資料表 pane 有資料（虛擬捲動表格、排序、複製、編輯儲存格並在單一交易內寫回）、結構（欄位、索引、外鍵、DDL）與 SQL 分頁；SQL pane 可執行選取或整段、多個結果集、取消與歷史紀錄。
- 檔案側欄：跟隨目前 pane 的 repo，顯示 git 標記與依類型上色的檔案 icon；新增、重新命名、刪除到垃圾桶、複製路徑、在 Finder 中顯示；檔名模糊搜尋與內容搜尋（有 ripgrep 就用，否則用內建搜尋）。
- 程式碼編輯器：14 種語言的 tree-sitter 上色、存檔衝突檢查、搜尋與取代；檔案在磁碟上被修改時自動重新載入，有 Agent 在改同一個檔案時會提示。
- Markdown 預覽（與編輯器即時同步，可切換原始碼）與圖片／SVG 檢視。repo 的 SVG 在獨立的行程裡畫，有時間與記憶體上限，超過時顯示佔位圖，圖片檢視會說明原因（Markdown 預覽只有佔位圖）。遠端 Space 的檔案可以瀏覽與唯讀開啟。
- Git（以 IntelliJ IDEA 為基準）：側欄 Git 模式可暫存、提交（amend、sign-off、略過 hooks、訊息歷史與範本）、推送；diff pane 可暫存或捨棄單一 hunk 或選取的行；提交紀錄與分支圖、篩選與右鍵操作；分支 popup（smart checkout、merge、rebase、比較）；三方合併解衝突；互動式 rebase；Annotate；stash、tag、remote、worktree、patch 管理；Git 設定頁。
- Git 會先問是否信任資料夾：信任前，不執行任何會啟動 repo 自己的設定指定的程式（filter、SSH 指令、remote helper）的 Git 指令，包括 Git 為還沒 checkout 的 submodule 留在 `.git/modules` 裡、或從那裡指向的 git 資料夾的設定，push（branch 或 tag）也要等信任後才會檢查或推送 submodule；partial clone 也會先問，因為它讀取時會向 remote 抓缺少的物件（Linux 上 Files 面板的資料夾監看也會等信任）。信任前，Yuzora 自動執行的讀取維持離線；背景 fetch 與你自己按下的 fetch、pull、push 信任前只用 Git 內建的傳輸，不用 remote helper，在設定會執行程式的資料夾也要等信任。背景 fetch 不跑 hook；你自己執行的 Git（例如 commit、push）一般 hook 照常執行。
- 未信任的資料夾：在 Git 模式信任資料夾之前，Yuzora 不會在那裡執行任何可能啟動 repo 自己設定的程式（filter、fsmonitor hook）的 Git 指令。Changes 與 Files 面板、worktree 對話框會顯示資料夾還沒有信任，Spaces 清單只從 refs 讀取分支與 ↑↓；Linux 上監看這類資料夾時，也不再向 Git 查詢被忽略的資料夾。
- 同步 herdr-app 上游到 f6fe216（v0.1.12 之後）：可選用終端機字型連字（`settings.json` 的 `font_ligatures`，預設關閉）、快捷鍵說明再按一次就關閉，並修正 pane 程式的啟動尺寸、焦點框與控制鍵別名等問題。
