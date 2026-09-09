# HERDR runtime 升級與相容性事故預防

研究日期：2026-09-09。這份文件是調查與建議，不代表下列中期方案已實作或 Windows／WSL 已完成驗收。

## 結論

本次應把 Yuzora 的 managed HERDR 與對應契約升至 **0.9.0／private protocol 22**，並讓既有 WSL host 能明確更新其 managed client／helper。單改安裝包內的版本不夠：現有 host 設定保存完整 binary／helper 路徑，重新連線會沿用這些路徑。更新動作必須保留來源選擇，且不能藉由停止既有 server、降版 server 或放寬 backend gate 達成表面恢復。[Y1][Y2][Y3]

HERDR 0.9.0 已建立穩定 endpoint generation 1 的另一套相容性契約；這值得中期評估，但它**不等於既有 private terminal connector 已獲得跨 protocol 保證**。短期維持現有 gate，新增可查證診斷、client 更新入口及混合版本測試，較能控制本次發布範圍。[H2][H3][H4]

## 已確認的事故證據

來源是使用者在此次對話中以 Yuzora 實際保存的 binary 路徑執行的唯讀輸出；尚未取得 installer SHA-256，因此不能宣稱安裝檔身分已完成驗證。

| 欄位 | 使用者提供的結果 |
|---|---|
| WSL 發行版／執行身分 | Ubuntu-24.04／WSL2／UID 1000 |
| Yuzora client | 0.8.2／protocol 20 |
| 執行中的 server | 0.9.0／protocol 22／running |
| `server.compatible` | `false` |
| Session | `default`，running |
| API socket | `/home/yuuzu/.config/herdr/herdr.sock` |
| binary 來源 | `~/.local/share/yuzora/runtimes/0.0.9-beta.3-linux-x86_64-fa8bc0288db5545d3355c37bb6ee8087e9dd3c9df15944c524eb5aeb6e7ee55a/herdr` |

這證明了舊 managed client 對上新 server，不能推論 server 過舊或資料遺失。舊 client 的 status JSON 未輸出新 endpoint 欄位，也不能據此推論 server 不支援 endpoint generation 1；需改用確定的 0.9.0 client 重新探測。[H2]

## 上游 0.9.0 的相容性契約

研究固定於官方 `v0.9.0`，tag 對應 commit `b99002ac99b09e00b4ca692436cb15a6b0d676f1`。release 發布於 2026-09-07。[H1]

### 三種訊號應分開

| 訊號 | v0.9.0 原始碼的精確語意 | Yuzora 應如何使用 |
|---|---|---|
| `server.compatible` | server private protocol 是否等於 client private protocol；缺資料為 null | 保留現有 private connector 的相容性 gate |
| `server.endpoint_compatible` | server 的 `capabilities.endpoint_protocol_generation` 是否等於 client 的 generation 1 | 診斷或未來 endpoint transport 使用，不可取代目前 gate |
| `server.restart_needed`／`update.restart_needed` | 執行中 server 的 endpoint generation 是否不等於 1 | 是上游 endpoint 升級訊號，不是強制停止 server 的命令 |
| `server.server_binary_stale`／`update.server_binary_stale` | server 與 client 的版本字串是否不同 | 顯示「版本不同」，不能翻成「server 過舊」 |

`client.endpoint_protocol_generation` 是 client 層級欄位；server 對應欄位在 `server.capabilities.endpoint_protocol_generation`。capabilities 也包含 `surface_interest`、`health_check`、`live_handoff`、`detached_server_daemon`。[H2][H5]

因此 `compatible:false`、`endpoint_compatible:true`、`restart_needed:false` 可以同時成立。任何單一布林值都不能代替 Yuzora 實際使用介面的完整契約檢查。版本字串相同也不能代替 schema／method／terminal codec 驗證。[H2][H3]

### 穩定 endpoint 的適用範圍

上游 `endpoint.rs` 明文區分 generation 與同安裝 CLI、direct-terminal、handoff 使用的 private binary protocol。generation 1 原則上永久保留，除非安全理由退役；新增 JSON 欄位應 optional 或有預設值，新增 enum 值應有 Unknown fallback，未協商為核心的未知控制訊息可忽略。[H3]

