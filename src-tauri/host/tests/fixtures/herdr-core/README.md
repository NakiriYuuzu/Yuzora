# herdr-core fixtures

用途：`yuzora-host` 的 `herdr_core::` 測試以這些檔案 replay（seq 套用與 missed 重新快照、framed 編解碼與 credit、document 版本衝突、能力判定、attach 首繪與 credit 視窗）。`src/herdr_core/fixtures.rs` 是讀取它們的唯一入口（`BUNDLED`＝內建版本，`OLDER`＝較舊版本）。

## 版本與雜湊

| | 版本 | macOS arm64 archive SHA-256 | 解開後 `herdr-core` SHA-256 | 本目錄保留 |
|---|---|---|---|---|
| 內建（pinned） | 0.1.14（tag `v0.1.14`，commit `29e6d375`，generation 1） | `ca524329d2e0630219550055dcc71f42aafd85fba356e0d543d6f51244487970` | `29e9184ec5094d302b130983ebc13c3210f04470233393f16e4a91ecb167a307` | `v0.1.14/` 全部 22 個檔 |
| 前一版 | 0.1.13（generation 1） | `fc1ffba3820b93b0e2faa5394a64b9ff5528f135611b6034469d81c0fa1c37aa` | `47b9c1e5ba99bc42590ee4fc18cb658cd88a5e9c75b7d9e08e94e8fef32d4d80` | 只有 `pins.json` 的 `previous`；錄製檔不保留 |
| 較舊（共存／升級測試） | 0.1.11（generation 1） | `cd314925cd848e222b5e23561a751b2377a2ae3874ff0110463ad486baf8eff4` | `38cc20dbd79ea278d8ac1c59b110d61f682311168a04d7f49e0ed9e9846ce46a` | `v0.1.11/` 三個檔（見下） |

完整 pin（含 Windows archive 雜湊，以及 Windows CI 解開後的檔案清單、SHA-256、大小與 Authenticode 狀態）見 `pins.json`。Windows 的錄製沒有放進本目錄：replay 測試只用 macOS 錄製（Windows 的 `rpc.discover` 與 macOS 逐字相同）。所有 archive 都經上游 manifest（`https://releases.herdr.dev/core/<版本>/manifest.json`）取得並比對 SHA-256 相符；上游 GitHub 只有 tag、沒有 release 資產。

0.1.14 與 0.1.13 在 API 與行為上沒有可觀察的差異（只修 Linux 舊版 systemd 啟動問題）；`hello` 只有 `version`、`core.id`、`core.name` 不同，`rpc.discover` 只有 `info.version` 不同。

## 錄製方式（隔離）

- 錄製日期：0.1.14 為 2026-10-30（台北，P0 S4）。0.1.11 的三個檔與 `released.openrpc.json` 來自同日更早的 S8 錄製。
- 錄製者：`spikes/gpui-native/s8-herdr-core` 的 `s8-probe`。只執行下載的副本，一律經隔離 wrapper：`env -i` 白名單，不傳任何繼承來的 `HC_*`／`HERDR_*`；`HOME` 與各 `XDG_*` 目錄在 `/tmp` 底下，`HC_SESSION=p0s4`、`HC_SOCKET=/tmp/yz-hc-p0/s4.sock`；解析出的 socket 不在 `/tmp/yz-hc-p0` 下就拒絕執行。絕不連使用者自己的 core。
- events 類錄製時 core 以 `HC_HEARTBEAT_MS=1000` 啟動，所以 lifecycle 檔裡有 heartbeat。
- 指令（probe 經 wrapper）：`s8-probe discover`、`attach`、`events`、`status`、`docs`、`keep`、`restart-seq --phase before`→`hc server restart`→`restart-seq --phase after`、`credit-only`、`kitty-reply`。
- 原始證據、wrapper、腳本與比對結果：`.yuuzu/eval/herdr-core-p0-2026-10-30/s4/`。

## 檔案

