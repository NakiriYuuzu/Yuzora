# beta.2 發布與更新（L7）

- 狀態：打包、更新通道與發布流程（§6，`app-release.yml`）都已實作。beta.2 至 beta.4 以 `App package` 的 artifacts 手動發布為 Pre-release（沒有簽章與更新清單）；0.1.0 Stable 起走 §6 的發布 workflow，把 `yuzora-update.json` 簽章後才讓已安裝的 beta 自動更新。
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
2. **NSIS（採用）**：`app/distribution/yuzora/windows/yuzora.nsi`。仍裝到上游更新機制預期的 per-user 資料夾，app 內更新（整個資料夾交換）才做得到，前提是沒有任何行程以該資料夾為工作目錄（見下一節）；另外提供開始功能表、使用者 PATH（`path.ps1`）、「設定 › 應用程式」的解除安裝項目。不需要系統管理員權限。

與 beta.1 的關係：

- beta.1（Tauri）以 MSI 安裝，per-machine（`C:\Program Files\Yuzora`），UpgradeCode `{A5829978-F7B9-5882-B5BB-B8521FCEBF5F}`（`src-tauri/src/nsis_migration.rs`）。
- beta.2 不能升級那個 MSI（per-user 與 per-machine 是兩套安裝）。NSIS 安裝完會用 `MsiEnumRelatedProductsW` 找舊 MSI，**詢問**是否移除（`msiexec /x … /passive`，由 msiexec 自己要求系統管理員權限）。靜默安裝（`/S`）時不移除。使用者資料不受影響。
- 旗標：`/S` 靜默、`/D=<資料夾>` 改位置（只收空資料夾，或解除安裝項目 `InstallLocation` 記錄的那個資料夾；其他非空資料夾一律拒絕，exit 2，因為更新與解除安裝都會處理整個資料夾；不帶 `/D=` 重跑會裝回 `InstallLocation`）；`/P`（只顯示進度）、`/R`（裝完開啟 Yuzora）、`/UPDATE`、`/ARGS` 是 Tauri updater 呼叫 NSIS 的方式（§5）。
- 已知限制：app 自我更新後，「設定 › 應用程式」顯示的版本仍是安裝時的版本；開始功能表捷徑沒有設定 AppUserModelID（工作列分組依 exe）。列入待辦。
- Windows 沒有 Authenticode 簽章，SmartScreen 會警告（與 beta.1 相同）。

### Windows：自我更新與「工作目錄」（2026-11-10 實測）

更新的交換是 `rename(Yuzora → Yuzora.old)`，再 `rename(Yuzora.new → Yuzora)`（`app/src/update.rs` 的 `swap`），由執行中的 `yuzora.exe` 自己做。上游與 NSIS 腳本的註解說「Windows 可以改名執行中程式的資料夾」。這句只對一半：**執行中的 exe 不擋改名，但資料夾只要是任何行程的工作目錄（cwd）就不能改名（os error 32）**。以前沒有實測；2026-11-10 在 CI 的 windows-2022 量過（探針分支 `spike/v010-win-update-probe`）：

