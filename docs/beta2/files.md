# L5 檔案＋編輯器

> beta.4 起，Files／Git／Database／SSH 是右側 dock 的模式（`panel` 右邊的模式列，沒有 panel 的 layout 是抽屜），不再是左側欄。本文原稱「側欄」「側欄模式」的地方已改稱「右側模式」；截圖與 beta.2／3 時期的設計描述（例如圖中的左側欄）未重寫。

beta.2 的檔案模式、程式碼編輯器、Markdown 預覽與圖片檢視。設計依據：設計稿 p2（檔案畫板）與 p6（右側模式「檔案」模式）。架構規則見 [architecture.md](architecture.md)、[lanes.md](lanes.md)。

## 範圍與檔案

| 路徑 | 內容 |
| --- | --- |
| `app/crates/yuzora-files/src/lib.rs` | `yuzora:files` HostModule：函式、3 個原生元件、TypeScript 宣告 |
| `src/highlight/` | tree-sitter 上色：14 種語言、`InputHighlighter` 實作、One Dark 色盤（mod 以主題色覆寫）、無狀態 `highlight()` API |
| `src/editor.rs` | 可重用的程式碼編輯器：`new_code_editor()`、`code_editor()`、`Look` |
| `src/docs.rs` | 開啟中的文件（每個檔案一份 buffer）、存檔衝突檢查、外部變更、搜尋取代、`Editor` 元件 |
| `src/markdown.rs` | `Markdown` 元件（gpui-base `TextView`，GFM） |
| `src/image.rs` | `ImageView` 元件（PNG／JPEG／GIF／WebP／BMP／TIFF／ICO／SVG） |
| `src/svg.rs`、`src/svg/` | repo 的 SVG 交給子行程點陣化：父行程的請求、快取與並行上限（`svg.rs`）、子行程的監管與資源上限（`process.rs`）、協定（`wire.rs`）、子行程本身（`child.rs`） |
| `src/fsops.rs`、`src/git.rs`、`src/search.rs` | 檔案操作、輕量 `git status`、模糊檔名搜尋與內容搜尋（ripgrep，本機沒有 rg 時用內建搜尋）；本機或 `hc exec --on` |
| `app/defaults/mods/yuzora-sidebar-files` | 右側模式「檔案」模式（含自己開的右鍵 popup） |
| `app/defaults/mods/yuzora-editor` | 編輯器 pane |
| `app/defaults/mods/yuzora-markdown` | Markdown pane（預覽／原始碼切換） |
| `app/defaults/mods/yuzora-files-image` | 圖片與 SVG 檢視 pane |

上游程式碼的改動都有 `// yuzora(L5):` 註記：`app/src/main.rs`（SVG 子行程的入口，在 `main()` 的開頭）、vendored gpui-pre 的 `svg_renderer.rs`（`<image>` 一律不載入、`ParsedSvg::tree()`）與 `assets.rs`（`RenderImage::with_scale_factor`）。共用檔只改了各自的 L5 標記區：`README.md`、`AGENTS.md`、`CLAUDE.md`、`app/defaults/AGENTS.md`，以及 `app/CHANGELOG.md` 的 0.1.0-beta.2 條目。

## 設計

### 三層分工

- **Rust（`yuzora-files`）**：所有磁碟 I/O、git、搜尋都在 worker thread；本機讀檔與「能不能寫」的檢查一律以 nonblocking 開檔、再透過 handle 確認是一般檔案（`open_regular`），檢查後才被換成 FIFO 或裝置也不會卡住 worker；編輯器、Markdown、圖片是 `HostModule::component` 原生元件。
- **JS mods**：版面、標頭、橫幅、搜尋列、右鍵選單、開檔策略。使用者可以 `mod.clone` 修改。

### 文件（`docs.rs`）

- 一個檔案（機器＋路徑）在整個 app 只有一份 `DocView`（GPUI global 的 registry），所有 pane 共用。`openDocument` 每呼叫一次加一個參考，`releaseDocument` 減一。
- **只有一個 pane 畫編輯器**（owner）。同一個檔案在另一個 pane 開啟時顯示「在這裡編輯」，按下才接手；避免同一個 GPUI entity 在同一個 frame 被畫兩次。
- 載入：`fsops::read_text`，換行正規化為 LF，記住原本是 LF／CRLF 與 BOM，存檔時還原。
- **存檔**：同資料夾的暫存檔（`.yuzora-save-<32 hex>`）fsync 後 rename 蓋過去（蓋過既有檔案時，暫存檔先以 0600 建立，最後一次比對完、rename 之前才套用原檔的權限：寫入期間原檔擋掉的人打不開它；存檔前確認可寫入時也以一般檔案開啟，換成 FIFO 不會等），保留權限、owner、ACL 與延伸屬性（Finder 標籤、resource fork；macOS 用 `fcopyfile`，Linux 逐一複製 xattr，POSIX ACL 也在其中，新檔自己從資料夾繼承、原檔沒有的會移掉）；Windows 目前只保留唯讀屬性，ACL 與其他屬性會變成資料夾繼承的（待改用 `ReplaceFileW`）。資料夾不能建暫存檔、或 owner、ACL、延伸屬性保不住時退回就地寫入：先把舊內容分塊複製到備份（不整份讀進記憶體；同資料夾，資料夾不可寫時放 app home 的 `backups/files`（0700）；一律新建檔案（O_EXCL，不寫進既有路徑或 symlink），只有擁有者可讀），新內容寫完才刪；寫入失敗時寫回舊內容（從建立備份時的同一個 handle 讀回，不用名稱重開：資料夾可能別人也能寫入），連寫回都失敗就留下備份並在錯誤裡指出位置。symlink 寫到它指向的檔案；指向的檔案已刪除時（還原），沿著連結寫回原本的位置。寫入前比對載入時的 `{len, mtime}`，不同就回報衝突、不寫入；一樣時再比對內容 hash（時間精度粗的檔案系統上，同長度的改寫可能 stamp 不變）。暫存檔寫完、rename 之前再比對一次（就地寫入則是備份複製完、覆寫之前），寫入或複製備份期間別人改了檔案也會回報衝突並保留對方的內容；rename 沒有「比對後才替換」的原子操作，只有最後一次比對到 rename 之間的改動仍會被蓋掉。
- **外部變更**：本機用 herdr 的 `watchFolder`（檔案所在資料夾）＋15 秒輪詢，遠端 5 秒輪詢，呼叫 `checkDisk`。stamp 變了才讀內容並比對 hash。只到秒的 stamp（遠端的 `stat`、時間精度粗的檔案系統）在同一秒內被改寫成同樣長度時不會變；更細的 stamp 也一樣，在時鐘的同一個 tick 內（stamp 只到毫秒，Linux 的 kernel tick 可能好幾毫秒）改寫也不會變。所以只到秒的 stamp，以及看到時修改時間還不到 2 秒的 stamp，在第一次看到 2 秒後的讀取確認內容相同之前，每次檢查都讀內容；這段期間 chmod 也照樣會被發現。檢查開始後文件被重新命名的，結果不套用（載入也一樣，改用新路徑重讀）：
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
- 圖片只讀本機的一般檔案（不是資料夾、裝置或 FIFO）且不超過 32 MiB；點陣圖解碼後不超過 512 MiB，SVG 交給子行程畫（見下方「repo 的 SVG」）；網路、`data:`、Windows UNC（`//host/share`）與遠端文件的圖片一律不讀。檢查在 worker 做：讀出 bytes（最多 32 MiB，不多讀）、檢查解碼大小，交給 GPUI 的就是這份 bytes（SVG 是子行程用它畫好的圖）而不是路徑，所以讀完之後檔案變大或被換掉也繞不過限制。畫面只讀快取，10 秒後重新 `stat`，stamp 變了才重讀，同一路徑換了圖也會更新；最後一個 frame 沒畫到、而且 1 分鐘沒畫到的圖片連同 bytes 放掉（SVG 連同它的 texture）（之後沒有新的 frame 也會在 1 分鐘後放掉；捲出畫面的圖片捲回來時重讀）。所以圖片在檢查回來後才出現。畫面上有找不到的圖片時，10 秒後會自己重畫再找一次，之後才建立的圖片也會出現。相對圖片從文件所在的資料夾找；文件被重新命名或移到別的資料夾時跟著換。Windows 上 `file:///C:/…` 指的是 `C:/…`。連結只開 http、https、mailto。
- 預覽的文字、連結、程式碼底色跟隨目前的主題（`theme()` 的 foreground、muted、ansi4、raised、border）；code block 的語法色和編輯器相同（`syntax`），換主題時 code block 重新上色（文字的上色快取以 highlighter 為 key，只有語法色變了才換一個）。
- 圖片 pane：PNG／JPEG／GIF／WebP／BMP／TIFF／ICO／SVG。GPUI 會把 GIF、動態 WebP 的每一幀都解碼成整張畫布（每像素 4 bytes）並留著，所以先用檔頭的尺寸與幀數（GIF 的 image block、WebP 的 `ANMF` chunk，不解碼）估算，超過 512 MiB 就不顯示；檔頭解析不了的交給解碼器。SVG 見下一節。

  檢視有「符合」與「100%」（原尺寸，比 pane 大時可捲動）、對比底色（`th.solid`：淺色主題是深底、深色主題是淺底，上面的提示文字用 `solid_foreground`）；檔案變了自動重新顯示：本機的 watch 事件指到這個檔案就重讀（不看大小與時間）；遠端只有秒級時間，看到新的 stamp 後 2.5 秒再讀一次，補上同一秒內的改寫。遠端圖片同樣只讀一般檔案。