endpoint hello／welcome 協商包含 snapshot、surface、input、blob codecs、methods 與 capabilities。上游自動連線驗證採 generation；saved federation 另外要求 `surface_interest`。這是建立未來跨版本相容性的可靠方向，但接入它需要正式 transport/schema 遷移，不能只刪除 protocol 比較。[H3][H6]

Yuzora 現在使用 public NDJSON API 搭配官方 `herdr terminal session` connector。v0.9.0 的該 connector 仍經 `do_handshake` 並要求 `TerminalAnsi` encoding；沒有足夠證據支持它可直接套用 endpoint generation 的跨 private protocol 保證。[Y3][H4]

### 更新與事件也有行為變化

- 官方 release 說明相容 server 與執行中 agents 可以保留；缺少非核心功能應只停用受影響操作。generation 1 之前的 server 仍需一次升級。remote server 替換先詢問、預設 No，experimental live handoff 維持 opt-in。[H1]
- 官方 updater 是否需要重啟依 target endpoint generation 判定；它不是單純要求 client/server release 版本相等。[H7]
- lifecycle events 的新訂閱改為只接收 live events，官方要求先訂閱、再取初始 snapshot。Yuzora bootstrap 必須處理 subscription 建立與 snapshot 之間的競爭；若保留先 snapshot 的流程，至少在 subscription ack 後補一份新 snapshot，並確認舊請求不覆蓋新狀態。[H1]
- server restart 後可恢復保存的版面及支援的 agent conversation，但原本的 processes 不存活。不能把「Session 有持久化」當作安全停止工作的證明。[H8]

## Yuzora 為何會保留舊 client

以下為修正前候選 commit `7a7a5a34749e49f2f743f7090a16db37b8e2c818` 的程式行為；並非對同時進行的修正最終結果下結論。

1. `host_prepare` 根據 bundled manifest 建立 `<version>-<platform>-<manifest hash>` 目錄。helper 一律 managed；HERDR 是否選 managed 則由 `use_managed_herdr` 與 host 上偵測到的 installed binary 決定。[Y2]
2. immutable 檔案部署已驗 SHA-256，拒絕 symlink，先寫 scratch 再 hard-link promotion；這提供良好的並存版本基礎，不需要覆寫舊 binary。[Y2]
3. `hostStore.setup` 將回傳 binary/helper 的絕對路徑寫進 `yuzora.runtime.hosts.v1`。設定沒有獨立的來源政策或 artifact generation。[Y1]
4. `reconcile` 使用 `connectHost(..., config.helper)` 與 `registerRuntimeHost(..., config.binary, ...)`；重新開啟新版本 Yuzora 不會因 bundled artifact 改變就重新 prepare host。[Y1]
5. prepare 會要求 helper 執行 `HerdrStart`；現有 server 若 compatible=false，startup 會拒絕並保留 server。現行錯誤字串卻引導 stop/restart 每個 affected Session，與本次「client 比 server 舊」的情境不合，應調整說明。[Y2][Y3][Y4]

所以「路徑不變」不只是 cache；它在目前設計中扮演持久的 runtime 選擇。修復應分開**使用者選擇**與**該選擇本次解析出的路徑**。[Y1][Y2]

## 建議的短期措施

### 1. 保留 gate，讓失敗原因可見

顯示主機標籤、Session 名稱、client/server version、private protocol、完整 binary、socket、endpoint generation（存在時）。保留 raw host ID 作為可複製診斷資料，不用它當一般頁面標題。UI 分開 loading、unsupported/error、成功空集合、成功有資料；有舊 snapshot 時標示其取得時間及過期狀態，不寫「已載入最新快照」。這些是根據事故與契約提出的建議，不是上游 UI 要求。[H2][Y1][Y3]

### 2. managed client 更新應有明確入口

使用現有 host 工具更新流程，保持原先 managed／installed 選擇。舊設定若缺來源欄位，只能在確認路徑屬於受管理的 runtime root 與預期 manifest 格式時遷移為 managed；其餘應保留原設定，顯示來源未知或明確選擇，不能一律改成 PATH 或 managed。[Y1][Y2]

每次操作明示「更新此主機的 Yuzora 工具」，呈現目前與目標版本；安裝於新 immutable 目錄，驗證 helper hello、binary 身分、Session status/schema 後才持久化新選擇。至少保留上一個有效路徑。當 probing 或部署失敗，保留可診斷的錯誤及原設定。[Y2]

