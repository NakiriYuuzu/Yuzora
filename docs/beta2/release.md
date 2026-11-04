# beta.2 發布與更新（L7）

- 狀態：L7 實作完成，待總指揮審查。**沒有任何東西已發布**：本 lane 不建 tag、不建 release、不動 `main` 與現行 `release.yml`。
- 範圍：`app/`（herdr-app 基底）的打包、簽章、更新通道、binary 名稱、beta.1 → beta.2 升級與資料遷移框架、打包 CI。
- 架構背景見 [architecture.md](architecture.md)，lane 規則見 [lanes.md](lanes.md)。

## 1. 名稱

| 項目 | 名稱 | 說明 |
| --- | --- | --- |
| Cargo package／binary | `herdr-app`（不變） | 上游測試（`CARGO_BIN_EXE_herdr-app`）、`scripts/sandbox`、`docs.ts`、tracing filter 都寫死這個名稱，改名會讓每次 subtree pull 都衝突（D5） |
| macOS | `Yuzora.app/Contents/MacOS/Yuzora` | bundle id `dev.yuuzu.yuzora`，Info.plist 範本 `app/distribution/yuzora/macos/Info.plist` |
| Windows | `%LOCALAPPDATA%\Programs\Yuzora\yuzora.exe` | 解除安裝程式放在旁邊的 `Yuzora-uninstall\`（更新會整個換掉 `Yuzora\`） |
| Linux | `~/.local/share/yuzora/yuzora` | 桌面檔 `dev.yuuzu.yuzora.desktop`，與 Wayland app id 一致 |
| CLI | `yuzora` | macOS：app 啟動時建立 `~/.local/bin/yuzora` 連結（`app/src/yuzora/release/cli_link.rs`）；Windows：安裝程式把資料夾加進使用者 PATH；Linux：`install` 腳本建立連結 |

與上游 `herdr-app` 的相容：

- Yuzora 從不在 PATH 上放 `herdr-app`。官方 Herdr 的 CLI（`~/.local/bin/herdr-app`）與 Yuzora 的 `yuzora` 互不覆蓋；`cli_link.rs` 只會修正原本就指向 `Yuzora.app` 的連結，使用者自己的檔案或其他連結一律不動。
- `yuzora` 與 `herdr-app` 是同一支程式，子指令相同（`yuzora status`、`yuzora do …`、`yuzora shot …`）。`yuzora --version` 仍印 `herdr-app <version>`：上游 `update.rs` 的 `version_of` 依這個前綴確認下載的版本，改了要一起改。
- 給使用者 agent 的說明寫在 `app/defaults/AGENTS.md` 的 L7 區塊：在 Yuzora 裡 `herdr-app <cmd>` 一律換成 `yuzora <cmd>`，PATH 上的 `herdr-app`（若有）控制的是官方 Herdr 的視窗。

## 2. 平台與打包

一律由 `app/scripts/yuzora/package [<triple>]` 產生，輸出到 `app/dist/`（`YUZORA_DIST` 可改）：

| 平台 | 更新用 archive | 下載用安裝檔 | 驗證 |
| --- | --- | --- | --- |
| macOS（僅 Apple Silicon） | `yuzora-<version>-macos-aarch64.tar.gz`（內含 `Yuzora.app`） | `Yuzora-macos-aarch64.dmg`（`Yuzora.app`＋`/Applications` 連結） | `lipo -archs` 必須只有 `arm64`；`codesign --verify --deep --strict`；DMG 掛載後再驗一次 |
| Windows x64 | `yuzora-<version>-windows-x86_64.tar.gz`（內含 `yuzora\`） | `Yuzora-windows-x64-setup.exe`（NSIS） | CI 靜默安裝、重裝、解除安裝 |
| Linux x86_64／aarch64 | `yuzora-<version>-linux-<arch>.tar.gz`（內含 `yuzora/` 與 `install`） | 同一個 tar.gz | CI 解開後用暫存 HOME 執行 `install` |

- Release build 一律在編譯時設 `HERDR_APP_RELEASE=1`：只有這種 build 會自我更新（上游慣例）。
- `YUZORA_PACKAGE_BIN=<herdr-app>` 可以拿現成的 binary 打包（不重編），用來檢查打包本身。
- 每個 archive 都內含 `LICENSE`、`THIRD-PARTY-NOTICES`，腳本會解開比對。

### macOS

- **ad-hoc 簽章**（`codesign --force --sign -`），不用 Developer ID、不公證、不 staple（AGENTS.md「Release platform support」）。沒有 hardened runtime，所以 QuickJS 的 MAP_JIT 不需要 entitlement。
- DMG 只用 `hdiutil`（UDZO），不用上游的 `create-dmg`／Finder AppleScript，在 headless runner 上也一樣能做。沒有背景圖與圖示位置；上游的背景是 Herdr 的美術，不沿用（待辦）。
- **Gatekeeper**：從瀏覽器下載的 DMG 帶 quarantine，第一次開啟會被擋（「無法驗證開發者」）。macOS 15 以後要到「系統設定 › 隱私權與安全性」按「強制打開」，或在終端機執行 `xattr -dr com.apple.quarantine /Applications/Yuzora.app`。README 要寫（見 §9）。
- **App 內更新不會再觸發 Gatekeeper**：更新由 `curl` 下載、`tar` 解開，不帶 quarantine。
- **TCC 權限**：ad-hoc 簽章的 designated requirement 是 cdhash，每個版本都不同，所以每次更新後 macOS 可能再問一次麥克風、相機、資料夾等權限。這是不用 Developer ID 的代價，Tauri 版一樣如此。
- 上游更新程式對 ad-hoc bundle 會在 Info.plist 的 `LSEnvironment` 加 `PATH` 再重新 ad-hoc 簽章（`prepare_macos_bundle`），所以範本保留空的 `LSEnvironment`。Yuzora 找 `hc` 不靠它（`core_bin::fill_path()`）。

### Windows：NSIS，per-user

評估過兩條路：

1. **沿用上游**：PowerShell 安裝腳本（`irm … | iex`）把資料夾放進 `%LOCALAPPDATA%\Programs`。問題是 Yuzora 沒有自己的下載伺服器，`curl | iex` 指令也不適合放在 README 當主要安裝方式。
2. **NSIS（採用）**：`app/distribution/yuzora/windows/yuzora.nsi`。仍裝到上游更新機制預期的 per-user 資料夾，所以 app 內更新（整個資料夾交換）照常運作；另外提供開始功能表、使用者 PATH（`path.ps1`）、「設定 › 應用程式」的解除安裝項目。不需要系統管理員權限。

與 beta.1 的關係：

- beta.1（Tauri）以 MSI 安裝，per-machine（`C:\Program Files\Yuzora`），UpgradeCode `{A5829978-F7B9-5882-B5BB-B8521FCEBF5F}`（`src-tauri/src/nsis_migration.rs`）。
- beta.2 不能升級那個 MSI（per-user 與 per-machine 是兩套安裝）。NSIS 安裝完會用 `MsiEnumRelatedProductsW` 找舊 MSI，**詢問**是否移除（`msiexec /x … /passive`，由 msiexec 自己要求系統管理員權限）。靜默安裝（`/S`）時不移除。使用者資料不受影響。
- 旗標：`/S` 靜默、`/D=<資料夾>` 改位置（只收空資料夾，或解除安裝項目 `InstallLocation` 記錄的那個資料夾；其他非空資料夾一律拒絕，exit 2，因為更新與解除安裝都會處理整個資料夾；不帶 `/D=` 重跑會裝回 `InstallLocation`）；`/P`（只顯示進度）、`/R`（裝完開啟 Yuzora）、`/UPDATE`、`/ARGS` 是 Tauri updater 呼叫 NSIS 的方式（§5）。
- 已知限制：app 自我更新後，「設定 › 應用程式」顯示的版本仍是安裝時的版本；開始功能表捷徑沒有設定 AppUserModelID（工作列分組依 exe）。列入待辦。
- Windows 沒有 Authenticode 簽章，SmartScreen 會警告（與 beta.1 相同）。

### Linux

上游只有 tar.gz 加安裝腳本，Yuzora 照做，不做 AppImage／deb／rpm：

- `yuzora/install` 把資料夾複製到 `~/.local/share/yuzora`，建立 `~/.local/bin/yuzora`、圖示與桌面檔。再次執行（從較新的資料夾）就是手動更新。
  - 會整個替換的 `~/.local/share/yuzora` 只接受不存在、空的，或桌面檔 `Exec` 指向的那個（它裝的）；其他一律拒絕、不動。複製先進 `mktemp -d` 的新資料夾再搬過去。
  - `~/.local/bin/yuzora` 只取代指向 Yuzora 資料夾的連結；桌面檔直接執行安裝好的程式。
- 不從 `curl | sh` 下載，也不在安裝時裝 herdr-core（Yuzora 開啟時的設定畫面會提供）。
- 以 ubuntu-22.04 建置，glibc 下限跟上游一致。

## 3. 更新通道

```
啟動 10 秒後，之後每小時（上游時鐘）
  └─ release::update::check(root)
       1. GET api.github.com/repos/NakiriYuuzu/Yuzora/releases?per_page=100
          （整頁都比目前新時再讀下一頁，最多 5 頁：stable 通道前面可能堆了一整頁 beta）
       2. channel::pick_in：非 draft、tag 是 v<semver>、屬於本通道、比目前新、
          帶 yuzora-update.json 與 .sig 的 release 中，版本最大的一個
       3. 下載 manifest 與簽章 → manifest::read：先驗簽章，再驗內容
       4. 英文與繁中的 changelog 各下載一份（驗 sha256）給「新功能」說明（見下方〈更新說明〉）
       5. crate::update::stage：下載本機 target 的 archive、驗 sha256、
          解開、執行 --version 確認版本、放到 <安裝位置>.new
  └─ 設定畫面顯示「重新啟動」；重啟或下次結束時交換（上游 swap/apply）
