# L1 外殼＋工作區

lane L1 負責 Yuzora 的外殼：右側 dock 的模式（Files、Git、Database、SSH）、模式列與抽屜、tab 列、pane 標頭與外框、狀態列、新增 pane 選單（混合分割），以及 `yuzora-dark`／`yuzora-light` theme、tokens 與上游畫面的品牌字樣。設計依據是設計稿 p1–p6；p6 的工作區模式在 beta.4 起不再是預設 layout 的一部分（見「設計決策」）。架構規則見 [architecture.md](architecture.md)，lane 流程見 [lanes.md](lanes.md)。

## 範圍與檔案

| 路徑 | 內容 |
| --- | --- |
| `app/crates/yuzora-shell/` | `yuzora:shell`：工作區樹模型、Space 徽章色、git 摘要、新 Space 的資料夾 |
| `app/src/yuzora/shell.rs` | app 端 glue：pane 外框的 `blocked` 邊框；人執行的 `space.new` 改開資料夾對話框 |
| `app/defaults/mods/yuzora-sidebar-workspace/` | **legacy**（beta.4 起預設 layout 不再使用，只給 beta.2／3 的舊 clone）：側欄工作區模式；`status/*.svg` 是 5 種 agent 狀態的形狀，其他 L1 mod 也引用 |
| `app/defaults/mods/yuzora-modebar/` | 模式列，依 `props().dock` 有三種形態：`bar`（右側 dock 的 40px 垂直列）、`drawer`（沒有 panel 的 layout 的抽屜）、`head`（抽屜開出的 window 標頭）；沒有 `dock` 時是舊行為（**legacy**：舊 yuzora clone 的橫向 5 模式列、工作區徽章、設定齒輪） |
| `app/defaults/mods/yuzora-shell-tabs/` | tab 列與新增 pane 選單（由指令開啟） |
| `app/defaults/mods/yuzora-shell-pane/` | pane 標頭 |
| `app/defaults/mods/yuzora-new-space/` | 新增 Space 的資料夾對話框 |
| `app/defaults/mods/yuzora-statusbar/` | 狀態列 |
| `app/defaults/mods/yuzora-brand/` | **legacy**（beta.4 起預設 layout 不再使用）：側欄頂端的品牌列，標誌與搜尋（開指令面板）；只給舊 clone，之後的 legacy 清理會刪 |
| `app/defaults/layouts/yuzora.json` | 預設 layout：左側是上游的 `workspace-chrome`（40px 頂列）＋`spaces`＋`agents`＋`workspace-chrome` footer，`min 44`；工作區改用上面的 tab 列與 pane 標頭，加上 `blocked` 邊框與 UI 字型；模式由右側 dock 注入（layout 不可引用 `yuzora-brand`、`yuzora-modebar`、`yuzora-sidebar-*`，否則被判定為 Own、不注入，`shell.rs` 的測試鎖住） |
| `app/defaults/themes/yuzora-{dark,light}.json`、`app/defaults/yuzora/tokens.json` | 深色／淺色 theme 與 tokens（`light` 區塊是淺色主題的覆寫值） |
| `app/defaults/frame/{welcome,whatsnew,feedback}/` | 上游畫面，只改品牌字樣（見「上游改動」） |

## `yuzora:shell`

完整的 TypeScript 宣告在 app home 的 `yuzora.d.ts`（來源 `crates/yuzora-shell/src/lib.rs` 的 `DECLARATIONS`）。

