# L1 外殼＋工作區

lane L1 負責 Yuzora 的外殼：側欄的工作區模式、模式列、tab 列、pane 標頭與外框、狀態列、「＋ 新增 pane」選單（混合分割），以及 `yuzora-dark`／`yuzora-light` theme、tokens 與上游畫面的品牌字樣。設計依據是設計稿 p1–p6，特別是 p6 的工作區模式。架構規則見 [architecture.md](architecture.md)，lane 流程見 [lanes.md](lanes.md)。

## 範圍與檔案

| 路徑 | 內容 |
| --- | --- |
| `app/crates/yuzora-shell/` | `yuzora:shell`：工作區樹模型、Space 徽章色、git 摘要 |
| `app/src/yuzora/shell.rs` | app 端 glue：pane 外框的 `blocked` 邊框 |
| `app/defaults/mods/yuzora-sidebar-workspace/` | 側欄工作區模式；`status/*.svg` 是 5 種 agent 狀態的形狀，其他 L1 mod 也引用 |
| `app/defaults/mods/yuzora-modebar/` | 模式列：切換 5 個模式、徽章、設定齒輪 |
| `app/defaults/mods/yuzora-shell-tabs/` | tab 列與「＋ 新增 pane ▾」選單 |
| `app/defaults/mods/yuzora-shell-pane/` | pane 標頭 |
| `app/defaults/mods/yuzora-statusbar/` | 狀態列 |
| `app/defaults/mods/yuzora-brand/` | 側欄頂端的品牌列：標誌與搜尋（開指令面板） |
| `app/defaults/layouts/yuzora.json` | 預設 layout：改用上面的 tab 列與 pane 標頭，加上 `blocked` 邊框與 UI 字型 |
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
| `yuzora.badge.workspace` | 等待回覆（blocked）的 agent 數，全部機器；0 時為 null。由模式列計算（它一直在畫面上，工作區側欄不一定） |
| `yuzora.space.colors` | 每個 Space 的徽章色（`{space id: "#rrggbb"}`）。tab 列、pane 標頭、狀態列在出現新 Space 時寫入；移除的 Space 保留顏色（遠端機器暫時離線，回來時顏色不變） |
| `yuzora.shell.agents` | 各機器 `knownAgents()` 的上次結果（新增 pane 選單先顯示，再背景更新） |
| `yuzora.shell.split` | 新增 pane 選單放在 `right` 或 `down` |

L1 讀的 key：

- 模式列：`yuzora.badge.<workspace|files|git|database|ssh>`，number 或 null；超過 99 顯示 `99+`。workspace 用琥珀色，其餘用藍色。
- 狀態列：`yuzora.status.<lane>`（`{text, tone?}`，tone 為 `muted`、`working`、`blocked`、`done`、`accent`），依序讀 `shell`、`git`、`db`、`database`、`ssh`、`files`、`settings`、`release`。lane 名稱若有不同，請在整合時告知 L1 補進 `LANES`（`yuzora-statusbar/main.js`）。

## 設計決策

