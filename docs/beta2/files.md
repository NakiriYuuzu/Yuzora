# L5 檔案＋編輯器

beta.2 的檔案側欄、程式碼編輯器、Markdown 預覽與圖片檢視。設計依據：設計稿 p2（檔案畫板）與 p6（側欄「檔案」模式）。架構規則見 [architecture.md](architecture.md)、[lanes.md](lanes.md)。

## 範圍與檔案

| 路徑 | 內容 |
| --- | --- |
| `app/crates/yuzora-files/src/lib.rs` | `yuzora:files` HostModule：函式、3 個原生元件、TypeScript 宣告 |
| `src/highlight/` | tree-sitter 上色：14 種語言、`InputHighlighter` 實作、One Dark 色盤、無狀態 `highlight()` API |
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

- **Rust（`yuzora-files`）**：所有磁碟 I/O、git、搜尋都在 worker thread；編輯器、Markdown、圖片是 `HostModule::component` 原生元件。
- **JS mods**：版面、標頭、橫幅、搜尋列、右鍵選單、開檔策略。使用者可以 `mod.clone` 修改。

### 文件（`docs.rs`）

- 一個檔案（機器＋路徑）在整個 app 只有一份 `DocView`（GPUI global 的 registry），所有 pane 共用。`openDocument` 每呼叫一次加一個參考，`releaseDocument` 減一。
- **只有一個 pane 畫編輯器**（owner）。同一個檔案在另一個 pane 開啟時顯示「在這裡編輯」，按下才接手；避免同一個 GPUI entity 在同一個 frame 被畫兩次。
- 載入：`fsops::read_text`，換行正規化為 LF，記住原本是 LF／CRLF 與 BOM，存檔時還原。
- **存檔**：同資料夾的暫存檔（`.yuzora-save-<32 hex>`）fsync 後 rename 蓋過去，保留權限與 owner；資料夾不能建暫存檔、或 owner 保不住時退回就地寫入。symlink 寫到它指向的檔案。寫入前比對載入時的 `{len, mtime}`，不同就回報衝突、不寫入；一樣時再比對內容 hash（時間精度粗的檔案系統上，同長度的改寫可能 stamp 不變）。
- **外部變更**：本機用 herdr 的 `watchFolder`（檔案所在資料夾）＋15 秒輪詢，遠端 5 秒輪詢，呼叫 `checkDisk`。stamp 變了才讀內容並比對 hash：
  - 內容相同（touch）：只更新 stamp。
  - 沒有未儲存變更：自動重新載入，保留游標與捲動位置。
  - 有未儲存變更：標成衝突，橫幅提供「重新載入（捨棄我的）／保留我的／覆寫」。
  - 檔案消失：橫幅提供「重新儲存」。
- **「<Agent> 也在改」**：文件記住最後一次被外部修改的時間（`externalAt`）。3 分鐘內，若有 agent 狀態為 working、且它所在 pane 的 cwd 包含這個檔案，標頭顯示該 agent 的名稱（Claude Code 用設計稿的 `#2a201c`／`#e8a27f`）。
- 關閉有未儲存變更的 pane：buffer 保留在記憶體（toast 提示），重新開啟同一檔案即可找回；Yuzora 結束才會遺失。
- 大檔保護：超過 16 MiB 不開；超過 2 MiB 以純文字開啟（關閉上色與摺疊）；含 NUL 的二進位檔不開；非 UTF-8 以唯讀開啟。
- 搜尋取代：用 gpui-base 內建的搜尋引擎（`set_search_query`、`next/previous_search_match`、`replace_*`），關掉它的內建面板（`searchable(false)`），讓 ⌘F 冒泡到 JS 的搜尋列。

### 上色（`highlight/`）