- 程式在 `X\`、cwd 在別處：`X → X.old`、`X.new → X`、PowerShell 重開連三輪都成功。只要有任何一個活著的行程以 `X` 為 cwd（自己或別的行程）就失敗。
- 開始功能表捷徑的 Start in 是安裝資料夾（NSIS 在 `CreateShortcut` 前 `SetOutPath "$INSTDIR"`），`/R` 的 `Exec` 也是（依 NSIS 文件，`Exec` 的工作目錄是 `$OUTDIR`；這一項沒有實測）。所以一般使用者的 app，cwd 就在要被改名的資料夾裡。出貨的 release build 用 `/S` 安裝、cwd 在安裝資料夾時，交換失敗（`update: cannot move … aside: … (os error 32)`），資料夾仍是舊版。
- app 啟動的 `herdr-core.exe connect`（app 的子行程）與它再啟動的長壽 `herdr-core.exe serve` 都繼承 app 的 cwd，所以只把 app 自己的 cwd 移走不夠，也要在啟動任何子行程之前移走。

**所以 beta.2–4 在 Windows 上的自我更新一定失敗**；Stable 帶著這個出貨就沒辦法靠更新修好。0.1.0 的修正（`app/src/yuzora/release/cwd.rs`）：

- `main()` 在 bare launch 時、啟動任何子行程之前（也在 `apply_on_startup()` 的交換之前），如果 cwd 是執行檔所在的資料夾或其底下，就把 cwd 移到家目錄（`mods::home()`）。之後的子行程（`hc connect`、herdr-core、PowerShell 重開）都繼承新的 cwd。比對時兩邊都先 `canonicalize`（`current_dir()` 是 `C:\…`，`current_exe().canonicalize()` 是 `\\?\C:\…`）。命令列（`yuzora do …`、`shot` 等）不動 cwd：它們可能收相對路徑。
- 這個改動在 Windows 才執行（POSIX 本來就可以改名有行程在裡面的資料夾）。
- 新增的終端機本來就不從安裝資料夾開始：本機的 terminal 一律帶明確的 cwd（space 的資料夾，沒有就是家目錄；`window.rs` 的 `folder_for`、`new_space`），所以這個改動不改變終端機的起始目錄。程式碼裡沒有依賴 cwd 的相對路徑。
- 常駐的檢查：`app-package.yml` 的 Windows job 在安裝程式檢查之後跑 `app/scripts/yuzora/check-win-update.py`：靜默安裝出貨檔，以安裝資料夾為 cwd 啟動（和捷徑一樣），預先放好暫存的更新（`Yuzora.new` 與收據），斷言資料夾換成新版、`Yuzora.old` 是舊版、PowerShell 重開的行程活著且安裝資料夾在它執行時還能改名，再做一次第二次更新。結束碼 1＝更新沒成功，2＝檢查本身跑不起來（視窗開不起來、安裝程式逾時、版本字串取代失敗，以及任何沒預期到的錯誤，不是更新的結果）。修正前這個檢查必定紅（探針 run1 的 `R1-install.swap_1`、`D1-install.swap_1`；修正前的 commit `64cb3d7e` 在第三輪探針的 F job 上用現在這支腳本跑，必須以 1 結束；結果見該輪報告）。
  - **它守住的**：出貨的安裝程式與封包、從捷徑式的 cwd 啟動、啟動時的交換與 PowerShell 重開、連續兩次、app 自己的子行程不握著資料夾。
  - **它沒守住的**：(1) **沒有 herdr-core**：runner 沒裝，最長壽的持有者（`herdr-core serve` 與每個終端機的 `hold`）完全沒被測到；只有一次性的探針量過。(2) **只走啟動時的交換**（`apply_on_startup`）；按「Restart」（`app.update`）與關閉視窗走同一個 `swap`，但只有探針量過。(3) 下載、驗簽、解壓是手動放好的，不經過 `stage`。(4) **觸發路徑**：`app-package.yml` 的 `paths` 沒有 `app/src/window.rs`、`hc.rs`、`spawn.rs`、`mods.rs`、`control.rs`；改這些不會啟動它（例如之後有人在 `Core::connect` 加上 `.current_dir(<exe 資料夾>)`）。刻意不加：打包是四個平台的冷建置，window.rs 幾乎每個 PR 都會動。發版前手動 `workflow_dispatch` 跑一次。(5) **版本字串取代**：把 exe 裡的版本文字換成同長度的新版本。只換「整個版本」（前後不是更長版本的一部分），所以 `0.1.1` 不會把內嵌的最低 core 版本 `distribution/core-version`（`0.1.11`）換成 `9.9.11`（macOS 的除錯版 binary 實測：直接取代會換掉 20 處，限定整個版本後 0 處）；它會印出換了幾份、因為是更長版本的一部分而留下幾份。換不出可用的 patched exe（`--version` 不對）就以 2 結束，不是更新失敗。
- 交換失敗時，「重新啟動」現在會說原因（見〈交換失敗〉）。

**仍然會失敗的情況**：使用者自己的行程把安裝資料夾當 cwd（例如在終端機裡 `cd` 進去）、防毒軟體握著檔案，以及**beta 啟動、現在還活著的 herdr-core**。

- **為什麼**：beta.2–4 的 app 從開始功能表捷徑啟動，cwd 是安裝資料夾；它啟動的 `herdr-core serve` 繼承這個 cwd（實測，探針 C2）。core 開終端機時，`serve` 再啟動 `herdr-core hold`（每個終端機一個持有者：擁有 PTY、是裡面程式的父行程）。啟動 core 與 holder 的程式碼都沒有指定 cwd，所以 holder 也繼承（讀 herdr-core 0.1.19 原始碼：`client.rs` 的 `spawn_detached` 沒有 `current_dir`）。holder 的 cwd 沒有量到：第三輪探針的 C6 想量，但 `terminal.open` 回 `no space yet`，沒有 `hold` 行程可讀，結果是 `not-measured`；`serve` 的 cwd 在安裝資料夾則是量到的（C2、C6、C10）。NSIS 覆蓋安裝不會停掉 core（`yuzora.nsi` 只處理安裝資料夾裡的 `yuzora.exe`，core 也不在安裝資料夾裡）。
- **0.1.0 的 app 會沿用還活著的舊 core**（讀碼）：污染不會自己消退。新終端機要嘛進舊 `serve` 現有的 holder，要嘛由舊 `serve` 再生一個，cwd 都在安裝資料夾。
- **怎麼清掉它**：
  - **重新開機一次（或登出再登入）**。這是唯一簡單、同時涵蓋 `serve`、holder 與使用者自己的 shell 的做法，release notes 與失敗說明都這樣寫。
  - **進階：在 Yuzora 以外的終端機執行 `hc server stop`**（`hc` 是 herdr-core 的命令列）。這會結束 server，**所有終端機與裡面的 agent 都會結束**（讀碼：`api.rs` 的 `server_stop` → `runtime.rs` 的 `shutdown` kill 所有 live terminal，holder 收到 Shutdown 後結束）；損失與重新開機一樣，只是不必重開機。實測（第三輪探針 run `42871881561`），**只涵蓋沒有終端機開著的情況**：C7 與 C10 的 `terminal.open` 都回 `no space yet`，`hc server stop` 前只有 `serve` 和 `connect`，沒有 `hold`。C7 在 `hc server stop` 之後沒有任何 herdr-core 行程以安裝資料夾為 cwd，`yuzora do app.update`（CLI，不是滑鼠）一次就裝好；C10 從模擬的 beta core（用 `herdr-core connect` 在安裝資料夾的 cwd 啟動 0.1.20，不是真的 beta 時期安裝的 core）經安裝程式覆蓋安裝，第一次自動更新失敗（os error 32），`hc server stop` 之後同一個 `yuzora do app.update` 成功。**有終端機開著時**，`hc server stop` 會不會讓 holder 結束、資料夾會不會放開，只有讀碼的結論（`holder.rs` 收到 `Goodbye::Shutdown` 會結束），沒有量。
  - **不要**在工作管理員只結束 `herdr-core.exe serve`：holder 在 core 沒回來時會自己用 `spawn_core` 重啟一個，新的 `serve` 繼承 holder 的 cwd，仍在安裝資料夾（讀碼，`holder.rs` 的 `supervise`）。`hc server restart` 與 `hc update` 不當成出路寫給使用者：它們走 handover，holder 留著（讀碼）。實測只涵蓋一半：第三輪探針的 C6 在**沒有任何終端機開著**（`holders: []`）時執行 `hc server restart`，新的 `serve` 繼承執行 `hc` 的那個終端機的 cwd，安裝資料夾被放開，Restart 成功。有終端機開著時 holder 的 cwd 沒有量到（探針的 `terminal.open` 回 `no space yet`，沒有 `hold` 行程可讀），所以「有終端機時 restart 沒用」仍然只是讀碼的結論。
  - **不讓安裝程式或 app 自動結束舊 core**：那會無聲結束使用者所有終端機與 agent，違背「關掉、更新 Yuzora，agent 繼續跑」的承諾。要做在程式裡的話，應是交換失敗且原因是「被佔用」時，提供一個需要明確同意的動作「停止 herdr-core 並重試（會結束所有終端機）」，不在 0.1.0。
- **誰會碰到**：從 beta 手動裝上 0.1.0、而且**beta 啟動的 `herdr-core serve` 還活著**的人，**第一次**自動更新（0.1.0 → 0.1.1）會失敗；失敗時他們會看到下面的說明。`serve` 還活著就足以擋住更新，不需要終端機：C10 沒有開任何終端機，安裝程式前後都只有 `serve`（cwd 在安裝資料夾）和 `connect`，第一次自動更新仍以 os error 32 失敗。beta 時期開的終端機還活著時，它的 holder 也一樣握著資料夾（讀碼，沒有量）。0.1.0 全新安裝的人、或 core 與所有終端機都是 0.1.0 啟動的人（沒有 beta 時期的 core 或終端機還活著）不受影響。
- **所以 release notes 要寫**：Windows 的 beta 使用者請手動下載安裝程式，裝完重新開機一次（文字見〈Release notes 建議文字〉）。

**交換失敗**（`update::swap` 回傳 false）：

- 資料夾沒有被動過（`rename(root → .old)` 沒成功就什麼都沒搬；第二步失敗會 rollback）。暫存的 `Yuzora.new` 與收據留著。
- **原因的全文放在「設定 › Updates」**（`update::status()["app"]["failure"]`，設定畫面在 status 變動時重畫）：app 那一列下面一段可換行的文字，標題「The update could not be installed」，內文是下面的說明。只要暫存的更新還在，它就在；交換成功（程式隨即重開）或暫存的更新被丟掉就消失。啟動時（`apply_on_startup`）交換失敗、10 秒後的檢查又看到暫存的版本時，不必按任何東西，設定頁就看得到。
- 按右下角更新提示的「Restart」（或設定頁的「Restart to update」，`app.update`）：開一個 toast：標題 `Yuzora could not install the update`，內文 `Install folder in use. Click for steps.`（非「被佔用」的失敗：`Click for the reason and what to do.`），30 秒才消失、一次只一個，點下去開「設定 › Updates」。toast 的標題與內文各只有一行（`mods/toast` 截成一行、340 px），所以全文放不進去；之前把整段 283 字放在內文，視窗裡只看得到開頭一行（審查 P1）。`yuzora do app.update`（CLI）仍回傳全文。
- 說明全文（被佔用）：`Yuzora could not install the update because another program is using its install folder. Close any terminal opened in that folder. If you came from a beta, restart your PC once. Then choose Restart again, or download the installer from https://github.com/NakiriYuuzu/Yuzora/releases.`（其他錯誤：`Yuzora could not install the update: <系統訊息>. Choose Restart to try again, or download the installer from …`）。網址是文字，不能點（mod 沒有開網址的函式）；列為之後的改善。
- 關閉視窗時（`on_app_quit`）失敗只記 log（`update: cannot move … aside: …`）；視窗不在，沒有地方顯示，下次啟動時照上面再來一次。
- 不會重複下載：`stage` 看到收據的版本相同就直接回傳（`staged_version`），所以每小時的檢查只有 release 列表與 manifest 這幾個小請求。
- 更新被停用時（安裝資料夾裡有 `yuzora.package` 沒列的檔案，`is_install` 的刻意設計：交換會把整個資料夾換掉，使用者放的東西會跟著被搬進 `.old`）會記一行 `update: <資料夾> has files that are not in yuzora.package (…), so this install does not update itself`；這一行只在 stdout，Windows 的 GUI 沒有 console 也沒有 log 檔，所以使用者看到的仍是無聲停用（既有狀況，沒有處理）。
- 這個 PR 重構了 `is_install`（`release.rs`：改成 `strays()` 回傳多餘的檔案，並加上上面那一行 warn）；判斷邏輯等價：清單是空的、讀不到資料夾、讀不到某個項目、檔名不是 UTF-8 仍然都回 false。

**beta.2–4 的 Windows 使用者升到 0.1.0**：這些版本的更新程式在交換時一定失敗（它們啟動時的 cwd 就是安裝資料夾，自己握著它），要手動下載安裝程式（`Yuzora-windows-x64-setup.exe`，覆蓋安裝）。如果 0.1.0 的 release 附了 `yuzora-update.json`（Windows 的 target 也在裡面），他們的 app 會（beta build 一律走 Stable 通道）下載並暫存，右下角顯示的是**舊版的字串「Herdr 0.1.0 is ready」**（beta.2–4 的 `window.rs` 是 `Herdr {a} is ready`；「Yuzora … is ready」是這個修正之後的版本，已發出的 beta 不會變；探針 C2 的截圖也是「Herdr 99999999.9.1 is ready」），按「Restart」只會看到 `app.update failed: no update is ready yet`（beta 的 `apply()` 沒有說原因）；每次關閉與啟動都再試一次並失敗，不會重複下載（見上），資料夾不變。讀碼推導，沒有在 beta 實機驗證。若不想讓他們看到永遠卡住的提示，0.1.0 的 manifest 可以不列 `windows-x86_64`（0.1.0 自己不讀自己的 manifest；0.1.1 的 manifest 照常列）。