| 函式 | 用途 |
| --- | --- |
| `workspace({spaces, panes, agents, machines, hosts, scope, folded, colors})` | 側欄的整個模型：machine → Space → worktree → agent 的列、需要注意清單（blocked 優先，再來是 done，各自新的在前）、scope chips（每台機器的 LED 與數量）、Agents 分頁的扁平清單（最急的在前，其餘照 Herdr 順序），以及給 `setAgentOrder` 的順序。純函式，有單元測試 |
| `spaceBadges(spaces, kept)` | 每個 Space 的徽章：名稱第一個字母或數字（大寫）＋顏色。顏色取自 `tokens().space_badges`，`kept` 是 `shared("yuzora.space.colors")`（`{space id: 顏色}`）；沒有顏色的 Space 拿目前最少用的顏色，回傳 `changed: true` 與新的 `kept`，呼叫端寫回同一個 key。純函式，不寫檔 |
| `hostname()` | 這台電腦的名稱（去掉網域），側欄「這台電腦」列使用 |
| `agentBrand(app, key)` | agent 品牌（`claude`／`codex`／`pi`／`gemini`），決定徽章色 |
| `gitStatus(cwd, {on})` | branch、upstream、ahead、behind（本機或 `hc exec --on`，每步 10 秒上限）；不在 repo 時回傳 null。只讀 ref、config 與 commit（`symbolic-ref`、`config`、`for-each-ref`），不跑 `git status`，所以 repo 自己設定的程式（clean filter、fsmonitor hook、ssh 指令）都不會執行，也不會在 `.git` 寫東西。`changed`、`conflicted` 需要 `git status`，在 folder 被信任前固定是 null（見「設計決策」） |
| `planSpace(folder, spaces, {on})` | 新 Space 的資料夾在它所屬的機器上解析成實際路徑（`~`、symlink、結尾分隔符號；只有 Windows 路徑把 `\` 當分隔符號，其他系統上的 `\` 和結尾空白都是名稱的一部分），回傳 `{folder, name, existing}`：`name` 是最後一段、`existing` 是 `spaces`（同一台機器的 `{id, folder}`）裡根目錄相同的 Space。遠端用 `hc exec --on` 跑固定的 `sh` 腳本，路徑當參數傳，不拼進指令，結果每筆以 NUL 結尾（名稱可以含換行）；那台機器沒有 `sh`（Windows）時改用 `powershell.exe`（和上游在 Windows 開終端機的做法一樣），路徑寫成單引號字串（裡面不展開任何東西），不跟隨連結；10 秒上限。失敗回傳 `{error: "not_a_folder" \| "unreachable", detail}` |
| `beginSpace(machine, lease?)`／`endSpace(machine, lease)` | 同一台機器同時只開一個新增 Space 對話框。`beginSpace` 回傳這個對話框的租約編號，另一個還開著時回傳 null；建立 Space 前帶著租約再呼叫一次，確認還握著。`endSpace` 只放掉自己的租約，不會清掉別的對話框的。10 分鐘沒有再握的租約（對話框沒說自己關了）可以被新對話框接手，舊對話框這時建立會被拒絕 |

worktree 的分組依 agent 所在 pane 的 branch 與資料夾；不在 git 裡的 agent 以資料夾名稱分組。同一個 Space 裡兩個資料夾剛好在同名 branch 時是兩列，各自帶資料夾名稱（列的 `detail`），「＋」開在該列自己的資料夾。`hosts` 照 `hc machine ls --json` 的樣子傳入；欄位型別不符（例如 `via` 是布林）只會略過該欄位，不會讓整個側欄失敗。

**其他 lane 可用**：`spaceBadges` 讓各 lane 的側欄標頭、pane 與 L1 畫出同樣的 Space 徽章（例如 L5 檔案模式、L2 Git 模式的「Y」方塊）：

```js
const r = spaceBadges(spaces().map((s) => ({ id: s.id, name: s.name })), shared("yuzora.space.colors"));
if (r.changed) setShared("yuzora.space.colors", r.kept);
const badge = r.badges[spaceId]; // { initial, color }
```

同一份 `kept` 算出的結果固定，所以多個 mod 同時寫回也一致。

## 共享狀態

L1 寫的 key：

| key | 內容 |
| --- | --- |
| `yuzora.badge.workspace` | **legacy**（只有舊 clone 的 legacy 模式列寫，預設 layout 不用）：等待回覆（blocked）的 agent 數，全部機器；0 時為 null |
| `yuzora.space.colors` | 每個 Space 的徽章色（`{space id: "#rrggbb"}`）。tab 列、pane 標頭、狀態列在出現新 Space 時寫入；移除的 Space 保留顏色（遠端機器暫時離線，回來時顏色不變） |
| `yuzora.shell.agents` | 各機器 `knownAgents()` 的上次結果（新增 pane 選單先顯示，再背景更新） |
| `yuzora.shell.split` | 新增 pane 選單放在 `right` 或 `down` |
| `yuzora.dock.last` | 右側 dock 上次開的模式（`files`／`git`／`database`／`ssh`），`yuzora-modebar.toggle` 用它 |
| `yuzora.startup.applied` | 「啟動時開啟的右側模式」已套用的那一次執行（`session()`）：每次啟動只由第一個 bar 或抽屜套用一次，換 layout 不會再套用 |
| `yuzora.startup.honored` | 有 dock 套用啟動模式時設為 true；設定 › 一般頁看到它才顯示「Right-hand dock」這個選項（沒有人套用的設定不顯示） |
| `yuzora.dock.open` | 抽屜的 window 現在開的是哪個（`{id, mode, session}`），由 window 標頭寫入；關閉鈕與標頭的 `close()`（換 layout、結束 App；只在自己的 id 還是這個值時）清成 null。`session` 是 `yuzora:core` 的 `session()`（每次執行不同）：shared 會存進 shared.json、overlay id 每次執行從 o1 重算，讀的人（`openNow()`）只信這一次執行寫的，上次留下的視為沒開 |

dock 另外寫 Herdr 的 `space.colors`（上游 `spaces` 讀的 Space 色）：`yuzora.space.colors` 有 hex、而且 `space.colors` **不是 `#` hex** 的 Space（只限目前存在的），模式列／抽屜把同一個 hex 寫進去（把 beta.3 使用者的 Yuzora 色帶過去）；`space.colors` 已是 `#` hex 的不覆蓋（那是使用者在上游 UI 的選擇）；Yuzora 沒有顏色時不寫。不能只填空：上游 `spaces`、`picker` 每次 render 都替沒有顏色的 Space 填 `ansi*` 色名，而且左側先於模式列 render，「只填空」永遠寫不進去（render 測試 R5 先跑 `spaces` 再跑模式列來守住）。左側欄與上游 `main.json` 的 sidebar 一致（只差寬 248、頂列 40），由 `shell.rs` 的測試守住（`min 44`、`window_drag`、footer prop）。`yuzora.badge.workspace` 與 legacy 模式列的 `WAITING` 只剩舊 clone 在用，等 legacy 清理一併移除。`workspace.panels` 不動（它只決定 tab 列的 Files／Changes 兩顆鈕）。

L1 讀的 key：

- 模式列：`yuzora.badge.<workspace|files|git|database|ssh>`，number 或 null；超過 99 顯示 `99+`。workspace 用琥珀色，其餘用藍色。
- 狀態列：`yuzora.status.<lane>`（`{text, tone?}`，tone 為 `muted`、`working`、`blocked`、`done`、`accent`），依序讀 `shell`、`git`、`db`、`database`、`ssh`、`files`、`settings`、`release`。lane 名稱若有不同，請在整合時告知 L1 補進 `LANES`（`yuzora-statusbar/main.js`）。

## 設計決策