- **tab 列列出所有 Space 的 tab**：設計稿 p1 的 tab 列同時有不同 Space 的 tab（以徽章區分），所以 tab 列依 Space 順序列出每個 Space 的 tab，Space 之間留一點間距；點別的 Space 的 tab 會一併切換 Space（`tab.select` 本來就如此）。一個 tab 仍只屬於一個 Space，不做跨 Space 的組合 tab。已收起（settled）的 tab 不顯示，除非正在看它。
- **右側面板（Files／Changes）**：沿用上游 tab 列的切換鈕與「每個 Space 記住自己的面板」：切換 Space 時恢復該 Space 上次的選擇（沒選過就關閉）。`shared("workspace.panels")` 決定顯示哪幾顆。tab 列的分割鈕只在有 focus 的 model pane 時可用（layout 的終端機區塊有 focus 時停用）。
- **「＋」與「＋ 新增 pane ▾」**：「＋」開上游的 `new-menu`（新 tab：終端機、agent、新 Space、worktree）。「新增 pane」在目前的 tab 內混合分割：工具 pane（`yuzora:core` 的 `panes()`）用 `pane.split {direction, pane, mod, props}`、終端機用 `pane.split {direction, pane}`、agent 用 `terminal.open {argv, place}`；位置（右側／下方）在選單頂端切換。沒有 focus 的 pane（還沒有 Space，或 focus 在 layout 的終端機 block）時改開新 tab。
- **blocked 外框**：上游的 pane frame 只有 `border` 與 `focus`。L1 在 `PaneFrame` 加了 `blocked`（`{width, color}`），agent 需要回覆時優先使用（即使該 pane 有 focus，因為那裡需要人）。標頭底色 `#1d1a14` 由 `yuzora-shell-pane` 自己畫。工具 pane 沒有 agent，不會變色。
- **狀態形狀**：每種狀態同時用顏色與形狀表示（工作中綠點、需要回覆琥珀三角、完成未查看藍菱形、閒置空心圓、未知虛線圓）。GPUI 沒有 CSS 旋轉與三角形 border，所以 5 種形狀做成 SVG（`yuzora-sidebar-workspace/status/`），以 tokens 的 `status.<x>.mark` 上色。
- **設定齒輪**：L6 的設定框架合併前，先開上游的 `app.settings`。圖形是 6 齒（8 齒在 1x 時齒縫會被線寬填滿）。
- **品牌標誌**：側欄頂端畫 Yuzora 正式標誌（`public/brand/yuzora-mark.svg`），用 `image()` 保留四色；`svg()` 會把它當單色遮罩。gpui 不支援 CSS `var()`，所以 `yuzora-brand/mark.svg` 直接寫入原檔的 fallback 色碼。淺色 chrome（判斷方式同 `settings.rs` 的 `is_light`）改用 `mark-light.svg`：銀色面改成 slate `#94a3b8`，摺面依原本的比例加深成 `#668296`，否則銀色面在淺色底上幾乎看不見。
- **鍵盤與腳本入口**：`yuzora-shell-tabs.new-pane` 開新增 pane 選單（沒有滑鼠位置可對齊，所以置中顯示；點按鈕時則開在游標下）；`yuzora-sidebar-workspace.spaces`／`agents` 切換分頁，`add`／`tools` 開對應選單。可綁進 `keys.json`（`"alt-n": "yuzora-shell-tabs:new-pane"`），也可用 `herdr-app do yuzora-shell-tabs.new-pane`。
- **Space 排序**：在側欄把 Space 拖到同一台機器的另一個 Space 上即可移動（`space.move`），右鍵開上游的 Space 選單（改名、worktree、關閉）。放開在其他地方或拖出側欄就取消。
- **機器的「新增 Space」**：只在該機器的 layout 已讀到（LED 綠燈）時提供；讀取中 Herdr 會拒絕結構性指令。狀態列的 git 摘要在 focus 的終端機所屬機器上執行（終端機位址 `<machine>/<id>`），不是目前 Space 的機器。
- **狀態列的 git 不跑 `git status`**：終端機一 `cd` 進資料夾，狀態列就會自動查詢，那個 repo 可能是剛下載、不可信的。`git status` 會看工作樹，並執行 repo 自己 config 指定的程式（`filter.<x>.clean`／`process`、`core.fsmonitor` 等），所以只讀 ref 與 commit：`symbolic-ref` 取 branch，`for-each-ref` 的 `%(upstream:short)`、`%(upstream:track)` 取 upstream 與 ↑↓。partial clone（promisor remote）缺 commit 時，走訪 commit 會用 repo 的 ssh 指令 lazy fetch，所以這種 repo 加 `--no-lazy-fetch`（git 2.44 起）；更舊的 git 不顯示 ↑↓。變更數與衝突數要等 L2 的 trust 判斷搬進 `yuzora-core`（#194）後，只在 folder 已信任時才用 `git status` 查；在那之前回傳 null，狀態列本來就只畫 branch 與 ↑↓。測試用會留下標記檔的 clean filter 與 ssh 指令當 oracle，並確認舊指令確實會觸發它們。
- **scope**：記住的是使用者選的 chip。選的機器暫時不在（剛啟動還沒列出）或離線（LED 熄滅）時先顯示全部，機器回來後自動回到該機器；連線中（LED 琥珀）時維持原 scope。
- **新增 pane 選單的高度**：依工具 pane 與 agent 數量（上次 `knownAgents()` 的快取）決定，最高 520px；超過時位置切換列固定、下方清單捲動。還沒有 Space 時工具 pane 停用並註明「需要先有 Space」（`tab.new` 可以替終端機或 agent 順便建立 Space，mod 不行）。
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

