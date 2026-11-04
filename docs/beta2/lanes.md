# beta.2 lanes

L0 建好基底後，L1–L10 從 `beta2/integration` 各開一條分支並行開發（L8–L10 是後來加開的）。原則是**每條 lane 只改自己擁有的目錄**；共用檔只改自己的標記區塊。架構決策見 [architecture.md](architecture.md)。

## 總覽

| Lane | 範圍 | crate（JS 模組） | mods（`app/defaults/mods/`） | 其他擁有 | lane 文件 | Cargo slot |
| --- | --- | --- | --- | --- | --- | --- |
| L1 外殼工作區 | Spaces／Sessions／Agents 樹、需要注意區、scope chips、「＋ 新增 pane」選單、tab 列與 pane 標頭樣式、狀態列、模式列 | `yuzora-shell`（`yuzora:shell`） | `yuzora-sidebar-workspace`、`yuzora-modebar`、`yuzora-statusbar`、`yuzora-brand`、`yuzora-shell-*` | `app/defaults/layouts/yuzora.json`、`app/defaults/themes/yuzora-*.json`、`app/defaults/yuzora/tokens.json` | `docs/beta2/shell.md` | slot-1 |
| L2 Git | 以 IntelliJ IDEA Community 為基準的 Git：暫存／提交、分支、分支圖與提交紀錄、diff／merge、patch 確認 | `yuzora-git`（`yuzora:git`） | `yuzora-sidebar-git`、`yuzora-git-*` | | `docs/beta2/git.md` | slot-2 |
| L3 資料庫 | 連線、資料表格（資料／結構／SQL）、SQL 編輯、寫回 | `yuzora-db`（`yuzora:db`） | `yuzora-sidebar-database`、`yuzora-db-*` | `app/vendor/tiberius-0.12.3-yuzora` | `docs/beta2/db.md` | slot-2 |
| L4 SSH／SFTP | 主機清單、SFTP 雙欄傳輸、tunnel（供 L3 連遠端 DB）、已知主機 | `yuzora-ssh`（`yuzora:ssh`） | `yuzora-sidebar-ssh`、`yuzora-ssh-*`、`yuzora-sftp-*` | `app/vendor/russh-0.62.7-yuzora`、`app/vendor/russh-sftp-3.0.0-yuzora` | `docs/beta2/ssh.md` | slot-3 |
| L5 檔案＋編輯器 | 檔案樹（git 標記）、程式碼編輯器、Markdown 預覽 | `yuzora-files`（`yuzora:files`） | `yuzora-sidebar-files`、`yuzora-files-*`、`yuzora-editor-*`、`yuzora-markdown-*` | | `docs/beta2/files.md` | slot-1 |
| L6 設定＋MOD／Template | 設定框架（`settingsPages()`）、`general` 設定（含語系）、mods 管理、herdr-core templates 管理 | `yuzora-settings`（`yuzora:settings`） | `yuzora-settings-*` | Yuzora `settings.json` 的 `general` section | `docs/beta2/settings.md` | slot-3 |
| L7 發布更新 | 打包、簽章、更新通道（GitHub Releases）、binary 最終名稱 | 不另設 crate；app 端 glue 放 `app/src/yuzora/update.rs` 等新檔 | | `app/scripts/yuzora/bundle-*`、`app/assets/icons/yuzora*`、新的 app 發布 workflow | `docs/beta2/release.md` | slot-4 |
| L8 效能監控 | 右下角的資源用量 chip 與 popover：App 本體、herdr-core（終端機與 agents）、Yuzora 工具 | `yuzora-perf`（`yuzora:perf`） | `yuzora-perf` | 總指揮特准：`app/Cargo.toml` 與 `app/src/yuzora/mod.rs` 掛上 crate、`app/defaults/layouts/yuzora.json` 的 `status` 節點 | `docs/beta2/perf.md` | 無固定（slot-1～4 擇未上鎖者） |
| L9 Herdr 相容與上游更新 | 與官方 Herdr.app 共用 herdr-core 的盤點與修正、herdr-app 上游同步、herdr-core 版本把關 | 不另設 crate；app 端 glue 放 `app/src/yuzora/herdr_compat.rs` | | `app/scripts/yuzora/sync-check`、上游同步 PR（`git subtree pull`，merge commit 合併） | `docs/beta2/herdr-compat.md` | 依總指揮指派 |
| L10 文件與官網收斂 | 對外文件與官網反映 integration 上實際做出來的東西 | | | `README.md`、`AGENTS.md`、`CLAUDE.md`、`app/defaults/AGENTS.md` 標記區塊外的內容，以及已合併 lane 標記區塊的措辭；`README.zh-TW.md`（沒有標記區塊，跟著英文版同步）；根目錄 `CHANGELOG.md`、`CHANGELOG.zh-TW.md`；`docs/operations.md`；`site/` | 本檔與 PR 描述 | slot-3（只在需要自己截圖時建置） |