### Release notes 建議文字（Windows、從 beta 升上來）

英文：

> **Windows, if you are on a beta (0.1.0-beta.2 to beta.4):** the beta cannot update itself on Windows, so close Yuzora, download `Yuzora-windows-x64-setup.exe` from this release and run it over your install. (With the beta still open, the installer may not be able to replace its files.) Then restart your PC once. The beta started a background program (herdr-core) that still holds Yuzora's install folder, and that stops the first automatic update (0.1.0 to the next) until it is gone. Restarting ends it, and your terminals with it. If you would rather not restart, run `hc server stop` in a terminal outside Yuzora instead: it ends every terminal and the agents running in them, the same as a restart would. From 0.1.0 on, Yuzora updates itself.

繁體中文：

> **Windows 上還在用 beta（0.1.0-beta.2 到 beta.4）的人：** beta 版在 Windows 上無法自動更新，請先完全關閉 Yuzora，再從這個 release 下載 `Yuzora-windows-x64-setup.exe`，直接執行覆蓋安裝（beta 還開著時，安裝程式可能無法取代它的檔案），裝完請重新開機一次。beta 版啟動了一個背景程式（herdr-core），它還握著 Yuzora 的安裝資料夾，會讓 0.1.0 之後的第一次自動更新失敗，直到它結束為止；重新開機會結束它，裡面的終端機也會一起結束。不想重新開機的話，可以在 Yuzora 以外的終端機執行 `hc server stop`：它會結束所有終端機與裡面執行的 agent，和重新開機的損失一樣。從 0.1.0 起，Yuzora 會自己更新。

（兩段的重點：先關閉 Yuzora，再手動下載裝一次（`yuzora.nsi` 遇到 `yuzora.exe` 時先 `Delete`，失敗再把整個資料夾 `Rename` 成 `.old`，最後 `File` 寫入；執行中的 beta 兩步都會失敗，因為 exe 在執行、資料夾又是它的 cwd，之後 `File` 得直接寫入執行中的 exe。這是讀碼推論，沒有用真的 beta 開著量過：C3、C9 開著的是修正後的 build，cwd 已在家目錄，所以資料夾改名成功；所以兩段只寫「可能無法取代」，沒有寫會怎樣）；重新開機是為了讓 0.1.0 → 下一版的第一次自動更新直接成功，不必先失敗一次；不要建議只在工作管理員結束 herdr-core，見上。`hc server stop` 在沒有終端機時夠用是實測（第三輪探針 C7、C10，兩者都沒有終端機開著）；有終端機時夠用是讀碼（`server_stop` 會結束所有終端機與 holder）；「重新開機」沒有在 runner 上量，它結束所有行程，推論一定夠用。）

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
- **錯誤處理**：任何一步失敗都只記 log、30 分鐘後重試（上游 RETRY）；交換失敗見〈Windows：自我更新與「工作目錄」〉；GitHub API 403（每小時 60 次的未驗證上限）也是。一個 IP 後面很多台 app 時可能偶爾撞到上限，之後再評估改走 `releases/latest/download`。
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

「新功能」與「重新啟動」畫面的更新說明只顯示英文（beta.3 起介面只有英文）。繁中的 changelog 仍隨 repo 與 release 出貨，給讀中文的人，App 不顯示。

- **manifest**：英文是 `notes`（`changelog.md`），繁中是 `localized_notes["zh-TW"]`（`changelog.zh-TW.md`）；`manifest.ts` 看到 dist 裡有哪份就寫哪份。`localized_notes` 是新增欄位，`schema` 維持 1：沒有這個欄位的舊 manifest 照樣能讀；App 只用 `notes`，`localized_notes` 仍照樣檢查網址與 sha256 格式，但不下載。
- **下載**：找到新版時下載英文 `notes`。比對 manifest 裡的 sha256，不符就丟掉並記 log，**不會改用另一份**；更新說明是選用的，失敗不影響更新本身。
- **快取**：app home 裡的 `update-notes-cache.md`（上游原本的檔名）。舊版留下的 `update-notes-cache.zh-TW.md` 不再讀取。
- **兩份 changelog**：
  - `app/CHANGELOG.md`（上游檔）是英文，Yuzora 的段落在 Herdr 的上面。上游的解析器、`scripts/yuzora/sync-check` 與 justfile 都照舊可用。
  - `app/CHANGELOG.zh-TW.md`（Yuzora 自有）只放 Yuzora 的段落，是同一批版本的繁中版。標題仍用英文的 `### New`／`### Fixed`，因為 App 靠這個標題判斷 Fixed。
  - 只有英文的 `CHANGELOG.md` 內嵌進 build（`release::notes::bundled`）；繁中檔由測試守著與英文一致，但不進 App 的畫面。
  - 發布時 `app-package.yml` 把兩份複製成 `dist/changelog.md`、`dist/changelog.zh-TW.md`，manifest 帶兩份的 sha256。
- **新增條目**：每筆都要兩份各加一行，英文進 `app/CHANGELOG.md`，繁中進 `app/CHANGELOG.zh-TW.md`。測試會擋下這些情況：
  - 任一份的第一段不是 `CARGO_PKG_VERSION`；
  - 中文有、英文沒有的版本；
  - 同一版本兩份的日期、spotlight，或各標題下的項目數不同。
- **上游的接點**：`update_notes.rs` 只改了標 `yuzora(L7)` 的幾行，把快取與內建說明改成取自 `release::notes`（只有英文）。

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
- **不在 PR／打包 workflow 讀任何 signing secret**：`app-package.yml` 產出的 manifest 未簽章。簽章屬於發布步驟（§6）：`sign` job 引用 environment `yuzora-release`，要設了 required reviewers 才有人工閘門（§6.3 第 0 步）。金鑰本身是 repository 層級的 secret，不在那個 environment 裡。

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

**既定決定（2026-11-03，使用者）**：beta.2 release **不附 `latest.json`**，beta.1 就不會看到它（也不會報錯）；release notes 與 README 提示手動下載 DMG／NSIS 安裝程式。新 App 自己的更新也一樣只收 Stable（§3，2026-11-04）。beta.3 同樣是手動下載的 Pre-release（draft 先給使用者在 Windows 試過再發布），不附 `latest.json`、`yuzora-update.json` 與固定別名檔名；README 與 release notes 已指向 `v0.1.0-beta.3`。beta.4 比照辦理（beta.3 → beta.4 也是手動下載、不附 `latest.json`），README 與 release notes 指向 `v0.1.0-beta.4`。

**0.1.0 Stable 也不附 `latest.json`**（v0.1.0 發布計畫的決定）：換成另一個產品、資料不搬，不替 v0.0.19 與 beta.1 的使用者自動做這個決定，之後可以補。這兩個版本的使用者看不到 0.1.0，要手動下載；README 與 release notes 要寫明。裝著 beta.2 至 beta.4 的人則由 `yuzora-update.json` 自動升上來（§3），但 Stable 的 manifest 先不列 `windows-x86_64`（§6.1）。

## 6. 發布流程（Releasing）

發布由 `.github/workflows/app-release.yml` 完成，一次 dispatch 只做一個階段，階段之間由人看過再繼續。程式與 workflow 都寫在 repo 裡（`app/scripts/yuzora/release.ts`、`manifest.ts`、`verify-manifest/`、`fake-releases.ts`），不靠手動腳本。

### 6.1 原則

