# Yuzora v0.1.0-beta.2 架構（ADR）

- 狀態：Accepted（L0 基底，2026-11-02）
- 範圍：`app/`（herdr-app fork）與它在本 repo 的建置、CI、文件。`src-tauri/` 與 React 前端保留為移植來源，不在此 ADR 內。
- 來源：總指揮整理的 `.yuzora-lanes/beta2-research/decisions.md`。之後以本檔為準。
- 各 lane 的範圍、目錄所有權與工作流程見 [lanes.md](lanes.md)。

## 背景

beta.2 改以 [herdrdev/herdr-app](https://github.com/herdrdev/herdr-app)（Rust＋GPUI＋libghostty＋QuickJS mods，Apache-2.0）為基底，把 Yuzora 的 SQL、SSH／SFTP、Git、Files、設定內建進去，並依新設計稿重做 UI。herdr-app 核心刻意很小：終端機與 machines 由 herdr-core（`hc`）執行，app 只負責畫面；UI 幾乎全是 JS mods（`app/defaults/mods/*`）。

上游節奏很快，熱點是 `defaults/AGENTS.md`、`src/window.rs`、`src/commands.rs`、`defaults/herdr.d.ts`。因此所有決策的共同原則是：**能新增檔案就不改上游檔；非改不可時只做最少改動，並在該行加 `// yuzora:` 註記**。

## 決策

### D1. 以 git subtree 引入 herdr-app 到 `app/`

- 指令：`git subtree add --prefix=app https://github.com/herdrdev/herdr-app.git master --squash`。
- 上游 commit：`9c678b0fc84625b57ba7be4f56319802ee366765`（herdr-app v0.1.12 之後的 master）。squash commit `32fd5c6f`、merge commit `b486feb0` 帶有 `git-subtree-dir: app`／`git-subtree-split` metadata。
- ghostty 是上游的 submodule。subtree 只帶進 gitlink（`app/vendor/ghostty` → `291fd050b9e9c293c8adb6a140742649b7e42d3a`），所以 root `.gitmodules` 另外登記同一個 URL：

  ```bash
  git submodule update --init app/vendor/ghostty
  ```

- `app/.gitmodules` 是上游原檔，保留不動（subtree pull 會更新它）；本 repo 實際生效的是 root `.gitmodules`。上游換 URL 時要同步改 root 的。
- **L0 的 PR 與每次上游同步的 PR 都必須用 merge commit 合併**，squash 或 rebase 會弄丟 subtree metadata，之後的 `git subtree pull` 就找不到上次的 split。

### D2. `app/` 是獨立的 Cargo workspace

- root `Cargo.toml` 的 `exclude` 加入 `app`。`app/` 有自己的 `Cargo.lock` 與 target：`app/.cargo/config.toml` 設 `target-dir = "target"`，root `.gitignore` 忽略 `app/target/`。
- 在 lane worktree 一律用 `CARGO_TARGET_DIR` 指向共用 slot（見 lanes.md）。
- `[workspace.package] version = "0.1.0-beta.2"`：app 與每個 `yuzora-*` crate 共用同一個版本。
- workspace `members = ["crates/*"]`、`default-members = [".", "crates/*"]`，所以 `cargo build`／`cargo test` 會涵蓋 ghostty-sys 與全部 `yuzora-*` crate，新 crate 放進 `app/crates/` 就自動加入。
- `src-tauri/vendor/{russh-0.62.7-yuzora,russh-sftp-3.0.0-yuzora,tiberius-0.12.3-yuzora}` 複製到 `app/vendor/`，`app/Cargo.toml` 的 `[patch.crates-io]` 掛上，保留原本的 patch 註解。在 L3／L4 真正依賴這些 crate 之前，cargo 會印 `patch … was not used`，屬預期警告。
- 建置需要 Zig 0.16.0（ghostty-sys）與 Rust 1.95.0 以上（上游 CI 用 1.95.0）。

### D3. 品牌與外連

Yuzora 自己的常數與開關都在 `app/src/yuzora/mod.rs`；上游檔只在帶 `// yuzora:` 註記的地方呼叫它。

| 項目 | 決定 |
| --- | --- |
| 顯示名稱 | `Yuzora`（視窗標題、setup 畫面） |
| bundle id／app id | `dev.yuuzu.yuzora`（沿用 Tauri 版的 identifier）。Linux／Wayland app id 與 Windows AppUserModelID 用同一個。`YUZORA_APP_ID` 可給並排的開發版另一個 id |
| macOS bundle | `Yuzora.app`，執行檔 `Contents/MacOS/Yuzora`，圖示 `app/assets/icons/Yuzora.icns`。本機用 `app/scripts/yuzora/bundle-macos` 包裝；正式打包、簽章屬 L7 |
| app home | `~/.config/yuzora`，`$XDG_CONFIG_HOME/yuzora` 優先，`YUZORA_APP_HOME` 最優先。**完全不讀 `HERDR_APP_HOME`**，不會與官方 Herdr 的 `~/.config/herdr-app` 共用 |
| 遙測 | 完全關閉：`telemetry::endpoint()` 回傳 `None`，不論設定或環境變數怎麼寫，`collecting()` 都是 false |
| feedback | 不送 herdr.dev：`app.feedback` 改成開 `https://github.com/NakiriYuuzu/Yuzora/issues/new?body=…`（附版本與 OS），`feedback::endpoint()` 回傳 `None` |
| app 自我更新 | 停用（`update::checks_app()` 回傳 `None`）。L7 之後接 GitHub Releases |
| herdr-core 安裝／更新 | 保留上游官方通道（setup 畫面的 core 安裝指令、core 的更新檢查） |
| update notes 媒體 | 停用：上游 CHANGELOG 內嵌 `media.herdr.dev` 圖片，啟動時會自動下載，現在不抓 |
| setup 畫面的 app 安裝連結 | 改成 Yuzora 的 GitHub Releases |

驗證：`app/src/yuzora/mod.rs` 的 `nothing_goes_to_herdr_dev` 測試；L0 實機以隔離環境啟動 75 秒，app 與所有子行程沒有任何 inet socket（見 PR 證據）。

### D4. herdr-core 的 layout 文件名：`yuzora`

herdr-core 每台機器依名稱存一份 layout 文件（`doc.get`／`doc.set {name}`），內容是 spaces、tabs、panes 的配置。上游 app 用 `herdr-app`。

- **決定**：Yuzora 用自己的文件名 `yuzora`（`CORE_DOCUMENT`，`src/mirror_sync.rs` 兩處改動）。
- **理由**：使用者可能同時裝官方 Herdr 與 Yuzora，兩者連到同一個 core（預設 session）。若共用 `herdr-app`，兩個 app 會互相覆寫對方的 spaces、tabs 與 layout，而且上游 layout 格式隨版本演進，Yuzora 落後上游時可能寫出對方讀不懂的內容。
- **代價**：
  - 第一次啟動 Yuzora 時沒有既有的 space 配置，會從空白開始；不會搬移官方 Herdr 的配置。
  - core 的終端機本來就是共享的。兩個 app 都會看到對方開的終端機，並各自收進自己的 space；兩邊寫的 terminal label（`herdr.space`、`herdr.seen`、`where`）可能互相改動，label 的改動不會關閉終端機。但收進來的終端機在任一個 app 關閉（Space、tab、pane 或 `terminal.kill`）都會在 core 結束，另一個 app 也跟著失去它（2026-11-03 L9 實測更正，見 [herdr-compat.md](herdr-compat.md) A5 與決定 1）。
- 驗證：隔離 core 上 `doc.get yuzora` 有 Yuzora 的 space，`doc.get herdr-app` 為 null。

### D5. binary／package 名稱：beta.2 期間維持 `herdr-app`

- Cargo package 與執行檔名維持 `herdr-app`。使用者看到的名稱是 `Yuzora.app`／`Contents/MacOS/Yuzora`。
- **理由**：上游測試用 `CARGO_BIN_EXE_herdr-app`；`scripts/sandbox`、`docs.ts`、`bench.py`、tracing filter（`herdr_app`）、CLI 說明都寫死這個名稱。改名會動到大量上游檔，每次 subtree pull 都會衝突。
- 上游的 macOS CLI 連結（`~/.local/bin/herdr-app`）只在執行檔是 `Herdr.app/Contents/MacOS/Herdr` 時建立，`Yuzora.app` 不會觸發，不會蓋掉官方 Herdr 的 CLI。
- **後續**：L7 打包時決定最終名稱。建議只改打包後的執行檔與 CLI 連結名稱（例如 `yuzora`），Cargo package 繼續沿用上游名稱。

### D6. 三層架構

1. **herdr 殼**：上游程式碼（`app/src`、`app/defaults` 的上游 mods／frame、`app/vendor`）。盡量不改。
2. **Yuzora Rust 原生層**：`app/crates/yuzora-*`。每個 crate 提供一個 gpui-shell `HostModule`，JS 端以 `import { … } from "yuzora:<name>"` 使用。async 函式（DB 連線、SSH session、git、大量檔案 I/O）和重量級原生元件（資料表格、分支圖、diff／merge view、程式碼編輯器，`HostModule::component`）都放在這層。
3. **Yuzora JS mods**：`app/defaults/mods/yuzora-*`。負責組合側欄模式與工具 pane。使用者可以 `mod.clone` 後修改，這就是 Yuzora 的「原生 MOD」。

`yuzora-core` 是第 2 層的共用底座，lane crate 都只依賴它：

| 模組 | 內容 |
| --- | --- |
| `paths` | app home、`mods_dir()`、`yuzora_dir()`（`<home>/yuzora`，Yuzora 自己的資料） |
| `remote` | `ExecRequest`：本機或 `hc exec --json --on <machine>` 執行程式，可自訂 timeout 與輸出上限（mod 的 `exec` 固定 30 秒、8 MiB） |
| `secrets` | 系統憑證庫（keyring 4）；service `dev.yuuzu.yuzora`，account `v2:<len>:<namespace>:<len>:<id>`，與 Tauri 版的舊條目分開 |
| `settings` | `<home>/yuzora/settings.json`，一個 lane 一個 section；寫入走暫存檔＋rename。Herdr 自己的 `settings.json`（外觀、按鍵、音效）不動 |
| `i18n` | 見 D9 |
| `discovery` | 見 D8 |
| `tokens` | 見 D10 |
| `host` | 寫 `host_module()` 的工具：JSON ↔ `HostValue`、`yuzora:<lane>` 命名 |
| `js` | `yuzora:core` 模組本身 |

### D7. 唯一整合點：`app/src/yuzora/mod.rs`

- `host_modules(mod_name)` 列出 `yuzora:core` 與 6 個 lane 模組；`with_modules(policy, mod_name)` 把它們掛進每個 mod copy 的 policy。
- 上游 `src/mods.rs` 只有 4 處呼叫：`app_dir()`、`write_workspace()`、`install_policy()`（`herdr-app check` 用）、`prepare_load()`（每個 mod copy）。任務原本要求「只加一處」，但 check 的 policy 與每個 copy 的 policy 是分開建立的，至少要兩處；另外兩處是 app home 與 `yuzora.d.ts`。
- `app/Cargo.toml` 已預先依賴全部 `yuzora-*` crate，lane 之後不需要改它，也不需要改 `mod.rs`：在自己 crate 的 `host_module()` 加函式或元件即可。
- 啟動時在 app home 寫出 `yuzora.d.ts`（與 `herdr.d.ts` 並列），內容是每個 `yuzora:*` 模組的 TypeScript 宣告。
- `HostModule` 的 `declarations` 必須與註冊的函式完全一致，否則啟動時 `validate` 失敗。新增函式時，`export function …` 那一行要一起加。

### D8. 探索慣例：tool pane 與設定頁

mod 在自己的 `mod.json` 宣告，不需要任何人去改共用清單：

```json
{
  "place": "none",
  "yuzora": {
    "pane": { "title": "pane.title", "icon": "mods/yuzora-git/icon.svg", "order": 20, "props": {} },
    "settings": { "title": "settings.title", "icon": "mods/yuzora-git/icon.svg", "order": 20, "section": "git" }
  }
}
```

- `pane`：這個 mod 可以當 tool pane 開啟（`pane.split {mod, props}`）。L1 的「＋ 新增 pane」選單用 `panes()` 列出。
- `settings`：這個 mod 畫一個設定頁，`section` 是它在 Yuzora `settings.json` 的 key（預設為 mod 名稱）。L6 的設定框架用 `settingsPages()` 列出。
- `title` 是 mod 自己 i18n 字串的 key（沒有對應字串時原樣顯示）；`order` 預設 100，相同時依名稱排序。
- 掃描範圍是 app home 的 `mods/`（已安裝的 mods，含使用者 clone 的），結果快取 2 秒。

### D9. i18n

- 每個 Yuzora mod 把字串放在自己旁邊：`app/defaults/mods/<mod>/i18n/en.json`。巢狀物件攤平成點號 key（`commit.button`），`{name}` 由變數代入。
- JS：`import { t } from "yuzora:core"; t("commit.button", { count: 3 })` 讀「這個 mod」的字串；`translate(mod, key, vars)` 讀其他 mod 的。
- Rust：crate 自己的字串用 `yuzora_core::i18n::Catalog::from_json(include_str!(…en.json))`；mod 的字串用 `i18n::translate(mod, key, vars)`。
- 找不到時顯示 key 本身，缺字串會直接顯示 key，不會變空白。
- **介面只有英文**（beta.3 起，使用者 2026-11-04 決定）：不出貨 `zh-TW.json`，沒有語言設定。`locale()` 永遠回傳 `"en"`；`setLocale()` 保留以免舊 mod 呼叫出錯，接受任何字串、什麼都不做；設定檔裡舊的 `general.locale`（例如 `"zh-TW"`）直接忽略，照常啟動。文件類（README／CHANGELOG 的 zh-TW 版、官網、`docs/`）不屬於介面，保留。
- UI 文字只填 `en.json`；`app/scripts/yuzora-i18n.test.ts` 守著：不得出現 `zh-TW.json`、設定頁不得有語言選項、每個 `t("…")` 的 key 都在 `en.json`。上游 mods 沒有 i18n，維持英文。

### D10. 設計 tokens 與 theme

- `app/defaults/themes/yuzora-dark.json`：設計稿的 zinc 深色，對應 herdr theme 色名（`canvas`、`chrome`、`popover`、`raised`、`hover`、`selected`、`border`、`input`、`divider`、`muted`、`dim`、`accent`、`working`、`blocked`、`done`、`idle`、`unknown`…）。mod 端用 `theme()` 讀取。
- `app/defaults/yuzora/tokens.json`：theme 沒有名稱的設計值，包括字型與尺寸（側欄 248、標題列 36、pane 標頭 28、狀態列 26、間距 6、圓角 6）、diff 色、Agent 品牌色、Space 徽章色、狀態形狀（工作中圓點、需要回覆三角形、完成菱形、閒置空心圓、未知虛線圓）、檔案狀態色、資料庫色。JS 端用 `tokens()` 讀取，Rust 端用 `yuzora_core::tokens::tokens()`。
- 設計稿的 Inter＋Noto Sans TC 字型尚未設定，light theme 尚未提供。

### D11. 預設 layout 與佔位 mods

- 新安裝的預設 layout 是 `app/defaults/layouts/yuzora.json`（`DEFAULT_LAYOUT`）。已經有 layout 的 app home 不受影響。
  - 左側 248px 側欄由上而下是 `yuzora-brand`、`stack` `yuzora-sidebar`（5 個模式 block：`yuzora-sidebar-workspace`／`files`／`git`／`database`／`ssh`）、`yuzora-modebar`（44px，以 `layout.select {stack: "yuzora-sidebar", child}` 切換，最右是設定齒輪 `app.settings`）。
  - 右側由上而下是 tab 列（上游 `tabs` mod）、主區 `{tab: "active"}`（pane 間距 6、圓角 6、標頭 28）、`yuzora-statusbar`（26px）。
  - 另有一個隱藏的 `panel` 欄（files／changes），保留給上游 tabs、welcome 會用到的指令。
- 佔位 mods 只顯示 i18n 標題與「由 Lx lane 實作」。全部都能通過 `herdr-app check`（`app/scripts/yuzora/check-mods`）。

### D12. 遠端策略

- 終端機與 machines（SSH 遠端）交給 herdr-core 與官方 saved machines。
- 遠端 Space 的 Git 與 Files 走 `hc exec --on <machine>`，用 `yuzora_core::remote` 自訂 timeout 與輸出上限。
- DB 連遠端走 SSH tunnel，tunnel API 由 L4（`yuzora-ssh`）提供，L3 呼叫。
- Tauri 版的遠端 agent `yuzora-host`（wire protocol）不移植；之後若 `hc exec` 不敷使用再評估。

## 目錄與所有權

| 路徑 | 擁有者 | 說明 |
| --- | --- | --- |
| `app/`（除下列之外） | 上游 | herdr-app subtree，只能做帶 `// yuzora:` 註記的最少改動 |
| `app/src/yuzora/` | L0／總指揮 | 唯一整合點與品牌常數。lane 需要 app 端 glue 時，新增 `app/src/yuzora/<lane>.rs`，並在 `mod.rs` 加一行 `mod` |
| `app/crates/yuzora-core/` | L0／總指揮 | 共用層；變更規則見 lanes.md |
| `app/crates/yuzora-<lane>/` | 各 lane | 見 lanes.md |
| `app/defaults/mods/yuzora-*` | 各 lane | 以 mod 名稱前綴劃分，見 lanes.md |
| `app/defaults/layouts/yuzora.json`、`themes/yuzora-*.json`、`defaults/yuzora/` | L1 | 其他 lane 透過探索慣例加入，不改 layout |
| `app/vendor/{russh,russh-sftp,tiberius}-*-yuzora` | L4、L4、L3 | Yuzora 的 vendored patches |
| `app/scripts/yuzora/` | L0（打包相關歸 L7） | `bundle-macos`、`check-mods` |
| `.github/workflows/app-ci.yml` | L0／總指揮 | app 的 CI |
| `docs/beta2/` | 總指揮；`docs/beta2/<lane>.md` 歸各 lane | |
| `src-tauri/`、`src/`（React） | 凍結 | 只當移植來源，beta.2 不刪 |

## 上游同步流程

由總指揮或指定 session 在自己的分支執行，以 PR（merge commit）併入 `beta2/integration`：

```bash
git subtree pull --prefix=app https://github.com/herdrdev/herdr-app.git master --squash
git submodule update --init app/vendor/ghostty
```

同步後檢查：

1. `git grep -n 'yuzora[:(]' -- app/src app/build.rs app/tests app/scripts`：逐一確認 Yuzora 的改動仍在，沒有被上游改寫蓋掉。
2. `app/CHANGELOG.md`：上游新版本會加在最上面，但 `update_notes` 測試要求第一筆版本等於 `CARGO_PKG_VERSION`。保持 Yuzora 的 `0.1.0-beta.2` 那筆在最上面。
3. `app/.gitmodules` 與 root `.gitmodules` 的 ghostty URL 一致。
4. `app/Cargo.lock` 有變動時，重產 `app/THIRD-PARTY-NOTICES`（見 lanes.md）。
5. 跑完整的 app 測試與 `scripts/yuzora/check-mods`。上游新增的 telemetry、feedback、更新等外連端點要補上 D3 的開關。

subtree pull commit 之後執行 `app/scripts/yuzora/sync-check`，會依序做上面五項，並擋下只用 `HERDR_APP_HOME` 隔離的測試。加 `--quick` 不跑 cargo test 與 check-mods。說明見 [herdr-compat.md](herdr-compat.md) C2。

## 合併規則

- integration 分支是 `beta2/integration`（從 `main` `d7cbfbe9` 開出）。每條 lane 從它最新版開分支，PR 的 base 也是它。
- lane 完成時：把 `origin/beta2/integration` merge 進自己的分支並解衝突 → 跑完所有檢查 → push → 開 PR → 回報總指揮。總指揮審查並合併。
- 總指揮只合併；lane 只在自己的分支 commit、push、開 PR。任何 session 都不得 push 或合併到 `main`，也不得建立 tag 或 release（合併到 `main` 就等於發版，須使用者另行授權）。
- **L0 PR 與上游同步 PR 必須用 merge commit 合併**，其餘 PR 由總指揮決定。
- 共用檔只能改自己的標記區塊（見 lanes.md）。
- commit trailer：`Co-Authored-By: Claude Opus 5.5 <yuuzu@yuuzu.net>`；PR 描述結尾 `🤖 Generated with [Claude Code](https://claude.com/claude-code)`。上游 `app/AGENTS.md` 的 commit 規則（例如不加 AI co-author）只適用於向上游貢獻。

## 參考資料

- `docs/beta2/research/intellij-git-research.md`：IntelliJ Git 功能清單與效能架構（L2 必讀）。
- `docs/beta2/research/design/p1-outline.txt`～`p6-outline.txt`：設計稿 6 張畫板的結構大綱（p6＝共用側欄 5 模式）。
- 設計稿原檔（11 MB）不進 repo，位置在 `/Users/yuuzu/HanaokaYuuzu/App/Tauri/.yuzora-lanes/beta2-research/design/Yuzora ADE Redesign × GPUI Kit.html`；同目錄的 `p<N>/markup.html`、`logic.js` 是解包結果，`unpack*.py`、`outline.py` 是解包腳本。
- herdr-app 的 mod API：`app/defaults/herdr.d.ts`、`app/defaults/AGENTS.md`；HostModule：`app/vendor/gpui-kit/crates/shell/src/host_modules.rs`。