- 「`yuzora-<x>-*`」是 mod 名稱前綴，例如 L2 的 tool pane 叫 `yuzora-git-log`、`yuzora-git-diff`。
- 每個 crate 的 `Cargo.toml`、原始碼與測試都歸該 lane。lane 的 Rust 字串目錄放在 crate 內（例如 `app/crates/yuzora-git/i18n/`），mod 的字串放在 mod 內的 `i18n/`。
- 所有 crate 都已列在 `app/Cargo.toml`，所有 HostModule 都已掛進 `app/src/yuzora/mod.rs`。**lane 不需要、也不應該改這兩個檔**。

## 各 lane 可用的掛點

1. **JS 模組**：在自己 crate 的 `host_module()` 加函式（`.function`／`.async_function`）或原生元件（`.component`），並在 `DECLARATIONS` 加對應的 `export function …` 宣告（必須完全一致，否則 `validate` 失敗）。mod 端 `import { … } from "yuzora:<name>"`。
2. **共用層 `yuzora:core`**（JS）／`yuzora_core`（Rust）：`t`／`translate`、`locale`（永遠 `"en"`）／`setLocale`（不做事，留給舊 mod）、`tokens`、`panes`／`settingsPages`、`getSetting`／`setSetting`、`exec`（可帶 `on`、`timeoutMs`、`maxBytes`）、`appHome`、`version`。Rust 端另有 `secrets`（keyring）、`remote::ExecRequest`、`settings::section`、`paths`。完整宣告在 app home 的 `yuzora.d.ts`。
3. **tool pane**：在 mod 的 `mod.json` 宣告 `"yuzora": { "pane": {…} }`，L1 的「＋ 新增 pane」選單會列出它。
4. **設定頁**：在 mod 的 `mod.json` 宣告 `"yuzora": { "settings": {…} }`，L6 的設定框架會列出它。設定值存在 Yuzora `settings.json` 自己的 section，只讀寫自己的 section。
5. **側欄模式**：L2–L5 直接把佔位的 `yuzora-sidebar-<mode>` mod 換成真正的內容，不需要改 layout。
6. **憑證**：`yuzora_core::secrets`，namespace 用 lane 名稱（`db`、`ssh`…）。不要把密碼寫進檔案。
7. **遠端**：`yuzora_core::remote::ExecRequest::new(program, args).on(Some(machine)).timeout(…)`，在 worker thread 呼叫（它會 block）。

## 共用檔規則

### 文件標記區塊

`README.md`、`AGENTS.md`、`CLAUDE.md`、`app/defaults/AGENTS.md` 都有各 lane 的標記區塊（L1–L9；`CLAUDE.md` 只到 L8）：

```markdown
<!-- yuzora:lane:L2:start -->
（L2 的內容）
<!-- yuzora:lane:L2:end -->
```

- 只改自己的區塊內容，保留兩行標記本身，不要移動區塊。
- 例外：L10（收斂 lane）可以整理**已合併** lane 的區塊措辭、對齊兩語系，也可以把和區塊外重複的內容併到區塊外的對應段落（PR 描述要列出搬了什麼）；不改事實，標記本身不動。還開著的 lane 的區塊不動，由該 lane 自己改。
- 區塊外的內容由 L10 收斂。需要改區塊外的內容時，寫進 PR 描述請總指揮轉交。`README.zh-TW.md` 沒有標記區塊，由 L10 依英文版同步。
- 根目錄三份是給開發者與開發用 agent 讀的（建置、測試、目錄）；`app/defaults/AGENTS.md` 每次啟動都會寫進使用者的 app home，給**使用者的 agent** 讀（怎麼操作、怎麼改 Yuzora 的 mods），兩者寫的對象不同。
- 較長的設計與決策寫在 `docs/beta2/<lane>.md`。