1. **驗收過的位元組就是出貨的位元組。** 候選版由 `App package` 在 release-prep commit 上產出；發布只下載那一個 run 的 artifacts，簽、上傳，不重建。run 必須是 `push` 或 `workflow_dispatch` 觸發（`pull_request` 的 run 建的是 GitHub 合併出來的 commit，不是會被打 tag 的 commit），而且成功、屬於本 repo 的分支。
2. **簽章只發生在 `sign` job。** 只有它讀 `TAURI_SIGNING_PRIVATE_KEY`、`TAURI_SIGNING_PRIVATE_KEY_PASSWORD`；打包與 PR workflow 不讀。寫 release 的 job 不 checkout、不執行 repo 裡的程式；執行 repo 程式的 job 只有讀取權限。
3. **分階段、可停、可重跑。** 重跑時，release 上已有同名同內容的檔案就略過，同名不同內容就失敗（絕不覆蓋）。
4. **不刪 tag、不改寫已發布的舊 release。** tag 是 annotated tag，由 `draft` 階段用 API 建在被驗收的 commit 上。
5. 0.1.0 **不附 `latest.json`**（不自動升級 v0.0.19／beta.1 的 Tauri 使用者），manifest **不列 `windows-x86_64`**（beta.2 至 beta.4 的 Windows 自我更新一定失敗；Windows 的 archive 與安裝檔仍照常上傳）。0.1.1 起 Windows 照常列（每一版發布前都要決定這一列：一旦某一版列出 Windows，還沒手動升級的 beta Windows 使用者就會看到按不過去的更新提示）。哪個版本不列哪些 target 由 repo 裡的表決定：`app/scripts/yuzora/manifest.ts` 的 `UNLISTED_TARGETS`（以完整版本字串為 key，沒有那一列就全部列出）。`plan` 讀它來寫 manifest，`verify-manifest` job 再自己讀它算出該有的 target 清單，用 `--targets` 逐一比對（多列或少列都會失敗）；dispatch 沒有任何輸入可以改這件事，所以漏打參數不會列出 Windows。要改就改表、過 review。
6. 根目錄 `package.json`、`src-tauri/tauri.conf.json`、`src-tauri/Cargo.toml` 等 Tauri 版本字串維持 `0.1.0-beta.1`。改它們會讓 `release.yml` 想發布 Tauri 版；`release.yml` 已加守衛（`app/Cargo.toml` 存在就在建 tag 前失敗），但不要依賴守衛。

### 6.2 階段

| 階段 | 做什麼 | 單向嗎 |
| --- | --- | --- |
| `dry-run` | 用正式金鑰簽一份 manifest，用 App 自己的 `manifest.rs` 驗過，存成 artifact `manifest-verified`。不建 tag、不建 release、不寫 repository。artifact 在公開 repo 是任何登入的 GitHub 使用者都下載得到的（保留 3 天），內容的網址全是本機，任何已安裝的 App 在正式環境都不接受。 | 否 |
| `draft` | 建 annotated tag（指向被驗收的 commit）與 **draft** release（`prerelease=false`、不是 Latest），上傳版本化安裝檔、固定別名、四個 tar.gz、兩份 changelog。release 內文取自根 `CHANGELOG.zh-TW.md`／`CHANGELOG.md` 該版本段落（繁中在前）。draft 沒有人看得到。 | tag 是；draft 可刪 |
| `publish` | draft 變公開，**不是 Latest**（API 的 `make_latest` 預設是 `true`，所以明確送 `false`）。檢查沒有 manifest／簽章、`releases/latest` 不是它、匿名下載每個檔案的 sha256 與 GitHub 的 digest 相符。更新程式在這一步仍看不到它。 | 是（公開） |
| `manifest` | 簽 `yuzora-update.json`，驗過，先上傳 manifest，**最後**上傳 `.sig`。兩個檔都在，已安裝的 beta 才會在下一次檢查（啟動 10 秒後，之後每小時）看到更新。 | 是（使用者端開始下載、暫存、交換，無法收回已暫存的） |
| `latest` | 設為 Latest。官網與 README 的 `releases/latest/download/...` 從此指向它。 | 是（外部連結立刻改變）；`gh release edit v0.0.19 --latest` 可切回 |

dispatch：

```bash
gh workflow run "App release" --ref main \
  -f stage=draft -f version=0.1.0 -f package_run_id=<被驗收的 App package run id>
# 之後每個階段同樣的 version 與 package_run_id，只換 stage（manifest 要列哪些 target 由 repo 的表決定，不是輸入）
gh run watch <run id>        # 每個階段的 job summary 會列出做了什麼、檔案與 sha256
```

各階段實際會跑的 job（其餘都是 skipped）：

| 階段 | 跑的 job | 讀 signing secret | 能寫什麼 | 執行 repo 程式 |
| --- | --- | --- | --- | --- |
| `dry-run` | `plan` → `build-manifest` → `sign` → `verify-manifest` | 只有 `sign` | 無（`contents: read`） | `plan`、`build-manifest`、`verify-manifest`、`sign`（只有 `bun install --frozen-lockfile --ignore-scripts`，與 lockfile 裝出來的 Tauri CLI；lockfile 是 dispatch 的 ref 上的） |
| `draft` | `plan` → `draft` | 無 | `draft`：tag、draft release、上傳檔案（`contents: write`） | 只有 `plan` |
| `publish` | `plan` → `publish` | 無 | `publish`：release 變公開、不是 Latest | 只有 `plan` |
| `manifest` | `plan` → `build-manifest` → `sign` → `verify-manifest` → `upload-manifest` | 只有 `sign` | `upload-manifest`：上傳 manifest，最後上傳 `.sig` | `plan`、`build-manifest`、`verify-manifest`、`sign` |
| `latest` | `plan` → `latest` | 無 | `latest`：設為 Latest | 只有 `plan` |

寫入的 job（`draft`、`publish`、`upload-manifest`、`latest`）不 checkout、只跑 `gh`、`jq`、`curl`、`sha256sum` 與 shell，不讀任何 secret；跑 repo 程式的 job 只有讀取權限。這兩件事由 `app-release.test.ts` 守著（見 §8）。

每個階段的輸入都先經 `plan` job 檢查：run 是否合格、`app/Cargo.toml` 在該 commit 的版本等於 `version`、tag 的位置、`draft` 時該 commit 是否已在 `main`。`draft` 之後的階段要求 tag 已存在且指向同一個 commit。

### 6.3 逐步手冊

下面每一步：做之前確認什麼、做完怎麼驗證、哪些是單向、失敗怎麼回復。

**0. 前置**（做一次）

- 確認 secrets 存在：dry-run 會替你確認（簽不出來就是缺或密碼錯）。不要讀、不要印它的值。
- 確認 repo 沒開 immutable releases（開了就不能在發布後補上傳 manifest）：`gh api repos/NakiriYuuzu/Yuzora/immutable-releases` 應回 `"enabled":false`。
- **第一個 dispatch 之前**：在 GitHub Settings › Environments 建立 `yuzora-release`，加上 required reviewers。`sign` 與四個寫 release 的 job 都引用它；設了之後，每個引用它的 job 都停在 Waiting for review，等 reviewer 在 Actions 頁核准（`manifest` 階段會停兩次：簽章前的 `sign`、上傳前的 `upload-manifest`）。
  - 它擋的是「workflow 檔沒變、只有 `release.ts`、`manifest.ts` 或 lockfile 變了」的 dispatch，以及從還帶著 push 觸發的舊 commit 切出的分支（見下）：這兩種情況下 `sign` 都還引用這個 environment，會停下等人。
  - 它**不保護金鑰本身**。`TAURI_SIGNING_PRIVATE_KEY` 與 `_PASSWORD` 是 repository 層級的 secret（`environments/yuzora-release/secrets` 是空的），能推 workflow 檔的人可以寫一個不引用 environment 的 workflow 去讀。所以金鑰的保管邊界是「誰有 repository 的寫入權與 workflow 權限」，這個 environment 只是 `app-release.yml` 自己的人工閘門。沒設 required reviewers 之前，不要宣稱簽章有閘門。
  - 不把 secret 搬進這個 environment：`release.yml` 的 `Validate updater signing inputs` 在沒有 environment 的 guard job 裡直接讀它們，搬走之後每次 main 的 CI 成功都會讓 `Release` 變紅。要搬的話先改 `release.yml`。