```

- **通道**：`stable` 只收非 pre-release、版本不帶 `-` 的 release；`beta` 兩者都收。**beta build（版本帶 `-`）一律走 stable，不管設定寫什麼**：beta 之間不自動更新（2026-11-04 使用者決定，一律手動下載），0.1.0 Stable 發布後，裝著 beta 的人會自動升上去。stable build 預設 stable，Yuzora `settings.json` 的 `update` section（`{"channel": "stable" | "beta"}`）可以改成 beta。這個 section 屬 L7；設定 UI 由 L6 接（見 §10）。已經下載好、等著重啟套用的版本若不在目前通道內（beta build 上的 beta、或 stable build 改成 stable 後的 beta），會在下一次檢查、結束或啟動時丟掉，不會套用（`staged_version` 的 L7 接點，`update::keeps`）。
- **只有安裝好的 release build 會檢查**（`HERDR_APP_RELEASE`＋安裝形狀：macOS 是 `.app`；其他平台是有 `yuzora.package` 的資料夾，而且資料夾裡只有這份清單列出的東西，避免把解壓到使用者資料夾上的封包當成安裝、更新時整個資料夾被換掉）。從原始碼建置的不會連網。
- **版本比較**：上游 `newer()` 把 pre-release 當字串比，`beta.10` 會排在 `beta.9` 前面。已修正為 semver 規則（`release::prerelease_cmp`，`update.rs` 一行改動）。
- **錯誤處理**：任何一步失敗都只記 log、30 分鐘後重試（上游 RETRY）；GitHub API 403（每小時 60 次的未驗證上限）也是。一個 IP 後面很多台 app 時可能偶爾撞到上限，之後再評估改走 `releases/latest/download`。
- **測試伺服器**：`YUZORA_UPDATE_URL=<base>` 讓任何 build（包括 release build）改向一個仿 GitHub 路徑的伺服器檢查（`<base>/repos/NakiriYuuzu/Yuzora/releases`、`<base>/NakiriYuuzu/Yuzora/releases/download/v<version>/…`）。簽章照樣必須由信任的金鑰簽。
- **網路連線**：release build 每小時連 `api.github.com` 與 `github.com`（Yuzora 更新），以及 `releases.herdr.dev`（herdr-core 的版本，D3 保留上游 core 通道）。從原始碼建置的 app 兩者都不連。

### manifest：`yuzora-update.json`

```json
{
  "schema": 1,
  "name": "yuzora",
  "version": "0.1.0-beta.3",
  "channel": "beta",
  "pub_date": "2026-11-03T00:00:00Z",
  "notes": { "url": ".../v0.1.0-beta.3/changelog.md", "sha256": "…", "size": 1234 },
  "localized_notes": { "zh-TW": { "url": ".../v0.1.0-beta.3/changelog.zh-TW.md", "sha256": "…", "size": 1234 } },
  "targets": {
    "macos-aarch64": { "url": ".../v0.1.0-beta.3/yuzora-0.1.0-beta.3-macos-aarch64.tar.gz", "sha256": "…", "size": 1 },
    "windows-x86_64": { … },
    "linux-x86_64": { … },
    "linux-aarch64": { … }
  }
}
```

由 `bun app/scripts/yuzora/manifest.ts yuzora <dist>` 產生（只寫、不簽）。app 端的檢查（`app/src/yuzora/release/manifest.rs`）：

1. **簽章先驗**：`yuzora-update.json.sig` 是 minisign 對 manifest **原始位元組**的簽章，只接受 prehashed（`ED`）演算法；base64 包裝（`tauri signer sign` 的格式）或原始文字都可以。驗不過就不解析內容。
2. `schema` 必須是 1（新格式一律拒絕，請使用者手動更新）、`name` 是 `yuzora`、`version` 等於 release tag 的版本、`channel` 與版本一致（帶 `-` 就是 beta）。
3. 每個檔案（含 `notes` 與 `localized_notes` 裡每個語系的 changelog）的 URL 都必須在 `https://github.com/NakiriYuuzu/Yuzora/releases/download/v<version>/` 底下，檔名只能有英數與 `._-`；`sha256` 必須是 64 個小寫 hex。app 不認得的語系一樣要通過這些檢查，但不會因為不認得而拒絕整份 manifest（以後加語系時，舊版 app 照樣能更新）。
4. 下載後由上游 `unpack` 比對 sha256，再執行新 binary 的 `--version` 確認版本。