- **tab 列只列出目前 Space 的 tab**（使用者回報，2026-11-04）：沿用上游 Herdr tab 列的做法，只列 active Space 的 tab，以 Space 區分；沒有 active Space 時列表是空的。切換 Space 時列表跟著換（側欄的 Space 清單、`tab.select` 跨 Space 切換不受影響）。徽章仍顯示目前 Space 的色塊與縮寫，tooltip 仍帶 Space 名稱。設計稿 p1 把不同 Space 的 tab 並列在同一條 tab 列，與這個決定不一致，以此條為準。已收起（settled）的 tab 不顯示，除非正在看它。拖曳排序只在同一個 Space 內（列表本來就只有一個 Space）。測試：`render_tests.rs` 的 `yuzora_tab_bar_lists_only_the_current_space`。
- **右側面板（Files／Changes）**：沿用上游 tab 列的切換鈕與「每個 Space 記住自己的面板」：切換 Space 時恢復該 Space 上次的選擇（沒選過就關閉）。`shared("workspace.panels")` 決定顯示哪幾顆。tab 列不再提供分割鈕；分割在 pane 標頭（`yuzora-shell-pane`）。
- **右側 dock 的模式列與抽屜**（beta.4 PR-2，設計見 `specs/right-dock-spec.md`）：`RIGHT_DOCK` 開啟後，沒有自己放 Yuzora 模式的 layout 都會得到 dock（`src/yuzora/dock.rs` 注入，不寫進 layout 檔）。有 `panel`／`panel-body` 的 layout（`main`）：四個模式加進 `panel-body` stack，`panel` 右邊多一條 40px 的垂直模式列（`yuzora-modebar {dock: "bar"}`，一直看得到）；點沒顯示的模式 = `layout.select` ＋ `layout.show`，點正在顯示的模式 = `layout.hide`。沒有 panel 的 layout（`terminals`、`win98`、自訂）：root 的 floating 加一個 peek 40 的抽屜（`{dock: "drawer"}`），只當啟動列（一個 float 只能放一個 mod，而且每次按壓都會把焦點還給 terminal），點模式開一個 `kind: "window"` 的 overlay（key `yuzora-dock`、`keep`、380px × 85%，標頭 `{dock: "head"}` 有四個模式分頁與關閉鈕），同一個 key 再開會在原位置取代。抽屜在左右邊緣時圖示在靠邊的一側、名稱在內側；拖到上下邊緣就變成只有圖示的橫列。指令 `yuzora-modebar.files|git|database|ssh|toggle`（可綁 `keys.json`，狀態列的 branch 點擊走 `yuzora-modebar.git`）。beta.2／3 複製出來、自己放 Yuzora 模式的 yuzora layout 判定為 Own，不注入，畫面不變。出貨的 `yuzora` layout 已改版（PR-3，見「yuzora layout 改版」）：它是 Panel，和 `main` 一樣。
- **右側 panel：Yuzora 模式全域保持，Files／Changes 仍按 Space 記憶**（使用者決定 Q1＝b，2026-11-04）：`tabs` 與 `yuzora-shell-tabs` 的 `syncPanel` 各在 `const now = …` 後加 2 行：畫面上顯示的是 `yuzora-dock-*` 時，切換 Space 不記錄、不恢復、不關閉。不這樣做的話，切到沒開過 panel 的 Space 會把 dock 關掉，而且之後不會恢復。`scripts/yuzora-dock-panels.test.ts` 在兩個檔案各自實際的 `syncPanel` 原始碼上跑切換情境。
- **個人 dock 開關**（beta.4 PR-4，使用者決定 Q5，2026-11-04）：shipped layout 是唯讀的，所以 `main`、`terminals`、`win98` 不能靠 layout 檔的 `"yuzora": {"dock": …}` 關掉 dock。設定 › Mods › Layouts 的每一列多一個「Yuzora dock」三段切換（Auto、Drawer、Off；`yuzora-settings-mods`），選擇存在 Yuzora `settings.json` 的 `shell` section：`{"shell": {"dock": {"layouts": {"win98": "off"}}}}`。優先順序：**個人設定 > layout 檔的 `yuzora.dock` > auto**（個人設定的 `auto` 也蓋過 layout 檔的 `off`）；沒設或不認得的字等於沒有個人設定。切換目前顯示的 layout 時，頁面接著執行 `layout.switch {name: <目前的>}`（同名時 `switch_layout` 仍會存檔再載入），所以立刻生效；其他 layout 只寫設定。讀取在 `dock.rs`（`yuzora_core::settings::section("shell")`），`inject`／`ensure`／`as_read` 每次讀一次，上游檔不必多一個 hook。dock 關掉（或換成抽屜）的期間，使用者在 dock 上的點選（開的模式、panel 寬度以外的 bar 隱藏、抽屜位置）不會丟：`save` 算點選時用 `dock::keep_clicks` 把 `.state` 裡沒顯示的 dock 部分帶過去（`window.rs` 一行），改回 Auto 後還在。這頁只列出 layout，不顯示 layout 檔自己的 `yuzora.dock`；沒有個人設定時，切換上顯示的是 layout 檔的值（沒有欄位就是 Auto），所以 `listMods()` 的 layout 多一個 `dock` 欄位。
- **預設快捷鍵與啟動模式**（beta.4 PR-4）：`cmd-alt-b`（macOS）與 `ctrl-alt-b`（其他）綁 `yuzora-modebar.toggle`（顯示上次的模式，或收起來；VS Code 的 secondary sidebar 用同一組）。上游 v0.1.14 的預設鍵沒有占用這兩個。`keys.json` 只在檔案不存在時寫入，但 `keys.rs` 的 `SHIPPED_KEYS` 會把「原樣的 beta.2／beta.3 `keys.json`」升級成目前的預設，所以沒改過的使用者拿得到新鍵（測試 `untouched_yuzora_beta_keys_are_upgraded…` 守住）；**改過 `keys.json` 的使用者不會被補**：AGENTS.md 要求替換既有按鍵前先問人，App 不在使用者的檔案裡加鍵，release note 要寫「自己加一行 `"cmd-alt-b": "yuzora-modebar.toggle"`」。設定 › 一般 › 「When Yuzora opens」的「Right-hand dock」（`general.startup.dock`：`last` 預設、`none` 收起、`files`／`git`／`database`／`ssh`；選項沒有 workspace，Q6）：第一個出現的 bar 或抽屜（`yuzora-modebar` 的 `start()`）每次執行只套用一次；bar 是 `showMode` 或 `layout.hide`，抽屜是開那個模式的 window（`none` 時什麼都不做，`keep` 的 window 照常重開）。從沒有 dock 的 layout（`yuzora.dock: "off"`）起動時，要等切到第一個有 dock 的 layout 才套用。舊的 `general.startup.sidebar` 不再讀。
- **「＋」與新增 pane 選單**：「＋」開上游的 `new-menu`（新 tab：終端機、agent、新 Space、worktree）。tab 列不再提供新增 pane／分割鈕（使用者要求，2026-11-04）；新增 pane 選單保留，從 `yuzora-shell-tabs.new-pane` 開啟。選單在目前的 tab 內混合分割：工具 pane（`yuzora:core` 的 `panes()`）用 `pane.split {direction, pane, mod, props}`、終端機用 `pane.split {direction, pane}`、agent 用 `terminal.open {argv, place}`；位置（右側／下方）在選單頂端切換。沒有 focus 的 pane（還沒有 Space，或 focus 在 layout 的終端機 block）時改開新 tab。
- **blocked 外框**：上游的 pane frame 只有 `border` 與 `focus`。L1 在 `PaneFrame` 加了 `blocked`（`{width, color}`），agent 需要回覆時優先使用（即使該 pane 有 focus，因為那裡需要人）。標頭底色 `#1d1a14` 由 `yuzora-shell-pane` 自己畫。工具 pane 沒有 agent，不會變色。
- **狀態形狀**：每種狀態同時用顏色與形狀表示（工作中綠點、需要回覆琥珀三角、完成未查看藍菱形、閒置空心圓、未知虛線圓）。GPUI 沒有 CSS 旋轉與三角形 border，所以 5 種形狀做成 SVG（`yuzora-sidebar-workspace/status/`），以 tokens 的 `status.<x>.mark` 上色。
- **設定齒輪**：L6 的設定框架合併前，先開上游的 `app.settings`。圖形是 6 齒（8 齒在 1x 時齒縫會被線寬填滿）。
- **yuzora layout 改版**（beta.4 PR-3，使用者決定 Q3、Q6，2026-11-04）：左側改用上游原生的 `workspace-chrome`（頂列 40px、`window_drag`、id `brand`）、`spaces`、`agents`、`workspace-chrome` footer（Settings、Add machine），寬度 248、`min 44`（上游的 rail 才縮得下去）；不放 `yuzora-brand`，也不做它的窄版。Yuzora 的四個模式移到右側 dock（`panel` 右邊的 40px 垂直列）。「工作區」模式不保留：scope chip、「需要注意」清單、LED、合併式「＋」選單、HERDR 工具選單隨 `yuzora-sidebar-workspace` 一起退場，Space 排序、新增 Space 先選資料夾、機器分組、worktree 由上游 `spaces` 負責（`run_as` 攔截對它的「＋」同樣有效）。root 的 `traffic_lights` 維持 `[14, 13]`。左側的字串是上游寫死的英文（整個 App 只留英文）。舊的 `.state/yuzora.json` 裡 `yuzora-sidebar` 的點選會因 id 不存在被忽略；改過 `yuzora.json` 的使用者照例得到 `yuzora-edited`。
- **品牌標誌**（legacy：beta.4 起預設 layout 的左上角是上游的 `workspace-chrome`，顯示「Herdr」；Yuzora 標誌只剩 app icon、welcome 與官網）：側欄頂端畫 Yuzora 正式標誌（`public/brand/yuzora-mark.svg`），用 `image()` 保留四色；`svg()` 會把它當單色遮罩。gpui 不支援 CSS `var()`，所以 `yuzora-brand/mark.svg` 直接寫入原檔的 fallback 色碼。淺色 chrome（判斷方式同 `settings.rs` 的 `is_light`）改用 `mark-light.svg`：銀色面改成 slate `#94a3b8`，摺面依原本的比例加深成 `#668296`，否則銀色面在淺色底上幾乎看不見。
- **鍵盤與腳本入口**：`yuzora-shell-tabs.new-pane` 開新增 pane 選單（置中顯示的 dialog；tab 列已沒有對應的按鈕）；`yuzora-sidebar-workspace.spaces`／`agents` 切換分頁，`add`／`tools` 開對應選單。可綁進 `keys.json`（`"alt-n": "yuzora-shell-tabs.new-pane"`），也可用 `herdr-app do yuzora-shell-tabs.new-pane`。
- **Space 排序**：在側欄把 Space 拖到同一台機器的另一個 Space 上即可移動（`space.move`），右鍵開上游的 Space 選單（改名、worktree、關閉）。放開在其他地方或拖出側欄就取消。
- **側欄「HERDR 工具」選單**：只有「指令面板」與「HERDR 說明」兩列。不放「下一個 layout」（使用者要求，2026-11-04）：從 `yuzora` layout 按下去會繞到上游的 `main`，Yuzora 的功能全部消失且回不來。換 layout 走設定 › Mods › Layouts 的「切換」；`layout.switch` 指令本身不動。同一個原因，預設快捷鍵（`keys.json` 的 `ctrl-shift-l`、`keys.macos.json` 的 `cmd-shift-l`）與指令面板的「Next layout」也拿掉（`frame/palette/main.js` 的 NAMED 與 HIDDEN，標 `// yuzora(L1):`）；指令面板的「Layout: <名稱>」照舊。`keys.json` 只在檔案不存在時寫入，所以已裝 beta.2 的使用者檔案裡還留著舊綁定：`keys.rs` 的 `parse` 在 global 區段略過「舊預設鍵 → `layout.switch`」這一條（`yuzora(L1)` 註記）；使用者改綁到別的鍵、或綁 `layout.switch:<名稱>` 的都保留。
- **機器的「新增 Space」**：只在該機器的 layout 已讀到（LED 綠燈）時提供；讀取中 Herdr 會拒絕結構性指令。狀態列的 git 摘要在 focus 的終端機所屬機器上執行（終端機位址 `<machine>/<id>`），不是目前 Space 的機器。
- **新增 Space 先選資料夾**（beta.1 的「開啟資料夾成為 Space」）：人執行、而且只帶 `machine`（或什麼都不帶）的 `space.new`，以及沒有任何 Space 時不帶參數的 `tab.new`（它會順便建立 Space）——快捷鍵、指令面板、選單、側欄與 picker 的「＋」——由 `window.rs` 的 `run_as` 攔下，改開 `yuzora-new-space` 對話框（`shell.rs` 的 `asks_folder`）；沒帶 machine 時沿用上游規則，用目前 Space 的機器。帶 name、folder、tab、argv 或 task 的呼叫（worktree、SSH session、腳本、對話框自己）照原樣執行；`herdr-app do`（腳本與 agent）也不攔。本機開系統的資料夾選擇器（`pickFolder`），取消就關閉、什麼都不建；遠端機器不能用本機選擇器，改輸入路徑（預設 `~/`），由 `planSpace` 在那台機器上確認，所以不會拿本機路徑去建遠端 Space。L4 的 SFTP 瀏覽要先過 SSH 認證、herdr 也沒有列目錄的 API，所以先用輸入框。解析後的實際路徑若已是同一台機器上某個 Space 的根目錄（folder 空白的 Space 視為家目錄），就切換到它並提示，不建第二個。對話框開著時同一台機器不能再開一個（`beginSpace`），檢查中顯示狀態，錯誤（找不到資料夾、機器沒回應或離線、選擇器開不起來）以 en／zh-TW 顯示在對話框裡。**不保留「不選資料夾」的捷徑**：每個 Space 都有有意義的根目錄，去重才有依據；要在家目錄開就選家目錄；腳本仍可直接 `space.new`。
- **狀態列的 git 不跑 `git status`**：終端機一 `cd` 進資料夾，狀態列就會自動查詢，那個 repo 可能是剛下載、不可信的。`git status` 會看工作樹，並執行 repo 自己 config 指定的程式（`filter.<x>.clean`／`process`、`core.fsmonitor` 等），所以只讀 ref 與 commit：`symbolic-ref` 取 branch，`for-each-ref` 的 `%(upstream:short)`、`%(upstream:track)` 取 upstream 與 ↑↓。partial clone（promisor remote）缺 commit 時，走訪 commit 會用 repo 的 ssh 指令 lazy fetch，所以這種 repo 加 `--no-lazy-fetch`（git 2.44 起）；更舊的 git 不顯示 ↑↓。變更數與衝突數要等 L2 的 trust 判斷搬進 `yuzora-core`（#194）後，只在 folder 已信任時才用 `git status` 查；在那之前回傳 null，狀態列本來就只畫 branch 與 ↑↓。測試用會留下標記檔的 clean filter 與 ssh 指令當 oracle，並確認舊指令確實會觸發它們。
- **scope**：記住的是使用者選的 chip。選的機器暫時不在（剛啟動還沒列出）或離線（LED 熄滅）時先顯示全部，機器回來後自動回到該機器；連線中（LED 琥珀）時維持原 scope。
- **新增 pane 選單的高度**：跟著列數長高，最高 520px（`MENU_MAX`）；超過時位置切換列固定、下方清單捲動。原本以 popup 開啟時 popup 會依內容決定高度；現在只從指令以 dialog 開啟（不給 `height`），沿用同一套寫法（`set_fit_content`，`height` 只是上限），所以選單裡不能有等待「填滿高度」的元素：根元素不用 `size_full`，清單不用 `flex_1`，也不用 `overflow_y_scrollbar`（它的捲動區會填滿父層，改用 `overflow_y_scroll`，和上游的 machine-add、picker 一樣），否則清單高度會塌成 0，只剩位置切換列（曾經發生過，`render_tests.rs` 的 `yuzora_new_pane_menu_*` 鎖住）。選中的位置用 accent 底色，點了立即更新，也記在 `yuzora.shell.split` 給下一次用（截圖：`evidence/l1-pane-menu-before.png` 修正前、`l1-pane-menu-popup.png` 修正後、`l1-pane-menu-below.png` 選「下方」）。沒有工具 pane 或 agent 時顯示說明。還沒有 Space 時工具 pane 停用並註明「需要先有 Space」（`tab.new` 可以替終端機或 agent 順便建立 Space，mod 不行）。
- **Herdr 的「檔案」「變更」panel 預設關閉**：Yuzora 的右側 dock 已有檔案（L5）與 Git（L2）模式，兩個原生 panel 重複。shared `workspace.panels` 沒有值（undefined／null）時視為 `[]`（`yuzora-shell-tabs` 的 `panelsOn`、welcome frame 的 `panels()`，後者標 `// yuzora(L1):`），tab 列就不顯示這兩顆按鈕；使用者存過的值照舊。layout 的 `panel` 節點維持 `hidden: true`。重新開啟的入口：welcome 第三頁的開關，以及 L6 一般設定頁新增的「Panel」區塊（兩個開關，關掉正在顯示的那個會一併收起）。不放指令面板：它的指令名稱是上游寫死的英文清單，沒有 i18n，也看不出目前是開是關（截圖：`evidence/l1-panels-off.png` 預設的 tab 列、`l1-panels-settings.png` 設定頁）。上游的 `tabs` mod 不在 Yuzora layout 裡，不動。
- **側欄的＋選單**依列數決定高度（最高 360px，超過時捲動）；「需要注意」最多 5 列高，更多時在框內捲動，下方的樹不會被擠出畫面。
- **狀態列**：各 lane 的狀態文字共用中間剩下的寬度，太長就截短（再不夠就裁掉），右側的 HERDR 版本與指令面板快捷鍵一定看得到。
- **Space 徽章色放 shared state**：不寫 Yuzora `settings.json`，所以測試與 render 不會碰 app home 以外的檔案，也不會蓋掉別的設定；L6 若要在外觀頁讓使用者改色，改寫 `yuzora.space.colors` 即可。
- **Yuzora 主題色調**：`yuzora-dark` 保留設計稿的中性底色層次（canvas、chrome、surface、popover、raised、border、divider），互動色改用品牌色（logo 的藍 `#3b82f6`、青 `#14b8a6`）：accent `#60a5fa`（品牌藍淺一階，在深色底上當文字也達 AA）配品牌墨色 `#0e1026` 當 accent_foreground；ring 用品牌藍；hover／selected 照 beta.1 的推導方式，把品牌藍以 9%／15% 混入中性底色；done `#89b4fa`（品牌藍 60% 混白）；Space 徽章第一、二色是品牌藍與青（都改成白字達 AA 的深一階）；終端的 blue／cyan 換成品牌藍與青。`yuzora-light` 是對應的淺色版（accent `#1d55d4` 配白字）。layout 的 theme 是 `{light: yuzora-light, dark: yuzora-dark}`，跟隨系統或 settings.json 的 `appearance`。`tokens.json` 的值給深色用，`light` 區塊是淺色主題的覆寫值；`tokens()` 依目前主題背景的亮度選用（app 在 `yuzora/mod.rs` 以 `tokens::set_light` 告訴 yuzora-core），所以其他 mod 不必改。對比由 yuzora-core 的 `both_yuzora_themes_read_at_wcag_aa` 測試把關：文字 4.5，狀態標記與 ring 3，終端除了黑色以外的 ansi 4.5。layout 的 theme 欄位一改，upstream 會丟掉使用者在這個 layout 上點選的主題（`layout_sync::forget_edited`）。這次是 beta.2 發布前唯一一次改這個欄位，之後調色只改 theme 檔；舊版 `yuzora-dark` 列在 `settings.rs` 的 `PREVIOUS_THEMES`，沒改過的會自動更新，改過的保留。
- **淺色主題的對比**：Yuzora mods 與 layout 裡的上游 mods 不寫死顏色（只剩全透明的 `#00000000`）。L3 資料庫畫面原本的深色專用色碼（SQL 高亮、DataGrid 的錯誤字、編輯中的列、主按鈕字色）在白底只有 1.1–3.2:1，現在改從 tokens 取：`tokens().sql`（keyword、type、string、number、comment、function，mod 的 `highlight` 與原生 `SqlEditor` 共用）、`database.edited_row`、`database.error`，主按鈕字色是 DataGrid 新增的 `buttonText`（傳 `accent_foreground`）。`both_yuzora_themes_read_at_wcag_aa` 一併檢查這些色在兩個主題各底色上的對比（文字 4.5，圖示 3）。不隨主題變的：上游 picker／spaces 的徽章（固定色配白字）、agent 品牌色徽章、settings 的主題預覽卡。
- **字型**：見下節。