- `App release` 已經登記為 active workflow（`release-dryrun/1` 的兩次 push run，42874436208 與 42874964284，是在拿掉 push 觸發之前跑的）。GitHub 文件對 `workflow_dispatch` 的說明是「This event will only trigger a workflow run if the workflow file exists on the default branch」，以及「Once a workflow has run at least once, you can dispatch it against any branch or tag via the GitHub API or GitHub CLI」。所以合併進 main 之前也能對分支 dispatch（`gh workflow run "App release" --ref <分支>`），合併之後一律 `--ref main`。已確認的是登記狀態；「對分支 dispatch」本身在這個 workflow 上還沒有實測，第 1 步就是第一次。
- 沒有 push 觸發，這是刻意的：push 的 run 用的是被推送 commit 上的 workflow 檔，任何從還帶著 push 觸發的舊 commit（例如現在遠端的 `release-dryrun/1`）切出的分支，推上去就會用正式金鑰簽章。要啟動這個 workflow，一定得有 Actions 的寫入權限（dispatch）。
- dispatch 的 ref 就是這次發布的程式：`plan`、`build-manifest`、`sign`、`verify-manifest` 跑的都是該 ref 上的 `release.ts`、`manifest.ts`、lockfile；`draft`、`publish`、`upload-manifest`、`latest` 不跑 repo 程式，但它們用的 tag、commit、檔案清單、release 內文全是 `plan` 在該 ref 上算出來的。所以除了 dry-run，一律 `--ref main`（合併之後）。`plan` 另外要求該 ref 的 `app/Cargo.toml` 版本等於 `version`（dry-run 除外）：從 main（0.1.0）對 beta.4 的 run dispatch `manifest` 會被擋，不會改到已發布的舊 release。main 在 0.1.0 發完之前就前進到下一版時，改用 `--ref v0.1.0`（tag 在 `draft` 階段已建立）。

**1. 簽章試跑（dry-run）**，在任何單向動作之前；不需要候選版

```bash
git switch -c release-dryrun/1 <含 app-release.yml 的 commit>
git push origin release-dryrun/1          # 只是讓分支存在，不會啟動任何 run
gh workflow run "App release" --ref release-dryrun/1 -f stage=dry-run -f version=0.1.0
gh run list --workflow "App release" --limit 3
gh run watch <run id>
```

分支名稱沒有特別意義。沒有 `package_run_id` 時用合成的 archive，版本 0.1.0。

- 成功長這樣：`plan`、`build-manifest`、`sign`、`verify-manifest` 四個 job 綠；`verify-manifest` 的 summary 有一行 `verified yuzora 0.1.0 (stable) targets=linux-aarch64,linux-x86_64,macos-aarch64 …` 與 `tampered copy refused: … signature …`；artifact `manifest-verified` 含 `yuzora-update.json` 與 `yuzora-update.json.sig`。
- 失敗長這樣：`sign` 紅、訊息 `TAURI_SIGNING_PRIVATE_KEY is empty or missing`（secret 沒有）；或 Tauri CLI 解不開金鑰（密碼錯）；或 `verify-manifest` 紅、`The signature was created with a different key`（secret 裡的不是 `TRUSTED_KEYS` 那把）。這三種都在任何公開動作之前發現。
- 這一步用合成的 archive，不需要候選版，只回答「簽得出來、驗得過」。
- 單向？否。回復：刪掉 `release-dryrun/*` 分支即可（`git push origin --delete release-dryrun/1`）。第 3 步還要用這個分支，所以留到那時再刪。

**2. release-prep 合併、候選版與驗收**（先有候選，第 3 步才有東西可以檢查）

- release-prep（版本、CHANGELOG、README、NOTICES）合併進 `beta2/integration`，`App package` 在那個 commit 上的 `push` run 成功，就是候選版。記下 run id 與 commit sha（`gh run view <id> --json headSha,conclusion,event`）。
- 對那個 run 的 artifacts 做 macOS 隔離實機驗收（Windows、Linux 沒有真機驗收，release notes 要明講）。驗收過的位元組就是之後出貨的，不要再重跑 `App package` 產生另一份。
- 單向？否。

**3. 打 tag 前的隔離更新檢查**（需要第 2 步的候選 `App package` run；已發布的 beta.4 吃正式金鑰簽的候選 manifest，走完驗簽→暫存→交換）

先用候選 archive 簽一次（dry-run 階段，帶 `package_run_id`）。對 `release-dryrun/1`，或 infra 已合併進的 `beta2/integration`（這時 workflow 檔已在那裡）dispatch：

```bash
gh workflow run "App release" --ref release-dryrun/1 \
  -f stage=dry-run -f version=0.1.0 -f package_run_id=<候選 App package run id> \
  -f downloads_base=http://127.0.0.1:8765/NakiriYuuzu/Yuzora/releases/download
```

  `downloads_base` 只接受本機（`http://127.0.0.1` 或 `localhost`），所以這份用正式金鑰簽過、網址指向本機的 manifest，任何已安裝的 App 在正式環境都不會接受。artifact 只留 3 天；公開 repo 的 artifact 任何登入的 GitHub 使用者都能下載，不要拿這份當別的用途。
- dry run 的 id：`gh run list --workflow app-release.yml --limit 3`（下面的 `<dry-run id>`）。

用 beta 的 run（例如 beta.4 的 `App package`）簽出來的 manifest，dry-run 會把 archive 改名成 `yuzora-0.1.0-<target>.tar.gz`，簽章與內容檢查都過；但裡面是 beta 的 binary，`--version` 印的不是 0.1.0，所以下面的隔離檢查走不完交換。要走完整個更新，得用 release-prep 的候選（第 2 步）。

接著做隔離檢查。這一步會開出 App 視窗；依使用者全域規則，開之前先取得同意。

```bash
T=/tmp/yz-update-check; mkdir -p $T/serve $T/pkg $T/app $T/home $T/tmp $T/xdg/{config,data,state}
# 候選 archive 與 dry run 簽的 manifest（同一個 package run）
gh run download <候選 App package run id> -n yuzora-macos-aarch64 -D $T/pkg
gh run download <候選 App package run id> -n yuzora-update-unsigned -D $T/pkg
gh run download <dry-run id> -n manifest-verified -D $T/serve
cp $T/pkg/yuzora-0.1.0-macos-aarch64.tar.gz $T/pkg/changelog.md $T/pkg/changelog.zh-TW.md $T/serve/
# 已發布的 beta.4（要裝在 .app 形狀裡，更新才會啟用）
gh release download v0.1.0-beta.4 -p 'yuzora-0.1.0-beta.4-macos-aarch64.tar.gz' -D $T
tar -xzf $T/yuzora-0.1.0-beta.4-macos-aarch64.tar.gz -C $T/app
# 仿 GitHub 的伺服器：release 列表 + 下載，路徑與 App 要的一樣
bun app/scripts/yuzora/fake-releases.ts $T/serve --version 0.1.0 --port 8765 &
# 隔離啟動（沿用 beta.4 驗收的做法：env -i，App home 與 herdr session 都在 $T）
env -i HOME=$T/home USER=$USER TMPDIR=$T/tmp TERM=xterm-256color LANG=en_US.UTF-8 PATH=/usr/bin:/bin:/usr/sbin:/sbin \
  YUZORA_APP_HOME=$T/home/yuzora HC_SESSION=yuzora-update-check XDG_CONFIG_HOME=$T/xdg/config \
  XDG_DATA_HOME=$T/xdg/data XDG_STATE_HOME=$T/xdg/state HERDR_APP_NO_SOUND=1 \
  YUZORA_UPDATE_URL=http://127.0.0.1:8765 $T/app/Yuzora.app/Contents/MacOS/Yuzora
```

- 啟動後用 `ps eww` 與 `lsof -a -p` 確認 App 的 `HOME`、`YUZORA_APP_HOME` 都在 `$T`，沒有任何 `HERDR_*`；不要碰使用者真正的 `~/.config/yuzora` 與 herdr-core。
- 成功長這樣：伺服器 log 依序出現 `GET /repos/NakiriYuuzu/Yuzora/releases?per_page=100`、`yuzora-update.json`、`yuzora-update.json.sig`、`changelog.md`、`yuzora-0.1.0-macos-aarch64.tar.gz`（啟動約 10 秒後）；`$T/app` 旁出現 `Yuzora.app.new.noindex`；設定畫面出現「重新啟動」。重新啟動（或結束 App，結束時也會交換）之後，`$T/app/Yuzora.app/Contents/Info.plist` 的 `CFBundleShortVersionString` 是 `0.1.0`，`Yuzora --version` 印 `herdr-app 0.1.0`。
- 失敗長這樣：log 出現 `update check: …yuzora-update.json.sig: …`（簽章不被接受）、`checksum mismatch`（archive 與 manifest 的 sha256 不同，通常是 dry run 用了別的 run id）、`not in this release`（`downloads_base` 與伺服器網址不一致）。伺服器若根本沒收到請求，是 App 沒走 `YUZORA_UPDATE_URL`（確認環境變數、App 是 `.app` 形狀）。
- 收尾：結束 App、`kill` 伺服器的行程（只殺你記下的 pid）、`HC_SESSION=yuzora-update-check <hc> server stop`、`rm -rf $T`。
- 先確認 beta.4 與現在的更新程式碼沒有變：`git diff --stat v0.1.0-beta.4 <release-prep commit> -- app/src/yuzora/release app/src/update.rs app/src/update_notes.rs` 要是空的（#261 之類的修正會動到它；有動就要重看這一步與 A1 的結論）。
- 做完刪掉試跑分支：`git push origin --delete release-dryrun/1`。單向？否。