### 上游檔（`app/` 內非 Yuzora 的檔案）

- 能新增檔案就不改上游檔。app 端需要 glue 時，新增 `app/src/yuzora/<lane>.rs`，並在 `app/src/yuzora/mod.rs` 加一行 `mod <lane>;`（唯一允許的 `mod.rs` 改動）。
- 非改不可時只做最少改動，同一行或上一行加 `// yuzora(L<n>): <原因>` 註記，並在 PR 描述列出每個上游檔的改動。這些註記是同步上游時檢查的依據。
- 不要改上游 mods（`app/defaults/mods/` 下非 `yuzora-*` 的 mod）；要改行為時新增 `yuzora-*` mod。

### `app/crates/yuzora-core`

- 先在自己的 crate 實作。確定有兩條以上 lane 需要時，才提到 `yuzora-core`。
- 只能新增（新檔或新函式），不能改既有函式的簽章或行為。PR 標題或描述標明「touches yuzora-core」。

### 依賴、`Cargo.lock`、notices

- 新依賴只加在自己 crate 的 `Cargo.toml`，不改 `app/Cargo.toml`。
- `app/Cargo.lock` 是衝突熱點。merge integration 時若衝突：先取 integration 的版本（`git checkout --theirs app/Cargo.lock`），再跑一次不帶 `--locked` 的 `cargo check`，讓 cargo 只補上自己的依賴。不要跑 `cargo update`。
- `app/Cargo.lock` 一有變動就要重產 `app/THIRD-PARTY-NOTICES`，否則 `tests/notices.rs` 會失敗：

  ```bash
  # 一次性安裝（cargo-about 0.9.3，需要 cli feature）
  cargo install cargo-about --version 0.9.3 --locked --features cli --root /tmp/yuzora-tools
  cd app && CARGO_ABOUT=/tmp/yuzora-tools/bin/cargo-about bun scripts/notices.ts
  ```

- `app/CHANGELOG.md` 第一筆必須是 `0.1.0-beta.2`（`update_notes` 測試）。各 lane 在該筆底下加自己的條目即可，不要新增版本標題。
- App 內的更新說明分成兩份：英文在 `app/CHANGELOG.md`，繁體中文在 `app/CHANGELOG.zh-TW.md`（只收 Yuzora 的版本，#206 起）。lane 加條目時兩個檔各加一行，英文進 `app/CHANGELOG.md`、繁中進 `app/CHANGELOG.zh-TW.md`；段落標題兩個檔一律用英文 `### New`／`### Fixed`。細節見 [release.md](release.md)（L7 負責）。
- 根目錄的 `CHANGELOG.md`（英文）與 `CHANGELOG.zh-TW.md`（繁中）是 GitHub Release 的內文，由 L10 維護，兩份的版本、段落與項目要一一對應（`scripts/release-notes.test.ts` 會比對）。

## 分支與 PR 流程

1. 從最新的 `origin/beta2/integration` 開 worktree 分支，例如 `beta2/l2-git`。
2. 開發期間在自己的分支 commit、push。
3. 完成時把 `origin/beta2/integration` merge 進自己的分支（不要 rebase 已 push 的分支），解衝突。
4. 跑完下面的檢查並全部通過。
5. push 後開 PR，base 為 `beta2/integration`，描述包含：範圍、改到的上游檔（含 `// yuzora(L<n>):` 清單）、驗證證據、已知限制。PR 描述結尾加 `🤖 Generated with [Claude Code](https://claude.com/claude-code)`。
6. 回報總指揮，由總指揮審查並合併。lane 不合併自己的 PR，任何 session 都不碰 `main`、tag 與 release。

commit trailer：`Co-Authored-By: Claude Opus 5.5 <yuuzu@yuuzu.net>`。

## 建置與檢查