## 字型

設計稿用 Inter＋Noto Sans TC。授權與打包方式已確認：

- Inter 與 Noto Sans TC 都是 SIL Open Font License 1.1，可以隨 app 散布，需附授權文字。
- herdr-app 會在啟動時載入 mod 的 `fonts/` 或 app home `fonts/` 下的 `.ttf`／`.otf`（Herdr 98 的字型就是這樣帶的）；`build.rs` 會把 `defaults/` 打包進執行檔。
- 這台機器與 repo 都沒有這兩套字型，取得需要從外部下載；Noto Sans TC 可變字型約 12 MB，打包會讓執行檔與 repo 明顯變大。

所以 beta.2 先**退回系統字型**：`yuzora.json` 的 `ui_font` 設為 `.SystemUIFont`（GPUI 的系統 UI 字型：macOS 為 SF Pro，中文由系統字型 fallback 到蘋方；Windows 為 Segoe UI；Linux 為系統預設 sans）。終端機字型不變（JetBrains Mono）。要改用 Inter 時：把 `InterVariable.ttf` 與 OFL 授權放進會留下來的 Yuzora mod（例如 `defaults/mods/yuzora-shell-pane/fonts/`，不放 `yuzora-brand`，它是 legacy，之後會刪）的 `fonts/`，`ui_font` 改為 `Inter`，並在 `THIRD-PARTY-NOTICES` 補上授權（列在待辦）。

