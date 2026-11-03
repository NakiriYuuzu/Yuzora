# L5 檔案＋編輯器

beta.2 的檔案側欄、程式碼編輯器、Markdown 預覽與圖片檢視。設計依據：設計稿 p2（檔案畫板）與 p6（側欄「檔案」模式）。架構規則見 [architecture.md](architecture.md)、[lanes.md](lanes.md)。

## 範圍與檔案

| 路徑 | 內容 |
| --- | --- |
| `app/crates/yuzora-files/src/lib.rs` | `yuzora:files` HostModule：函式、3 個原生元件、TypeScript 宣告 |
| `src/highlight/` | tree-sitter 上色：14 種語言、`InputHighlighter` 實作、One Dark 色盤（mod 以主題色覆寫）、無狀態 `highlight()` API |
| `src/editor.rs` | 可重用的程式碼編輯器：`new_code_editor()`、`code_editor()`、`Look` |
| `src/docs.rs` | 開啟中的文件（每個檔案一份 buffer）、存檔衝突檢查、外部變更、搜尋取代、`Editor` 元件 |
| `src/markdown.rs` | `Markdown` 元件（gpui-base `TextView`，GFM） |
| `src/image.rs` | `ImageView` 元件（PNG／JPEG／GIF／WebP／BMP／TIFF／ICO／SVG） |
| `src/fsops.rs`、`src/git.rs`、`src/search.rs` | 檔案操作、輕量 `git status`、模糊檔名搜尋與內容搜尋（ripgrep，本機沒有 rg 時用內建搜尋）；本機或 `hc exec --on` |
| `app/defaults/mods/yuzora-sidebar-files` | 側欄「檔案」模式（含自己開的右鍵 popup） |
| `app/defaults/mods/yuzora-editor` | 編輯器 pane |
| `app/defaults/mods/yuzora-markdown` | Markdown pane（預覽／原始碼切換） |
| `app/defaults/mods/yuzora-files-image` | 圖片與 SVG 檢視 pane |

上游程式碼：無改動（沒有 `// yuzora(L5):` 註記）。共用檔只改了各自的 L5 標記區：`README.md`、`AGENTS.md`、`CLAUDE.md`、`app/defaults/AGENTS.md`，以及 `app/CHANGELOG.md` 的 0.1.0-beta.2 條目。

## 設計

### 三層分工

- **Rust（`yuzora-files`）**：所有磁碟 I/O、git、搜尋都在 worker thread；本機讀檔與「能不能寫」的檢查一律以 nonblocking 開檔、再透過 handle 確認是一般檔案（`open_regular`），檢查後才被換成 FIFO 或裝置也不會卡住 worker；編輯器、Markdown、圖片是 `HostModule::component` 原生元件。
- **JS mods**：版面、標頭、橫幅、搜尋列、右鍵選單、開檔策略。使用者可以 `mod.clone` 修改。

### 文件（`docs.rs`）

