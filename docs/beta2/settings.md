# L6 設定＋MOD／Template

- 擁有者：L6。crate `app/crates/yuzora-settings`（JS `yuzora:settings`），mods `app/defaults/mods/yuzora-settings-*`，Yuzora `settings.json` 的 `general` section。
- 架構與掛點見 [architecture.md](architecture.md)、[lanes.md](lanes.md)。

## 範圍與交付

| 項目 | 位置 | 狀態 |
| --- | --- | --- |
| 設定框架接上 Yuzora（品牌、隱私、「Yuzora」分組） | `app/defaults/frame/settings/yuzora.js`（新檔）＋ `main.js` 帶 `// yuzora(L6):` 的改動 | 完成 |
| `general` 設定頁：語系、外觀（主題、字型、字級、縮放）、啟動行為、終端機剪貼簿 | `yuzora-settings-general` | 完成；「啟動」區塊要等 L1 套用並設 `yuzora.startup.honored` 才顯示（見後續整合） |
| Template 管理：各 machine 列出、表單＋body 編輯、存前驗證、移到垃圾桶 | `yuzora-settings-templates`、`yuzora-settings` 的 `templates`／`trash` | 完成 |
| 以模板啟動 agent（選 Space／tab、輸入任務、`hc run` 帶 label） | `yuzora-settings-launch`、`yuzora-settings` 的 `launch` | 完成；「＋」選單入口由 L1 串接 |
| MOD 管理：mods／layouts 清單、clone、開資料夾、check、還原內建、切換 layout、從資料夾安裝（含信任警告） | `yuzora-settings-mods`、`yuzora-settings` 的 `mods` | 完成；「以編輯器開啟」與 mod 立即還原需整合（見後續整合） |

## 決策：改上游 settings frame，而不是另做 overlay

兩種做法：

1. **改上游 frame**：`app.settings`（模式列齒輪、macOS「設定…」選單、⌘,、app 內「Updates」提示）都開同一個 `frame-settings`。在它的 nav 加「Yuzora」分組。
2. **Yuzora 自己的 overlay**：模式列齒輪改開 Yuzora overlay，上游 frame 仍由選單與快捷鍵開啟。

選 1。理由：

- 品牌字樣與隱私頁本來就在上游 frame 裡，做法 2 也必須改 frame，改動量相同，卻多出第二個設定對話框，兩個入口開出不同的設定，使用者會困惑。
- mod 無法把另一個 mod 嵌進自己的畫面，但 overlay 可以是 layout。「Yuzora」頁因此改用 layout overlay 呈現：一列兩個 block，左邊是 frame 本身（`props {yuzora: <mod>}`，只畫 nav），右邊是該頁的 mod（`props {section}`）。nav 的樣子與位置不變，看起來是同一個對話框換頁。點回 Herdr 的分頁時關掉這個 overlay，改用 `app.settings {section}` 開回原本的 frame。
- 改動集中：Yuzora 的邏輯全在新檔 `frame/settings/yuzora.js`（frame 允許相對 import，`whatsnew` 也這樣用），字串在新檔 `frame/settings/i18n/*.json`；上游 `main.js` 只有 15 處帶 `// yuzora(L6):` 的呼叫，`+23 −7` 行。

### 行為

- `app.settings` 不帶 section：開 Yuzora「一般」頁（`yz.DEFAULT_PAGE`）。frame 先畫一個空白底，量到對話框實際高度（620，視窗太矮時是視窗的 85%）後換成同寬同高的 layout overlay，換頁不會跳動；量不到時 500 ms 後以 620 開。
- `app.settings {section}`：Herdr 的分頁（look、keyboard、machines、alerts、updates、privacy）照舊；也可以是 Yuzora 頁的 mod 名稱或 section key（`{section: "templates"}`），直接開該頁。Herdr 的分頁名稱優先。
- 分組來源是 `settingsPages()`（各 mod 的 `mod.json` `yuzora.settings`），依 `order` 排序。L6 自己的頁：`general` 0、`templates` 800、`mods` 900；其他 lane 的頁放在 1–799 之間就會排在中間。
- 同時只有一個設定對話框：`app.settings` 只認得 id 為 `settings` 的 overlay，所以 Yuzora 頁的 overlay 記在 `shared("yuzora.settings.overlay")`（`{id, run}`）；frame 每次開啟（⌘,、選單、齒輪）都先關掉它。不必改 `window.rs`。overlay id 每次啟動都從 `o1` 重新編號，而 `shared` 會存檔，所以只有 `run` 等於本次啟動的 `session()` 時才關，否則只清掉這個值（避免關到重啟後拿到同一個 id 的別的 overlay）。
- 從 nav 點回 Herdr 分頁、從 Herdr 分頁點 Yuzora 頁，都以目前對話框的高度開新的那一個。