此入口可以作為本次候選的有界修正。它避免全面自動部署的行為擴張，但仍要求使用者主動更新一次；不能宣稱已自動避免所有未來版本漂移。

### 3. 更新 client 與停止 server 分離

此次 server 已是 0.9.0，目標是用新的 0.9.0 client 連回它。更新程序先列 Session、以新 client 讀取指定 Session 的 status，compatible 成立就重用 server。不執行 `server stop`、`session stop`、強制終止、WSL shutdown 或實驗性 handoff。若某個 named Session 不相容，單獨顯示其問題，不停止其他 Session。[H1][Y3]

helper／Yuzora connector 的重新連線可能造成短暫顯示中斷，應與 server pane processes 的存續分開驗證。[Y3][Y4]

### 4. 新候選必須驗既有環境

保存修正前候選的 localStorage host config、mixed-version status fixture 與 v0.9.0 schema fixture；測試從舊 managed path 遷移、來源保留、錯誤時不變成成功空集合。重新建置的 installer 需比對 SHA-256，並在同一 Ubuntu-24.04／UID 1000／default Session 中重新驗收。CI 綠燈不能代替這一項。

## 建議的中期措施

### A. 讓 host 設定保存政策，讓路徑成為可驗證的解析結果

建議未來的版本化 host config 區分 `sourcePolicy`（managed／installed／explicit）、`resolvedBinary`、`resolvedHelper`、`artifactIdentity`、`lastVerifiedAt`。這些是建議名稱，不是既有 API。[Y1][Y2]

managed policy 可在 app 升級後比較 bundled manifest identity，先顯示「主機工具可更新」；部署一律並存，只有驗證成功才切 active resolved paths。installed／explicit 不自動替換使用者 binary，但每次連線重驗身分與 compatibility。helper 有獨立 wire contract，不能因 HERDR 版本符合就略過 helper 驗證。

不要直接改成全域 mutable symlink：它會讓保存的 binary 身分在未驗證時改變，也弱化回滾與現有 immutable 部署設計。將 artifact manifest hash 作為實際身分，比只比較 app version 更可靠。[Y2]

### B. 維護明確的 runtime 支援契約

每個候選記錄 HERDR release/commit、artifact SHA-256、private protocol、endpoint generation（若使用）、必要 API methods、schema revision、terminal connector 能力。建置時交叉驗證 bundled resources 與 fixture；執行時按功能驗證，失敗的核心 transport 維持 gate。[H3][Y2][Y3]

若日後採用 stable endpoint，先以實驗 adapter 驗 snapshot、terminal input/resize/scroll、named Sessions、事件重連及未知欄位，通過後才能擴大相容範圍。feature gating 應建立在該 transport 的協商結果上，不能將「未知」一律當作「支援」。[H3][H6]

### C. 不把未知新版或可持久化 Session 當作安全保證

更高版本 server 不自動代表可相容；較低版本也不自動代表必須停掉。先依本次 transport 契約判斷。確實需要 server 更換時，另開使用者可理解的維護流程：列出 affected Session／pane／agent、保存可保存的工作、說明不能保證 process 存活、取得明確同意，再使用上游受支援流程。live_handoff:true 只代表能力存在，仍不等於本案已驗證無損遷移。[H1][H3][H8]

## 驗證矩陣

此表是待執行的驗收計畫；研究本身未操作任何使用者 runtime 或執行 browser/E2E。

| 場景 | 必須觀察的結果 |
|---|---|
| 舊 managed 0.8.2 client＋0.9.0 server | 明確 incompatible、呈現 20/22，保留 server、pane 與 Session |
| 新 managed 0.9.0 client＋原 0.9.0 server | compatible，連回同 socket；原 pane/process 仍存活，snapshot/terminal 正常 |
| 升級 app 但保留舊 host config | 不假稱 runtime 已更新；入口可把 managed 路徑更新到新 artifact |
| managed／installed／explicit 三種來源 | 更新或重連不暗中改變來源政策 |
| 新 client＋舊 server，private mismatch | gate 阻擋，不自動 downgrade、stop 或 handoff |
| endpoint compatible 但 private incompatible | 現有 private connector 仍拒絕；UI 不把 restart_needed:false 當可用 |
| API schema/method 不完整 | 受影響功能停用並給出具體缺項；不以版本字串繞過 |
| default 正常、named Session 不相容 | 只標示受影響 Session，其餘繼續使用 |
| 更新中斷、checksum 錯、helper hello 失敗 | 不啟用未驗證 artifact；原設定及 server 保留 |
| 過期 snapshot＋新請求 unsupported/error | 舊資料明確標過期，不顯示成功空集合 |
| lifecycle event 在 bootstrap/subscription 之間發生 | 初始資料最終一致；重連後無永久漏事件 |
| 重連期間舊請求晚於新請求完成 | 舊 generation 的結果不覆蓋新狀態 |
| 同機多 client 不同版 | 更新 Yuzora 工具不停止另一 client 的 agents；不假設都已相容 |
| 同一 Windows／WSL 升級驗收 | 保存 installer hash、distro、UID、binary/helper、status、Session list 及操作結果 |