簽章保證「這份清單是 Yuzora 發的」，清單裡的 sha256 保證「下載的檔案就是清單上的那個」。舊版 manifest 不會被接受成新版（版本必須比目前新），換 tag 冒用也不行（版本必須等於 tag）。

### 更新說明（notes）

「新功能」與「重新啟動」畫面的更新說明依目前語系（`yuzora_core::i18n::current()`，跟著 `general.locale`）顯示：zh-TW 顯示繁中，其他語系顯示英文。

- **manifest**：英文是 `notes`（`changelog.md`），繁中是 `localized_notes["zh-TW"]`（`changelog.zh-TW.md`）；`manifest.ts` 看到 dist 裡有哪份就寫哪份。`localized_notes` 是新增欄位，`schema` 維持 1：沒有這個欄位的舊 manifest 照樣能讀，繁中退回英文；舊版 app 讀到新 manifest 會略過這個欄位，只用 `notes`。
- **下載**：找到新版時，每個語系各下載一份：繁中用 `localized_notes["zh-TW"]`，沒有就用英文 `notes`。每份都比對 manifest 裡的 sha256，不符就丟掉並記 log，**不會改用另一份**；更新說明是選用的，失敗不影響更新本身。
- **快取**：app home 裡一個語系一份，`update-notes-cache.md`（英文，上游原本的檔名）與 `update-notes-cache.zh-TW.md`，所以切換語系後馬上顯示另一份，不必重新下載。
- **兩份 changelog**：
  - `app/CHANGELOG.md`（上游檔）是英文，Yuzora 的段落在 Herdr 的上面。上游的解析器、`scripts/yuzora/sync-check` 與 justfile 都照舊可用。
  - `app/CHANGELOG.zh-TW.md`（Yuzora 自有）只放 Yuzora 的段落，是同一批版本的繁中版。標題仍用英文的 `### New`／`### Fixed`，因為 App 靠這個標題判斷 Fixed。
  - 兩份都內嵌進 build（`release::notes::bundled`）。zh-TW 先取中文，中文檔沒有的版本（例如 Herdr 的版本）逐版改用英文；其他語系一律英文。
  - 發布時 `app-package.yml` 把兩份複製成 `dist/changelog.md`、`dist/changelog.zh-TW.md`，manifest 帶兩份的 sha256。