## 上游改動

全部以 `// yuzora(L1):` 註記；右側 dock（lane 名稱 `dock`，beta.4 PR-1，設計見 `specs/right-dock-spec.md`）的改動以 `// yuzora(dock):` 註記，列在表中標有「dock」的列：

| 檔案 | 改動 |
| --- | --- |
| `app/src/layout.rs` | `PaneFrame` 新增 `blocked: Option<Line>` 欄位 |
| `app/src/layout.rs`（dock） | `Node` 新增 root 欄位 `yuzora: Option<Value>`（個別 layout 的 `"yuzora": {"dock": "auto" \| "drawer" \| "off"}`；沒有欄位的話 `text_of` 重寫檔案時會把它丟掉）與 `Node::new` 的一行 |
| `app/src/window.rs`（dock） | 非測試 diff 為 +8／−4（7 處，皆 `yuzora(dock)` 註記；8 行新增裡 4 行是整行改寫、4 行是純新增）：`load_layout` 在 `keep_ids`／`fill_ids` 之後、`file_chrome = chrome.clone()` 之前 `dock::inject`（H1，1 行）；`check_layout_file` 在 `keep_ids` 之後 `dock::inject`（H2，1 行）；`save` 寫檔前 `dock::strip`（H3，改 1 行）；`layout.set` 的 shipped 分支 `dock::ensure`（H4，新增 1 行；`ensure` 多收 `Some(&self.file_chrome)`，重建 dock 時讓 stack 退回檔案原本的選擇、再選回 dock 的點選，並把 `let n` 改成 `let mut n`，改 1 行）；`layout.set` 的個人分支 `dock::ensure` 後寫檔前 `dock::strip`（H5，新增 1 行、改寫 `let text` 1 行），並把 `file_chrome` 改成 `dock::as_read(…)`（寫出去的樹重新注入，改 1 行；不能存沒 strip 的樹，否則下一次 `layout.set` 或存檔會把 dock 的 `active` 寫進檔案）。`save` 另有一行 `dock::keep_clicks`（PR-4，H6：把 `.state` 裡沒顯示的 dock 點選帶過去，見「個人 dock 開關」）。`check_layout_file` 在重算 state 後也呼叫 `dock::keep_clicks`（PR-4 修正輪 S2，+1 行：off／drawer 期間 layout 檔被改，dock 點選不丟）；`load_layout` 與 `check_layout_file` 套用 state 時改成 `dock::without_panel_click(&state, &chrome).apply(…)`（各改 1 行：dock 不在 panel 時，為 dock 模式開的 panel 不顯示，否則抽屜旁會殘留上游 Files panel；原始 state 照寫，改回 Auto 時 panel 還開著）。`RIGHT_DOCK` 為 false 時 inject、ensure 不做事（不 strip），save 與 `layout.set` 仍 strip（只動保留 id）。測試新增約 414 行（`mod tests`）：W1／W1b（連續 `layout.set` 加結構存檔）、W4（個人選擇立即生效且 `.state` 點選不丟）、啟動模式（第一個 bar 套用一次）、W2（檔案 opt-out 來回、`file_chrome` 有 dock、存檔不改檔案；以檔案內容比對，mtime 為輔助）、SF1（反轉 row、刪掉 dock 節點、改成 drawer 後，檔案的 stack 選擇與 dock 點選保留）、W3／W3b（`load_layout`／`check_layout_file` 的 `keep_ids` 順序）、S2（`layout.set` 前的 dock 點選重啟後還在）、重複 id／無 id 不 panic、shipped `layout.set`、原始碼守衛（也涵蓋 `window/*.rs` 子模組）。已知限制（N3）：layout 有重複 id 時，`strip` 的 fallback 以 id 查找，找到的不一定是被移除 dock 節點所在的那個 stack，檔案原本的選擇可能還原不了（只在 id 重複的 layout 發生，不會 panic）。已知限制（N4）：root 的 `yuzora` opt-out 在 root 被 `dock()` 包進新 row 時變成子節點，重啟後失效（與上游 `titlebar`、`traffic_lights`、`preview` 這些只讀 root 的欄位相同） |
| `app/src/window.rs` | `frame_pane` 多一行：呼叫 `crate::yuzora::shell::frame_line` 決定邊框；`run_as` 在 `space.new` 前多一段：人執行的 bare `space.new`（以及沒有 Space 時的 bare `tab.new`）改開資料夾對話框 |
| `app/src/window/movable.rs`（dock） | `open_kept_windows` 在每個 keep 的 window 前加 4 行（`// yuzora(dock)`）：key 為 `yuzora-dock` 的 window 在目前 layout 沒有抽屜（個人設定或 layout 檔改成 off／auto 之後）時不重開，並 forget 這個 key（`windows.json` 不留 keep）；判斷在 `dock::stale_window`。設定頁的 `setDock` 同時把當下開著的抽屜 window 關掉（只關本次執行寫入的 `yuzora.dock.open`） |
| `app/defaults/mods/tabs/main.js`（dock） | **上游 mod，lanes.md「不改上游 mods」的例外（使用者同意，2026-11-04）**：`syncPanel` 在 `const now = …` 之後加 2 行（1 行 `// yuzora(dock):` 註記、1 行 early return）：目前顯示的是 `yuzora-dock-*` 時只更新 `this.space`、清掉 `pending` 就 return，不記錄也不恢復。`yuzora-shell-tabs` 有同樣的 2 行。同步上游時若 `syncPanel` 被改動，這 2 行會衝突，或 `scripts/yuzora-dock-panels.test.ts` 會失敗 |
| `app/src/yuzora/mod.rs` | 加一行 `pub mod shell;`（lanes.md 允許的改動）；dock 再加一行 `pub mod dock;` |
| `app/src/mods/render_tests.rs` | welcome 第一頁的標題改為 Yuzora；新增 `yuzora_new_pane_menu_*` 兩個測試（新增 pane 選單在無固定高度的 overlay 裡的版面、點選後的指令）與 `yuzora_herdr_panels_start_off`（Herdr panel 預設關閉、設定頁開關） |
| `app/defaults/frame/welcome/main.js` | 標題、說明中的 app 名稱改為 Yuzora；圖示改 `yuzora.png`；請 agent 讀的 AGENTS.md 改用 `appHome()` 組出實際路徑（`YUZORA_APP_HOME`／`XDG_CONFIG_HOME` 改過時也正確）；`panels()` 沒有設定時視為沒有 panel |
| `app/defaults/frame/whatsnew/main.js` | 標題與圖示 |
| `app/defaults/frame/feedback/main.js` | 送出後的感謝文字 |
| `app/defaults/frame/welcome/yuzora.png` | 新檔：Yuzora 圖示（160px，由 `assets/icons/yuzora.png` 縮圖） |