- 一個檔案（機器＋路徑）在整個 app 只有一份 `DocView`（GPUI global 的 registry），所有 pane 共用。`openDocument` 每呼叫一次加一個參考，`releaseDocument` 減一。
- **只有一個 pane 畫編輯器**（owner）。同一個檔案在另一個 pane 開啟時顯示「在這裡編輯」，按下才接手；避免同一個 GPUI entity 在同一個 frame 被畫兩次。
- 載入：`fsops::read_text`，換行正規化為 LF，記住原本是 LF／CRLF 與 BOM，存檔時還原。
- **存檔**：同資料夾的暫存檔（`.yuzora-save-<32 hex>`）fsync 後 rename 蓋過去（蓋過既有檔案時，暫存檔先以 0600 建立，最後一次比對完、rename 之前才套用原檔的權限：寫入期間原檔擋掉的人打不開它；存檔前確認可寫入時也以一般檔案開啟，換成 FIFO 不會等），保留權限、owner、ACL 與延伸屬性（Finder 標籤、resource fork；macOS 用 `fcopyfile`，Linux 逐一複製 xattr，POSIX ACL 也在其中，新檔自己從資料夾繼承、原檔沒有的會移掉）；Windows 目前只保留唯讀屬性，ACL 與其他屬性會變成資料夾繼承的（待改用 `ReplaceFileW`）。資料夾不能建暫存檔、或 owner、ACL、延伸屬性保不住時退回就地寫入：先把舊內容分塊複製到備份（不整份讀進記憶體；同資料夾，資料夾不可寫時放 app home 的 `backups/files`（0700）；一律新建檔案（O_EXCL，不寫進既有路徑或 symlink），只有擁有者可讀），新內容寫完才刪；寫入失敗時寫回舊內容（從建立備份時的同一個 handle 讀回，不用名稱重開：資料夾可能別人也能寫入），連寫回都失敗就留下備份並在錯誤裡指出位置。symlink 寫到它指向的檔案；指向的檔案已刪除時（還原），沿著連結寫回原本的位置。寫入前比對載入時的 `{len, mtime}`，不同就回報衝突、不寫入；一樣時再比對內容 hash（時間精度粗的檔案系統上，同長度的改寫可能 stamp 不變）。暫存檔寫完、rename 之前再比對一次（就地寫入則是備份複製完、覆寫之前），寫入或複製備份期間別人改了檔案也會回報衝突並保留對方的內容；rename 沒有「比對後才替換」的原子操作，只有最後一次比對到 rename 之間的改動仍會被蓋掉。
- **外部變更**：本機用 herdr 的 `watchFolder`（檔案所在資料夾）＋15 秒輪詢，遠端 5 秒輪詢，呼叫 `checkDisk`。stamp 變了才讀內容並比對 hash。只到秒的 stamp（遠端的 `stat`、時間精度粗的檔案系統）在同一秒內被改寫成同樣長度時不會變，所以這種 stamp 在第一次看到 2 秒後的讀取確認內容相同之前，每次檢查都讀內容。檢查開始後文件被重新命名的，結果不套用（載入也一樣，改用新路徑重讀）：
  - 內容相同（touch）：只更新 stamp。
  - 沒有未儲存變更：自動重新載入，保留游標與捲動位置。
  - 有未儲存變更：標成衝突，橫幅提供「重新載入（捨棄我的）／保留我的／覆寫」。衝突橫幅的底色與邊框取 `tokens().status.blocked` 的 `header_bg`／`pane_border`（與 L1 等待中 pane 的標頭相同，淺色主題自動換成淺色版本），文字是 `th.blocked`。
  - 檔案消失：橫幅提供「重新儲存」。
  - 只改權限（chmod 不動大小與時間）：本機檢查也比對權限位元，變了才重新判斷能不能寫，編輯器跟著切換唯讀。
- **「<Agent> 也在改」**：文件記住最後一次被外部修改的時間（`externalAt`）。3 分鐘內，若有 agent 狀態為 working、且它所在 pane 的 cwd 包含這個檔案（Windows 路徑兩種斜線都認、不分大小寫），標頭顯示該 agent 的名稱：`raised` 底、`muted` 字，前面一個品牌色圓點（`tokens()` 的 agent 品牌色，沒有時用 `th.blocked`）。品牌色直接當文字在淺色底上不到 4.5:1，所以只用在圓點。
- 關閉有未儲存變更的 pane：buffer 保留在記憶體（toast 提示），重新開啟同一檔案即可找回；Yuzora 結束才會遺失。
- 大檔保護：超過 16 MiB 不開；超過 2 MiB 以純文字開啟（關閉上色與摺疊）；含 NUL 的二進位檔不開；非 UTF-8 以唯讀開啟。重新讀取時（外部變更、Reload），檔案變成二進位或超過上限就不再顯示編輯器，未存的修改一併丟掉，不會再被存回去；大小跨過 2 MiB 時重建編輯器，上色與摺疊跟著開或關。
- 搜尋取代：用 gpui-base 內建的搜尋引擎（`set_search_query`、`next/previous_search_match`、`replace_*`），關掉它的內建面板（`searchable(false)`），讓 ⌘F 冒泡到 JS 的搜尋列。文件還在載入時輸入的查詢會記住，編輯器建立後再套用。

### 上色（`highlight/`）