- **新增條目**：每筆都要兩份各加一行，英文進 `app/CHANGELOG.md`，繁中進 `app/CHANGELOG.zh-TW.md`。測試會擋下這些情況：
  - 任一份的第一段不是 `CARGO_PKG_VERSION`；
  - 中文有、英文沒有的版本；
  - 同一版本兩份的日期、spotlight，或各標題下的項目數不同。
- **上游的接點**：`update_notes.rs` 只改了標 `yuzora(L7)` 的幾行，把快取與內建說明改成依語系取得。

## 4. 金鑰管理

- **沿用 Tauri updater 的 minisign 金鑰**（key id `742456BCCDC22A24`，公鑰在 `src-tauri/tauri.conf.json`）。理由：
  - beta.1 → beta.2 的 Tauri updater 升級（§5）無論如何都要用這把金鑰簽；
  - 私鑰已在 GitHub Actions secrets（`TAURI_SIGNING_PRIVATE_KEY`、`TAURI_SIGNING_PRIVATE_KEY_PASSWORD`），保管與備份依本機權威文件 `.yuuzu/adr/0003-updater-signing-key-custody.html`，不需要新的保管流程；
  - 只多一種用途，不多一把要顧的金鑰。
- 公鑰內嵌在 `app/src/yuzora/release/manifest.rs` 的 `TRUSTED_KEYS`。**私鑰永遠不進 repo**；repo 裡只有測試 fixture 的公鑰（`app/src/yuzora/release/fixtures/test-key.pub`，一次性金鑰，私鑰簽完 fixture 即刪）。
- **簽法**（release 時，在有 secrets 的受保護環境）：

  ```bash
  # 寫出 dist/yuzora-update.json.sig（tauri 一律寫成 <file>.sig，正好是 app 要的名稱）
  bunx @tauri-apps/cli signer sign -k "$TAURI_SIGNING_PRIVATE_KEY" -p "$TAURI_SIGNING_PRIVATE_KEY_PASSWORD" dist/yuzora-update.json
  ```

  簽完不可再改 manifest 的任何位元組（包括換行）。