保留不改：welcome 的「從 Herdr TUI 搬設定」段落（指的是 Herdr TUI）、`Herdr 98`（layout 名稱）、`herdr-core`。settings frame 歸 L6。

## 驗證

```bash
cd app
cargo test --locked -p yuzora-shell        # 樹模型、徽章色、git 解析與不執行 repo 程式、新 Space 的資料夾、HostModule 宣告
cargo test --locked --bin herdr-app yuzora # blocked 邊框、layout 引用的 mod 都有出貨、哪些 space.new 先問資料夾
scripts/yuzora/check-mods
bun test scripts/yuzora-dock-panels.test.ts   # 兩個 syncPanel 的 dock 例外（CI 不跑）
cargo test --locked --bin herdr-app yuzora::dock   # 注入規則、個人設定優先順序（T9）、keep_clicks
cargo test --locked -p yuzora-settings            # startup()、layout 的 dock 欄位
```

實機以 lanes.md 的隔離方式執行（暫存 `YUZORA_APP_HOME`、`HC_SESSION`），用 OSC 7501 假 agent 造出各種狀態：

```bash
printf '\033]7501;state=blocked:app=codex\033\\'; exec sleep 100000
```

工具 pane 用一個只放在測試 app home、不出貨的示範 mod（`mod.json` 宣告 `"yuzora": {"pane": …}`）。遠端機器用同一台電腦上另一個隔離的 hc session 模擬：