## 主要來源

- [H1] [HERDR v0.9.0 官方 release](https://github.com/herdrdev/herdr/releases/tag/v0.9.0)：更新策略、事件訂閱語意、live handoff 範圍。
- [H2] [v0.9.0 `src/cli/status.rs`](https://github.com/herdrdev/herdr/blob/b99002ac99b09e00b4ca692436cb15a6b0d676f1/src/cli/status.rs#L248-L382)：status JSON 欄位與四項相容性/更新訊號。
- [H3] [v0.9.0 `src/protocol/endpoint.rs`](https://github.com/herdrdev/herdr/blob/b99002ac99b09e00b4ca692436cb15a6b0d676f1/src/protocol/endpoint.rs#L1-L145)：generation 1、private protocol 分界、codec 與 feature 協商。
- [H4] [v0.9.0 `src/client/terminal_sessions.rs`](https://github.com/herdrdev/herdr/blob/b99002ac99b09e00b4ca692436cb15a6b0d676f1/src/client/terminal_sessions.rs#L72-L120)：官方 terminal session connector handshake。
- [H5] [v0.9.0 `src/api/schema/server.rs`](https://github.com/herdrdev/herdr/blob/b99002ac99b09e00b4ca692436cb15a6b0d676f1/src/api/schema/server.rs#L17-L30)：server capabilities。
- [H6] [v0.9.0 `src/server/autodetect.rs`](https://github.com/herdrdev/herdr/blob/b99002ac99b09e00b4ca692436cb15a6b0d676f1/src/server/autodetect.rs#L145-L178)：endpoint generation 與 saved federation surface interest gate。
- [H7] [v0.9.0 `src/update.rs`](https://github.com/herdrdev/herdr/blob/b99002ac99b09e00b4ca692436cb15a6b0d676f1/src/update.rs#L835-L847)：server restart 判斷使用 target endpoint generation。
- [H8] [v0.9.0 `docs/next/README.md`](https://github.com/herdrdev/herdr/blob/b99002ac99b09e00b4ca692436cb15a6b0d676f1/docs/next/README.md#L31)：detach、恢復 Session 與原 process 存活的分別。
- [Y1] [修正前 Yuzora `src/state/hostStore.ts`](https://github.com/NakiriYuuzu/Yuzora/blob/7a7a5a34749e49f2f743f7090a16db37b8e2c818/src/state/hostStore.ts)：持久化 binary/helper、setup 與 reconcile。
- [Y2] [修正前 Yuzora `src-tauri/src/host_bootstrap.rs`](https://github.com/NakiriYuuzu/Yuzora/blob/7a7a5a34749e49f2f743f7090a16db37b8e2c818/src-tauri/src/host_bootstrap.rs)：manifest identity、artifact 校驗、immutable 部署、來源選擇與 prepare。
- [Y3] [修正前 Yuzora `src-tauri/host/src/herdr_service.rs`](https://github.com/NakiriYuuzu/Yuzora/blob/7a7a5a34749e49f2f743f7090a16db37b8e2c818/src-tauri/host/src/herdr_service.rs)：public API／terminal connectors、startup gate、既有 server 保留。
- [Y4] [修正前 Yuzora `src-tauri/host/src/server.rs`](https://github.com/NakiriYuuzu/Yuzora/blob/7a7a5a34749e49f2f743f7090a16db37b8e2c818/src-tauri/host/src/server.rs#L244-L253)：`HerdrStart` 呼叫 startup 流程。