- **換鑰**：先發一版把新公鑰加進 `TRUSTED_KEYS`（仍用舊鑰簽），等使用者大多更新後再改用新鑰簽，最後一版移除舊鑰。金鑰外洩時同樣流程，但要在 release notes 公告並請使用者手動下載。
- **debug build 的測試金鑰**：`YUZORA_UPDATE_TEST_KEY` 只在 `debug_assertions` 開啟且沒有 `HERDR_APP_RELEASE` 的 build 生效，用來在本機跑完整的更新流程；release build 永遠不讀它。
- **不在 PR／打包 workflow 讀任何 signing secret**：`app-package.yml` 產出的 manifest 未簽章。簽章屬於發布步驟（§6），應放進有 required reviewer 的 protected environment（operations.md §4 的既有建議）。

## 5. beta.1 → beta.2 升級

> 2026-11-03 已決定改採退路：beta.2 不附 `latest.json`，舊版 App 要手動下載（見本節末段與 decisions §10）。以下的分析保留作為日後帶 beta.1 升級時的依據。

### 結論：技術上可以用 Tauri updater 直接帶過來（macOS 與 Windows）；但 beta.2 不附 `latest.json`（見本節末）

依 beta.1 的程式（`src-tauri/src/update_channel.rs`）與 tauri-plugin-updater 2.14.1 原始碼：

- beta.1 的 preview 通道從 `api.github.com/repos/NakiriYuuzu/Yuzora/releases` 挑「非 draft、帶 `latest.json`、semver 比目前新」的最大版本，再讀 `releases/download/<tag>/latest.json`。beta.2 是 pre-release、帶 `latest.json`，就會被挑中。stable 通道不看 pre-release，所以 **0.0.19 stable 使用者不受影響**。
- beta.1 的 `latest.json` 缺目前平台的 key 時會直接報錯（設定頁顯示「無法檢查更新」），所以 beta.2 的 `latest.json` 必須同時有 `darwin-aarch64` 與 `windows-x86_64`。
- 更新需要使用者在設定頁按「下載」與「安裝並重新啟動」，beta.1 的未存檔保護與確認對話框照常運作；Windows 會先停掉 bundled HERDR。

**macOS**：`darwin-aarch64` 指向 `yuzora-<version>-macos-aarch64.tar.gz`（與 Yuzora 自己的更新 archive 是同一個檔案）。Tauri updater 會丟掉 tarball 的第一層（`Yuzora.app/`）、把整個 bundle 原地換掉，重啟時讀**新** Info.plist 的 `CFBundleExecutable`（`Yuzora`），所以執行檔改名不影響。bundle id 不變。bundle 寫入沒有權限時，Tauri 會用 osascript 要求系統管理員權限。

**Windows**：`windows-x86_64` 指向 `Yuzora-windows-x64-setup.exe`。Tauri updater 依位元組判斷安裝程式類型（不是看副檔名），NSIS 會以 `/P /UPDATE /R /ARGS …` 執行，並在啟動安裝程式後立刻結束 beta.1。NSIS 腳本處理這些旗標：`/P` 跳過完成頁、只顯示進度，`/R` 裝完開啟 Yuzora，最後詢問是否移除 beta.1 的 MSI（§2）。

**簽章**：`latest.json` 的 `signature` 是 Tauri 格式（minisign `.sig` 文字的 base64），用同一把金鑰對 archive 與安裝程式各簽一次：

```bash
bunx @tauri-apps/cli signer sign -k "$TAURI_SIGNING_PRIVATE_KEY" -p "$TAURI_SIGNING_PRIVATE_KEY_PASSWORD" dist/yuzora-<version>-macos-aarch64.tar.gz
bunx @tauri-apps/cli signer sign -k "$TAURI_SIGNING_PRIVATE_KEY" -p "$TAURI_SIGNING_PRIVATE_KEY_PASSWORD" dist/Yuzora-windows-x64-setup.exe
bun app/scripts/yuzora/manifest.ts tauri dist        # 缺任何一個 .sig 就拒絕寫出
```

**風險與注意**：

- 只有 beta.1（以及其他帶 preview 通道的 Tauri 版）會被帶過來；更舊、沒有 updater artifacts 的 beta 要手動下載。
- 換成 beta.2 後，資料不會自動搬：beta.2 的 app home 是 `~/.config/yuzora`，beta.1 的資料留在原處（§7）。在資料遷移接上之前，release notes 要明講「連線與設定需要重新設定」。
- macOS 的 TCC 權限會重問一次（ad-hoc 簽章）。
- 實機驗證（真的從已安裝的 beta.1 按更新）尚未做，要在發布前用候選 artifacts 跑一次（需要使用者同意 GUI 操作）。