**4. integration → main**

- 用 merge commit（不能 squash 或 rebase：要保留 subtree metadata）。合併前 `ci.yml` 的 5 個 required check 要綠。
- 合併後確認：`gh run list --workflow Release --limit 1` 的 `Release` run 是 success 而且沒有 build／create-tag 等 job 執行（`release-state` 回 `shouldBuild:false`，因為 `package.json` 仍是已發布的 `0.1.0-beta.1`）；`App CI`／`App package` 也在 main 上跑起來；Pages 重新部署但 `site/` 沒變。
- `app-release.yml` 這時在 main 上，之後的 dispatch 都用 `--ref main`。
- 單向？commit 進 main 後只能 revert 或新 commit。

**5. `draft`**

- 做之前：run 的 `headSha` 就是 release-prep commit，且已在 main（上一步的合併保證）；tag `v0.1.0` 還不存在。
- 做完驗證：job summary 的 sha256 表與驗收紀錄一致；`gh release view v0.1.0 --json isDraft,assets` 是 draft、10 個檔案（4 個 `yuzora-0.1.0-<target>.tar.gz`、`Yuzora_0.1.0_aarch64.dmg`、`Yuzora-macos-aarch64.dmg`、`Yuzora_0.1.0_x64-setup.exe`、`Yuzora-windows-x64-setup.exe`、`changelog.md`、`changelog.zh-TW.md`）；`git ls-remote origin refs/tags/v0.1.0^{}` 是被驗收的 commit。
- draft 的 `tag_name` 只在 API 裡看得到（網頁網址一律是 `untagged-…`）。手動改 draft（補說明文字之類）時，PATCH 一定要帶 `tag_name`，否則 GitHub 會把它改掉，之後的階段找不到這個 release。
- 單向？tag 是。回復：draft 可以整個刪（`gh release delete v0.1.0 --yes`，不要加 `--cleanup-tag`），tag 留著，重跑 `draft` 會沿用同一個 tag。只有一個檔案內容不對：`gh release delete-asset v0.1.0 <名稱> --yes` 後重跑。tag 指錯 commit：不要動 tag，改發 0.1.1。
- 如果 `draft` 在建 tag 時被 GitHub 拒絕（403／404）：`GITHUB_TOKEN` 不能對「與預設分支的 `.github/workflows/` 不同」的 commit 建 release（REST 文件只對 create release 這樣寫；`plan` 會先給警告）。補救是讓 main 的 workflow 檔與該 commit 一致，或由使用者自己建 tag。

**6. `publish`**

- 做之前：draft 內容是上一步驗過的。
- 做完驗證：summary 列出每個公開檔案的 sha256（匿名下載算的，與 GitHub digest 相符）；`gh api repos/NakiriYuuzu/Yuzora/releases/latest --jq .tag_name` 仍是 `v0.0.19`；`yuzora-update.json` 與 `.sig` 還沒有。
- 單向？是（公開、被索引）。回復：可以轉回 draft 或刪除（`gh release edit v0.1.0 --draft`），但已被別人抓走的檔案收不回。
- 之後做官網的 PR（下一節）。

**7. 官網下載連結（`site/downloads.js`）**

- 這個變更（`fix(site): the Windows download is v0.1.0's setup.exe`）把 Windows 連結釘在 `releases/download/v0.1.0/Yuzora-windows-x64-setup.exe`，因為 `releases/latest/download/Yuzora-windows-x64.msi` 在 Latest 切到 v0.1.0 之後會 404（0.1.0 沒有 MSI）。釘死的連結在 v0.1.0 公開之後才有效，**所以它必須在 `publish` 之後、`latest` 之前進 main**：進得太早，v0.1.0 還沒公開的這段時間 Windows 按鈕是 404（官網現在的 `.msi` 連結指向 v0.0.19，還是好的）；進得太晚（`latest` 之後才部署），從 Latest 切換到 Pages 部署完成這段時間是 404。照這個順序沒有空窗。
- 做法：它在另一個分支 `claude/v010-site-download`（從發布基礎建設的某個中間 commit 長出來，再加兩個官網 commit；基礎建設後來的 commit 不在這個分支裡），不在 integration → main 的合併裡。基礎建設合併進 main 之後，這個分支對 `main` 開 PR 的差異只剩那兩個 commit。`publish` 之後合併它、等 Pages 部署完成（`gh run list --workflow "Deploy Pages" --limit 1`），再 dispatch `latest`。
- 做完驗證：`curl -sL -r 0-0 -o /dev/null -w '%{http_code}\n' https://github.com/NakiriYuuzu/Yuzora/releases/download/v0.1.0/Yuzora-windows-x64-setup.exe` 要回 200 或 206（用 GET 取一個位元組；簽名過的下載網址 HEAD 會被拒）。
- `latest` 之後要把 `site/downloads.js` 改回 `${RELEASE_DOWNLOAD_BASE}Yuzora-windows-x64-setup.exe`（同一個檔案，拿掉釘死）：不改的話，發布 0.1.1 之後官網的 Windows 按鈕仍然給 0.1.0。#209 的官網重設計（動到同樣的檔案）合併時要保留 Windows 這個連結。

**8. `manifest`**

- 做之前：第 3 步的隔離檢查成功過；release 已公開；你決定好讓已安裝的 beta 開始更新。
- 指令：`-f stage=manifest`（不需要另外指定不列 Windows：`UNLISTED_TARGETS["0.1.0"]` 決定，`plan` 的 summary 會寫「manifest leaves out: windows-x86_64」）。
- 做完驗證：summary 有 `verified yuzora 0.1.0 (stable) targets=linux-aarch64,linux-x86_64,macos-aarch64 …`（沒有 windows）；`curl -sL https://github.com/NakiriYuuzu/Yuzora/releases/download/v0.1.0/yuzora-update.json | jq '.targets|keys'`；`gh release view v0.1.0 --json assets --jq '.assets[].name'` 多了 `yuzora-update.json` 與 `yuzora-update.json.sig`。
- 單向？是。已暫存的更新收不回。**停止鍵**：刪掉 `.sig`（`gh release delete-asset v0.1.0 yuzora-update.json.sig --yes`）就讓之後的檢查看不到這個 release；已經暫存的 App 仍會在重啟時交換。
- 重跑：manifest 與 `.sig` 已在 release 上就略過；manifest 內容與這次的輸入不同（例如換了 package run 或 `UNLISTED_TARGETS`）會失敗，不會覆蓋。
- **收回（manifest 已上傳之後才發現列錯，例如多列或少列了某個 target）**：
  1. 先刪 `.sig`：`gh release delete-asset v0.1.0 yuzora-update.json.sig --yes`。release 要兩個檔都在才會被更新檢查看到（`channel.rs` 的 `pick_in`），所以從這一刻起沒有新的 App 會讀到它。
  2. 再刪 manifest：`gh release delete-asset v0.1.0 yuzora-update.json --yes`。不刪的話，重跑 `manifest` 會因為內容不同（`cmp`）失敗。
  3. 修正 `UNLISTED_TARGETS`，過 review 合併進 main，再 `gh workflow run "App release" --ref main -f stage=manifest -f version=0.1.0 -f package_run_id=<同一個 run id>`。重跑時 release 上沒有 manifest，會依新的表寫、簽、驗，先傳 manifest、最後傳 `.sig`。
  - **不受收回影響的**：已經讀過舊 manifest、把 archive 下載並暫存（`<安裝位置>.new`）的 App，重啟或結束時仍會交換到 0.1.0。這一步收不回，所以列錯 target 的代價是：列了 Windows 就是 Windows 的 beta 使用者看到一個按不過去的更新；少列 macOS 或 Linux 就是那些使用者要等收回後的重跑才收得到更新。已經暫存的更新不會因為重傳而換成別的內容。