所以 beta.2 先**退回系統字型**：`yuzora.json` 的 `ui_font` 設為 `.SystemUIFont`（GPUI 的系統 UI 字型：macOS 為 SF Pro，中文由系統字型 fallback 到蘋方；Windows 為 Segoe UI；Linux 為系統預設 sans）。終端機字型不變（JetBrains Mono）。要改用 Inter 時：把 `InterVariable.ttf` 與 OFL 授權放進 `defaults/mods/yuzora-brand/fonts/`，`ui_font` 改為 `Inter`，並在 `THIRD-PARTY-NOTICES` 補上授權（列在待辦）。

## 上游改動

全部以 `// yuzora(L1):` 註記：

| 檔案 | 改動 |
| --- | --- |
| `app/src/layout.rs` | `PaneFrame` 新增 `blocked: Option<Line>` 欄位 |
| `app/src/window.rs` | `frame_pane` 多一行：呼叫 `crate::yuzora::shell::frame_line` 決定邊框 |
| `app/src/yuzora/mod.rs` | 加一行 `pub mod shell;`（lanes.md 允許的改動） |
| `app/src/mods/render_tests.rs` | welcome 第一頁的標題改為 Yuzora |
| `app/defaults/frame/welcome/main.js` | 標題、說明中的 app 名稱改為 Yuzora；圖示改 `yuzora.png`；請 agent 讀的 AGENTS.md 改用 `appHome()` 組出實際路徑（`YUZORA_APP_HOME`／`XDG_CONFIG_HOME` 改過時也正確） |
| `app/defaults/frame/whatsnew/main.js` | 標題與圖示 |
| `app/defaults/frame/feedback/main.js` | 送出後的感謝文字 |
| `app/defaults/frame/welcome/yuzora.png` | 新檔：Yuzora 圖示（160px，由 `assets/icons/yuzora.png` 縮圖） |

保留不改：welcome 的「從 Herdr TUI 搬設定」段落（指的是 Herdr TUI）、`Herdr 98`（layout 名稱）、`herdr-core`。settings frame 歸 L6。

## 驗證

```bash
cd app
cargo test --locked -p yuzora-shell        # 樹模型、徽章色、git 解析與不執行 repo 程式、HostModule 宣告
cargo test --locked --bin herdr-app yuzora # blocked 邊框、layout 引用的 mod 都有出貨
scripts/yuzora/check-mods
```

實機以 lanes.md 的隔離方式執行（暫存 `YUZORA_APP_HOME`、`HC_SESSION`），用 OSC 7501 假 agent 造出各種狀態：

```bash
printf '\033]7501;state=blocked:app=codex\033\\'; exec sleep 100000
```

工具 pane 用一個只放在測試 app home、不出貨的示範 mod（`mod.json` 宣告 `"yuzora": {"pane": …}`）。遠端機器用同一台電腦上另一個隔離的 hc session 模擬：

```bash
hc machine add --via 'sh -c' --name lab-01 --session <另一個測試 session>
```

截圖在 `docs/beta2/evidence/`：