### 不可行時的退路

**既定決定（2026-11-03，使用者）**：beta.2 release **不附 `latest.json`**，beta.1 就不會看到它（也不會報錯）；release notes 與 README 提示手動下載 DMG／NSIS 安裝程式。新 App 自己的更新也一樣只收 Stable（§3，2026-11-04）。

## 6. 發布流程（未實作，需使用者授權）

現行 `release.yml` 是 Tauri app 的發布：main 一 push 就自動 tag 與發布。beta.2 **不改它的觸發行為**。建議另開一個 beta.2 發布 workflow（或在 release PR 中改寫 `release.yml`，由使用者決定），步驟：

1. 觸發 `app-package.yml`（workflow_dispatch）取得四個平台的 artifacts 與未簽章 manifest。
2. 在 protected environment（required reviewer）下載 artifacts，加入 `changelog.md`（`app/CHANGELOG.md`）與 `changelog.zh-TW.md`（`app/CHANGELOG.zh-TW.md`；`app-package.yml` 的 artifact 已帶兩份），重產並簽 `yuzora-update.json` → `.sig`。
3. beta.2 不附 `latest.json`（§5 末段的既定決定），這一步略過。0.1.0 Stable 才簽 macOS archive 與 NSIS 安裝程式、產生 `latest.json`（§5）。
4. 建 draft release `v<version>`（beta：`prerelease=true`、不設 Latest），上傳：

   | 檔案 | beta | stable |
   | --- | --- | --- |
   | `yuzora-<version>-<target>.tar.gz`（4 個） | ✓ | ✓ |
   | `yuzora-update.json`、`yuzora-update.json.sig` | ✓ | ✓ |
   | `changelog.md`、`changelog.zh-TW.md` | ✓ | ✓ |
   | `Yuzora_<version>_aarch64.dmg`、`Yuzora_<version>_x64-setup.exe`（版本化） | ✓ | ✓ |
   | `Yuzora-macos-aarch64.dmg`、`Yuzora-windows-x64-setup.exe`（固定別名） | ✗ | ✓ |
   | `latest.json`＋兩個 `.sig`（帶 beta.1） | ✗（不附，既定決定） | 0.1.0 Stable 要附（decisions §10）：0.0.19 與 beta.1 靠它帶到新 App，搭配 §5 的 Tauri 簽章流程 |

5. 驗證 asset 清單完整（缺任一平台或簽章就不發布）、`yuzora-update.json.sig` 能用 `TRUSTED_KEYS` 驗過，再 publish。

注意：`Yuzora-windows-x64-setup.exe` 這個固定別名在 v0.0.18 以前是 Tauri NSIS 安裝程式，之後停用；stable 0.1.0 重新使用時，舊連結會開始拿到 beta.2 的安裝程式。這是預期中的，但官網與 operations.md 要同步。

## 7. 資料遷移框架

`app/crates/yuzora-core/src/migrate.rs`（touches yuzora-core，只新增）：

- **只讀**：beta.1 的檔案、localStorage、credential store 的內容都只讀不寫，回到 beta.1 仍可使用。唯一的例外是 SQLite 本身：localStorage 是 WAL 模式，`sqlite3 -readonly` 讀取時仍會在 `-shm` 記錄讀取鎖，但不會改動資料。
- **`Beta1`**：找出 beta.1 留下的資料。

  | 資料 | 位置 | API |
  | --- | --- | --- |
  | DB 連線 | `<data>/database-profiles-v1.json`（`version: 1`） | `db_profiles()` |
  | DB 密碼 | credential store，service `io.yuzora.database`，account `v1:18:io.yuzora.database:<len>:<descriptorId>:<len>:<generation>` | `db_password()` |
  | SSH host key | `~/.yuzora/known_hosts.json` | `known_hosts()` |
  | SSH 主機（`yuzora.hosts.ssh.v2`）、語系（`yuzora:language`）、外觀、終端機、編輯器、按鍵 | WebView localStorage | `local_storage(key)` |

  `<data>` 是 Tauri 的 app data（macOS `~/Library/Application Support/dev.yuuzu.yuzora`、Windows `%APPDATA%\dev.yuuzu.yuzora`）。localStorage 在 macOS 是 WebKit 的 SQLite（`~/Library/WebKit/dev.yuuzu.yuzora/WebsiteData/Default/<hash>/<hash>/LocalStorage/localstorage.sqlite3`，值為 UTF-16LE），以系統內建的 `/usr/bin/sqlite3 -readonly` 讀，**只讀 top 與 frame origin 都正好是 `tauri://localhost` 的那份**：beta.1 的預覽窗格用同一個資料區開外部網頁，任何網站都能在自己的 origin 留下同名的 key（例如假的 `yuzora.hosts.ssh.v2`）。**Windows 是 WebView2 的 LevelDB，目前讀不到**：SSH 主機與 localStorage 裡的設定在 Windows 要使用者重新設定。beta.1 本來就不存 SSH 密碼與 passphrase。