**9. `latest`**

- 做之前：官網的 PR 已部署（第 7 步）。
- 做完驗證：summary 列出 `releases/latest/download/Yuzora-macos-aarch64.dmg` 與 `Yuzora-windows-x64-setup.exe` 都回 200／206；`gh api repos/NakiriYuuzu/Yuzora/releases/latest --jq .tag_name` 是 `v0.1.0`；官網的 macOS 與 Windows 按鈕各點一次。
- 單向？是（外部連結立刻改變）。回復：`gh release edit v0.0.19 --latest`。

**10. 發布後**

- 用 beta.4 的真實安裝（不是隔離的）在乾淨機器等一個小時內的更新檢查，確認真的升上 0.1.0（`gh` 的 API 未驗證請求一小時 60 次，同 IP 多台會偶發 403，30 分鐘後重試）。
- Tauri 版（v0.0.19、beta.1）使用者沒有更新入口：README 與 release notes 要寫手動下載、資料不會搬；Windows 的 beta 使用者要手動安裝並重新啟動。

### 6.4 失敗時看哪裡

| 現象 | 原因 | 怎麼辦 |
| --- | --- | --- |
| `plan` 紅：`did not succeed`／`not a run of …`／`merge commit` | 給的 run id 不合格 | 換成 `push` 或 `workflow_dispatch` 觸發、成功的 `App package` run |
| `plan` 紅：`is not on main` | `draft` 時該 commit 不在 main | 先合併 |
| `plan` 紅：`… is not the release this workflow runs from` | dispatch 的 ref 的 `app/Cargo.toml` 不是這個版本（例如從 main 對舊版 dispatch） | 對正確的版本、用該版本所在的 ref（`--ref main`，或 `--ref v<版本>`）；這是刻意的，不改舊 release |
| `plan` 紅：`gh api …` 開頭的錯誤 | GitHub 的 API 沒回答，或回應太大／`gh` 跑不起來（訊息會寫原因） | 過一會再重跑；沒有任何東西被改動 |
| `draft`／`publish`／`latest` 紅：`is under the tag untagged-… after the change` | GitHub 把 release 改到別的 tag 名稱底下。對 draft 做不帶 `tag_name` 的 PATCH 會這樣（2026-11-12 在 v0.1.0 的 draft 上實測：只帶 `body` 的 PATCH 之後 `tag_name` 變成 `untagged-<hash>`）；workflow 的每個 PATCH 都經過 `patch_release`，一律帶 `tag_name` 並讀回 | 還是 draft：`gh api -X PATCH repos/NakiriYuuzu/Yuzora/releases/<id> -f tag_name=v<版本>` 設回去，用 id 讀回確認後重新 dispatch。已經公開：不要再跑任何階段，先看 `git ls-remote origin 'refs/tags/untagged-*'` 與 release 頁面，再決定收回還是改發下一版 |
| `plan` 紅：`points at … not at the accepted commit` | 這個版本的 tag 已存在但指向別的 commit | 不動 tag；改發新版本 |
| `draft` 紅：`the run's artifacts are not the files 0.1.0 ships` | artifacts 過期（保留 7 天）或缺平台 | 在 7 天內做完 `draft`；缺平台就查那個 run |
| 任何階段紅：`… is already on v0.1.0 with other content` | 重跑時檔案內容不同 | 查為什麼不同；draft 才能刪單一檔案後重跑，公開後不改，改發新版本 |
| `publish` 紅：`the draft already has the manifest or its signature` | 有人提早上傳了 | 刪掉那兩個檔再 `publish` |
| `manifest` 紅：`the manifest on v0.1.0 is not the one these inputs write` | 重跑用了不同的 package run，或 `UNLISTED_TARGETS` 在兩次之間改過 | 用原本的輸入；真的要換內容就走第 8 步的收回 |
| `verify-manifest` 紅 | 簽章或內容不被 App 的驗證接受 | 第 1 步的失敗說明；沒有東西被上傳 |

## 7. 資料遷移框架

`app/crates/yuzora-core/src/migrate.rs`（touches yuzora-core，只新增）：

- **只讀**：beta.1 的檔案、localStorage、credential store 的內容都只讀不寫，回到 beta.1 仍可使用。唯一的例外是 SQLite 本身：localStorage 是 WAL 模式，`sqlite3 -readonly` 讀取時仍會在 `-shm` 記錄讀取鎖，但不會改動資料。
- **`Beta1`**：找出 beta.1 留下的資料。

  | 資料 | 位置 | API |
  | --- | --- | --- |
  | DB 連線 | `<data>/database-profiles-v1.json`（`version: 1`） | `db_profiles()` |
  | DB 密碼 | credential store，service `io.yuzora.database`，account `v1:18:io.yuzora.database:<len>:<descriptorId>:<len>:<generation>` | `db_password()` |
  | SSH host key | `~/.yuzora/known_hosts.json` | `known_hosts()` |
  | SSH 主機（`yuzora.hosts.ssh.v2`）、語系（`yuzora:language`，beta.3 起不匯入）、外觀、終端機、編輯器、按鍵 | WebView localStorage | `local_storage(key)` |

  `<data>` 是 Tauri 的 app data（macOS `~/Library/Application Support/dev.yuuzu.yuzora`、Windows `%APPDATA%\dev.yuuzu.yuzora`）。localStorage 在 macOS 是 WebKit 的 SQLite（`~/Library/WebKit/dev.yuuzu.yuzora/WebsiteData/Default/<hash>/<hash>/LocalStorage/localstorage.sqlite3`，值為 UTF-16LE），以系統內建的 `/usr/bin/sqlite3 -readonly` 讀，**只讀 top 與 frame origin 都正好是 `tauri://localhost` 的那份**：beta.1 的預覽窗格用同一個資料區開外部網頁，任何網站都能在自己的 origin 留下同名的 key（例如假的 `yuzora.hosts.ssh.v2`）。**Windows 是 WebView2 的 LevelDB，目前讀不到**：SSH 主機與 localStorage 裡的設定在 Windows 要使用者重新設定。beta.1 本來就不存 SSH 密碼與 passphrase。
- **`Importer`**：每條 lane 實作一個，`section()` 是它在 Yuzora `settings.json` 的 section，也是 `yuzora_core::secrets` 的 namespace。`import(from, into)` 讀 `Beta1`、合併進 `into.value()`（不要覆蓋使用者已在 beta.2 設好的內容）、用 `into.set_secret()` 存密碼，回傳 `Imported { items, left }`（`left` 是沒帶過來的項目，給使用者看）。
- **`run(importers, from, yuzora_dir, secrets)`**：每個 importer 只跑一次，記在 `<yuzora dir>/migrations.json`；失敗的下次再跑；`version()` 提高時重跑一次。沒有 beta.1 資料時什麼都不記。
- 測試涵蓋：讀 beta.1 格式（含 keychain account 格式）、執行一次與重跑、失敗重試、版本提升、其他 section 不受影響、beta.1 檔案位元組不變、macOS localStorage（以 sqlite3 建的 fixture）。

### 欄位對應（L3、L4、L6 合併後補）

| section | lane | 來源 | 待決定 |
| --- | --- | --- | --- |
| `db` | L3 | `db_profiles()`＋`db_password()`（profiles 已依 beta.1 的 journal 收尾：沒做完的刪除與移除密碼直接套用；沒存完的新增或修改放在 `unfinished`，importer 要匯入或在 `left` 說明，不可默默略過） | `target`（sqlite／postgres／mssql 欄位、`viaHost` 對到 L4 的主機 id、`transportMode`／`trustCert`）對到 L3 的連線格式 |
| `ssh` | L4 | `local_storage("yuzora.hosts.ssh.v2")`、`known_hosts()` | 主機欄位（`id`、`name`、`host`、`port`、`user`、`authKind`、`keyPath`）；known_hosts 格式 |
| `general` | L6 | `local_storage("yuzora:language")`（beta.3 起不匯入：介面只有英文） | 外觀、終端機、編輯器設定要不要帶 |

接線（總指揮）：在啟動流程的 worker thread 呼叫 `migrate::run(&[…各 lane importer], Beta1::locate().as_ref(), &paths::yuzora_dir(), &KeyringStore)`，把結果（`Imported.left`）用 toast 或設定頁顯示。`KeyringLegacy` 讀 beta.1 的 credential 時，macOS 會跳 Keychain 存取確認（不同 app 簽章），要在 UI 說明。

