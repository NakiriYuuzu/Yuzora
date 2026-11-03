# AGENTS.md

## beta.2 架構（`app/`）

v0.1.0-beta.2 以 [herdr-app](https://github.com/herdrdev/herdr-app)（Rust＋GPUI＋libghostty＋QuickJS mods）為基底，用 `git subtree` 放在 `app/`。`src-tauri/` 與 `src/`（Tauri 2＋React）是 v0.1.0-beta.1 以前的版本，保留為移植來源直到 beta.2 發布：不刪，也不當設計依據。

- 架構決策（ADR）：`docs/beta2/architecture.md`。lane 範圍、目錄所有權、共用檔規則與流程：`docs/beta2/lanes.md`。各功能的設計與限制：`docs/beta2/<lane>.md`（shell、git、db、ssh、settings、release、perf、herdr-compat；files 隨 #192 合併）。研究資料：`docs/beta2/research/`。
- `app/` 是獨立的 Cargo workspace（root `Cargo.toml` 將它 exclude），有自己的 `Cargo.lock` 與 target（`app/target`）。版本 `0.1.0-beta.2`，app 與所有 `yuzora-*` crate 共用。Cargo package 與執行檔維持上游名稱 `herdr-app`，打包後才叫 `Yuzora.app`／`yuzora`。
- **三層**：herdr 殼（上游 `app/src`、上游 mods 與 frame）→ Yuzora Rust 原生層（`app/crates/yuzora-*`，各提供 gpui-shell `HostModule`：async 函式與原生元件，JS 以 `import … from "yuzora:<name>"` 使用）→ Yuzora JS mods（`app/defaults/mods/yuzora-*`，組合側欄模式與 tool pane，使用者可 `mod.clone` 後修改）。
- herdr-core（`hc`）執行終端機與 machines，app 只畫畫面；遠端 Space 的 Git、Files 等走 `hc exec --on <machine>`（`yuzora_core::remote`）。
- 對上游檔只做最少改動，並以 `// yuzora:`（lane 用 `// yuzora(L<n>):`）註記。能新增檔案就不改上游檔。
- 上游同步：`git subtree pull --prefix=app https://github.com/herdrdev/herdr-app.git master --squash`，commit 後跑 `app/scripts/yuzora/sync-check`。L0 PR 與上游同步 PR 必須用 merge commit 合併，否則會弄丟 subtree metadata。
- `app/AGENTS.md` 是上游給 herdr-app 開發者的指南（建置、測試、隔離執行），在 `app/` 內工作時一併遵守；它的 commit 規則（例如不加 AI co-author）只適用於向上游貢獻。`app/defaults/AGENTS.md` 是寫進使用者 app home、給使用者 agent 讀的操作指南。

### 目錄地圖

| 路徑 | 內容 |
| --- | --- |
| `app/src/` | herdr-app 殼（上游）。`app/src/yuzora/mod.rs` 是 Yuzora 唯一整合點：品牌常數、外連開關、掛上所有 `yuzora:*` HostModule；同目錄另有 lane glue（`shell.rs`、`herdr_compat.rs`、`release/` 更新通道與 manifest 驗證） |
| `app/crates/yuzora-core/` | 共用層：app home、`hc exec --on` 遠端執行、keyring、i18n、Yuzora 設定、mod 探索、design tokens、beta.1 資料遷移框架（`migrate`） |
| `app/crates/yuzora-{shell,git,db,ssh,files,settings,perf}/` | 各 lane 的原生層；各自提供 `host_module()`（JS `import … from "yuzora:<name>"`） |
| `app/defaults/mods/yuzora-*` | Yuzora 的 JS mods（側欄模式、模式列、狀態列、tool panes、設定頁）；i18n 在各 mod 的 `i18n/{en,zh-TW}.json` |
| `app/defaults/layouts/yuzora.json` | 新安裝的預設 layout（248px 側欄＋5 模式 stack＋右側 tab 區與狀態列） |
| `app/defaults/themes/yuzora-{dark,light}.json`、`app/defaults/yuzora/tokens.json` | 深淺兩套 theme 色名與 theme 沒有的設計 tokens（`light` 區塊是淺色覆寫值） |
| `app/vendor/` | 上游 vendored crates、ghostty submodule、Yuzora 的 russh／russh-sftp／tiberius patches |
| `app/scripts/yuzora/` | `check-mods`（在複本上跑 `herdr-app check`）、`bundle-macos`（本機包成 `Yuzora.app`）、`package`／`mac-dmg`（發布用安裝檔）、`manifest.ts`（更新 manifest）、`sync-check`（上游同步檢查） |
| `app/distribution/yuzora/` | 各平台打包範本：macOS `Info.plist`、Windows NSIS（`yuzora.nsi`）、Linux `install` 與桌面檔 |
| `.github/workflows/app-ci.yml`、`app-package.yml` | app 的 CI（base 為 `beta2/integration` 的 PR，Linux＋macOS）與四平台打包檢查（不建 tag、不發布） |

### 建置與測試

```bash
export PATH="$HOME/.local/zig-0.16.0:$PATH"   # ghostty 需要 Zig 0.16.0
git submodule update --init app/vendor/ghostty
cd app
cargo test --locked                # app、ghostty-sys 與全部 yuzora-* crate
cargo test --locked -p yuzora-git  # 單一 crate
scripts/yuzora/check-mods          # 每個 yuzora-* mod 的 herdr-app check
bun test scripts/yuzora/           # 打包與 manifest 腳本
scripts/yuzora/package             # release build 並打包到 dist/（YUZORA_PACKAGE_BIN=<binary> 只檢查打包）
```

- Rust 1.95 以上（上游 CI 用 1.95.0）。在 worktree／lane 建置前，先依下方「Rust 建置目錄重用」設定 `CARGO_TARGET_DIR`。
- 不要直接對 `app/defaults/mods/` 執行 `herdr-app check`：它會寫入 `gpui-kit.d.ts`、`jsconfig.json`，`build.rs` 會把它們打包進 app。
- `app/Cargo.lock` 有變動就要重產 `app/THIRD-PARTY-NOTICES`（cargo-about 0.9.3，`bun scripts/notices.ts`），否則 `tests/notices.rs` 失敗。
- `app/CHANGELOG.md` 第一筆必須是目前版本（`update_notes` 測試），App 會把它當成這一版的「新功能」說明。
- 實機執行一律用暫存 `YUZORA_APP_HOME` 與隔離的 `HC_SESSION`／`XDG_STATE_HOME`，並先清掉所有 `HERDR_*`、`HC_*` 環境變數，不得碰使用者真實的 Herdr、herdr-core 或 Yuzora 資料。Yuzora 不讀 `HERDR_APP_HOME`。完整指令見 `docs/beta2/lanes.md`。
- 要用 computer-use 或瀏覽器自動化做 GUI 驗收前，先取得使用者明確同意；優先用 `herdr-app status`／`shot`／`check` 留證。

### Lane 段落

各 lane 只改自己的標記區塊（規則見 `docs/beta2/lanes.md`）。

<!-- yuzora:lane:L1:start -->
#### L1 外殼工作區

- 文件：`docs/beta2/shell.md`。mods：`yuzora-sidebar-workspace`、`yuzora-modebar`、`yuzora-statusbar`、`yuzora-shell-tabs`（tab 列＋新增 pane 選單）、`yuzora-shell-pane`（pane 標頭）、`yuzora-new-space`（新增 Space 的資料夾對話框）；crate `yuzora-shell`（`workspace()`、`spaceBadges()`、`hostname()`、`agentBrand()`、`gitStatus()`、`planSpace()`）。
- 側欄模型在 Rust（`app/crates/yuzora-shell/src/workspace.rs`，純函式＋單元測試），mod 只負責畫。改分組、排序或計數先改測試。
- 5 種 agent 狀態形狀是 `yuzora-sidebar-workspace/status/*.svg`，其他 mod 以路徑引用，不要複製；顏色取 `tokens().status.<狀態>.mark`。
- 上游改動只有 `PaneFrame.blocked`（`app/src/layout.rs`、`app/src/window.rs` 各一處）、人執行的 bare `space.new` 改開資料夾對話框（`app/src/window.rs` 的 `run_as`），邏輯都在 `app/src/yuzora/shell.rs`；另有 welcome／whatsnew／feedback 的品牌字樣，都標 `// yuzora(L1):`。
- 實機假 agent：`printf '\033]7501;state=blocked:app=codex\033\\'; exec sleep 100000`（state 可為 working／blocked／done／idle）。
<!-- yuzora:lane:L1:end -->

<!-- yuzora:lane:L2:start -->
### L2 · Git（`yuzora-git`、`yuzora-sidebar-git`、`yuzora-git-*`）

- 設計、A1–A12 完成度與效能數據：`docs/beta2/git.md`。git 只走 CLI（不用 libgit2），一律經 `crates/yuzora-git/src/exec.rs` 的執行器（讀取不拿鎖、寫入拿 repo 鎖、可取消、逾時、遠端走 `hc exec --on`）；不要在 mod 裡用 herdr 的 `exec("git", …)`。
- 路徑一律是字面路徑（`GIT_LITERAL_PATHSPECS=1`）。自動執行的讀取不能讓 repo 自己的設定挑程式來跑：讀取關掉 fsmonitor、簽章檢查用使用者自己的 gpg、blame 不跑 textconv；repo 本地設定若有 filter／ssh／credential 等程式，所有 `yuzora:git` 呼叫（`status` 只回傳提示）都要等使用者信任資料夾（`trust.rs`，在 `repo_fn` 統一檢查，log 的載入與 diff／merge session 的寫入不經過 `repo_fn`，各自檢查；偵測與信任清單在 `yuzora_core::git_trust`，其他 lane 自己跑 git 時也用它）。新增函式一律走 `repo_fn`，新增讀取照這個原則。
- merge pane 的編輯與結果邏輯有 bun 測試：`bun test scripts/yuzora-git-merge.test.ts`（在 `app/` 下執行）。
- 測試不得讀寫真實的 app home：Yuzora 自己的檔案一律經 `settings::own_dir()`（測試時指向暫存目錄）。
- 測試：`cargo test --locked -p yuzora-git`（每個測試在暫存 repo 上跑真的 git）。大型 repo 效能：`YUZORA_GIT_PERF_REPO=<repo> cargo test -p yuzora-git --lib --config 'profile.dev.package.yuzora-git.opt-level=2' -- --ignored --nocapture perf`，只對 clone 出來的複本量。
- mods 的 `v_virtual_list` item renderer 裡不能註冊事件（gpui-shell 會丟例外、清單整個不畫，`check-mods` 抓不到）：用清單的 `on_item_click`／`on_item_secondary_click`。
- 實機時 mods 可以熱更新：app 執行中把 `app/defaults/mods/<mod>` 複製到隔離 app home 的 `mods/<mod>`（換新 binary 重啟時，與內建版本不同的副本會被搬到 `<mod>-edited`，改裝內建版本）；`herdr-app do tab.new` 要用旗標形式（`--mod … --props '{…}'`），單一 JSON 物件會掉 `mod`。
<!-- yuzora:lane:L2:end -->

<!-- yuzora:lane:L3:start -->
#### L3 資料庫

- `app/crates/yuzora-db`（`yuzora:db`）＋ mods `yuzora-sidebar-database`、`yuzora-db-connection`、`yuzora-db-table`、`yuzora-db-query`。設計、API、共用狀態 key 與限制見 `docs/beta2/db.md`。
- 引擎 integration tests 在 `app/crates/yuzora-db/tests/engines.rs`：SQLite 一律跑；PG／MSSQL 需要 `tests/database/docker-compose.yml` 的容器，並設 `YUZORA_DATABASE_TEST_ENGINES=postgres,mssql`（沒設就印 skip 後通過）。效能量測是 `#[ignore]`，用 `--ignored --nocapture` 跑。
- 隔離實機執行加 `YUZORA_DB_SECRETS=memory`，密碼只留在該 process，不碰系統 keychain。
<!-- yuzora:lane:L3:end -->

<!-- yuzora:lane:L4:start -->
#### L4 SSH／SFTP

- crate `app/crates/yuzora-ssh`（`yuzora:ssh`），mods `yuzora-sidebar-ssh`、`yuzora-sftp`、`yuzora-ssh-{prompt,host,known-hosts,herdr}`。設計與 tunnel API：`docs/beta2/ssh.md`。
- 測試：`cargo test --locked -p yuzora-ssh`。整合測試在測試行程內起 russh SSH 伺服器，SFTP 用系統的 OpenSSH `sftp-server`（沒有時 skip），一律用暫存目錄與記憶體憑證庫。
- 絕不寫入使用者的 `~/.ssh`：known_hosts 只寫 `<app home>/yuzora/known_hosts`，`~/.ssh` 唯讀；密碼與密語只在使用者勾選時存進系統鑰匙圈（namespace `ssh`）。
<!-- yuzora:lane:L4:end -->

<!-- yuzora:lane:L5:start -->
<!-- yuzora:lane:L5:end -->

<!-- yuzora:lane:L6:start -->
#### L6 設定＋MOD／Template

- crate `app/crates/yuzora-settings`（`yuzora:settings`）：`templates`（與 hc 0.1.13 對齊的模板解析、檢查、寫入、遠端 sh 腳本）、`launch`（`hc run` 參數與 label）、`mods`（mods／layouts 分類、還原、從資料夾安裝）、`trash`（各平台垃圾桶）。
- mods：`yuzora-settings-general`、`-templates`、`-mods`（設定頁）與 `-launch`（「從模板啟動」對話框，`place: none`）。
- 上游改動只有 `app/defaults/frame/settings/main.js` 帶 `// yuzora(L6):` 的行；邏輯在新檔 `frame/settings/yuzora.js`，字串在 `frame/settings/i18n/`。
- 測試：`cargo test --locked -p yuzora-settings`；mods 用 `scripts/yuzora/check-mods yuzora-settings-general yuzora-settings-templates yuzora-settings-launch yuzora-settings-mods`；frame 由 `app/src/mods/render_tests.rs` 守，測試環境沒有任何 Yuzora 設定頁，frame 在 `settingsPages()` 為空時也要照常畫出。
- 測遠端模板腳本時不得碰到真正的垃圾桶：測試的 PATH 只放 cat／mkdir／mv／rm／date／ln。
- 設計與決策：`docs/beta2/settings.md`。
<!-- yuzora:lane:L6:end -->

<!-- yuzora:lane:L7:start -->
#### L7 發布與更新

設計、金鑰與發布步驟：`docs/beta2/release.md`。

- 打包：`app/scripts/yuzora/package [<triple>]` 輸出到 `app/dist/`；`YUZORA_PACKAGE_BIN=<binary>` 可跳過 release build，只檢查打包。macOS 只做 aarch64、ad-hoc 簽章、`Yuzora-macos-aarch64.dmg`（`scripts/yuzora/mac-dmg`）；Windows 是 per-user NSIS（`app/distribution/yuzora/windows/yuzora.nsi`）；Linux 是 tar.gz 加 `install`。
- 名稱：Cargo 的 binary 維持 `herdr-app`，打包後叫 `Yuzora`／`yuzora`，CLI 是 `yuzora`。不要在任何地方安裝名為 `herdr-app` 的東西。
- 更新：`app/src/yuzora/release/`（通道、manifest 簽章驗證、CLI 連結）。上游 `app/src/update.rs`、`app/src/main.rs` 只有標了 `// yuzora(L7):` 的接點，其餘照上游。
- manifest：`bun app/scripts/yuzora/manifest.ts yuzora|tauri <dist>`；測試在 `app/` 下 `bun test scripts/yuzora/`。
- **私鑰永遠不進 repo；PR 與打包 workflow 不讀任何 signing secret**。簽章只在發布步驟。不新增、不放寬任何安全例外。
- `.github/workflows/app-package.yml` 只產 artifacts，不建 tag 或 release，不改 `release.yml`。
- 本機測完整更新流程：debug build 加 `YUZORA_UPDATE_URL=<仿 GitHub 路徑的測試伺服器>` 與 `YUZORA_UPDATE_TEST_KEY=<一次性公鑰>`（測試金鑰只有 debug build 會讀；`YUZORA_UPDATE_URL` 任何 build 都會讀，但簽章一樣只認信任的金鑰），一律在 lanes.md 的隔離環境下跑。
- beta.1 資料遷移：`yuzora_core::migrate`。beta.1 的資料只讀不寫；各 lane 實作自己的 `Importer`。
<!-- yuzora:lane:L7:end -->

<!-- yuzora:lane:L8:start -->
#### L8 效能監控

- 文件：`docs/beta2/perf.md`。mod：`yuzora-perf`（右下角 chip＋popover，指令 `yuzora-perf.toggle`）；crate `yuzora-perf`（`tick()`、`state()`）。
- 聚合與分類在 `app/crates/yuzora-perf/src/tree.rs`（純函式＋單元測試）。三類加總＝總量是不變式，改分類先改測試。
- herdr-core 的 server 在行程樹裡的位置不固定（由 app 的 `hc connect` 啟動時是 app 的孫行程，否則在 launchd 底下），也可能同時有好幾個。一律用 app 的 `hc server status --json` 取 `pid` 與 `holders` 辨識；只在 app 底下有 `hc` 子行程時才問，因為沒有 core 時 `hc server status` 會自己啟動一個。
- 取樣只在 worker thread。量單次取樣成本：用 app 的隔離環境變數執行 `cargo run --release -p yuzora-perf --example cost -- <app pid>`。
<!-- yuzora:lane:L8:end -->

<!-- yuzora:lane:L9:start -->
#### L9 Herdr 相容與上游同步

- 文件：`docs/beta2/herdr-compat.md`（Yuzora 與官方 Herdr.app 共用 herdr-core 時的盤點、已修項目與 2026-11-03 使用者決定的取捨）。
- 上游同步：照 `docs/beta2/architecture.md`「上游同步流程」做 `git subtree pull`，commit 後跑 `app/scripts/yuzora/sync-check`（`--quick` 不跑 cargo test）。同步 PR 一律 merge commit。
- Yuzora 只讀 `YUZORA_APP_HOME`，不讀 `HERDR_APP_HOME`。上游測試若只設 `HERDR_APP_HOME`，會在真實的 `~/.config/yuzora` 開 app home，要補上 `YUZORA_APP_HOME`（sync-check 會擋）。
- 和 Herdr.app 一起實測：
  - 兩邊用同一組 `env -i`、`HC_SESSION`、XDG 目錄；Herdr.app 另外設 `HERDR_APP_HOME`、`HERDR_TELEMETRY=0`、`HERDR_RELEASES_URL=http://127.0.0.1:9`（擋掉自我更新）。
  - 直接執行 `/Applications/Herdr.app/Contents/MacOS/Herdr`，不要用 `open`（會帶進 `HERDR_*`）。
  - 兩個 app 都會收養對方的終端機；在任一邊關閉收養來的 Space／tab／pane，會在 core 裡結束那個終端機。
- `hc server stop` 在任何 app 開著時，1 秒內就會被它的 `hc connect` 重新啟動（程式全部結束）。只對隔離 session 做。
<!-- yuzora:lane:L9:end -->

## Release platform support

- 從 v0.0.9 起，macOS App 僅支援 Apple Silicon（M 系列）。候選版與正式版使用 `aarch64-apple-darwin`，不得重新加入 Intel／universal App 安裝包或 `darwin-x86_64` updater entry。beta.2 的更新 archive（`yuzora-<version>-macos-aarch64.tar.gz`）與 `yuzora-update.json` 也只有 `macos-aarch64`，不得加入 `macos-x86_64` 或 universal target。
- macOS 固定下載檔名為 `Yuzora-macos-aarch64.dmg`；更新發布流程時須同步核對官網、README、`docs/operations.md` 與 updater metadata。
- 遠端 Host 保留 `macos-x86_64` 與 `macos-aarch64`；App 停止 Intel 支援不代表移除既有 Intel macOS 遠端工作區。
- 已發布的舊版 Intel／universal artifacts 保持不可變，不重寫舊版 release 或 tag。
- macOS 發布不使用 Apple Developer ID 簽章／公證；Stable 仍須 Tauri updater 簽章。正式 build 不可使用會一併停用 updater 簽章的 `--no-sign`；README 與 release notes 須說明 Gatekeeper 提示。
- beta.2（`app/`）起：
  - Windows 改用 per-user NSIS 安裝程式，固定下載檔名 `Yuzora-windows-x64-setup.exe`，不再出 MSI；Linux 另有 x86_64／aarch64 的 `yuzora-<version>-linux-<arch>.tar.gz`（沒有固定別名）。固定下載別名只上傳到 Stable release，Beta 只有版本化檔名（`docs/beta2/release.md` §6）。
  - Stable 與 Beta 的更新 manifest（`yuzora-update.json`）都必須以 release 金鑰（minisign，key id `742456BCCDC22A24`，與 Tauri updater 同一把）簽章，App 驗不過就不更新；帶 beta.1 升級的 `latest.json` 照樣要 Tauri updater 簽章。
  - PR 與打包 workflow（`app-package.yml`）不讀任何 signing secret，簽章只在發布步驟；私鑰永遠不進 repo。
  - macOS 只做 ad-hoc 簽章（`codesign --sign -`），Windows 沒有 Authenticode；README 與 release notes 須說明 Gatekeeper 與 SmartScreen 提示。

## UI components — shadcn first（Tauri／React app）

本節只適用 `src/` 的 React UI；beta.2 的 `app/` 用 GPUI 與 herdr mods，沒有 shadcn。

All UI components **must** prefer [shadcn/ui](https://ui.shadcn.com) (this project: `radix-nova`, alias `@/components/ui`).

Rules:

1. Before writing a custom component, check the shadcn registry / docs (`bunx --bun shadcn@latest search`, `docs <name>`).
2. If shadcn provides the component, install or reuse it from `src/components/ui/` and compose it. Do **not** reimplement buttons, dialogs, menus, scroll areas, tabs, inputs, etc.
3. Custom UI is allowed **only** when the shadcn registry has no suitable component (or the need is domain-specific chrome that is not a generic primitive). Document why in the PR / change notes when inventing a new primitive.
4. App-owned scrollable surfaces use `ScrollArea` from `@/components/ui/scroll-area`. Do not use bare `overflow-auto` / `overflow-y-auto` / `overflow-x-auto` as the scrollbar UI for content lists and reading panes.
5. Exceptions that keep native / library overflow (not ScrollArea): structural `overflow-hidden` clipping, text truncate, native `<textarea>`, CodeMirror `.cm-scroller` / merge hosts, xterm viewports, iframe / native child webview documents, and intentional hidden-overflow chrome (e.g. tab strips with keyboard-only overflow navigation).

## Rust 建置目錄重用

repo 有兩個 Cargo workspace，各自一份 `Cargo.lock` 與 target：

- 根目錄 workspace：`src-tauri`（app）、`src-tauri/host`（host）與 `src-tauri/companion`（gateway），共用根 `.cargo/config.toml` 設定的 `src-tauri/target`（`companion:prepare` 另用其下的 `src-tauri/target/companion`）。
- `app/` workspace（beta.2）：`app/.cargo/config.toml` 設定的 `app/target`。

一份完整的 target 從數 GB 到上百 GB 都有可能。過去每條 lane、每輪實驗都各開一個 `CARGO_TARGET_DIR`，曾經累積超過 450 GB，把磁碟塞滿。

- 主 checkout（`yuzora/`）使用上述預設 target，不另外設定 `CARGO_TARGET_DIR`。
- 不在主 checkout 的建置，包括 `../.yuzora-lanes/<lane>` worktree、`.claude/worktrees/` 底下的 session worktree、`output/` 底下的原始碼快照，一律重用固定的 slot。不得以 lane 或 experiment 名稱建立新的 target：

  ```bash
  LANES="$(dirname "$(dirname "$(git rev-parse --path-format=absolute --git-common-dir)")")/.yuzora-lanes"
  export CARGO_TARGET_DIR="$LANES/_targets/slot-1-app"   # workspace 的所有 crate 共用這個 slot，不再依 crate 分 slot
  ```

- slot 平時只有 `slot-1`、`slot-2` 兩組。cargo 會對 target 目錄上鎖，出現 `Blocking waiting for file lock on build directory` 代表這個 slot 正在使用中，請改用 `slot-2`。兩組都在使用中就等待，不要建立 `slot-3` 或 lane 專屬的 target。
- **beta.2 並行期間暫准 `slot-1`～`slot-4`**，分配見 `docs/beta2/lanes.md` 總覽表（L1＋L5 → `slot-1`、L2＋L3 → `slot-2`、L4＋L6＋L10 → `slot-3`、L7 → `slot-4`，L8 擇未上鎖者）。遇到鎖就等待，不要再建新的 slot。beta.2 結束後回到兩組。
- 不同 worktree 共用同一個 slot 時，第三方依賴可以直接重用，只有 yuzora 自己的 crate 會重新編譯。slot 裡的執行檔會被下一個使用者覆寫，量測或交付需要的 binary 請先複製出去。
- 效能或評估迴圈不要每一輪都把整份 repo 複製到 `output/`，請改用 lane worktree 或 patch。必要的快照在量測完成後刪除，只保留報告與數據。
- 開始 Rust build 前，如果 `df -h` 顯示可用空間低於 50 GB，先停下來向使用者回報，不要自行刪除其他 lane 或實驗的資料。

## Agent skills

### Issue tracker

工作項目與 PRD 追蹤於 GitHub `NakiriYuuzu/Yuzora` Issues。見 `docs/agents/issue-tracker.md`。

### Triage labels

使用五個 canonical triage roles：`needs-triage`、`needs-info`、`ready-for-agent`、`ready-for-human`、`wontfix`。見 `docs/agents/triage-labels.md`。

### Domain docs

本 repo 採 single-context domain layout，以 `.yuuzu/CONTEXT.html` 與 `.yuuzu/adr/` 為權威來源。見 `docs/agents/domain.md`。beta.2 的架構決策以 `docs/beta2/architecture.md` 為準。