- tree-sitter 0.27 與 13 個語法 crate：rust、typescript／tsx、javascript（含 JSX）、json、toml、yaml、markdown（block 語法）、python、go、sql（tree-sitter-sequel）、bash、css、html。
- 增量解析：`InputEdit` 轉成 `tree_sitter::InputEdit`，用舊 tree 重新解析（一批 edit 只重解析一次）；`styles(range)` 只對可見範圍跑 `QueryCursor`。debug 建置下 2 MiB 的 Rust 全文解析約 0.9 秒、增量約 80 ms、一屏約 1–2 ms。
- capture 名稱對應到固定的語意名稱（keyword、function、type、string…），由 `SyntaxPalette`（`HighlightStyleResolver`）決定顏色；預設 One Dark（設計稿的配色）。編輯器與 Markdown 由 `doc-pane.js` 的 `SYNTAX` 傳入主題色（`syntax: {名稱: 色碼}`，對應到 `ansi1`–`ansi14` 與 `dim`），淺色主題也看得清楚：One Dark 有上色的每個名稱都列出來（覆寫依前綴比對，只列 `function` 的話 `function.builtin` 也會變成 function 的色）。淺色主題的目前行用 hover 與背景各半，程式碼在上面仍有 4.5:1；深色主題維持 hover。語言徽章同樣是主題色的名稱（`languages()` 的 `color`、文件的 `badgeColor`）。
- 同一段文字有多個 capture 時：範圍最小的優先，一樣大時比 pattern 的限定程度（節點數＋predicate），再相同才取後面的。上游各語言的查詢對先後順序的假設互相矛盾（JS 要後面的贏、JSON 與 Go 要前面的贏），單一規則無法全對。
- 上游查詢的兩處執行期修補（各有測試）：tree-sitter-sequel 把數字寫成 Lua 的 `%d`，換成 `[0-9]`；tree-sitter-toml-ng 的 `(pair (bare_key)) @property` 會把整個 pair 上色，改成 `(pair (bare_key) @property)`。
- 摺疊範圍來自語法樹（成對括號跨行的節點、python `block`、markdown `section`…）。
- Markdown 只用 block 語法：fenced code 依 info string 上色，`*強調*`、連結等 inline 語法不上色。

### Markdown 與圖片

- `Markdown` 元件跟著文件的文字版本（每次編輯或重新載入加一）重畫，游標閃爍不會讓它重抓全文；同一份文件在原始碼與預覽間切換時即時同步。
- 圖片只讀本機的一般檔案（不是資料夾、裝置或 FIFO）且不超過 32 MiB，解碼後也不超過 512 MiB（見下方圖片 pane）；網路、`data:`、Windows UNC（`//host/share`）與遠端文件的圖片一律不讀。檢查在 worker 做：讀出 bytes（最多 32 MiB，不多讀）、檢查解碼大小，交給 GPUI 的就是這份 bytes 而不是路徑，所以讀完之後檔案變大或被換掉也繞不過限制。畫面只讀快取，10 秒後重新 `stat`，stamp 變了才重讀，同一路徑換了圖也會更新；最後一個 frame 沒畫到、而且 1 分鐘沒畫到的圖片連同 bytes 放掉（之後沒有新的 frame 也會在 1 分鐘後放掉；捲出畫面的圖片捲回來時重讀）。所以圖片在檢查回來後才出現。畫面上有找不到的圖片時，10 秒後會自己重畫再找一次，之後才建立的圖片也會出現。相對圖片從文件所在的資料夾找；文件被重新命名或移到別的資料夾時跟著換。Windows 上 `file:///C:/…` 指的是 `C:/…`。連結只開 http、https、mailto。
- 預覽的文字、連結、程式碼底色跟隨目前的主題（`theme()` 的 foreground、muted、ansi4、raised、border）；code block 的語法色和編輯器相同（`syntax`），換主題時 code block 重新上色（文字的上色快取以 highlighter 為 key，只有語法色變了才換一個）。
- 圖片 pane：PNG／JPEG／GIF／WebP／BMP／TIFF／ICO／SVG。GPUI 會把 GIF、動態 WebP 的每一幀都解碼成整張畫布（每像素 4 bytes）並留著，所以先用檔頭的尺寸與幀數（GIF 的 image block、WebP 的 `ANMF` chunk，不解碼）估算，超過 512 MiB 就不顯示；SVG 與檔頭解析不了的交給解碼器。檢視有「符合」與「100%」（原尺寸，比 pane 大時可捲動）、對比底色（`th.solid`：淺色主題是深底、深色主題是淺底，上面的提示文字用 `solid_foreground`）；檔案變了自動重新顯示：本機的 watch 事件指到這個檔案就重讀（不看大小與時間）；遠端只有秒級時間，看到新的 stamp 後 2.5 秒再讀一次，補上同一秒內的改寫。遠端圖片同樣只讀一般檔案。

### 側欄（`yuzora-sidebar-files`）