### repo 的 SVG（子行程點陣化）

在 App 行程裡解析 repo 的 SVG，前三輪審查每補一種上限（巢狀深度、參照循環、節點數、layer 與 filter 的像素預算），下一輪就找到新的放大方式，用特製的小檔案耗盡記憶體、CPU 或 stack，讓整個 App 中止。所以圖片 pane 與 Markdown 預覽的 SVG 一律交給子行程點陣化，App 本體不解析，原本的檢查與預算都已刪除。

- **子行程**：同一支執行檔（`std::env::current_exe()`）加隱藏參數 `--yuzora-rasterize-svg`。`app/src/main.rs` 的 `main()` 一開頭看到這個參數就交給 `yuzora_files::svg::child_main()`，不做其他初始化。沒有新增 binary，打包不變。
  - 子行程（`src/svg/child.rs`）從 stdin 讀一份 SVG，用 GPUI 自己的 `SvgRenderer` 在 16 MiB stack 的 thread 上畫（usvg＋resvg、相同的字型；`<image>` 一律不載入，見下方），長邊不超過 4096 px（GPUI 畫 SVG 用兩倍尺寸），寫到 stdout 後結束。
  - env 清空，只留找系統字型需要的變數：macOS 留 `HOME`；Linux 留 `HOME`、`XDG_CONFIG_HOME`、`XDG_DATA_HOME`、`XDG_DATA_DIRS`、`FONTCONFIG_FILE`、`FONTCONFIG_PATH`；Windows 留 `SystemRoot`、`USERPROFILE`。
  - Unix 上子行程先設 `alarm(10)`：父行程不在了（例如 App crash），它也不會一直跑下去。
- **父行程**（`src/svg.rs`、`src/svg/process.rs`）只做不需要解析的檢查：大小不超過 32 MiB；開頭是 gzip magic（`1f 8b`）或副檔名是 `.svgz` 就拒絕（usvg 會整份解壓，沒有上限）。usvg、resvg、roxmltree、simplecss 都不是 `yuzora-files` 的依賴，父行程的程式碼也不呼叫 `SvgRenderer`；測試會檢查這兩點。
  - 收到的 RGBA 換成 GPUI 用的 BGRA，scale 和 GPUI 畫的 SVG 一樣是 2（vendored gpui-pre 的 `RenderImage::with_scale_factor`）。一般 SVG 的結果和改版前在 App 裡畫的逐 byte 相同（整合測試比對）。
- **協定**（`src/svg/wire.rs`，數字都是 little-endian u32）：
  - stdin：`YZSVGIN1`、長邊上限、SVG 的長度，接著是 SVG。
  - stdout：`YZSVGPX1`、狀態（0 畫好、1 usvg 讀不了或畫不出來）、寬、高；畫好時接著是寬×高×4 bytes 的 RGBA（straight alpha）。
  - 兩邊都只讀 header 指定的長度，不等 EOF：pipe 的一端被別的子行程意外繼承時，也不會卡住。

#### 上限（`src/svg.rs` 的常數）

| 常數 | 值 | 做法 |
| --- | --- | --- |
| `TIMEOUT` | 5 秒 | 牆鐘時間，從子行程啟動起算（排隊的時間不算），超過就殺。 |
| `MEMORY_LIMIT` | 512 MiB | macOS：每 10 ms 用 `proc_pid_rusage` 讀 `phys_footprint`，超過就殺。Linux：`pre_exec` 裡 `setrlimit(RLIMIT_AS)`。Windows：Job Object 設 `JOB_OBJECT_LIMIT_PROCESS_MEMORY` 與 `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`，送出任何資料前就把子行程放進 job。 |
| `MAX_EDGE` | 4096 px | stdout 最多讀 header 加上寬×高×4。header 的寬或高超過上限（或是 0），或在那之後多送任何一個 byte，都算輸出格式錯誤。 |
| `CONCURRENT` | 2 | 同時最多 2 個子行程，其餘排隊。 |
| `MAX_SVG_BYTES` | 32 MiB | 和圖片的大小上限相同。 |

- 每個子行程在父行程佔用的記憶體：像素最多 64 MiB，加上 64 KiB 的讀取緩衝與 4 KiB 的 stderr。
- spawn 時持有行程內的一把鎖：macOS 的 std 先建 pipe，再設 close-on-exec，平行 spawn 時，另一個子行程可能繼承到這個 pipe 的一端。std 在 `spawn` 裡就關掉父行程中屬於子行程的那一端，所以這一步在鎖內完成。
- 子行程樹：Unix 上子行程是自己 process group 的 leader，Windows 上它在自己的 Job Object 裡。殺子行程時連同它啟動的行程一起殺；子行程自己結束時，剩下的行程也一併清掉。

#### 失敗時

- 逾時、超過記憶體、crash（非 0 結束或被 signal 終止）、輸出格式錯誤：殺掉子行程樹，回傳原因。Linux 與 Windows 上，超過記憶體上限時配置會失敗，Rust 會在 stderr 寫 `memory allocation of …` 後 abort，父行程據此判為記憶體不足（stderr 只留前 4 KiB）。
- 圖片 pane 顯示佔位圖（灰框打叉，由父行程直接產生，不是 SVG）和原因：`image.svg_timeout`、`svg_memory`、`svg_crashed`、`svg_output`、`svg_invalid`、`svg_spawn`、`svgz`，都在 `i18n/en.json`。
- Markdown 預覽顯示同一張佔位圖。`TextView` 的圖片欄位只接受圖片，原因的文字無法放進預覽。
- 不自動重試。結果依內容的 hash 快取（行程內隨機金鑰，無法造出相撞的檔案），同一份 SVG 只 spawn 一次，失敗的結果也記著；檔案內容改了才重畫。多個請求同時要同一份 SVG 時，等第一個的結果。快取最多 128 MiB 的像素、512 筆，先進先出。取消與子行程無法啟動的結果不快取。
- 取消：圖片 pane 換檔或關閉、Markdown 預覽關閉或換文件時，排隊中的請求直接放棄，畫到一半的子行程被殺。