- tree-sitter 0.27 與 13 個語法 crate：rust、typescript／tsx、javascript（含 JSX）、json、toml、yaml、markdown（block 語法）、python、go、sql（tree-sitter-sequel）、bash、css、html。
- 增量解析：`InputEdit` 轉成 `tree_sitter::InputEdit`，用舊 tree 重新解析（一批 edit 只重解析一次）；`styles(range)` 只對可見範圍跑 `QueryCursor`。debug 建置下 2 MiB 的 Rust 全文解析約 0.9 秒、增量約 80 ms、一屏約 1–2 ms。
- capture 名稱對應到固定的語意名稱（keyword、function、type、string…），由 `SyntaxPalette`（`HighlightStyleResolver`）決定顏色；預設 One Dark（設計稿的配色）。
- 同一段文字有多個 capture 時：範圍最小的優先，一樣大時比 pattern 的限定程度（節點數＋predicate），再相同才取後面的。上游各語言的查詢對先後順序的假設互相矛盾（JS 要後面的贏、JSON 與 Go 要前面的贏），單一規則無法全對。
- 上游查詢的兩處執行期修補（各有測試）：tree-sitter-sequel 把數字寫成 Lua 的 `%d`，換成 `[0-9]`；tree-sitter-toml-ng 的 `(pair (bare_key)) @property` 會把整個 pair 上色，改成 `(pair (bare_key) @property)`。
- 摺疊範圍來自語法樹（成對括號跨行的節點、python `block`、markdown `section`…）。
- Markdown 只用 block 語法：fenced code 依 info string 上色，`*強調*`、連結等 inline 語法不上色。

### Markdown 與圖片

- `Markdown` 元件跟著文件的文字版本（每次編輯或重新載入加一）重畫，游標閃爍不會讓它重抓全文；同一份文件在原始碼與預覽間切換時即時同步。
- 圖片只讀本機的一般檔案（不是資料夾、裝置或 FIFO）且不超過 32 MiB；網路、`data:`、Windows UNC（`//host/share`）與遠端文件的圖片一律不讀。檢查在 worker 做（畫面只讀快取，10 秒後重新檢查），所以圖片在檢查回來後才出現。連結只開 http、https、mailto。
- 預覽的文字、連結、程式碼底色跟隨目前的主題（`theme()` 的 foreground、muted、ansi4、raised、border）；code block 的語法色仍是 One Dark。
- 圖片 pane：PNG／JPEG／GIF／WebP／BMP／TIFF／ICO／SVG，「符合」與「100%」（原尺寸，比 pane 大時可捲動）、淺色背景；檔案變了（大小或修改時間）自動重新顯示。遠端圖片同樣只讀一般檔案。

### 側欄（`yuzora-sidebar-files`）

- 跟隨目前 focused pane 的 Space 與 cwd（沿用上游 `files` mod 的定位方式）；focused 的是工具 pane（沒有 cwd）時保留原本的資料夾。在 repo 裡時以 `git rev-parse --show-toplevel` 為根。
- 頂部：Space 徽章（`tokens().space_badges` 依 Space 順序取色、名稱首字）、「本機」或機器名、`路徑 · 分支`。
- 動作列：新增檔案、新增資料夾、重新整理、全部收合、搜尋、顯示隱藏檔（記在 localStorage；切換時重跑搜尋）。
- 樹：`uniform_list` 虛擬捲動，每個資料夾最多 2000 項、總共 20000 列；git 標記 M／A／D／U／?，收合的資料夾若含變更顯示 `•`。`watchFolder` 監看根目錄＋10 秒重新整理。
- 搜尋：檔名（模糊比對，`.gitignore` 生效，清單快取 15 秒，最多列 200,000 個檔案，超過時提示「只搜尋了前 200,000 個檔案」）與內容（有 `rg` 就用 ripgrep；本機沒有時改用內建搜尋：`ignore`＋`regex`、同樣的規則；遠端沒有 `rg` 時顯示提示）。點內容結果會開檔並跳到該行。
- 右鍵：同一個 mod 以 popup 開啟自己（`props().menu`）。開啟、在終端機開啟、新增檔案／資料夾、重新命名、刪除到垃圾桶、複製路徑／相對路徑、在 Finder（檔案總管／檔案管理員）中顯示。新增與重新命名透過 `shared("yuzora.files.request")` 交回主側欄，在樹上方顯示輸入列。

### 開檔

- 圖片／SVG → `yuzora-files-image`；`.md` → `yuzora-markdown`（預設預覽）；其他 → `yuzora-editor`。
- 同一個檔案已有 pane 顯示時，`pane.focus` 過去；否則把 focused pane 往右分割，沒有 focused pane 時開新 tab。
- pane 不知道自己的 pane id，所以每次編輯器取得焦點時，用 `focused().pane` 記到文件上（`notePane`），之後才能 focus 回來。圖片沒有文件：側欄在 `pane.split` 之後記下取得焦點的新 pane，下次開同一張圖時 focus 過去（只記得從側欄開的圖片）。
- 帶 `line` 開 `.md`（例如內容搜尋的結果）時以原始碼開啟並跳到該行；預覽／原始碼的偏好不變。

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