- 跟隨目前 focused pane 的 Space 與 cwd（沿用上游 `files` mod 的定位方式）；focused 的是工具 pane（沒有 cwd）時保留原本的資料夾。在 repo 裡時以 `git rev-parse --show-toplevel` 為根。換到別的資料夾時，取消正在輸入的新增或重新命名（它屬於原本的資料夾）；搜尋開著時，在新的根目錄重跑；樹重新整理時（watch 事件、手動、遠端的 10 秒輪詢）也重跑，檔名清單重新列出。本機有 watch 時，10 秒計時不重跑搜尋。
- 標題（2026-11-03 改版，B1「路徑卡」，取代同日的設計 D；標題列和其他側欄一樣）：
  - 標題列：照資料庫、SSH 側欄的寫法，高 32、左右 12：左邊是「檔案」（側欄變窄時先截斷），右邊 6 顆 22×22 ghost 圖示鈕（間距 2）：新增檔案、新增資料夾、重新整理、全部收合、搜尋、顯示隱藏檔（記在 localStorage；切換時重跑搜尋）。按鈕平常沒有底色、muted 字，hover 時 `hover` 底＋foreground，開著的 toggle 用 `selected` 底＋foreground；按鈕不縮，不換行也不溢出。遠端資料夾時新增檔案、新增資料夾停用（dim、不能點），tooltip 說明遠端在這一版是唯讀。不再顯示 Space 徽章。
  - 路徑卡（標題列下方 4px、左右 10px；`raised` 底、1px `border`、圓角 7）：第一行是資料夾 icon、上層路徑（`~` 形式、結尾帶分隔符、dim）與資料夾名稱（粗體），空間不夠時先縮上層路徑，名稱盡量完整；最右邊是「本機」或機器名的 chip（`hover` 底，和卡片的 `raised` 分得開）。第二行是 git 分支 icon＋分支名（accent、11.5px、過長截斷），不是 repo 或沒有分支時不顯示。拆法在 `paths.js` 的 `heading()`（純函式）：家目錄本身顯示 `~`、根目錄（`/`、`C:\`）只有名稱；`~` 只用在本機，遠端路徑照原樣。沒開資料夾時沒有路徑卡，樹照舊顯示空狀態。
  - 沒有新 token，卡片上的字都用主題色：dim、foreground、accent 在 `raised` 上，深色 5.84／12.37／6.17、淺色 6.34／16.70／6.00；chip 的 muted 在 `hover` 上，深色 6.10、淺色 6.55。由 `yuzora-core` 的 `both_yuzora_themes_read_at_wcag_aa` 守著。
- 樹：`uniform_list` 虛擬捲動，每個資料夾最多 2000 項（本機列資料夾時邊讀邊排序，只留前 2001 項，超大的資料夾也不會整份放進記憶體）、總共 20000 列（超過時樹下方提示收合一些資料夾）；根目錄被截斷、而且留下的 2000 項都是隱藏檔（隱藏檔沒有顯示）時，樹不寫「沒有檔案」，改提示前 2000 項都是隱藏檔；名稱不是 UTF-8 的項目不列出（本機與遠端都一樣：用替代字元拼出的路徑可能指到旁邊的檔案；git 狀態裡這種路徑也不標記），讀不到類型的項目也不列出、讀到一半失敗就停（讀的期間被刪掉的不算），這些情況樹下方都會提示有項目無法讀取或顯示；git 標記 M／A／D／U／?，收合的資料夾若含變更顯示 `•`。每列依類型顯示 icon（`icons/files/*.svg`，自己畫的 16×16、stroke 1.4 單色遮罩，與模式列一致）：資料夾分開啟／收合，常見副檔名與特殊檔名（`Cargo.toml`、`package.json`、`Dockerfile`、`.gitignore`、lock 檔⋯）各有 icon，其他用預設檔案 icon。對照表在 `icons.js`（純函式，mod 載入時建表一次，每列只查表），顏色是主題色的名稱（`ansi0`–`ansi15`、`muted`、`dim`），淺色與深色主題都跟著換；搜尋結果用同一個對照表。`watchFolder` 監看根目錄＋10 秒重新整理；收合過的資料夾再展開時，先顯示上次的內容並重新列出。根目錄重新列出失敗時（被刪除或沒有權限），樹上方顯示錯誤，下面是上次列出的內容。
- 搜尋：檔名（模糊比對，`.gitignore` 生效，清單快取 15 秒（從開始列的時間算；較早開始、較晚結束的清單不會蓋掉較新的），最多列 200,000 個檔案，超過時提示「只搜尋了前 200,000 個檔案」）與內容（有 `rg` 就用 ripgrep；本機沒有時改用內建搜尋：`ignore`＋`regex`、同樣的規則，每個檔案最多讀 2 MiB（檢查大小之後檔案又變大也一樣），內容不全是 UTF-8 的檔案照樣搜尋（壞掉的 byte 顯示成 U+FFFD，和 rg 的行一樣），一行有很多比對時只標出顯示的前 300 字內的比對（不在後面多花時間）；遠端沒有 `rg` 時顯示提示）。ripgrep 把不是 UTF-8 的行給成 base64 `bytes`：解碼後以替代字元顯示；不是 UTF-8 的路徑沒辦法用名稱開啟，不列出（內建搜尋、本機與遠端的檔名清單也一樣，不用替代字元拼出可能指到別的檔案的路徑，並提示結果不完整）。有檔案或資料夾讀不到時（ripgrep exit 2、路徑不是 UTF-8、`find` 或 git 失敗（還沒列出任何檔案也算）、本機走訪錯誤），保留找到的結果並提示「有些檔案或資料夾無法讀取，結果可能不完整」。點內容結果會開檔並跳到該行。
- 右鍵：同一個 mod 以 popup 開啟自己（`props().menu`）。開啟、在終端機開啟、新增檔案／資料夾、重新命名、刪除到垃圾桶、複製路徑／相對路徑、在 Finder（檔案總管／檔案管理員）中顯示。新增與重新命名透過 `shared("yuzora.files.request")` 交回主側欄，在樹上方顯示輸入列。重新命名不會覆蓋已存在的檔案或資料夾：macOS 與 Linux 用一次系統呼叫完成（`renamex_np(RENAME_EXCL)`／`renameat2(RENAME_NOREPLACE)`），檢查與搬移之間不會被別的程式搶先建立同名檔案；其他平台與不支援的檔案系統則先檢查目標。新路徑上還開著文件時（檔案已刪除、pane 還在），拒絕重新命名並請先關掉它，避免同一個檔案出現兩份緩衝。開著的文件（該檔案或資料夾底下的）正在存檔時拒絕重新命名；重新命名進行中，這些文件也不能存檔（回 `busy`），存檔不會寫回舊路徑。重新命名後，開著的文件（只算該路徑本身與它底下的；只有 Windows 把 `\` 當分隔符，其他平台的 `foo\a.txt` 是 `foo` 旁邊的檔案）會換到新路徑，pane 繼續顯示，不會被當成已刪除；換了副檔名時，語言徽章與上色跟著新檔名，編輯器（含未存的修改與復原紀錄）不變。

### 開檔

- 圖片／SVG → `yuzora-files-image`；`.md` → `yuzora-markdown`（預設預覽）；其他 → `yuzora-editor`。
- 同一個檔案已有 pane 顯示時，`pane.focus` 過去；否則把 focused pane 往右分割，沒有 focused pane 時開新 tab。
- pane 不知道自己的 pane id，所以每次編輯器取得焦點時，用 `focused().pane` 記到文件上（`notePane`），之後才能 focus 回來。圖片沒有文件：側欄在 `pane.split` 之後記下取得焦點的新 pane，下次開同一張圖時 focus 過去（只記得從側欄開的圖片）。從側欄重新命名這張圖（或它所在的資料夾）時，pane 的 props 改不了：在舊 pane 右邊開一個新路徑的圖片 pane、再關掉舊的，新 pane 留在原來的位置，焦點不變。
- 帶 `line` 開 `.md`（例如內容搜尋的結果）時以原始碼開啟並跳到該行；預覽／原始碼的偏好不變。檔案已經開在預覽的 pane 時，側欄用 `revealLine` 告訴文件，那個 pane 換成原始碼，等它的編輯器建好後再跳過去（同樣 10 秒內有效）；pane 開啟之前的請求不套用。文件還在載入（還沒有編輯器）時，這一行先記在文件上，編輯器建好時（10 秒內）再跳過去。
- 路徑 helper（`yuzora-sidebar-files/paths.js`）：Windows 路徑（`C:\…`、`\\server\…`）`/` 與 `\` 都當分隔符；其他平台的 `\` 是檔名的一部分。
- 跳行與聚焦只在開啟後 10 秒內套用（props 的 `opened`），重啟還原 pane 時不再跳。手動 `pane.split` 只帶 `line`、沒有 `opened` 時也會跳：跳過的請求（機器＋路徑＋行列）記在 `shared("yuzora.files.went")`，紀錄早於這次啟動（`startedAt()`）就是重啟還原的 pane，不再跳也不搶焦點；同一次執行裡再開同樣的請求則照跳。紀錄保留 7 天。

### 共享狀態

- `yuzora.badge.files`：未儲存的文件數（沒有時 `null`）。
- `yuzora.status.files`：有未儲存文件時 `{text: "N 個未儲存", tone: "blocked"}`，否則 `null`。
- `yuzora.files.request`、`yuzora.files.refresh`：側欄 popup 與主側欄之間的請求。

## 可重用 API（給其他 lane）

Rust：

```rust
use yuzora_files::{editor, highlight};

// 一個帶上色的編輯器（L3 的 SQL 編輯器）
let look = editor::Look::default();
let state = editor::new_code_editor(editor::CodeEditorOptions { lang: Some(highlight::Lang::Sql), text: "select 1", ..Default::default() }, &look, window, cx);
// 畫出來
editor::code_editor("sql-editor", &state, &look, cx)

// 無狀態上色（例如 diff 裡的程式碼）
let spans: Vec<(Range<usize>, &'static str)> = highlight::highlight(highlight::Lang::Rust, "fn main() {}");
```

JS（`yuzora:files`，完整宣告在 app home 的 `yuzora.d.ts`）：`openDocument`／`documentState`／`documentChanged`／`saveDocument`…、`Editor.new(id, {doc, owner})`、`Markdown.new(id, {doc})`、`ImageView.new(id, {path})`、`listDir`、`gitStatus`、`findFiles`、`grep`。

## 遠端 Space

- 讀取與瀏覽：`listDir`、`gitStatus`、`findFiles`（`git ls-files`，扣掉工作樹已刪除的檔案／`rg --files`／`find`）、`grep`（遠端 `rg`）、開檔、圖片，都走 `yuzora_core::remote`（`hc exec --on`，各自的 timeout）。遠端 `stat` 只在確認不存在時才回報「不存在」：往上找到第一個存在的資料夾，而且能進入。路上有進不去的資料夾時回傳錯誤，文件不會被標成已刪除，與本機相同。遠端既沒有 git 也沒有 rg 時，`find` 不認得 ignore 檔：`findFiles` 回傳 `unfiltered: true`，側欄提示結果包含被忽略的檔案。
- **寫入未做**（存檔、新增、重新命名、刪除）。`createFile`／`createFolder`／`rename` 帶機器名稱一律拒絕，存檔回 `remote`：
  - 原因：`hc exec` 只能把資料放在 argv 或 stdout，沒有 stdin，也沒有「比對後才寫入」的原子操作。用 base64 塞進 argv 會碰到 `ARG_MAX`，也沒辦法安全做衝突檢查；做半套反而可能蓋掉 agent 剛寫的內容。
  - 後續計畫：L4 的 SFTP session（`yuzora:ssh`）合併後，遠端存檔改走 SFTP：寫暫存檔 → `stat` 比對 → rename，與本機相同的衝突語意；新增、重新命名、刪除也用 SFTP。沒有 SSH 設定的機器（只透過 herdr-core 連線）再評估 `hc` 是否提供檔案寫入 API。

## 驗證

- `cargo test --locked -p yuzora-files`（148 個）：上色（每種語言的首選 query 都能編譯並產生 span、增量解析＝重新解析、摺疊、色盤）、檔案操作（大資料夾邊讀邊截斷、原子存檔、衝突（含 stamp 相同但內容不同、寫入期間被改）、CRLF／BOM、二進位與大檔、遠端 sh 腳本在本機跑出與原生相同的結果、名稱不是 UTF-8 的項目不列出並提示（遠端解析時其他檔案的大小不錯位；本機要能建立這種檔名的檔案系統，APFS 上略過）、讀不到類型或讀到一半失敗的項目（假的資料夾項目）、原子存檔保留延伸屬性與 ACL（確認換了 inode；ACL 只在 macOS 驗）、就地寫入在備份複製完才比對（複製期間的外部修改不被覆寫、備份刪掉）、存檔的暫存檔在換上去之前只有擁有者能開、遠端圖片只讀一般檔案、圖片解碼大小（GIF 幀數、WebP `ANMF`）、遠端變更一律拒絕）、git porcelain 解析（不是 UTF-8 的路徑不標記）與真 repo（不改動 `.git`；遠端清單扣掉已刪除的追蹤檔）、模糊搜尋排序與檔名清單快取（舊的不蓋新的）、rg JSON 解析（含空字串比對的欄位、不是 UTF-8 的行與路徑）與內建搜尋（一行很多比對時只看顯示範圍；不全是 UTF-8 的檔案照樣找得到；不是 UTF-8 的路徑不列出：要能建立這種檔名的檔案系統，APFS 上略過）、文件載入／外部變更自動重載（含 stamp 同一秒內的改寫）／刪除／共享／釋放、改名前開始的檢查不套用、存檔與重新命名不重疊、載入中要求的行在編輯器建好後套用、開檔不等 FIFO、在測試視窗裡真的編輯後遇到外部變更（衝突、保留我的、存檔）與存檔時才發現變更或檔案已刪、Markdown 圖片與連結規則（交給 GPUI 的是檢查過的 bytes、同一路徑換圖會更新、沒有副檔名時看 bytes 判斷格式、不再畫到的圖片放掉）、i18n 兩語系 key 一致、HostModule 宣告一致、兩份 `doc-pane.js` 相同、程式碼顏色在兩個 Yuzora 主題都讀得清楚（在 QuickJS 跑 `doc-pane.js` 的 `SYNTAX`／`syntax`／`mix`：One Dark 有上色的名稱都有主題色；淺色主題在背景、code block、目前行上都 ≥ 4.5，語言徽章也是；深色主題不低於 One Dark 與原本的徽章色）、`mix` 混色、Markdown code block 用傳入的語法色且只在語法色變了才換 highlighter、「<Agent> 也在改」的資料夾比對（在 QuickJS 跑 `doc-pane.js` 的 `within`／`editingAgent`：Windows 兩種斜線、大小寫、UNC，其他平台反斜線是檔名的一部分）、側欄路徑 helper（在 QuickJS 跑 `paths.js`：Windows 與 POSIX 路徑；路徑卡的資料夾名稱與上層路徑：家目錄、根目錄、根目錄下的資料夾、遠端路徑、沒有分支、上層路徑結尾的分隔符、Windows 兩種斜線）、檔案 icon 對照（在 QuickJS 跑 `icons.js`：類型、特殊檔名、資料夾、只用主題有的顏色、每個 icon 都有 16×16 的 SVG、2 萬列查表不到 1 秒）、`revealLine` 每次都是新的請求、改名後圖片 pane 換成新路徑（在 QuickJS 跑 `open.js`，假的 Herdr：位置、焦點、資料夾改名、已關掉的 pane 不重開）。
- `cargo test --locked`（app workspace）：上游 `shipped_initial_descriptions` 會以空 props 與 fixture props 各渲染每個內建 mod 一次，4 個 L5 mod 都包含在內。
- `scripts/yuzora/check-mods`：全部 yuzora mod 通過。
- 隔離實機（lanes.md 的 `env -i`、暫存 app home 與 `HC_SESSION`），用 `herdr-app do`／`shot` 操作與留證：
  - `l5-01-files.png`：檔案模式（Space 徽章、本機、`路徑 · 分支`、動作列、樹與變更標記）。
  - `l5-02-editor.png`：`pane.split` 開 TSX，tree-sitter 上色、檔案列、狀態列。
  - `l5-03-markdown.png`：CLAUDE.md 預覽（標題、清單、inline code、表格、上色的 code block、本機 SVG 圖片）。
  - `l5-04-image.png`：PNG 檢視；側欄同時因 `watchFolder` 自動出現新檔的標記。
  - `l5-05-external-change.png`：在 app 外改寫開著的 TSX，編輯器自動重新載入（第 8–9 行）。
  - `l5-06-git-marks.png`：在 app 外修改 README.md、刪掉 package.json，樹即時更新（`M`、檔案消失）。
  - `l5-07-goto-line.png`：`pane.split` 帶 `line: 15, column: 7` 開檔，游標在 Ln 15, Col 7，編輯器取得焦點。
  - `l5-08-markdown-goto-line.png`：帶 `line: 20` 開 `.md`，以原始碼開啟、游標在 Ln 20。
  - `l5-10-goto-line-by-hand.png`：`pane.split` 只帶 `line: 15, column: 7`（沒有 `opened`）開檔，游標在 Ln 15, Col 7；重啟後這個 pane 停在 Ln 1，焦點留在原本的 pane。
  - `l5-09-image-actual-size.png`：2400×1600 的圖以 100% 顯示（下方 pane，暫時的 probe mod），從左上角原尺寸排版、可捲動，不再置中裁切。
  - `l5-13-file-icons.png`：檔案樹的類型 icon（深色主題 yuzora-dark）：收合的資料夾、程式碼（Rust 琥珀、TS 藍、JS 黃、Go／Python）、圖片、shell、lock、`Cargo.toml`／`package.json`、設定檔、`Dockerfile`、Markdown 與純文字、沒有對照的 `LICENSE`。
  - `l5-14-file-icons-light.png`：同一個資料夾在淺色主題 paper：顏色換成主題的深色 ANSI，在白底上同樣清楚。
  - 淺色主題 yuzora-light（Yuzora 的 UI 文字在淺色主題都要清楚）：
    - `l5-15-light-editor.png`：TSX 的語法色、語言徽章、目前行、狀態列（右下顯示目前的主題名稱）；側欄的樹與 git 標記。
    - `l5-16-light-markdown.png`：Markdown 預覽中 Rust 與 TS 的 code block，各種 token 在 code block 底色上都清楚。
    - `l5-17-light-banner.png`：在 app 外刪掉開著的檔案，警告橫幅是淺琥珀底、深琥珀字（`tokens().status.blocked`）。
    - `l5-18-light-image.png`：圖片 pane 的 `IMG` 徽章與切換鈕。「對比背景」開啟時的畫面要點擊才能切換，沒有截圖（`solid_foreground` 在 `solid` 上是 17.7:1）。
    - `l5-19-dark-editor.png`：同一個檔案在 yuzora-dark，作為對照。
    - 側欄右鍵選單的刪除項目（`ansi1`）要右鍵才會出現，沒有截圖；用色碼計算是 popover 上 5.69、hover 上 4.60。
  - 檔案側欄標題改版（B1）：
    - `l5-20-sidebar-header-dark.png`（yuzora-dark）、`l5-21-sidebar-header-light.png`（yuzora-light）：標題列的「檔案」與 6 顆圖示鈕（「顯示隱藏檔」開著，`selected` 底；樹裡多了 `.yuuzu`）、路徑卡（上層路徑 `/private/tmp/yz-l5-run/` 放不下時先截斷、資料夾名稱 `demo` 完整、「本機」chip、分支 `feat/gpui-dock`）。
    - 沒有截圖的：hover、遠端時停用的新增鈕、沒有分支時第二行消失、側欄再變窄時截斷「檔案」（要點擊或拖動）。遠端與沒有分支的資料由 `heading()` 的單元測試涵蓋（遠端路徑不用 `~`、沒有分支、上層路徑與名稱的拆法），排版靠 `flex_none`／`min_w(0)`／`truncate`。
- 沒有在實機驗的（需要鍵盤或滑鼠；未取得 computer-use 授權）：輸入後的未儲存標記與存檔、衝突橫幅、⌘F 搜尋取代、右鍵選單、樹的展開、側欄搜尋、Markdown 原始碼切換、「<Agent> 也在改」。其中存檔與衝突判斷有 `fsops` 單元測試，互動本身都沒有在實機驗過。

## 已知限制

- 混用 LF 與 CRLF 的檔案：載入時以第一個換行決定，存檔時整份統一成那一種（與 VS Code 相同）。沒有修改時 ⌘S 不會寫入（是否修改在按下時向 Rust 重讀，剛打完字馬上按也算數）。
- 沒有未儲存變更時的自動重新載入用 `set_value`，會清掉該文件的 undo 歷史。
- 修改 app home 裡這幾個 mod 造成的熱重載不會呼叫 `close()`：已開的文件多一個參考（Yuzora 結束前不會釋放），舊的 owner 留在文件上，重載後的 pane 要按「在這裡編輯」接手。
- 側欄的檔名與內容搜尋只丟掉過期結果，背景工作不會取消；大 repo 剛輸入時可能同時跑多個完整列舉。
- 有未儲存變更且檔案已從磁碟刪除時，只能「重新儲存」，沒有「捨棄」。

## 延後到合併後的 follow-up PR

#192 的 Codex P2 可以延後（P0／P1 一律在合併前修好）；延後的項目列在這裡，合併後由 L5 另開 PR 處理。

- Windows 存檔保留 ACL 與其他屬性：目前 rename 換檔後只保留唯讀屬性，ACL 變成資料夾繼承的。改用 `ReplaceFileW`（保留 DACL、屬性、建立時間），失敗時（`ERROR_UNABLE_TO_MOVE_REPLACEMENT`）要把暫存檔放回原位，不能刪；需要 CI 在 Windows 編譯並測試 app（第 24 輪）。
- Windows 上 `doc-pane.js` 的 `parent` 只認 `/`：原生路徑（`C:\…`）的檔案不會即時 watch，只靠 15 秒一次的檢查重新載入；watch 回報的路徑也要用和 `within()` 相同的方式比對（第 23 輪修 agent 判斷時發現）。

## 待辦

- 遠端寫入（見上）。
- 編輯器 gutter 的變更標記（新增／修改／agent 修改的行，設計稿 p2 左側色條）：需要 L2 的 diff API。
- git 標記改用 `yuzora:git`（L2 合併後）。
- Markdown 預覽：相對連結（`./other.md`）開檔、遠端檔案的相對圖片、fenced code 以外的 inline 語法上色（tree-sitter-md inline 語法）。
- 拖放檔案到側欄（複製／移動）。
- pane 標頭整合：目前編輯器 pane 自己畫 28px 檔案列；L1 的 pane 標頭可依 `mod.json` 的 `"yuzora": {"pane": {"header": "own"}}` 隱藏或改用 `documentState` 顯示檔名。