#### `<image>` 與 mod 的 SVG

SVG 裡的 `<image>` 一律不跟：usvg 預設會把它指的任何本機路徑整個讀進來（不管大小，`/dev/zero`、FIFO、UNC 分享都會照讀）；data URI 也一律不解：點陣圖會被 resvg 依檔頭宣稱的尺寸解碼（60000² 的 PNG 只要幾十 bytes），SVG 會用同一個 resolver 一層層 parse（75 層就讓 GPUI 的解碼 thread 爆 stack、整個 app 結束；50 次 `<use>` 同一個內嵌 SVG 要 25 秒 CPU），gzip 的也照樣整份解壓。所以 vendored gpui-pre 的 `SvgRenderer`（`svg_renderer.rs`，`// yuzora(L5):`）改用自己的 `image_href_resolver`，什麼都不載入；同一處的 `ParsedSvg::tree()` 讓子行程讀出尺寸，決定縮放。這是全域設定，所有 mod 的 `svg()`／`img()` 也一樣；內建的 SVG 都沒有用到 `<image>`。mod 自己的 SVG 仍由 GPUI 在 App 裡解碼（它們是 app 或 mod 的資產，不是 repo 的檔案），不經子行程。畫好的 SVG 是一張 texture：換圖、圖片不再顯示、pane 或預覽關掉時用 `drop_image` 放掉。

#### 已知限制

- Linux 的 `RLIMIT_AS` 限制的是位址空間：執行檔、shared library、thread stack 的 mapping 都算在 512 MiB 裡，實際能用的 heap 比 macOS、Windows 少。App 本體在 Linux 上的基本用量尚未量測。
- macOS 的記憶體上限靠輪詢：兩次輪詢之間（10 ms）子行程可能多配置一些才被殺。
- 每個子行程第一次畫文字時都要載入系統字型。
- Windows 的 Job Object 實作只做過交叉編譯檢查，沒有實際執行（`app-ci.yml` 的 Windows job 只跑 console 視窗測試，不跑這裡的）。

### 右側模式（`yuzora-sidebar-files`）