- 讀取與瀏覽：`listDir`、`gitStatus`、`findFiles`（`git ls-files`，扣掉工作樹已刪除的檔案／`rg --files`／`find`）、`grep`（遠端 `rg`）、開檔、圖片，都走 `yuzora_core::remote`（`hc exec --on`，各自的 timeout）。遠端既沒有 git 也沒有 rg 時，`find` 不認得 ignore 檔：`findFiles` 回傳 `unfiltered: true`，側欄提示結果包含被忽略的檔案。
- **寫入未做**（存檔、新增、重新命名、刪除）。`createFile`／`createFolder`／`rename` 帶機器名稱一律拒絕，存檔回 `remote`：
  - 原因：`hc exec` 只能把資料放在 argv 或 stdout，沒有 stdin，也沒有「比對後才寫入」的原子操作。用 base64 塞進 argv 會碰到 `ARG_MAX`，也沒辦法安全做衝突檢查；做半套反而可能蓋掉 agent 剛寫的內容。
  - 後續計畫：L4 的 SFTP session（`yuzora:ssh`）合併後，遠端存檔改走 SFTP：寫暫存檔 → `stat` 比對 → rename，與本機相同的衝突語意；新增、重新命名、刪除也用 SFTP。沒有 SSH 設定的機器（只透過 herdr-core 連線）再評估 `hc` 是否提供檔案寫入 API。

## 驗證

- `cargo test --locked -p yuzora-files`（94 個）：上色（每種語言的首選 query 都能編譯並產生 span、增量解析＝重新解析、摺疊、色盤）、檔案操作（原子存檔、衝突（含 stamp 相同但內容不同）、CRLF／BOM、二進位與大檔、遠端 sh 腳本在本機跑出與原生相同的結果、遠端圖片只讀一般檔案、遠端變更一律拒絕）、git porcelain 解析與真 repo（不改動 `.git`；遠端清單扣掉已刪除的追蹤檔）、模糊搜尋排序、rg JSON 解析與內建搜尋、文件載入／外部變更自動重載／刪除／共享／釋放、在測試視窗裡真的編輯後遇到外部變更（衝突、保留我的、存檔）與存檔時才發現變更或檔案已刪、Markdown 圖片與連結規則、i18n 兩語系 key 一致、HostModule 宣告一致、兩份 `doc-pane.js` 相同。
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
  - `l5-09-image-actual-size.png`：2400×1600 的圖以 100% 顯示（下方 pane，暫時的 probe mod），從左上角原尺寸排版、可捲動，不再置中裁切。
- 沒有在實機驗的（需要鍵盤或滑鼠；未取得 computer-use 授權）：輸入後的未儲存標記與存檔、衝突橫幅、⌘F 搜尋取代、右鍵選單、樹的展開、側欄搜尋、Markdown 原始碼切換、「<Agent> 也在改」。其中存檔與衝突判斷有 `fsops` 單元測試，互動本身都沒有在實機驗過。

## 已知限制

- 混用 LF 與 CRLF 的檔案：載入時以第一個換行決定，存檔時整份統一成那一種（與 VS Code 相同）。沒有修改時 ⌘S 不會寫入。
- 沒有未儲存變更時的自動重新載入用 `set_value`，會清掉該文件的 undo 歷史。
- 修改 app home 裡這幾個 mod 造成的熱重載不會呼叫 `close()`：已開的文件多一個參考（Yuzora 結束前不會釋放），舊的 owner 留在文件上，重載後的 pane 要按「在這裡編輯」接手。
- 側欄的檔名與內容搜尋只丟掉過期結果，背景工作不會取消；大 repo 剛輸入時可能同時跑多個完整列舉。
- 有未儲存變更且檔案已從磁碟刪除時，只能「重新儲存」，沒有「捨棄」。

## 待辦

- 遠端寫入（見上）。
- 編輯器 gutter 的變更標記（新增／修改／agent 修改的行，設計稿 p2 左側色條）：需要 L2 的 diff API。
- git 標記改用 `yuzora:git`（L2 合併後）。
- Markdown 預覽：相對連結（`./other.md`）開檔、遠端檔案的相對圖片、fenced code 以外的 inline 語法上色（tree-sitter-md inline 語法）。
- 拖放檔案到側欄（複製／移動）。
- pane 標頭整合：目前編輯器 pane 自己畫 28px 檔案列；L1 的 pane 標頭可依 `mod.json` 的 `"yuzora": {"pane": {"header": "own"}}` 隱藏或改用 `documentState` 顯示檔名。