### 上游 `main.js` 的改動（全部帶 `// yuzora(L6):`）

| 位置 | 改動 |
| --- | --- |
| import | `import * as yz from "./yuzora.js"` |
| `LOOKS` | 加 `yuzora: "Yuzora"`，排第一 |
| `init` | 判斷 nav-only 模式（`props().yuzora`）；要求的是 Yuzora 頁（或沒指定）時改開 layout overlay；關掉其他 Yuzora 設定 overlay（`yz.closeOthers`） |
| `alerts` | 預設音效名稱「Herdr's sound」→ i18n「內建音效」 |
| `updates` | 標題與版本列改 Yuzora；「build from source」說明改為「Yuzora 目前不會自我更新」；頁尾改寫 |
| `privacy` | 整頁改為 `yz.privacy`：Yuzora 不收集任何資料（Herdr 的遙測已在 L0 關閉，toggle 與「What we send」都沒有意義） |
| `render` | 換頁中先畫空白並量高度；nav 插入 Yuzora 分組；nav-only 模式下 Herdr 分頁不標示選取、點了回原 frame；nav-only 模式只回傳 nav（含 Esc 與 Send feedback，與原 frame 相同）；記下對話框高度（`this.measured`） |

上游的英文字串（Look、Machines…）維持英文，只有 Yuzora 加的部分有 zh-TW。

## `general` section

`<app home>/yuzora/settings.json`：

```json
{
  "general": {
    "locale": "system",
    "startup": { "sidebar": "last" }
  }
}
```

| key | 值 | 誰套用 |
| --- | --- | --- |
| `locale` | `system`／`en`／`zh-TW` | `yuzora:core` `setLocale`（立即重畫所有 mod） |
| `startup.sidebar` | `last`（預設）／`workspace`／`files`／`git`／`database`／`ssh` | L1 的模式列在視窗開啟時呼叫 `yuzora:settings` 的 `startup()`，拿到 `yuzora-sidebar-<mode>` 就 `layout.select`（尚未串接；串接前一般頁不顯示這個設定） |

外觀與剪貼簿寫的是 Herdr 自己的 `settings.json`，寫法與上游 frame 相同（讀 → 改 → 只在沒人改過時寫回）：`appearance`、`font`、`ui_font`、`font_size`、`clipboard_write`；主題用 `app.theme`、縮放用 `app.zoom_*`。主題與字型本身由 L1 提供，L6 只負責挑選 UI。

`clipboard_write` 對應 L0 交接的 OSC 52 安全強化：這裡只提供開關，預設值仍是上游的 `true`（改預設值要動上游 `settings.rs` 與它的測試，留給總指揮決定）。

## Templates