- **`Importer`**：每條 lane 實作一個，`section()` 是它在 Yuzora `settings.json` 的 section，也是 `yuzora_core::secrets` 的 namespace。`import(from, into)` 讀 `Beta1`、合併進 `into.value()`（不要覆蓋使用者已在 beta.2 設好的內容）、用 `into.set_secret()` 存密碼，回傳 `Imported { items, left }`（`left` 是沒帶過來的項目，給使用者看）。
- **`run(importers, from, yuzora_dir, secrets)`**：每個 importer 只跑一次，記在 `<yuzora dir>/migrations.json`；失敗的下次再跑；`version()` 提高時重跑一次。沒有 beta.1 資料時什麼都不記。
- 測試涵蓋：讀 beta.1 格式（含 keychain account 格式）、執行一次與重跑、失敗重試、版本提升、其他 section 不受影響、beta.1 檔案位元組不變、macOS localStorage（以 sqlite3 建的 fixture）。

### 欄位對應（L3、L4、L6 合併後補）

| section | lane | 來源 | 待決定 |
| --- | --- | --- | --- |
| `db` | L3 | `db_profiles()`＋`db_password()`（profiles 已依 beta.1 的 journal 收尾：沒做完的刪除與移除密碼直接套用；沒存完的新增或修改放在 `unfinished`，importer 要匯入或在 `left` 說明，不可默默略過） | `target`（sqlite／postgres／mssql 欄位、`viaHost` 對到 L4 的主機 id、`transportMode`／`trustCert`）對到 L3 的連線格式 |
| `ssh` | L4 | `local_storage("yuzora.hosts.ssh.v2")`、`known_hosts()` | 主機欄位（`id`、`name`、`host`、`port`、`user`、`authKind`、`keyPath`）；known_hosts 格式 |
| `general` | L6 | `local_storage("yuzora:language")`（`system`／`en`／`zh-TW`） | 外觀、終端機、編輯器設定要不要帶 |

接線（總指揮）：在啟動流程的 worker thread 呼叫 `migrate::run(&[…各 lane importer], Beta1::locate().as_ref(), &paths::yuzora_dir(), &KeyringStore)`，把結果（`Imported.left`）用 toast 或設定頁顯示。`KeyringLegacy` 讀 beta.1 的 credential 時，macOS 會跳 Keychain 存取確認（不同 app 簽章），要在 UI 說明。

## 8. CI

### `app-package.yml`（新增）

- 觸發：PR 到 `beta2/integration` 且改到打包相關路徑、push 到 `beta2/integration`（同路徑）、`workflow_dispatch`。
- 平台：macos-15（aarch64）、windows-2022（x64）、ubuntu-22.04（x64）、ubuntu-22.04-arm（aarch64）。
- 每個平台：`scripts/yuzora/package` → 以使用者的方式打開檢查（macOS：arch、ad-hoc 簽章、Info.plist、`--version`、DMG 掛載；Windows：靜默安裝、PATH、解除安裝項目、重裝、解除安裝；Linux：暫存 HOME 執行 `install`）→ 上傳 artifacts（7 天）。
- 之後 `manifest` job 跑 `bun test scripts/yuzora/`、產生未簽章的 `yuzora-update.json`。固定狀態名稱 `App (package)`。
- **不建 tag、不建 release、不讀任何 secret**，權限只有 `contents: read`。

### `app-ci.yml` 的快取與執行時間評估

L0 PR #187 最後一次成功的執行（run 41480446879）：

| job | 總時間 | 建置測試 | 測試 | 快取 |
| --- | --- | --- | --- | --- |
| macOS | 8 分 | 5 分 54 秒 | 16 秒＋yuzora crates 53 秒＋shot_pixels 5 秒＋check-mods 3 秒 | 未命中 |
| Linux | 6 分 41 秒 | 4 分 51 秒 | 17 秒＋packaging／docs 3 秒 | 未命中 |