```bash
export PATH="$HOME/.local/zig-0.16.0:$PATH"
LANES=/Users/yuuzu/HanaokaYuuzu/App/Tauri/.yuzora-lanes
export CARGO_TARGET_DIR="$LANES/_targets/slot-<N>-app"   # 依上表的 slot
git submodule update --init app/vendor/ghostty
cd app
cargo test --locked                       # app、ghostty-sys 與全部 yuzora-* crate
cargo test --locked -p yuzora-git         # 只跑自己的 crate
scripts/yuzora/check-mods                 # 每個 yuzora-* mod 的 herdr-app check
scripts/yuzora/check-mods yuzora-git-log  # 指定 mod
```

- slot 分配：L1＋L5 → `slot-1`、L2＋L3 → `slot-2`、L4＋L6＋L10 → `slot-3`、L7 → `slot-4`，L8 擇未上鎖者。cargo 會對 target 上鎖，出現 `Blocking waiting for file lock` 就等，不要建新的 slot。
- 同一個 slot 的執行檔會被另一條 lane 覆寫；要量測或保留的 binary 先複製出去。
- 開建前先看 `df -h`，可用空間低於 50 GB 就停下回報總指揮。不要刪除其他 lane 或實驗的資料。
- **不要直接對 `defaults/mods/` 執行 `herdr-app check`**：它會在被檢查的資料夾寫入 `gpui-kit.d.ts`、`jsconfig.json`，而 `build.rs` 會把它們打包進 app。一律用 `scripts/yuzora/check-mods`（在複本上檢查）。

### 實機執行（隔離）

絕不能碰使用者真實的 Herdr／herdr-core／Yuzora 資料。啟動前清掉所有 `HERDR_*`、`HC_*` 環境變數，並給 app 自己的 home 與 core session：

```bash
RUN=/tmp/yz-<lane>-run
env -i HOME="$HOME" USER="$USER" TMPDIR="$TMPDIR" TERM=xterm-256color LANG=en_US.UTF-8 \
  PATH="$HOME/.local/bin:/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin" \
  YUZORA_APP_HOME=$RUN/home HC_SESSION=yuzora-<lane>-test \
  XDG_STATE_HOME=$RUN/state XDG_CONFIG_HOME=$RUN/config XDG_RUNTIME_DIR=$RUN/runtime \
  HERDR_APP_NO_SOUND=1 \
  "$CARGO_TARGET_DIR/debug/herdr-app"
```

- 用同一組環境變數執行 `herdr-app status`、`herdr-app do …`、`herdr-app shot <file.png>` 操作與留證；`shot` 擷取的是最後一次畫出的畫面，視窗被遮住時可能是舊畫面。
- 結束時關掉 app，並用同一組環境變數執行 `hc server stop` 停掉隔離的 core。
- 要用 computer-use 或瀏覽器自動化做 GUI 驗收前，先問使用者並等明確同意。
- macOS 需要 `Yuzora.app` 外觀（Dock 名稱、圖示）時，用 `app/scripts/yuzora/bundle-macos <out-dir>` 包裝後執行 `<out-dir>/Yuzora.app/Contents/MacOS/Yuzora`。

## CI

`.github/workflows/app-ci.yml` 對 base 為 `beta2/integration`、改到 `app/**` 的 PR，在 Linux（ubuntu-22.04）與 macOS（macos-15，aarch64）執行建置與 `cargo test --locked`（Linux 上 `shipped_initial_descriptions` 會以 headless 渲染每個內建 mod）。macOS 另跑 yuzora crates 測試、`scripts/yuzora/check-mods`（check 會開隱藏視窗，需要視窗伺服器）與 shot_pixels；Linux 另跑 packaging 與文件檢查；windows-2022 另有一個 job 只跑 `cfg(windows)` 的 console 視窗測試（`yuzora-core spawn::`、`yuzora-git exec::`、`yuzora-files fsops::tests::run_starts`，`--test-threads=1`；整個 workspace 的測試在 Windows 上有既有的失敗，不跑），也納入 aggregator。固定的狀態名稱是 `App (build · test)`。只改 `app/**` 與 `docs/beta2/**` 的 PR 不會觸發 Tauri 的 `ci.yml`。