- 跟隨目前 focused pane 的 Space 與 cwd（沿用上游 `files` mod 的定位方式）；focused 的是工具 pane（沒有 cwd）時保留原本的資料夾。在 repo 裡時以 `git rev-parse --show-toplevel` 為根。換到別的資料夾時，取消正在輸入的新增或重新命名（它屬於原本的資料夾）；搜尋開著時，在新的根目錄重跑；樹重新整理時（watch 事件、手動、遠端的 10 秒輪詢）也重跑，檔名清單重新列出。本機有 watch 時，10 秒計時不重跑搜尋。
- 標題（2026-11-03 改版，B1「路徑卡」，取代同日的設計 D；標題列和其他右側模式一樣）：
  - 標題列：照資料庫、SSH 模式的寫法，高 32、左右 12：左邊是「檔案」（右側模式變窄時先截斷），右邊 6 顆 22×22 ghost 圖示鈕（間距 2）：新增檔案、新增資料夾、重新整理、全部收合、搜尋、顯示隱藏檔（記在 localStorage；切換時重跑搜尋）。按鈕平常沒有底色、muted 字，hover 時 `hover` 底＋foreground，開著的 toggle 用 `selected` 底＋foreground；按鈕不縮，不換行也不溢出。遠端資料夾時新增檔案、新增資料夾停用（dim、不能點），tooltip 說明遠端在這一版是唯讀。不再顯示 Space 徽章。
  - 路徑卡（標題列下方 4px、左右 10px；`raised` 底、1px `border`、圓角 7）：第一行是資料夾 icon、上層路徑（`~` 形式、結尾帶分隔符、dim）與資料夾名稱（粗體），空間不夠時先縮上層路徑，名稱盡量完整；最右邊是「本機」或機器名的 chip（`hover` 底，和卡片的 `raised` 分得開）。第二行是 git 分支 icon＋分支名（accent、11.5px、過長截斷），不是 repo 或沒有分支時不顯示。拆法在 `paths.js` 的 `heading()`（純函式）：家目錄本身顯示 `~`、根目錄（`/`、`C:\`）只有名稱；`~` 只用在本機，遠端路徑照原樣。沒開資料夾時沒有路徑卡，樹照舊顯示空狀態。
  - 沒有新 token，卡片上的字都用主題色：dim、foreground、accent 在 `raised` 上，深色 5.84／12.37／6.17、淺色 6.34／16.70／6.00；chip 的 muted 在 `hover` 上，深色 6.10、淺色 6.55。由 `yuzora-core` 的 `both_yuzora_themes_read_at_wcag_aa` 守著。
- 樹：`uniform_list` 虛擬捲動，每個資料夾最多 2000 項（本機列資料夾時邊讀邊排序，只留前 2001 項，超大的資料夾也不會整份放進記憶體）、總共 20000 列（超過時樹下方提示收合一些資料夾）；根目錄被截斷、而且留下的 2000 項都是隱藏檔（隱藏檔沒有顯示）時，樹不寫「沒有檔案」，改提示前 2000 項都是隱藏檔；名稱不是 UTF-8 的項目不列出（本機與遠端都一樣：用替代字元拼出的路徑可能指到旁邊的檔案；git 狀態裡這種路徑也不標記），讀不到類型的項目也不列出、讀到一半失敗就停（讀的期間被刪掉的不算），這些情況樹下方都會提示有項目無法讀取或顯示；git 標記 M／A／D／U／?，收合的資料夾若含變更顯示 `•`；變更超過 20,000 個路徑（或 git 的輸出超過 16 MiB）時，樹下方提示「變更太多，有些變更的檔案沒有 Git 標記」。每列依類型顯示 icon（`icons/files/*.svg`，自己畫的 16×16、stroke 1.4 單色遮罩，與模式列一致）：資料夾分開啟／收合，常見副檔名與特殊檔名（`Cargo.toml`、`package.json`、`Dockerfile`、`.gitignore`、lock 檔⋯）各有 icon，其他用預設檔案 icon。對照表在 `icons.js`（純函式，mod 載入時建表一次，每列只查表），顏色是主題色的名稱（`ansi0`–`ansi15`、`muted`、`dim`），淺色與深色主題都跟著換；搜尋結果用同一個對照表。`watchFolder` 監看根目錄＋10 秒重新整理；收合過的資料夾再展開時，先顯示上次的內容並重新列出。根目錄重新列出失敗時（被刪除或沒有權限），樹上方顯示錯誤，下面是上次列出的內容。
- 未信任的資料夾：repo 自己的設定會執行程式（例如 `git status` 時跑 clean filter），而且還沒在 Git 模式信任時，不執行 `git status`（遠端也一樣）。偵測規則與信任清單沿用 L2（`yuzora-core`）。這時分支用 `symbolic-ref`、ahead／behind 用 `rev-list --left-right --count` 取得，這兩個指令都不碰工作樹。partial clone（promisor remote）缺 commit 時，`rev-list` 會用 repo 自己的 ssh command 去 fetch，所以改加 `--no-lazy-fetch`（git 2.44 以上；更舊的 git 不顯示 ahead／behind）。樹上不顯示 Git 標記，改顯示一行「未信任的資料夾，在 Git 模式信任後才會顯示 Git 標記」。信任後，10 秒內的下一次重新整理就會出現標記。遠端的檔名搜尋清單也一樣：未信任時不用 `git ls-files`（它會讀工作樹與 index，partial clone 的 sparse index 展開時還可能 fetch），改用 `rg --files` 或 `find`。判斷在 `git.rs` 的 `work_tree_open`，`gitStatus` 與檔名清單共用。信任檢查讀的是從 top level 找到的 repo，但 `core.worktree` 可以讓 top level 指到另一個 repo（例如 `evil/.git` 設了 filter，工作樹卻是乾淨的 `clean`），這時檢查的是 `clean`，`git status` 卻用 `evil` 的設定。所以先比對兩邊的 `rev-parse --absolute-git-dir`（從資料夾本身、從 top level），不同就當成未信任，信任 top level 也不會打開。檢查本身（L2 的 `git_trust::pending`）在未信任時每次都跑：讀 repo 的設定，並用 `ls-files -s` 掃 submodule（`GIT_NO_LAZY_FETCH=1`）。
- 不論信任與否，yuzora-files 跑的每個程式（本機與 `hc exec --on`）都帶 `GIT_NO_LAZY_FETCH=1` 與空的 `GIT_ALLOW_PROTOCOL`（`fsops::OFFLINE`，一律經 `fsops::run`）。檔案樹的標記與搜尋不需要連任何 remote：partial clone 缺物件時 git 不去 fetch，也不會依 promisor remote 的 URL 叫起 remote helper（例如 `zz://…` 對應 PATH 上的 `git-remote-zz`）；`GIT_ALLOW_PROTOCOL` 是環境變數，repo 的 `protocol.*.allow` 放寬不了。空值連 file 也不放行，所以也擋住 2.44 以前不認得 `GIT_NO_LAZY_FETCH` 的 git：promisor 是本機 repo 時，它會走 file transport 去抓，並執行 repo 自己設定的 `uploadpack`（原本放行 file 時擋不住，`offline_stops_an_older_git_fetching_from_a_local_promisor` 以拿掉 `GIT_NO_LAZY_FETCH` 的 git 模擬）。代價：缺 tree 的 clone（`--filter=tree:0`）裡 `git status` 會失敗，不會去 fetch。帶環境變數用的是 `yuzora_core::remote::ExecRequest::env`（遠端轉成 `hc exec --env`）。
- 樹的狀態與切換（v0.1.0 F1）：`tree-state.js` 是沒有 import 的純模組（主機呼叫由 `main.js` 傳入），`main.js` 只負責畫。每個（機器、資料夾）一份狀態，面板只有三種（`panel()`）：沒有資料夾、`empty`（根目錄還沒列過；只有這時寫「Loading files…」，列根目錄失敗就寫原因）、`showing`（已有根目錄的列，新鮮度是 `fresh`／`revalidating`／`{stale: 原因}`）。有快取卻顯示 loading 因此無法表示。
  - 切回看過的 Space、或重新打開 Files：第一個畫面就是上次的樹（展開與選取都在），同時在背景重讀。讀回來的結果逐項比對（名稱、種類、連結、隱藏；檔案大小變了不算），只有變了的資料夾才換、才重畫；沒變就不重畫（10 秒計時也不再每次重畫）。
  - 第一次顯示：git、根目錄與已展開的資料夾同時發出（列資料夾最多 6 個同時），根目錄列到就畫，git 標記之後補上。樹的根在 git 回答前先用資料夾本身；git 說 repo 的 top 在別處（資料夾是 repo 的子目錄）時，清掉暫時的列表，從 top 重列，只發生在第一次顯示那個資料夾。遠端的 `gitStatus` 還是 4 個循序 `hc exec`，但不再擋在列目錄前面。
  - 每次讀取（列資料夾、`gitStatus`）取一個遞增序號，結果只有比該項目已套用的新才套用（展開資料夾與重新整理同時列同一個資料夾時會碰到），而且只落在它自己的那份狀態：狀態已被擠出或 Space 已關閉就丟掉。同一份狀態同時只有一個重新整理在跑（既有行為）。
  - 上限：保留最近顯示的 8 份樹（含 60,000 個項目與標記）；多出來的丟掉樹，但記得根、展開與選取（最多 64 個資料夾）。Space 關閉時（10 秒計時檢查）清掉它的資料夾。
  - 信任關卡不變：每次重新讀取都呼叫 `gitStatus`（信任檢查在 Rust 裡，`git::status`），快取不回答它。快取裡的標記只是上一次的答案：未信任的資料夾本來就沒有標記；信任被收回時，新答案一到標記就清掉。
  - 過期的列：重新讀取還沒完成時點到檔案，先 `stat` 確認還在；不在就從清單拿掉並提示（`error.gone`），不會開出空檔。展開資料夾列不出來時也檢查，確實不在就移除。
  - 捲動位置：同一份狀態重讀時不動。`uniform_list` 的捲動位置是 GPUI 以 id 保管的，JS 讀不到也設不了，所以切到別的資料夾、或離開 Files 模式再回來，會回到最上面（切換前就是如此）；每份狀態用自己的 list id（`tree:<機器>|<資料夾>`），不會把別的資料夾的位置帶過來。要還原得換成 `v_virtual_list`＋`VirtualListScrollHandle` 並記下頂端的列，需要實機看版面，這一版沒做。
  - 量測（無視窗）：`app/crates/yuzora-files/examples/reload_cost.rs`＋`app/scripts/files_bench.py`：2 種 repo 大小（約 5k、30k 檔）× 展開 0 或 8 個資料夾 × 本機或模擬遠端（隔離的 hc session，`--via 'sh -c'`；`--rtt-ms` 替每次 `hc` 加延遲），輸出 T_tree（根目錄的列可畫）、T_full（所有已展開資料夾列完且 git 標記到齊）、子行程數與 `hc` 往返數；舊序列與新序列交錯執行，機器忙時兩邊受同樣影響。

- 搜尋：檔名（模糊比對，`.gitignore` 生效，清單快取 15 秒（從開始列的時間算；較早開始、較晚結束的清單不會蓋掉較新的），最多列 200,000 個檔案，超過時提示「只搜尋了前 200,000 個檔案」；符合的檔案超過 200 個時只顯示分數最高的 200 個，並提示「只顯示前 200 筆結果」）與內容（有 `rg` 就用 ripgrep；本機沒有時改用內建搜尋：`ignore`＋`regex`、同樣的規則，每個檔案最多讀 2 MiB（檢查大小之後檔案又變大也一樣），內容不全是 UTF-8 的檔案照樣搜尋（壞掉的 byte 顯示成 U+FFFD，和 rg 的行一樣），一行有很多比對時只標出顯示的前 300 字內的比對（不在後面多花時間）；遠端沒有 `rg` 時顯示提示）。ripgrep 把不是 UTF-8 的行給成 base64 `bytes`：解碼後以替代字元顯示；不是 UTF-8 的路徑沒辦法用名稱開啟，不列出（內建搜尋、本機與遠端的檔名清單也一樣，不用替代字元拼出可能指到別的檔案的路徑，並提示結果不完整）。有檔案或資料夾讀不到時（ripgrep exit 2、路徑不是 UTF-8、`find` 或 git 失敗（還沒列出任何檔案也算）、本機走訪錯誤），保留找到的結果並提示「有些檔案或資料夾無法讀取，結果可能不完整」。點內容結果會開檔並跳到該行。
- 右鍵：同一個 mod 以 popup 開啟自己（`props().menu`）。開啟、在終端機開啟、新增檔案／資料夾、重新命名、刪除到垃圾桶、複製路徑／相對路徑、在 Finder（檔案總管／檔案管理員）中顯示。新增與重新命名透過 `shared("yuzora.files.request")` 交回主模式，在樹上方顯示輸入列。重新命名不會覆蓋已存在的檔案或資料夾：macOS 與 Linux 用一次系統呼叫完成（`renamex_np(RENAME_EXCL)`／`renameat2(RENAME_NOREPLACE)`），檢查與搬移之間不會被別的程式搶先建立同名檔案；其他平台與不支援的檔案系統則先檢查目標。新路徑上還開著文件時（檔案已刪除、pane 還在），拒絕重新命名並請先關掉它，避免同一個檔案出現兩份緩衝。開著的文件（該檔案或資料夾底下的）正在存檔時拒絕重新命名；重新命名進行中，這些文件也不能存檔（回 `busy`），存檔不會寫回舊路徑。重新命名後，開著的文件（只算該路徑本身與它底下的；只有 Windows 把 `\` 當分隔符，其他平台的 `foo\a.txt` 是 `foo` 旁邊的檔案）會換到新路徑，pane 繼續顯示，不會被當成已刪除；換了副檔名時，語言徽章與上色跟著新檔名，編輯器（含未存的修改與復原紀錄）不變。

### 開檔

- 圖片／SVG → `yuzora-files-image`；`.md` → `yuzora-markdown`（預設預覽）；其他 → `yuzora-editor`。
- 同一個檔案已有 pane 顯示時，`pane.focus` 過去；否則把 focused pane 往右分割，沒有 focused pane 時開新 tab。
- pane 不知道自己的 pane id，所以每次編輯器取得焦點時，用 `focused().pane` 記到文件上（`notePane`），之後才能 focus 回來。圖片沒有文件：右側模式在 `pane.split` 之後記下取得焦點的新 pane，下次開同一張圖時 focus 過去（只記得從右側模式開的圖片）。從右側模式重新命名這張圖（或它所在的資料夾）時，pane 的 props 改不了：在舊 pane 右邊開一個新路徑的圖片 pane、再關掉舊的，新 pane 留在原來的位置，焦點不變。
- 帶 `line` 開 `.md`（例如內容搜尋的結果）時以原始碼開啟並跳到該行；預覽／原始碼的偏好不變。檔案已經開在預覽的 pane 時，右側模式用 `revealLine` 告訴文件，那個 pane 換成原始碼，等它的編輯器建好後再跳過去（同樣 10 秒內有效）；pane 開啟之前的請求不套用。文件還在載入（還沒有編輯器）時，這一行先記在文件上，編輯器建好時（10 秒內）再跳過去。
- 路徑 helper（`yuzora-sidebar-files/paths.js`）：Windows 路徑（`C:\…`、`\\server\…`）`/` 與 `\` 都當分隔符；其他平台的 `\` 是檔名的一部分。
- 跳行與聚焦只在開啟後 10 秒內套用（props 的 `opened`），重啟還原 pane 時不再跳。手動 `pane.split` 只帶 `line`、沒有 `opened` 時也會跳：跳過的請求（機器＋路徑＋行列）記在 `shared("yuzora.files.went")`，紀錄早於這次啟動（`startedAt()`）就是重啟還原的 pane，不再跳也不搶焦點；同一次執行裡再開同樣的請求則照跳。紀錄保留 7 天。

### 共享狀態

- `yuzora.badge.files`：未儲存的文件數（沒有時 `null`）。
- `yuzora.status.files`：有未儲存文件時 `{text: "N 個未儲存", tone: "blocked"}`，否則 `null`。
- `yuzora.files.request`、`yuzora.files.refresh`：右側模式 popup 與主模式之間的請求。

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

- 讀取與瀏覽：`listDir`、`gitStatus`、`findFiles`（`git ls-files`（未信任的資料夾不用），扣掉工作樹已刪除的檔案／`rg --files`／`find`）、`grep`（遠端 `rg`）、開檔、圖片，都走 `yuzora_core::remote`（`hc exec --on`，各自的 timeout）。遠端 `stat` 只在確認不存在時才回報「不存在」：往上找到第一個存在的資料夾，而且能進入。路上有進不去的資料夾時回傳錯誤，文件不會被標成已刪除，與本機相同。遠端既沒有 git 也沒有 rg 時，`find` 不認得 ignore 檔：`findFiles` 回傳 `unfiltered: true`，右側模式提示結果包含被忽略的檔案。
- **存檔已支援，新增、重新命名、刪除還沒有**（v0.1.0 R1）。遠端文件可以編輯，存檔走 `hc exec --on <機器> --stdin -- sh -c <腳本> sh <路徑> …`（`yuzora_core::remote::ExecRequest::stdin`；hc 0.1.19 起有 `--stdin`，上限 8 MiB、二進位安全）。新內容只走 stdin，路徑只走 argv，腳本不拼進任何輸入。`createFile`／`createFolder`／`rename` 帶機器名稱仍一律拒絕（R2），刪除沒有遠端版本（R3）。
  - 腳本（`remote_write.rs` 的 `WRITE_SCRIPT`，POSIX `sh`，本機與遠端同一份）的順序：解開符號連結（寫到連結指向的檔案，連結本身保留，與本機相同）→ 不是一般檔案、檔案或資料夾不可寫就拒絕 → 在同資料夾建立 `.yuzora-save-<32 位小寫 hex>`（與本機暫存檔同一樣式，`is_scratch_name` 與 `LIST_SCRIPT` 都不列出）、`umask 077`，收完 stdin，位元組數與送出的不符就放棄（不換入）→ 套用原檔 mode；owner 或 group 與原檔不同時嘗試 `chown`，失敗就拒絕 → **換入前最後一步**比對遠端現況與開檔時的 size、mtime 秒與 POSIX `cksum` 的 CRC32，不符就回報衝突 → 單一 `mv -f` 換入。新 stamp 在換入前由暫存檔取得（rename 保留 mtime 與大小），所以換入後沒有再被改寫的空窗。
  - 原子性：新內容完整寫進暫存檔之後才換入，換入是一次 rename，遠端的檔案不是舊的就是新的。任何一步失敗（連線斷、逾時、磁碟滿、權限不足、`chmod` 失敗），暫存檔由 `trap` 刪除；被 SIGKILL（逾時）時可能留下 `.yuzora-save-*`，名稱符合暫存檔樣式，樹與搜尋都不列，下次存檔不會清它。
  - 衝突：stamp 加 CRC32 都相同才算沒變（stamp 相同、內容不同也擋下，碰撞機率約 2⁻³²）。比對與換入在同一支腳本裡，衝突時遠端檔案不動，走本機既有流程（保留我的、重新載入、覆蓋）。FNV（`TextFile::hash`，本機用）在 `sh` 裡算不出來，所以遠端讀檔另外記 `TextFile::crc`。「覆蓋」與「再存一次」（強制存檔）不比對，檔案不見時會建立。比對與 `mv` 之間仍有一次行程啟動的窗口，在這之間寫進去的內容會被蓋掉，與本機的 rename 相同。
  - 拒絕（`Refusal`，原因顯示在存檔失敗的通知裡，不會靜默失敗）：超過 8 MiB（送出前就拒絕）、路徑不是以 `/` 開頭、那台機器沒有 `sh`（Windows，hc 回 `cannot start sh`）、herdr-core 太舊（hc 不認得 `--stdin` 時把它當成程式名稱，回 `cannot start --stdin`；對方 core 收到卻沒轉給程式時，腳本發現收到的位元組數少於送出的）、資料夾不可寫、檔案不可寫、檔案屬於別的使用者（換入會改 owner；本機會退回就地寫入，遠端沒有）、不是一般檔案。能力用 hc 的回應判斷，沒有寫死版本號。
  - 遠端檔案開啟時唯讀的條件：腳本回報不能存（檔案或所在資料夾不可寫、owner 不是自己且不是 root），或檔案大於 8 MiB。符號連結看連結指向的檔案所在的資料夾。權限在開檔之後才改變時，存檔才會被拒絕並說明原因（本機有 chmod 偵測，遠端沒有）。
  - 已知限制：換了 inode，所以 ACL、extended attributes、SELinux context 不保留，硬連結會分開（本機的原子存檔對 ACL 與 xattr 會退回就地寫入，遠端不這麼做，因為就地寫入不是原子的）；沒有 fsync（遠端 `sh` 沒有可攜的做法）；連線在換入之後、答覆回來之前斷掉時，編輯器會顯示失敗，但檔案其實已經寫入，下一次檢查會看到磁碟內容與記憶不同而跳衝突提示（保留我的或重新載入都不會丟內容）；busybox 的 `stat`、`od`、`readlink`、`cksum` 沒有實測。
  - 不走 SFTP：L4 的 SFTP 路線不再是前提。

## 驗證

- `cargo test --locked -p yuzora-files`（單元測試 176 個，另有 `tests/svg_child.rs` 的 13 個）：上色（每種語言的首選 query 都能編譯並產生 span、增量解析＝重新解析、摺疊、色盤）、檔案操作（大資料夾邊讀邊截斷、原子存檔、衝突（含 stamp 相同但內容不同、寫入期間被改）、CRLF／BOM、二進位與大檔、遠端 sh 腳本在本機跑出與原生相同的結果、遠端存檔（位元組原樣含 NUL 與非 UTF-8、CRLF 與 BOM、衝突含 stamp 相同內容不同與存檔期間被改、任一步失敗與逾時後原檔不動且沒有暫存檔、mode、符號連結、怪檔名、拒絕原因、舊 core 與沒有 sh、8 MiB 上限、`cksum` 與 `posix_crc` 一致，用假 `hc` 走 `--stdin` 的整條路徑；真 hc 的端到端是 `#[ignore]` 測試）、名稱不是 UTF-8 的項目不列出並提示（遠端解析時其他檔案的大小不錯位；本機要能建立這種檔名的檔案系統，APFS 上略過）、讀不到類型或讀到一半失敗的項目（假的資料夾項目）、原子存檔保留延伸屬性與 ACL（確認換了 inode；ACL 只在 macOS 驗）、就地寫入在備份複製完才比對（複製期間的外部修改不被覆寫、備份刪掉）、存檔的暫存檔在換上去之前只有擁有者能開、遠端圖片只讀一般檔案、圖片解碼大小（GIF 幀數、WebP `ANMF`）、SVG 的 `<image>`（用 GPUI 自己的 `SvgRenderer`、在 536,576 bytes 的 stack 上畫：真實紅色 PNG 的絕對路徑、開頭是紅色 PNG 的 64 MiB 檔案、Windows 的 `\\?\` 長路徑都不讀；FIFO 不會卡住、`/dev/zero` 不會讀；data URI 的點陣圖、60000² 檔頭的 PNG、內嵌的 SVG（base64 與百分比編碼）、gzip 都不解，75 層內嵌不會爆 stack）、repo 的 SVG 由子行程畫（單元測試的子行程是一律回一個綠色像素的 `sh`：圖片 pane 與 Markdown 預覽畫紅色的 SVG 得到綠色，本行程畫 SVG 的次數為 0；父行程的程式碼不出現 usvg、resvg、roxmltree、simplecss、`SvgRenderer`，這些也不是 crate 的依賴；gzip 與 `.svgz` 不啟動子行程；畫不出來的 SVG 在 Markdown 顯示佔位圖、檔案沒變不重試；預覽關閉時取消；圖片 pane 換檔或關閉時取消；協定的 header 與長度檢查；快取的位元組與筆數上限）、Markdown 的 SVG 換掉、放掉與預覽關掉時釋放 texture、遠端變更一律拒絕）、git porcelain 解析（不是 UTF-8 的路徑不標記）與真 repo（不改動 `.git`；遠端清單扣掉已刪除的追蹤檔；未信任的資料夾不跑 repo 的 clean filter、只讀 refs 時不 lazy fetch、遠端檔名清單不用 git，信任後 git 照常執行；promisor 的 URL 指到 PATH 上的 remote helper、又缺物件時，`git status` 與 sparse index 的 `ls-files` 都不叫起它，原本的請求會叫起（oracle）；每個請求都帶 `OFFLINE`，而且只有 `fsops::run` 對外執行程式；`core.worktree` 指到另一個 repo 時，信任 top level 也不會跑那個資料夾的 filter）、模糊搜尋排序（超過上限時回報還有更多）與檔名清單快取（舊的不蓋新的）、rg JSON 解析（含空字串比對的欄位、不是 UTF-8 的行與路徑）與內建搜尋（一行很多比對時只看顯示範圍；不全是 UTF-8 的檔案照樣找得到；不是 UTF-8 的路徑不列出：要能建立這種檔名的檔案系統，APFS 上略過）、文件載入／外部變更自動重載（含 stamp 同一秒內的改寫、剛修改的 stamp 在毫秒內被改寫、確認期間的 chmod）／刪除／共享／釋放、改名前開始的檢查不套用、存檔與重新命名不重疊、載入中要求的行在編輯器建好後套用、開檔不等 FIFO、在測試視窗裡真的編輯後遇到外部變更（衝突、保留我的、存檔）與存檔時才發現變更或檔案已刪、Markdown 圖片與連結規則（交給 GPUI 的是檢查過的 bytes、同一路徑換圖會更新、沒有副檔名時看 bytes 判斷格式、不再畫到的圖片放掉）、i18n 兩語系 key 一致、HostModule 宣告一致、兩份 `doc-pane.js` 相同、程式碼顏色在兩個 Yuzora 主題都讀得清楚（在 QuickJS 跑 `doc-pane.js` 的 `SYNTAX`／`syntax`／`mix`：One Dark 有上色的名稱都有主題色；淺色主題在背景、code block、目前行上都 ≥ 4.5，語言徽章也是；深色主題不低於 One Dark 與原本的徽章色）、`mix` 混色、Markdown code block 用傳入的語法色且只在語法色變了才換 highlighter、「<Agent> 也在改」的資料夾比對（在 QuickJS 跑 `doc-pane.js` 的 `within`／`editingAgent`：Windows 兩種斜線、大小寫、UNC，其他平台反斜線是檔名的一部分）、右側模式路徑 helper（在 QuickJS 跑 `paths.js`：Windows 與 POSIX 路徑；路徑卡的資料夾名稱與上層路徑：家目錄、根目錄、根目錄下的資料夾、遠端路徑、沒有分支、上層路徑結尾的分隔符、Windows 兩種斜線）、檔案 icon 對照（在 QuickJS 跑 `icons.js`：類型、特殊檔名、資料夾、只用主題有的顏色、每個 icon 都有 16×16 的 SVG、2 萬列查表不到 1 秒）、`revealLine` 每次都是新的請求、改名後圖片 pane 換成新路徑（在 QuickJS 跑 `open.js`，假的 Herdr：位置、焦點、資料夾改名、已關掉的 pane 不重開）。
- `cargo test --locked`（app workspace）：上游 `shipped_initial_descriptions` 會以空 props 與 fixture props 各渲染每個內建 mod 一次，4 個 L5 mod 都包含在內。
- `scripts/yuzora/check-mods`：全部 yuzora mod 通過。
- 隔離實機（lanes.md 的 `env -i`、暫存 app home 與 `HC_SESSION`），用 `herdr-app do`／`shot` 操作與留證：
  - `l5-01-files.png`：檔案模式的樹與變更標記（B1 改版前拍的，標題還是舊的 Space 徽章與 `路徑 · 分支`；現在的標題列與路徑卡見 `l5-20`／`l5-21`）。
  - `l5-02-editor.png`：`pane.split` 開 TSX，tree-sitter 上色、檔案列、狀態列。
  - `l5-03-markdown.png`：CLAUDE.md 預覽（標題、清單、inline code、表格、上色的 code block、本機 SVG 圖片）。
  - `l5-04-image.png`：PNG 檢視；右側模式同時因 `watchFolder` 自動出現新檔的標記。
  - `l5-05-external-change.png`：在 app 外改寫開著的 TSX，編輯器自動重新載入（第 8–9 行）。
  - `l5-06-git-marks.png`：在 app 外修改 README.md、刪掉 package.json，樹即時更新（`M`、檔案消失）。
  - `l5-07-goto-line.png`：`pane.split` 帶 `line: 15, column: 7` 開檔，游標在 Ln 15, Col 7，編輯器取得焦點。
  - `l5-08-markdown-goto-line.png`：帶 `line: 20` 開 `.md`，以原始碼開啟、游標在 Ln 20。
  - `l5-10-goto-line-by-hand.png`：`pane.split` 只帶 `line: 15, column: 7`（沒有 `opened`）開檔，游標在 Ln 15, Col 7；重啟後這個 pane 停在 Ln 1，焦點留在原本的 pane。
  - `l5-11-untrusted-folder.png`：repo 自己的設定有 clean filter（會留下標記檔）、資料夾未信任：樹上方顯示「未信任的資料夾…」，沒有 Git 標記，路徑卡仍有分支；標記檔沒有出現（2026-11-03 接上 #194 的 `git_trust` 後重拍）。
  - `l5-12-trusted-folder.png`：在 `trusted.json` 加入這個資料夾後，下一次重新整理出現 `M` 與資料夾的 `•`，標記檔出現（信任後 `git status` 照常執行 filter）。
  - `l5-09-image-actual-size.png`：2400×1600 的圖以 100% 顯示（下方 pane，暫時的 probe mod），從左上角原尺寸排版、可捲動，不再置中裁切。
  - `l5-13-file-icons.png`：檔案樹的類型 icon（深色主題 yuzora-dark）：收合的資料夾、程式碼（Rust 琥珀、TS 藍、JS 黃、Go／Python）、圖片、shell、lock、`Cargo.toml`／`package.json`、設定檔、`Dockerfile`、Markdown 與純文字、沒有對照的 `LICENSE`。
  - `l5-14-file-icons-light.png`：同一個資料夾在淺色主題 paper：顏色換成主題的深色 ANSI，在白底上同樣清楚。
  - 淺色主題 yuzora-light（Yuzora 的 UI 文字在淺色主題都要清楚）：
    - `l5-15-light-editor.png`：TSX 的語法色、語言徽章、目前行、狀態列（右下顯示目前的主題名稱）；右側模式的樹與 git 標記。
    - `l5-16-light-markdown.png`：Markdown 預覽中 Rust 與 TS 的 code block，各種 token 在 code block 底色上都清楚。
    - `l5-17-light-banner.png`：在 app 外刪掉開著的檔案，警告橫幅是淺琥珀底、深琥珀字（`tokens().status.blocked`）。
    - `l5-18-light-image.png`：圖片 pane 的 `IMG` 徽章與切換鈕。「對比背景」開啟時的畫面要點擊才能切換，沒有截圖（`solid_foreground` 在 `solid` 上是 17.7:1）。
    - `l5-19-dark-editor.png`：同一個檔案在 yuzora-dark，作為對照。
    - 右側模式右鍵選單的刪除項目（`ansi1`）要右鍵才會出現，沒有截圖；用色碼計算是 popover 上 5.69、hover 上 4.60。
  - 檔案模式標題改版（B1）：
    - `l5-20-sidebar-header-dark.png`（yuzora-dark）、`l5-21-sidebar-header-light.png`（yuzora-light）：標題列的「檔案」與 6 顆圖示鈕（「顯示隱藏檔」開著，`selected` 底；樹裡多了 `.yuuzu`）、路徑卡（上層路徑 `/private/tmp/yz-l5-run/` 放不下時先截斷、資料夾名稱 `demo` 完整、「本機」chip、分支 `feat/gpui-dock`）。
    - 沒有截圖的：hover、遠端時停用的新增鈕、沒有分支時第二行消失、右側模式再變窄時截斷「檔案」（要點擊或拖動）。遠端與沒有分支的資料由 `heading()` 的單元測試涵蓋（遠端路徑不用 `~`、沒有分支、上層路徑與名稱的拆法），排版靠 `flex_none`／`min_w(0)`／`truncate`。
- `tests/svg_child.rs`（`harness = false`：測試執行檔自己就是子行程，另外扮演各種出錯的子行程）：
  - 一般 SVG（矩形、漸層加虛線、中英文字、opacity group）由子行程畫出的結果，和在本行程用 GPUI 的 `SvgRenderer` 畫的逐 byte 相同；超過長邊上限的縮小方式也相同。usvg 讀不了的回 `svg_invalid`。
  - 只睡覺的子行程 2 秒逾時並被殺；一直配置記憶體的子行程在記憶體上限被殺，不是等到逾時；crash 回 `svg_crashed`；亂寫與只送一半的回 `svg_output`。
  - 子行程啟動的行程（逾時與 crash 兩種結束）都一起被殺。6 個請求同時進來，最多 2 個子行程在跑。取消時，排隊中的請求不啟動子行程，畫到一半的被殺。
  - 同一份 SVG 只 spawn 一次（兩個請求同時要也一樣），失敗的不重試。在 header 之後一直送資料（最多 256 MiB）的子行程，在第一個多餘的 byte 就被拒，本行程的峰值記憶體增加不到 32 MiB；header 宣稱 100000² 的，不配置記憶體就拒絕。
  - 常數就是文件寫的值；整個測試過程中，本行程沒有畫過任何 SVG。
- SVG 子行程的 mutation 抽查（每次只改一處，跑對應的測試）：拿掉逾時、拿掉 macOS 的記憶體 watchdog、拿掉並行上限、讓圖片 pane 或 Markdown 預覽在本行程畫、讓父行程直接呼叫子行程的繪製函式、拿掉 stdout 的讀取上限、拿掉 process group 的 kill，對應的測試都會失敗。
- SVG 子行程的跨平台檢查：
  - macOS：上面的測試全部實際執行；也用實際建置的 App 執行檔當子行程（`env -i`）畫出一般 SVG，usvg 讀不了的回狀態 1，不是請求的輸入以 2 結束。
  - Linux：`src/svg/process.rs` 與 `wire.rs` 原封不動，放進一個小的 runner，交叉編譯成 aarch64 Linux 執行檔，在 Ubuntu 24.04 container 裡跑：子行程的 `RLIMIT_AS` 是 512 MiB、env 已清空；一直配置記憶體的子行程回 `svg_memory`；逾時、crash、輸出過多、子行程樹（兩種結束）、取消都和 macOS 相同。這個 runner 不含 GPUI，所以 GPUI 執行檔在 512 MiB 位址空間下的實際表現，要等 `app-ci.yml` 的 Linux job 跑 `tests/svg_child.rs` 才知道。
  - Windows：`process.rs`（Job Object、thread 讀寫）只做 `x86_64-pc-windows-msvc` 的型別與 clippy 檢查，沒有實際執行。
- 沒有在實機驗的（需要鍵盤或滑鼠；未取得 computer-use 授權）：輸入後的未儲存標記與存檔、衝突橫幅、⌘F 搜尋取代、右鍵選單、樹的展開、右側模式搜尋、Markdown 原始碼切換、「<Agent> 也在改」。其中存檔與衝突判斷有 `fsops` 單元測試，互動本身都沒有在實機驗過。

## 已知限制

- 混用 LF 與 CRLF 的檔案：載入時以第一個換行決定，存檔時整份統一成那一種（與 VS Code 相同）。沒有修改時 ⌘S 不會寫入（是否修改在按下時向 Rust 重讀，剛打完字馬上按也算數）。
- 沒有未儲存變更時的自動重新載入用 `set_value`，會清掉該文件的 undo 歷史。
- 修改 app home 裡這幾個 mod 造成的熱重載不會呼叫 `close()`：已開的文件多一個參考（Yuzora 結束前不會釋放），舊的 owner 留在文件上，重載後的 pane 要按「在這裡編輯」接手。
- 右側模式的檔名與內容搜尋只丟掉過期結果，背景工作不會取消；大 repo 剛輸入時可能同時跑多個完整列舉。
- 有未儲存變更且檔案已從磁碟刪除時，只能「重新儲存」，沒有「捨棄」。

## 延後項目

#192 的 Codex P2 與 nit 先延後，合併後由 L5 另開 PR 處理。

### 仍延後（等 Windows CI）

- Windows 存檔保留 ACL 與其他屬性：目前 rename 換檔後只保留唯讀屬性，ACL 變成資料夾繼承的。改用 `ReplaceFileW`（保留 DACL、屬性、建立時間），失敗時（`ERROR_UNABLE_TO_MOVE_REPLACEMENT`）要把暫存檔放回原位，不能刪；需要 CI 在 Windows 編譯並測試 app（第 24 輪）。
- ~~Windows 上 `doc-pane.js` 的 `parent` 只認 `/`~~ **已處理（beta.3 win-misc PR）**。Windows CI（windows-2022，run 41770056432）驗證：`watchFolder` 本身 1 秒內有收到變更，但 `next_change` 的 `canonicalize` 讓回報的路徑帶 `\\?\`，永遠和 JS 的 `C:\…` 不相等；`parent`、圖片標題 `split("/")` 也只認 `/`。修正：`next_change` 回傳 `yuzora_core::paths::plain` 後的路徑（`app/src/files.rs`，`// yuzora(L5):`）；編輯器、Markdown、圖片三個 mod 的 `isWin`／`parent`／`isRoot`／`baseName`／`samePath`（Windows 兩種斜線、不分大小寫；其他平台只認 `/`），測試 `the_panes_follow_files_by_windows_paths_too`（QuickJS）與 app 的 `a_watched_windows_folder_reports_plain_paths_within_a_second`（Windows）。

### 已處理（follow-up PR）

- `gitStatus` 失敗的提示：`yuzora-sidebar-files/git-state.js` 的 `readGit` 把「回 null（不是 repo）」和「拋錯（git 失敗、逾時）」分開，後者記在狀態的 `git.failed`（`info()` 的 `gitFailed`），樹下方顯示 `git.failed`（中英文都有）；分支與標記照舊清掉。測試：`yuzora-files/src/lib.rs` 的 QuickJS 測試 `sidebar_git_state_tells_a_failure_from_not_a_repo`（CI 會跑），加上 `the_files_sidebar_is_wired_to_git_failures_and_long_queries` 以 `include_str!` 守 `main.js` 的接線（只守字面）。
- 遠端檔名清單的兩個 `rg --files` 加 `--no-config`（`search.rs` 的 `LIST_SCRIPT`），不再讀 `RIPGREP_CONFIG_PATH`；`the_list_script_*` 測試守著呼叫的參數。
- 遠端列資料夾不展開整個 glob：`fsops.rs` 的 `LIST_SCRIPT` 改用 `find . ! -name . -prune` 串流逐筆讀，到 `max + 1` 筆就 `break`（find 因 SIGPIPE 自己結束）。名稱含換行的項目分不開，`find` 對它印 `?`（真名稱一定以 `./` 開頭），腳本輸出 `?` 項目，`parse_listing` 設 `Listing.skipped`，右側模式顯示「有些檔案無法讀取」（`a_name_with_a_newline_is_reported_as_left_out`）。遠端沒有 `find` 時 exit 3，不會看起來像空資料夾。BSD find（macOS 遠端）會先讀完整個資料夾才輸出第一筆，所以成本仍隨資料夾大小成長，只是不再展開整個 glob、輸出也只到 `max + 1`。測試 `list_script_stops_reading_at_the_limit` 用永不結束的假 `find` 證明腳本會停。
- `fuzzy_match` 的配置上限：`m × n` 超過 `MAX_MATCH_CELLS`（2²⁰）時不建表，改回最左的子序列比對，分數為 `-路徑長度`（通常低於正常比對）；不是子序列照樣回 `None`。測試 `an_oversized_match_falls_back_to_a_cheap_one`。查詢本身也有長度上限：去掉空白後超過 `MAX_QUERY_CHARS`（256）就不比對（`fuzzy_filter` 回空，`findFiles` 直接回 `tooLong: true`，不列舉檔案），右側模式顯示 `search.too_long`（兩語系），測試 `a_query_past_the_length_limit_matches_nothing`。
- Markdown 預覽的圖片不再在同一個 blocking loop 逐張處理：`markdown.rs` 的 `read_images` 用最多 `IMAGE_WORKERS`（= `svg::CONCURRENT`）條執行緒並行讀，每張讀完就記錄、重畫；點陣圖排在 SVG 前面，不會被慢的 SVG 擋住（`a_bitmap_is_not_kept_waiting_by_svgs_that_are_slow_to_draw`）。一個預覽因此可以同時佔住全部 SVG 名額，其他 pane 的 SVG 最多多等一個逾時時間，這是取捨。`check_images` 在兩張圖之間逐張回報這件事，測試環境的 `docs::blocking` 是同步執行，觀察不到，只用 `the_preview_is_wired_to_the_checked_helpers` 以字面守接線。SVG 仍然經 `svg::draw` 的子行程與它的 watchdog、並行上限，沒有繞過。測試 `images_are_read_a_few_at_once_and_answered_as_each_is_done`。
- `image_source` 抽成 `source_of`（render 的閉包只剩呼叫它，同一個 wiring 測試守著；沒有畫出整個 `MarkdownView` 的 gpui 測試），測試 `the_preview_never_hands_a_path_to_gpui`：沒有答案時是空路徑，有答案時是畫好的像素（`ImageSource::Render`），遠端文件不讀本機；把 `.svg` 路徑交給 GPUI 會失敗。
- CHANGELOG（`app/CHANGELOG.md`、`app/CHANGELOG.zh-TW.md`）的 SVG 條目改成：原因由圖片檢視說明，Markdown 預覽只有佔位圖。

## 待辦

- 遠端新增檔案／資料夾、重新命名、刪除（R2、R3，見上）。
- 編輯器 gutter 的變更標記（新增／修改／agent 修改的行，設計稿 p2 左側色條）：需要 L2 的 diff API。
- git 標記改用 `yuzora:git`（L2 合併後）。
- Markdown 預覽：相對連結（`./other.md`）開檔、遠端檔案的相對圖片、fenced code 以外的 inline 語法上色（tree-sitter-md inline 語法）。
- 拖放檔案到右側模式（複製／移動）。
- pane 標頭整合：目前編輯器 pane 自己畫 28px 檔案列；L1 的 pane 標頭可依 `mod.json` 的 `"yuzora": {"pane": {"header": "own"}}` 隱藏或改用 `documentState` 顯示檔名。