| 檔案 | 內容 |
| --- | --- |
| `l1-workspace.png` | 本機＋lab-01 的樹、scope chips、需要注意、模式列徽章、tab 列、pane 標頭、狀態列（含其他 lane 的狀態） |
| `l1-scope-after-restart.png` | 選 lab-01 後重啟，scope 仍在 |
| `l1-blocked-and-badge.png` | 側欄在 Git 模式時新增 blocked agent：工作區徽章變 3、pane 琥珀外框與標頭 |
| `l1-agents-tab.png` | Agents 分頁（最急的在前） |
| `l1-theme-dark.png`、`l1-theme-light.png` | Yuzora 主題的全畫面：深色（yuzora-dark）與淺色（yuzora-light，`appearance: light`） |
| `l1-new-pane-menu.png` | 「新增 pane」選單（由 `yuzora-shell-tabs.new-pane` 開啟） |
| `l1-mixed-split-after-restart.png` | 終端機與工具 pane 混合分割，重啟後保留 |
| `l1-welcome.png` | welcome 的 Yuzora 品牌字樣 |
| `l1-modebar-gear.png` | 模式列（2x 裁切）：最右的設定按鈕是齒輪 |
| `l1-brand-dark.png`、`l1-brand-light.png` | 側欄頂端（2x 裁切）的 Yuzora 標誌，深色（yuzora-dark）與淺色（paper，`appearance: light`）主題 |
| `l1-light-*.png` | 淺色主題（`appearance: light`）的各畫面：`workspace`、`sidebar-files`、`sidebar-git`（L2、L5 尚未合併，仍是 L0 的佔位）、`sidebar-database`、`sidebar-ssh`、`database`（資料表與 SQL 查詢）、`sftp`、`settings`、`settings-mods`、`new-pane`、`palette`。SSH 主機來自假的 HOME（`build-server`、`staging-db`） |

## 待辦

- 隨 app 打包 Inter（與 Noto Sans TC 的取捨），見「字型」。
- 設計稿品牌列的第二顆按鈕用途未定（搜尋已加入，開指令面板）。
- pane 標頭的 `repo` 目前是 pane 目前資料夾的名稱；在 repo 子資料夾時不是 repo 名稱。
- 設計稿 tab 列右側兩顆圖示按鈕的用途未定，暫時做成「在右側／下方開終端機」。

## 後續整合

- **L6**：設定框架合併後，模式列齒輪改開 Yuzora 的設定（`yuzora-modebar/main.js` 的 `app.settings`）。外觀設定頁可編輯 `yuzora.space.colors`（Space 徽章色）。
- **L2**：#194 把 trust 判斷搬進 `yuzora-core` 後，`gitStatus` 改接那個 API：folder 已信任時再跑一次 `git status` 補上 `changed`、`conflicted`，未信任時維持現在的行為（只有 branch 與 ↑↓，不報錯）。
- **L2–L5**：側欄模式的標頭可用 `spaceBadges()` 畫 Space 徽章（用法見上）；徽章數與狀態列文字照「共享狀態」的 key 寫入。工具 pane 在 `mod.json` 宣告 `"yuzora": {"pane": …}` 就會出現在「＋ 新增 pane」選單。
- **L2、L5**：#194、#192 在淺色主題下的對比問題（git graph 色盤、ref chip、編輯器與 Markdown 的 One Dark 語法色、警告橫幅等）已列給總指揮轉交。原則同上：顏色從 theme 名稱或 `tokens()` 取，不寫死。
- **L8**：狀態列最右端（指令面板快捷鍵右邊）保留給 L8 的 `yuzora-perf`，由 L8 把 layout 的 `status` 節點改成 row。L1 維持 `status` 的 id 與 26px 高度，不在快捷鍵右邊加東西；`yuzora-statusbar` 根元素的背景（`theme().chrome`）或上緣分隔線（1px，`theme().divider ?? theme().border`）若要改，要在 PR 註明讓 L8 對齊。
- **總指揮**：lanes.md 的同步檢查用 `git grep -n 'yuzora:'`，抓不到 lane 用的 `yuzora(L1):` 註記，建議改成 `git grep -n 'yuzora[:(]'`。