- `released.openrpc.json`：上游 `schema/released.openrpc.json`（Apache-2.0）。`caps.rs` 的 `released_openrpc_is_a_subset_of_every_recorded_core` 拿它核對內建 core 宣告的方法。
- `pins.json`：各版本的來源、雜湊與 Windows 檔案清單。`bundled`＝0.1.14、`previous`＝0.1.13、`older`＝0.1.11。
- `v0.1.14/hello.json`、`rpc.discover.json`（完整 response 訊息）、`methods.json`（方法清單 32 個＋generation＋features）、`terminal.list.json`、`server.status.json`。
- `v0.1.14/*.ndjson`：第一行是 header（`fixture`、`version`、`binary_sha256`、`platform`、`recorded_by`、`description`），之後每行一則：
  - plain：`{"conn","dir":"out|in","mode":"plain","msg":<JSON-RPC>,"t_ms"}`
  - framed：`{"conn","dir","mode":"framed","channel","kind":"DATA|CREDIT|CLOSE|RESIZE|FOCUS|FACTS","len","t_ms"}`，加上 `msg`（channel 0 的 JSON-RPC）、`b64`（原始 payload）或 `elided:true`（每條 stream 超過 64 KiB 後只留長度）；CREDIT／RESIZE 另附 `credit`／`size`。
  - 註記：`{"note":{...}}`，例如 `final_list`（錄製結束時 core 自己的 `terminal.list {all:true}`，replay 結果要和它一致）。
- 檔案用途：`attach-basic`（first paint、輸入、RESIZE、FACTS、detach）、`attach-history`、`attach-observe`、`attach-close`（exited 3／killed）、`attach-credit`（6 MiB flood：準時 credit 的 P，與從不 credit、socket 約 1.5 s 未讀的 observe viewer S；S 的重繪出現在補 credit **之前**，觸發原因是 socket 未讀造成的落後）、`attach-credit-only`（6 MiB flood：viewer R 持續讀 socket（間隔 ≤ 4 ms）但不送 CREDIT，flood 結束後才補；補 credit 後第一批 DATA 就是重繪；補 credit 後保留 16 KiB 原始 payload）、`attach-kitty`、`attach-kitty-reply`（Kitty `a=q` 由 viewer 回覆：typing＋observe 時程式只讀到 1 次回覆，兩個 typing viewer 時讀到 2 次）、`events-lifecycle`、`events-missed`（plain subscriber 停讀 → 兩次 `missed`）、`events-keep`（SIGKILL → lost signaled → restarted）、`events-restart-before/after`（core 重啟後 seq 歸零）、`status-7501`、`docs-conflict`、`docs-limits`、`plain-session`。
- 時序相關的錄製（`attach-credit`、`attach-credit-only`、`events-lifecycle`、`events-keep`、`events-missed`）每次錄的數字本來就會變動；`events-keep` 裡 `foreground` 事件出現在 `size` 與 `lost` 之間與否是 race。測試不能依賴這些數字，只能依賴不變量（credit 視窗、seq 規則、最終 list）。

### `v0.1.11/` 為什麼只有三個檔

只保留 `hello.json`、`rpc.discover.json`、`methods.json`。0.1.11 只用於能力判定（`MIN_VERSION`）：`caps.rs` 的 `recorded_hello_and_discover_agree_for_every_pinned_version` 讀這三個檔，`gate_accepts_recorded_cores_and_refuses_bad_ones` 讀 `hello.json`，沒有其他測試碰 0.1.11。封存的 0.1.11 錄製另有 `plain-session.ndjson`、`server.status.json`、`terminal.list.json`，沒有任何測試讀取，0.1.11 也不再參與 attach／events 的 replay，所以不補回；真的需要時，依「錄製方式」在隔離 session 對 0.1.11 副本重錄。

## 後處理（僅此兩項）

1. core 的主機名稱（`core.name`）一律改成 `s8-host`（避免把錄製機器名稱放進 repo）。處理後逐檔確認文字與 base64 payload 內沒有主機名稱（`.yuuzu/eval/herdr-core-p0-2026-10-30/s4/raw/postproc-0.1.14.txt`）。
2. `docs-limits` 的 1 MiB／1 MiB+1 兩筆大型 `doc.set` 與讀回大文件的 `doc.get` 走未錄製的連線，結果以 note（`accepted_bytes`、`refused_bytes`、`refused_error`、`big_doc_*`）保存。

## 重錄

升級 pin 時：下載新版本（需使用者授權）→ 更新 `pins.json`（`bundled` 換成新版，原本的移到 `previous`）→ 把 `fixtures.rs` 的 `BUNDLED` 改成新版 → 在隔離 session 重跑上列指令 → `cargo test -p yuzora-host --lib herdr_core::` 必須全綠。