```bash
hc machine add --via 'sh -c' --name lab-01 --session <另一個測試 session>
```

新增 Space 的流程用 `herdr-app do mod.open --name <探針>` 驅動，探針只放在測試 app home：`yuzora-new-space` 的複本，只把系統選擇器換成讀固定檔案（本機）、或把路徑自動填入並送出（lab-01）；另一個探針以 mod 的 `run("space.new", …)`（算人執行）觸發攔截，開的是出貨的對話框。驗過：本機選 `/tmp/…/alpha-app` 建出 `alpha-app`，終端機 cwd 是 `/private/tmp/…/alpha-app`；再選 `link-to-alpha/`（symlink 加結尾斜線）切回同一個 Space、不新增；lab-01 輸入含引號、`&`、空白的路徑建出同名 Space，cwd 正確、沒有多出檔案；再輸入 `/tmp//…/` 切回它；目前 Space 在 lab-01 時不帶參數的 `space.new` 開 lab-01 的對話框；對話框開著時再開一次只跳提示；取消（選擇器回 null）什麼都不建，之後可以再開。系統的資料夾選擇器本身（`pickFolder`）要手動驗收。

截圖在 `docs/beta2/evidence/`：

| 檔案 | 內容 |
| --- | --- |
| `l1-new-space-remote.png` | 在 lab-01 新增 Space：路徑輸入框（由人執行的 `space.new {machine: "lab-01"}` 開啟） |
| `l1-new-space-existing.png` | 本機再選同一個資料夾（經 symlink）：切回 `alpha-app` 並提示，沒有新增 |
| `l1-new-space-errors.png` | lab-01 找不到資料夾的錯誤，以及同一台機器的對話框已開著時的提示 |
| `l1-scope-after-restart.png` | 選 lab-01 後重啟，scope 仍在 |
| `l1-blocked-and-badge.png` | 側欄在 Git 模式時新增 blocked agent：工作區徽章變 3、pane 琥珀外框與標頭 |
| `l1-agents-tab.png` | Agents 分頁（最急的在前） |
| `l1-theme-dark.png`、`l1-theme-light.png` | Yuzora 主題的全畫面：深色（yuzora-dark）與淺色（yuzora-light，`appearance: light`） |
| `l1-new-pane-menu.png` | 「新增 pane」選單（由 `yuzora-shell-tabs.new-pane` 開啟） |
| `l1-mixed-split-after-restart.png` | 終端機與工具 pane 混合分割，重啟後保留 |
| `l1-welcome.png` | welcome 的 Yuzora 品牌字樣 |
| `l1-modebar-gear.png` | 模式列（2x 裁切）：最右的設定按鈕是齒輪 |
| `l1-brand-dark.png`、`l1-brand-light.png` | （legacy，beta.2／3 的 `yuzora-brand`；beta.4 起左上角是 `workspace-chrome`，需要重拍）側欄頂端（2x 裁切）的 Yuzora 標誌，深色（yuzora-dark）與淺色（paper，`appearance: light`）主題 |
| `l1-light-*.png` | 淺色主題（`appearance: light`）的各畫面：`workspace`、`sidebar-files`、`sidebar-git`（L2、L5 尚未合併，仍是 L0 的佔位）、`sidebar-database`、`sidebar-ssh`、`database`（資料表與 SQL 查詢）、`sftp`、`settings`、`settings-mods`、`new-pane`、`palette`。SSH 主機來自假的 HOME（`build-server`、`staging-db`） |

beta.4 PR-3、PR-4 之後要重拍或新增的（截圖尚未重拍，圖檔不在此 PR 更新）：設定 › Mods › Layouts 的「Yuzora dock」切換、設定 › 一般的「Right-hand dock」啟動選項、yuzora layout 的全畫面（左上角 `workspace-chrome` 的 Herdr 圖示與字樣、Spaces／Agents、footer、右側 40px 模式列）、模式列開啟 Git 後的畫面、rail 狀態（`cmd-b`）、macOS 紅綠燈與 40px 頂列的位置、淺色主題。

## 已知限制（右側 dock）