- 目前**從來沒有命中過**：快取只在 push 到 `beta2/integration` 時存，而 L0 還沒合併。合併後第一次 push 會存，之後 lane PR 才會命中。
- 冷建置只要 5–6 分鐘（依賴用 `opt-level=0`），快取能省的有限；但 repo 的 Actions 快取總量已經 10.5 GB（超過 GitHub 預設的 10 GB），新的大快取會把 Tauri 的 `rust-host`、`database-integration` 等快取擠掉。
- 建議：
  1. 維持只在 integration push 存快取（現狀）。
  2. 觀察合併後的命中率；若 Tauri 的快取被頻繁擠掉，考慮 app-ci 只快取 `~/.cargo/registry`，不快取 `app/target`（冷建置成本只多幾分鐘）。
  3. `app-ci.yml` 沒有 Windows。Windows 只在 `app-package.yml` 建置；建議發布前與大型合併後手動跑一次 `app-package.yml`。
- `app-package.yml` 刻意**不用快取**：四個平台的 release target 很大，會擠掉其他快取；它只在打包相關變更時跑。

## 9. 需同步的項目

由總指揮在 beta.2 正式發布前收斂（本 lane 不改這些檔案的非標記區塊）：

- **官網（`site/`）**：
  - macOS 下載仍是 `Yuzora-macos-aarch64.dmg`（不變）。
  - Windows 由 `Yuzora-windows-x64.msi` 改為 `Yuzora-windows-x64-setup.exe`（`site/index.html`、`site/downloads.js`、`tests/site-downloads.test.js`）。
  - 新增 Linux 下載（`yuzora-<version>-linux-x86_64.tar.gz`／`aarch64`；沒有固定別名，或新增一個）。
  - Gatekeeper 說明改成 macOS 15 的「強制打開」流程。
- **README**：安裝方式（DMG、NSIS、Linux tar.gz＋`install`）、Gatekeeper、SmartScreen、`yuzora` CLI、從 beta.1 升級的說明（資料遷移狀態）。L7 區塊已寫入摘要。
- **`docs/operations.md`**：
  - §4 簽章邊界：beta.2 不再用 Tauri updater 的 bundle，改為簽 `yuzora-update.json`；同一把金鑰的新用途要寫進供應鏈敏感清單（`app/scripts/yuzora/manifest.ts`、`app/src/yuzora/release/manifest.rs` 的 `TRUSTED_KEYS`、`.github/workflows/app-package.yml`）。
  - §5 Stable／Beta 表格：beta.2 的 asset 清單（§6 表）。
  - §7 release workflow 階段、§7.3 固定檔名、§9 publish 後驗證：全部要為 beta.2 改寫。
  - MSI `ProductVersion` 編碼規則對 beta.2 不再適用（不出 MSI）。
- **AGENTS.md「Release platform support」**（建議文字，請總指揮收斂）：
  - 補一條：「beta.2 起 Windows 改用 per-user NSIS 安裝程式（`Yuzora-windows-x64-setup.exe`），不再出 MSI；Linux 提供 x86_64／aarch64 tar.gz。」
  - 「Stable 仍須 Tauri updater 簽章」改為「Stable 與 Beta 的更新 manifest（`yuzora-update.json`）都必須以 release 金鑰（minisign，key id `742456BCCDC22A24`）簽章，app 驗證不過就不更新」。
  - 「正式 build 不可使用 `--no-sign`」對 beta.2 不適用，可改為「打包 workflow 不讀 signing secrets；簽章只在發布步驟」。

## 10. 待辦與後續整合

**後續整合（合併後由總指揮串接）**

- L6：設定頁加「更新通道」選擇（寫 `update` section 的 `channel`，`stable`／`beta`），以及顯示目前通道。beta build 上這個選項沒有效果（一律只收 Stable，§3），顯示目前通道要用 `Channel::chosen()` 的結果，不能直接顯示 `update.channel` 的值，否則會在 beta build 上顯示「beta」、實際卻走 stable；可以從 `yuzora:core` 的 `getSetting`／`setSetting` 讀寫。
- L3／L4／L6：各自實作 `migrate::Importer`（§7 表），總指揮在啟動流程接上 `migrate::run` 與結果顯示。
- 發布 workflow（§6）：需要使用者授權發布方式與 secrets 的 environment。

**待辦**

- 實機驗證 beta.1 → beta.2 的 Tauri updater 升級（macOS、Windows）。
- Windows：自我更新後同步「設定 › 應用程式」的版本；捷徑的 AppUserModelID；Authenticode（未授權）。
- macOS：DMG 背景與圖示位置（Yuzora 自己的美術）。
- Windows 的 WebView2 localStorage（LevelDB）讀取，讓 SSH 主機與設定也能從 Windows 的 beta.1 帶過來。
- `yuzora --version` 仍印 `herdr-app`；若要改，`update.rs` 的 `version_of` 要一起改。
- 評估 stable 通道改走 `releases/latest/download/yuzora-update.json`（不受 API 次數限制）。