## 8. CI

### `app-package.yml`（新增）

- 觸發：PR 到 `main` 或 `beta2/integration` 且改到打包相關路徑、push 到 `main` 或 `beta2/integration`（同路徑）、`workflow_dispatch`。`app-ci.yml` 同樣涵蓋 `main`：integration 合併進 main 之後，main 上才有 `app/` 的 CI 與打包。
- 平台：macos-15（aarch64）、windows-2022（x64）、ubuntu-22.04（x64）、ubuntu-22.04-arm（aarch64）。
- 每個平台：`scripts/yuzora/package` → 以使用者的方式打開檢查（macOS：arch、ad-hoc 簽章、Info.plist、`--version`、DMG 掛載；Windows：靜默安裝、PATH、解除安裝項目、重裝、解除安裝；Linux：暫存 HOME 執行 `install`）→ 上傳 artifacts（7 天）。
- 之後 `manifest` job 跑 `bun test scripts/yuzora/`、產生未簽章的 `yuzora-update.json`。固定狀態名稱 `App (package)`。
- **不建 tag、不建 release、不讀任何 secret**，權限只有 `contents: read`。

### `app-release.yml`

- 只有 `workflow_dispatch`，沒有 push 觸發（push 的 run 用被推送 commit 上的 workflow 檔，見 §6.3 第 0 步）。階段、權限與檢查見 §6；契約（誰能寫、誰能讀金鑰、哪些 job 執行 repo 的程式）由 `app/scripts/yuzora/app-release.test.ts` 守著，在 App CI 的 Linux job 跑；`app-ci.yml` 的路徑清單包含 `app-release.yml`，所以只改 workflow 的 PR 也會跑到它。
- `verify-manifest/` 的測試（它編譯 `manifest.rs`，所以也跑 `manifest.rs` 自己的單元測試）同樣在 App CI 的 Linux job 跑。

### `app-ci.yml` 的快取與執行時間評估

L0 PR #187 最後一次成功的執行（run 41480446879）：

| job | 總時間 | 建置測試 | 測試 | 快取 |
| --- | --- | --- | --- | --- |
| macOS | 8 分 | 5 分 54 秒 | 16 秒＋yuzora crates 53 秒＋shot_pixels 5 秒＋check-mods 3 秒 | 未命中 |
| Linux | 6 分 41 秒 | 4 分 51 秒 | 17 秒＋packaging／docs 3 秒 | 未命中 |

- 快取的歸屬：GitHub 讓一個 run 還原自己分支、預設分支（`main`）與 PR 的 base 分支的快取。所以 push 到 `main` 還原不到 integration 上存的快取，合併後 main 的第一次 push 是冷建置，之後存下來，main 上的 push 與以 main 為 base 的 PR 才會命中。
- 冷建置只要 5–6 分鐘（依賴用 `opt-level=0`），快取能省的有限；但 repo 的 Actions 快取總量已經 10.5 GB（超過 GitHub 預設的 10 GB），新的大快取會把 Tauri 的 `rust-host`、`database-integration` 等快取擠掉。
- 建議：
  1. 只有 push 到 `main` 或 `beta2/integration` 才存快取（`SAVE_CACHES`），PR 只還原不存；而且只在精確 key 沒命中時存（key 是依賴與 workflow 檔的雜湊，不含原始碼）。兩個分支都還在用的期間會各存一份；integration 不再用之後，它的舊快取自然被淘汰。
  2. 觀察合併後的命中率；若 Tauri 的快取被頻繁擠掉，考慮 app-ci 只快取 `~/.cargo/registry`，不快取 `app/target`（冷建置成本只多幾分鐘）。
  3. `app-ci.yml` 的 Windows 只有一個 job（`windows-console`），只跑 `cfg(windows)` 的 console 視窗測試（`yuzora-core spawn::`、`yuzora-git exec::`、`yuzora-files fsops::tests::run_starts`），其餘 Windows 只在 `app-package.yml` 建置；建議發布前與大型合併後手動跑一次 `app-package.yml`。
- `app-package.yml` 刻意**不用快取**：四個平台的 release target 很大，會擠掉其他快取；它只在打包相關變更時跑。

## 9. 需同步的項目

原本由總指揮在 beta.2 正式發布前收斂的清單；下面標「已套用」的在 v0.1.0 發布基礎建設一併處理了，其餘待 release-prep：

- **官網（`site/`）**：
  - macOS 下載仍是 `Yuzora-macos-aarch64.dmg`（不變）。
  - Windows 由 `Yuzora-windows-x64.msi` 改為 `Yuzora-windows-x64-setup.exe`（`site/index.html`、`site/downloads.js`、`tests/site-downloads.test.js`）。已套用，但連結釘在 `v0.1.0`，何時進 main 見 §6.3 第 7 步。
  - 新增 Linux 下載（`yuzora-<version>-linux-x86_64.tar.gz`／`aarch64`；沒有固定別名，或新增一個）。
  - Gatekeeper 說明改成 macOS 15 的「強制打開」流程。
- **README**：安裝方式（DMG、NSIS、Linux tar.gz＋`install`）、Gatekeeper、SmartScreen、`yuzora` CLI、從 beta.1 升級的說明（資料遷移狀態）。L7 區塊已寫入摘要。
- **`docs/operations.md`**：
  - §4 簽章邊界：beta.2 不再用 Tauri updater 的 bundle，改為簽 `yuzora-update.json`；同一把金鑰的新用途要寫進供應鏈敏感清單（`app/scripts/yuzora/manifest.ts`、`app/src/yuzora/release/manifest.rs` 的 `TRUSTED_KEYS`、`.github/workflows/app-package.yml`、`.github/workflows/app-release.yml`、`app/scripts/yuzora/release.ts`、`app/scripts/yuzora/verify-manifest/`）。已套用。
  - §5 Stable／Beta 表格：beta.2 的 asset 清單（§6 表）。
  - §7 release workflow 階段、§7.3 固定檔名、§9 publish 後驗證：全部要為 beta.2 改寫。
  - MSI `ProductVersion` 編碼規則對 beta.2 不再適用（不出 MSI）。
- **AGENTS.md「Release platform support」**（已套用）：
  - 補一條：「beta.2 起 Windows 改用 per-user NSIS 安裝程式（`Yuzora-windows-x64-setup.exe`），不再出 MSI；Linux 提供 x86_64／aarch64 tar.gz。」
  - 「Stable 仍須 Tauri updater 簽章」改為「Stable 與 Beta 的更新 manifest（`yuzora-update.json`）都必須以 release 金鑰（minisign，key id `742456BCCDC22A24`）簽章，app 驗證不過就不更新」。
  - 「正式 build 不可使用 `--no-sign`」對 beta.2 不適用，可改為「打包 workflow 不讀 signing secrets；簽章只在發布步驟」。

## 10. 待辦與後續整合

**後續整合（合併後由總指揮串接）**

- L6：設定頁加「更新通道」選擇（寫 `update` section 的 `channel`，`stable`／`beta`），以及顯示目前通道。beta build 上這個選項沒有效果（一律只收 Stable，§3），顯示目前通道要用 `Channel::chosen()` 的結果，不能直接顯示 `update.channel` 的值，否則會在 beta build 上顯示「beta」、實際卻走 stable；可以從 `yuzora:core` 的 `getSetting`／`setSetting` 讀寫。
- L3／L4／L6：各自實作 `migrate::Importer`（§7 表），總指揮在啟動流程接上 `migrate::run` 與結果顯示。
- 發布 workflow（§6）：已實作。使用者要做的是在 Settings › Environments 建立 `yuzora-release` 並加 required reviewers（§6.3 第 0 步），第一個 dispatch 之前。

**待辦**

- 實機驗證 beta.1 → beta.2 的 Tauri updater 升級（macOS、Windows）。
- Windows：自我更新後同步「設定 › 應用程式」的版本；捷徑的 AppUserModelID；Authenticode（未授權）。
- macOS：DMG 背景與圖示位置（Yuzora 自己的美術）。
- Windows 的 WebView2 localStorage（LevelDB）讀取，讓 SSH 主機與設定也能從 Windows 的 beta.1 帶過來。
- `yuzora --version` 仍印 `herdr-app`；若要改，`update.rs` 的 `version_of` 要一起改。
- 評估 stable 通道改走 `releases/latest/download/yuzora-update.json`（不受 API 次數限制）。