herdr-core 的模板：`<config>/herdr-core/templates/<name>.md`，`<config>` 是 `$XDG_CONFIG_HOME`，否則 Windows 為 `%APPDATA%`，其他為 `~/.config`（[herdr-core docs/templates.md](https://github.com/herdrdev/herdr-core/blob/main/docs/templates.md)）。

### 解析規則（以 hc 0.1.13 實測對齊）

| 規則 | hc 的行為 |
| --- | --- |
| 第一行 | 必須剛好是 `---`；前面有空行、行尾空白或 CRLF 都算「no frontmatter」 |
| 結束行 | 去掉行尾空白後是 `---` |
| 標頭行 | `key: value`，第一個冒號切開、兩邊 trim；空行略過；註解行（`# …`）會錯；key 分大小寫；重複 key 取最後一個 |
| `command` | 必填；`Command:` 不算 |
| `paste_gap_ms` | 非負整數（`-5`、`1.5`、超過 u64 都錯） |
| 結束行與錯誤順序 | 先找結束行：沒有結束行就是 `never_closes`，即使中間有不是 `key: value` 的行（例如縮排的 `  ---`） |
| `ready` | `quiet` 以外的值不報錯，等同預設 |
| `submit`／`stop` | 列表時不檢查；`hc run` 時才回報「unknown key」並讓第一則訊息不送出 |
| 檔案 | 只認小寫 `.md`、不進子資料夾；檔名去掉 `.md` 就是名稱，`name:` 欄位被忽略；名稱不限字元（`my review`、`審查`、`-x` 都能 `hc run`） |

`yuzora-settings::templates::parse` 照這些規則實作，`Entry::error` 給出與 `hc templates --json` 相同的判定（`no_frontmatter`、`never_closes`、`not_key_value`、`no_command`、`paste_gap_number`）。

### 存檔前的檢查（`checkTemplate`）

錯誤（擋住儲存）：名稱空白或不是資料夾裡的一個檔案（含 `/`、`\` 或控制字元、以 `.` 開頭、超過 200 位元組；其餘與 hc 一樣不限）、沒有 command、值含換行、`paste_gap_ms` 不是整數。同一套名稱規則也用在移到垃圾桶、改名與啟動，本機與遠端都先在 Rust 檢查。
警告：值被引號包住（hc 會照字面吃進去）、`ready` 不是 `quiet`、`submit`／`stop` 有 hc 可能不認得的按鍵、body 空白、`codex` 沒加 `--no-daemon`（`hc --skill` 的要求）。

頁面 props：`{machine?, edit?}`。`edit` 為模板名稱時，清單讀完直接開該模板的表單；`""` 開新增表單（給其他 lane 的「新增模板」入口用）。

寫檔時已知欄位依固定順序輸出，編輯器沒有的欄位原樣保留在後面；寫入走暫存檔，並比對開啟時讀到的內容（被別處改過就拒絕，`changed`）。同一個 app 的存檔一次一個；編輯在 rename 前一刻再比對一次；新名稱（新增或改名）用 hard link 放上去，名稱在這之間被佔用就回 `exists`，不會蓋掉（不支援 hard link 的檔案系統才退回 rename）。遠端腳本同樣處理（`ln`、`mv` 前再比對）。改名時先寫新檔，再把舊檔移到垃圾桶；舊檔移不走時回 `old_kept`（新檔已存好，兩個檔案都在），頁面當作已儲存並提示。

模板是 symlink（例如用 stow 管理的 dotfiles）時：編輯寫進它指向的檔案，連結保留；移到垃圾桶移走的是連結本身，不動原檔。本機與遠端一致。

### 其他 machine

透過 `hc exec --on <machine> -- sh -c <script> sh <args…>`（`yuzora_core::remote`，20 秒）：列表、寫入、移到垃圾桶都是固定的 sh 腳本，資料只當 positional 參數傳入，不拼進腳本。遠端的垃圾桶依序用 `gio trash`、`trash` 指令，否則手動寫 freedesktop Trash（`files/`＋`info/*.trashinfo`）。Windows 遠端（沒有 sh、模板在 `%APPDATA%`）不支援。

### 垃圾桶（本機）

- macOS：`/usr/bin/trash`（macOS 15 起內建），沒有時用 Finder（osascript，會跳自動化權限）。
- Windows：PowerShell `Microsoft.VisualBasic.FileIO.FileSystem.DeleteFile(…, SendToRecycleBin)`，路徑經環境變數傳入。
- Linux 與其他：freedesktop Trash（`$XDG_DATA_HOME/Trash`），同名時加 `.2`、`.3`。

不另加 `trash` crate，避免新依賴與 `Cargo.lock`／notices 變動。

### 以模板啟動

```text
hc run --json --bg --no-parent [--on <machine>] [--cwd <dir>] [--name <n>]
  --label herdr.space=<id> --label herdr.tab=<id> -- <template> [<task>]
```

- adoption 只看 terminal 的 `herdr.space`／`herdr.tab` label 與 app 的 space／tab id（`window.rs` 的 `reconcile`／`adoptable`），跟 core 文件名無關，所以 L0 把文件名改成 `yuzora` 不影響。遠端 space 的 id 在 app 裡是 `<machine>/<id>`，寫 label 時去掉前綴（同 `mirror::local`）。
- 選「新 tab」時給一個 app 不認得的 tab id（`tpl…`），app 會在該 space 開新 tab。
- `hc run` 對不認得的名稱會當成程式執行，所以先列出該 machine 的模板，確認名稱存在且沒有錯誤才跑。
- `--no-parent`：從 app 內開的 agent 不該掛在任何 terminal 底下（從 herdr 終端機啟動 Yuzora 時環境裡可能有 `HC_TERMINAL_ID`）。
- 本機的 cwd 預設為 Space 的 folder，再來是聚焦 pane 的 cwd，最後是家目錄（hc 的預設是執行 hc 的資料夾，也就是 app 的）。
- 等待上限 90 秒（hc 預設最多等 30 秒的狀態回報）。`first_message.sent` 為 false 時跳 toast，附上 hc 給的原因。
- 對話框等 pane 出現後 `pane.focus` 它，再關掉自己（與開啟它的設定對話框）。
- 注意：官方 Herdr 若連在同一個 core，也會看到這個帶 label 的 terminal；它不認得這個 space id，會替它開一個新 space（D4 的已知代價）。

## Mods

- 清單來源：`<home>/mods/*`、`<home>/layouts/*.json`，以及 Herdr 的 `<home>/installed.json`（shipped mod 的每個檔案與 FNV-1a 指紋、shipped layout 的 `layout:<name>`）。
- 來源分類：`shipped`（與安裝時一致）、`edited`（shipped 但檔案變了或多了檔案；gpui-shell 自動寫的 `jsconfig.json`／`gpui-kit.d.ts` 不算，與上游 `as_shipped` 相同）、`clone`（`mod.json`／layout 有 `cloned_from`）、`personal`。
- 錯誤：`herdr-app status`（以 app 自己的執行檔跑 `status`，繼承 app 的環境，所以隔離環境下也查得到同一個視窗），取 `mod <name>: …` 開頭的行。
- check：`<app> check <home>/mods/<name>`（90 秒）。在 app home 內的 mod 上跑 check 是 AGENTS.md 的標準做法；它寫入的 helper 檔不會讓 shipped mod 變成 edited。
- clone：`mod.clone {name, as}`／`layout.clone {name, as}`（layout clone 會直接切換過去）。
- 還原內建：layout 用 `layout.reset {name}`。mod 沒有對應指令：`yuzora_settings::mods::reset_mod` 先把 edited 的資料夾移到 `<name>-edited`（與啟動時相同），再寫回 shipped 檔案。它需要 app 交出內嵌的 shipped 檔案（`set_shipped_mods(crate::mods::DEFAULT_MODS)`），目前尚未串接（見後續整合），所以 UI 只說明「下次啟動會還原」，不顯示按鈕。
- 從資料夾安裝：`pickFolder` → `planInstall`（只讀）→ 顯示「mods 是受信任的程式碼」警告、會裝成什麼名稱、程式碼會不會執行程式／連網／讀寫檔案（字串掃描，只是提示）、有沒有 `keys.json` → 使用者按「我信任它，安裝」→ `installFolder(plan.token)`。
  - 裝的就是看過的：`planInstall` 讀進來的檔案（已改寫名稱）留在 Rust（只留最新一份），`installFolder` 只寫這一份，不再讀資料夾；token 不符（例如另一個視窗又規劃了一次）回 `stale`，要重新選。
  - 一個 mod 資料夾（有 `main.js`），或 shared layout 的資料夾（`layouts/*.json`＋`mods/*/`）。
  - 不覆寫：名稱被占用就用 `-2`、`-3`…（連結、壞掉的連結也算占用），並把 `mods/<資料夾原名>/` 路徑與 `mod: "<資料夾原名>"` 改寫成安裝的名稱（`my mod` 裝成 `my-mod` 時也改；冒號前後可有任意空白），layout 內引用也一起改。寫入時 mod 資料夾用 `create_dir`、每個檔案用 `create_new`，名稱在這之間被占用就失敗，不會跟著連結寫到 home 外。新名稱也避開同一包裡其他 mod 的名稱（home 有 `x`、包裡有 `x` 與 `x-2` 時，`x` 裝成 `x-3`），改寫才不會連鎖。
  - 不複製 `.git`、`node_modules`、隱藏檔、根目錄的 `RECIPE.md`／`preview.png`；超過 2000 個檔案或 64 MB 拒絕；來源在 app home 內拒絕；失敗時刪掉這次寫入的東西。
  - `keys.json` 不自動合併（AGENTS.md 要求替換既有按鍵前先問人），提示使用者自行處理。

## JS API（`yuzora:settings`）

完整宣告在 `crates/yuzora-settings/src/lib.rs` 的 `DECLARATIONS`（啟動時寫進 app home 的 `yuzora.d.ts`）。

| 函式 | 用途 |
| --- | --- |
| `templatesDir()` | 本機模板資料夾 |
| `listTemplates(machine?)` | 該 machine 的模板（async） |
| `checkTemplate(name, fields, body)`、`templateText(fields, others, body)` | 存前檢查、檔案內容預覽 |
| `saveTemplate(machine, name, fields, others, body, expected, previous?)`、`trashTemplate(machine, name)` | 寫入、移到垃圾桶（async，回傳 `{ok, code?}`） |
| `runTemplate(launch)` | 以模板啟動（async） |
| `listMods()`、`resetMod(name)` | mods／layouts 清單（async：每次都讀過 shipped mod 的檔案並算指紋）、還原內建 mod |
| `appStatus()`、`checkMod(name)` | 視窗 status、`check`（async） |
| `planInstall(path)`、`installFolder(plan.token)` | 安裝計畫、安裝那份計畫讀到的檔案（async） |
| `reveal(path)` | 在檔案管理員顯示 |
| `startup()` | `general.startup` 要求的側欄模式 |
| `session()` | 本次啟動的識別值（給存在 `shared` 但重啟後就沒意義的值用，例如 overlay id） |

## 後續整合（需要其他 lane 或總指揮）

1. **L1 模式列套用啟動側欄**：模式列 init 時 `const s = startup().sidebar; if (s) run("layout.select", {stack: "yuzora-sidebar", child: s})`，每次 app 啟動只做一次（可用 `session()` 記本次啟動），並 `setShared("yuzora.startup.honored", true)`。一般頁要看到這個旗標才顯示「啟動」區塊，避免出現一個沒有作用的設定。
2. **L1「＋」選單／側欄「新增」加「從模板啟動」**：`openOverlay({mod: "yuzora-settings-launch", kind: "dialog", width: 560, props: {space, tab}})`，或不帶 props 用 `run("mod.open", {name: "yuzora-settings-launch"})`（預設目前的 space 與 tab）。
3. **L5 編輯器**：MOD 頁已預留「編輯」按鈕：偵測到 `yuzora:core` `panes()` 裡有 `yuzora-editor` 或 `yuzora-editor-*` 時，以 `tab.new {mod, props: {path}}` 開 `main.js`。L5 定案 props 介面後請對齊（`path` 是假設）。
4. **mod 立即還原**（總指揮）：新增 `app/src/yuzora/settings.rs`：`pub fn init() { yuzora_settings::mods::set_shipped_mods(crate::mods::DEFAULT_MODS); }`，在 `mod.rs` 加 `mod settings;` 並在 `init_once()` 呼叫 `settings::init()`。lanes.md 只允許 lane 加 `mod <lane>;` 那一行，所以留給總指揮決定；串上之後 MOD 頁的「還原」按鈕就會出現。
5. **OSC 52 預設值**：是否把 `clipboard_write` 預設改成 `false`（改上游 `settings.rs` 與測試），由總指揮決定。OSC 8 連結 scheme 白名單（L0 交接的另一項）未處理。

## 測試

- `cargo test --locked -p yuzora-settings`：模板解析與 hc 對齊、存檔衝突、遠端 sh 腳本（在本機 sh 上以暫存 config／trash 跑過，PATH 裡沒有 gio／trash，不會碰到真正的垃圾桶）、freedesktop trash、日期、mods 分類與 `as_shipped`、還原、安裝計畫與改名改寫、`hc run` 參數與回應解析、HostModule 宣告一致。
- `scripts/yuzora/check-mods yuzora-settings-general yuzora-settings-templates yuzora-settings-launch yuzora-settings-mods`。
- 隔離實機：見 PR 描述的截圖與步驟。

## 待辦

- Windows 遠端模板、Windows 本機垃圾桶未實機驗證（只有 CI 編譯）。
- 外觀頁與上游 Look 頁功能重疊；L1 確定 Yuzora 的主題／字型方案後，可考慮在 `LOOKS` 之外收斂。
- 模板 body 編輯器沒有 frontmatter 語法提示；hc 的按鍵名稱清單是近似值（只會警告）。
- macOS 沒有 `/usr/bin/trash`（macOS 15 以前）時改用 Finder；Finder 對 symlink 是否只移走連結未實測。