- **抽屜的 window 裡的模式 mod，localStorage 每次開啟換一個檔**：抽屜開出的 overlay 把 overlay id 放進 props，而 mod copy 的 localStorage 檔名由 props 的 hash 決定；overlay id 每次執行、每次開啟都不同（`o1`、`o2`…）。結果：在抽屜 window 裡的模式，靠 localStorage 記住的狀態每次開啟都重置，例如 Files 的「顯示隱藏檔」。Panel 形態（`main`、`yuzora`）的模式沒有 overlay id，不受影響。要根治得改上游 `window.rs` 的 `overlay_layout` 或 `mods.rs` 的 `prepare_load`（讓 hash 不含 overlay id），這次沒動。
- **root 被包進新 row 時，opt-out 變子節點**（PR-1 N4）：layout 的 root 若是 block 而不是容器，`dock()` 會把它包進新 row，root 的 `"yuzora": {"dock": "off"}` 就跑到子節點，重啟後失效；個人設定（設定 › Mods › Layouts）不受影響，因為它不靠 root 的欄位。與上游 `titlebar`、`traffic_lights`、`preview` 這些只讀 root 的欄位相同。
- **dock 隱藏期間選了 stack 的預設子節點（Files），改回 Auto 時 dock 的模式會蓋回來**（PR-4 r1 S3）：`.state` 只記「和檔案不同」的點選，選回檔案原本的預設子節點不留痕跡，`keep_clicks` 分不出「選了 Files」和「沒點過」，所以把 dock 隱藏前的模式帶回來；選 Changes（非預設）會保留。要可靠區分得另記一份「點選過」的標記，不值得這個改動。
- **win98 的抽屜 window 底端壓到工作列**（驗收 d-14）：window 高度是螢幕的 85%，win98 的工作列沒有被排除在可用區之外；不修。win98 抽屜選中項的字色改用主題的 foreground（原本的 token strong 在淺灰底上看不見）。
- **只有英文**：dock 與左側的上游 mod 字串都是英文（整個 App 只留英文）。
- **已安裝使用者改過 `keys.json` 時沒有新預設鍵**：見「預設快捷鍵與啟動模式」。
- **啟動模式只由第一個 dock 套用**：第一個 layout 沒有 dock（`off`／舊 clone）時，晚一點才切到有 dock 的 layout 才會套用。

## 待辦

- 隨 app 打包 Inter（與 Noto Sans TC 的取捨），見「字型」。
- pane 標頭的 `repo` 目前是 pane 目前資料夾的名稱；在 repo 子資料夾時不是 repo 名稱。
- 設計稿 tab 列右側兩顆圖示按鈕的用途未定，暫時做成「在右側／下方開終端機」。

## 後續整合

- **L6**：設定框架合併後，模式列齒輪改開 Yuzora 的設定（`yuzora-modebar/main.js` 的 `app.settings`）。外觀設定頁可編輯 `yuzora.space.colors`（Space 徽章色）。
- **L2**：#194 把 trust 判斷搬進 `yuzora-core` 後，`gitStatus` 改接那個 API：folder 已信任時再跑一次 `git status` 補上 `changed`、`conflicted`，未信任時維持現在的行為（只有 branch 與 ↑↓，不報錯）。
- **L2–L5**：側欄模式的標頭可用 `spaceBadges()` 畫 Space 徽章（用法見上）；徽章數與狀態列文字照「共享狀態」的 key 寫入。工具 pane 在 `mod.json` 宣告 `"yuzora": {"pane": …}` 就會出現在「＋ 新增 pane」選單。
- **L4**：`yuzora-ssh-herdr`、`yuzora-sidebar-ssh` 只帶 `machine` 的 `space.new`（在 herdr 機器新增 Space）現在也會先開資料夾對話框；帶 `argv` 的 SSH session 不受影響。之後若 L4 提供遠端目錄瀏覽，可以接進 `yuzora-new-space` 取代路徑輸入框。
- **L2、L5**：#194、#192 在淺色主題下的對比問題（git graph 色盤、ref chip、編輯器與 Markdown 的 One Dark 語法色、警告橫幅等）已列給總指揮轉交。原則同上：顏色從 theme 名稱或 `tokens()` 取，不寫死。
- **L8**：狀態列最右端（指令面板快捷鍵右邊）保留給 L8 的 `yuzora-perf`，由 L8 把 layout 的 `status` 節點改成 row。L1 維持 `status` 的 id 與 26px 高度，不在快捷鍵右邊加東西；`yuzora-statusbar` 根元素的背景（`theme().chrome`）或上緣分隔線（1px，`theme().divider ?? theme().border`）若要改，要在 PR 註明讓 L8 對齊。
- **總指揮**：lanes.md 的同步檢查用 `git grep -n 'yuzora:'`，抓不到 lane 用的 `yuzora(L1):` 註記，建議改成 `git grep -n 'yuzora[:(]'`。

## 狀態列 Git 的呼叫次數（beta.3 效能）

`yuzora-shell/src/git.rs` 的 `status()` 原本循序跑 3 個 `git`（`symbolic-ref`、`config --get-regexp`、`for-each-ref`），每個在 Windows 都付 `remote::run` 約 52 ms 的輪詢下限，遠端則各是一次 `hc exec` 往返。現在：

- **遠端（`machine` 有值）**：三步合成一個 `sh -c` 在那台機器上依序跑（選項、順序、停止條件與 `--no-lazy-fetch` 都和逐步版相同：不是 repo 或 detached 就停、promisor 才加 `--no-lazy-fetch`），一次往返。script 的每個 `git` 都直接寫出 `--no-optional-locks -c core.fsmonitor=false`（不經過未加引號的變數，所以 zsh 原生模式之類不拆字的 shell 也不會走偏）。只有 `rc=0`（分支）、`1`（detached）、`128`（不是 repo）算答案；輸出被截斷、格式不完整、或其他 rc（沒有 git、shell 沒照寫法執行）都退回逐步版。那台機器沒有 `sh`（或沒回應）同樣退回；逾時照樣當逾時，不重試。
- **本機與退回路徑**：`symbolic-ref` 與 `config` 互不相依，改成並行（3 輪變 2 輪）。代價：detached 或資料夾不是 repo 時，本機現在會多跑 1 個 `config`（只讀，不執行程式；等待時間沒有增加）。
- 安全條件不變（L1 #197）：只讀 refs 與 config，不讀 index 或 work tree，不執行 filter、fsmonitor；測試守著（`a_machines_repo_filter_never_runs`、`a_machines_partial_clone_is_not_fetched_either`）。
