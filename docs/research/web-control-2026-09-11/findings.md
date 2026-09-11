# Findings: Yuzora 公開網頁控制與單一使用者綁定

## Research Question

如何讓使用者登入公開網址，綁定正在執行的 Yuzora ADE，從自己的手機或另一台電腦查看狀態與進行控制？

**需求以本次最後澄清為準：每個 ADE 綁定一位使用者。** 不包含多人協作、分享控制或團隊權限。先以同一人、一台 ADE、多個瀏覽器工作階段研究；不把「單一 owner」解讀成永久限制帳號只能擁有一台裝置。本文是研究與建議，尚未形成已接受 ADR，也未實作功能。

## Executive Summary

**可行。建議新增「公開 Web Control + 雲端登入／Relay + ADE 內 Remote Access」三個部分。** 瀏覽器與 ADE 都主動連向 Relay，ADE 不需要開放家用路由器的入站埠。命令抵達 ADE 後，經綁定、範圍與控制權檢查，呼叫現有 HERDR／Host services。

第一版提供登入綁定、ADE 在線狀態、Named Sessions／Spaces／Agent 狀態、既有 terminal 的即時畫面與明確接管。後續加入檔案及 Git。保留同一個真實 Host Runtime 與 Terminal Session；網頁 UI 是它們的另一個操作介面，各自的版面與未儲存文件不會自動共享。

現有 typed API、官方 terminal connector、序號校驗與 Host generation 可以重用；仍需新增雲端身分、裝置配對、網路傳輸與桌面／網頁間的控制仲裁。不能把既有 Demo 上線便視為完成，也不能直接公開通用 Tauri invoke。[E1–E8]

## Key Findings

| 現況 | 已核對的證據 | 對設計的影響 |
|---|---|---|
| 前端是 Tauri IPC client | `ipc.ts` 直接匯入 Tauri `Channel`／`invoke`；`herdrProvider.ts` 組裝本機及 SSH／WSL 路由。[E1,E2] | 新增明確的 web transport；不模擬 `__TAURI_INTERNALS__` 作正式遠端介面。 |
| HERDR 操作已有 typed boundary | `herdrIpc.ts` 包含 sessions、snapshot、events、workspace／tab／pane、terminal、agent read 等 wrapper。[E3] | 第一版從小範圍允許清單提供 API，維持現有 capability／schema gate。 |
| Terminal 已支援 observe／control | 原生 adapter 接到 `HerdrManager`；manager 啟動官方 connector，`send_control` 檢查 control mode。[E4] | 可重用執行引擎；網頁 owner 驗證與 connector ownership 是額外必需的層。 |
| 桌面預設會接管 terminal | `createHerdrTerminalTransport` 預設 `control`、`takeover=true`，也有明確 `takeControl()`。[E5] | 單一人多裝置仍有競爭；兩端的自動重開／resync 都必須尊重控制權。 |
| 已有防止錯誤重放的慣例 | Terminal 輸入失敗會丟棄未送尾端；FrameTracker 要求 first-full、連續 seq，重複 frame 忽略、缺口 resync。[E5,E6] | 網路層新增 request／stream generation；重連不能自動重送 terminal 輸入。 |
| 遠端 Host 不等於網頁帳號 | `ConnectionOwner` 是 hostId + generation；Rust `host_request` 在操作前後重查連線。[E7] | 保留 Host identity，另加 account／ADE device identity，兩者不能混用。 |
| UI 狀態部分留在前端 | Workspace tabs 存 localStorage；HerdrBridge 背景恢復限制在已開啟且身分相符的 workspace。[E8] | HERDR snapshot 不代表完整桌面狀態。網頁焦點不應直接強迫桌面切換檔案根目錄。 |
| 既有公開 Demo 是模擬資料 | Demo README 明確說明使用 in-memory transport，不連真實 host。[E9] | Demo 可提供元件參考，不能當正式連線能力的證據。 |
| 現有 tunnel 用於桌面 loopback | `host_tunnels::open` 綁定 `127.0.0.1:0`，持有 host generation 與資源 owner。[E10] | 這是桌面連遠端服務的轉送，不是瀏覽器登入或 ADE 配對服務。 |
| App 關閉會釋放連線資源 | `lib.rs` 關閉 Host／SSH、釋放 HERDR connector，保留 HERDR server／pane。[E11] | 第一版 Remote Access 隨 ADE 程序存活；睡眠、離線或退出不能承諾繼續控制。 |

所檢查的入口、依賴與 Rust 網路實作未提供本文需要的公網 owner binding／Relay。這是針對上述範圍的分析，並非對整個 repository 做功能不存在的形式化證明。

## Evidence and Sources

### 本機來源

研究基準為 2026-09-11 工作樹，HEAD `7370efd`。工作樹已有大量未提交修改，引用的是當時實際檔案，不代表該 commit 本身已包含全部內容。逐檔 SHA-256 記錄於 sidecar，方便後續檢查變動。

- **E1** — [src/lib/ipc.ts:1](/Users/yuuzu/HanaokaYuuzu/App/Tauri/yuzora/src/lib/ipc.ts:1)：Tauri invoke 與 Channel。
- **E2** — [src/lib/herdrProvider.ts:89](/Users/yuuzu/HanaokaYuuzu/App/Tauri/yuzora/src/lib/herdrProvider.ts:89)：Host call／stream，以及 `invokeHerdr` 的路由與世代檢查。host map 位於本模組；不是可直接搬到公網的授權來源。
- **E3** — [src/lib/herdrIpc.ts:48](/Users/yuuzu/HanaokaYuuzu/App/Tauri/yuzora/src/lib/herdrIpc.ts:48)：typed HERDR API。
- **E4** — [src-tauri/src/herdr_service.rs:42](/Users/yuuzu/HanaokaYuuzu/App/Tauri/yuzora/src-tauri/src/herdr_service.rs:42)、[Host HerdrManager:2346](/Users/yuuzu/HanaokaYuuzu/App/Tauri/yuzora/src-tauri/host/src/herdr_service.rs:2346)、[send_control:2548](/Users/yuuzu/HanaokaYuuzu/App/Tauri/yuzora/src-tauri/host/src/herdr_service.rs:2548)：Tauri adapter、官方 connector、control mode 檢查。
- **E5** — [src/terminal/terminalTransport.ts:128](/Users/yuuzu/HanaokaYuuzu/App/Tauri/yuzora/src/terminal/terminalTransport.ts:128)：初始 takeover、輸入佇列、release 與接管。
- **E6** — [FrameTracker:587](/Users/yuuzu/HanaokaYuuzu/App/Tauri/yuzora/src-tauri/host/src/herdr_service.rs:587)：full frame／seq 驗證。
- **E7** — [src/lib/runtimeIdentity.ts:1](/Users/yuuzu/HanaokaYuuzu/App/Tauri/yuzora/src/lib/runtimeIdentity.ts:1)、[host_request:510](/Users/yuuzu/HanaokaYuuzu/App/Tauri/yuzora/src-tauri/src/host_service.rs:510)：Host identity 與前後世代檢查。
- **E8** — [workspaceSession.ts:113](/Users/yuuzu/HanaokaYuuzu/App/Tauri/yuzora/src/state/workspaceSession.ts:113)、[HerdrBridge.tsx:49](/Users/yuuzu/HanaokaYuuzu/App/Tauri/yuzora/src/workbench/HerdrBridge.tsx:49)：前端持久化與背景焦點限制。
- **E9** — [src/demo/README.md:7](/Users/yuuzu/HanaokaYuuzu/App/Tauri/yuzora/src/demo/README.md:7)：公開 Demo 的模擬範圍。
- **E10** — [host_tunnels.rs:124](/Users/yuuzu/HanaokaYuuzu/App/Tauri/yuzora/src-tauri/src/host_tunnels.rs:124)：loopback listener。
- **E11** — [src-tauri/src/lib.rs:429](/Users/yuuzu/HanaokaYuuzu/App/Tauri/yuzora/src-tauri/src/lib.rs:429)：App lifecycle cleanup。
- **E12** — [workspace_trust.rs:336](/Users/yuuzu/HanaokaYuuzu/App/Tauri/yuzora/src-tauri/host/src/workspace_trust.rs:336)、[db_credentials.rs:122](/Users/yuuzu/HanaokaYuuzu/App/Tauri/yuzora/src-tauri/src/db_credentials.rs:122)：既有 workspace trust 與 OS keyring 使用方式。裝置憑證應用自己的 namespace 與生命週期，不能借用 DB descriptor。
- **E13** — [.yuuzu/CONTEXT.html](/Users/yuuzu/HanaokaYuuzu/App/Tauri/yuzora/.yuuzu/CONTEXT.html)、[ADR-0004](/Users/yuuzu/HanaokaYuuzu/App/Tauri/yuzora/.yuuzu/adr/0004-herdr-terminals-browser-only.html)、[ADR-0005](/Users/yuuzu/HanaokaYuuzu/App/Tauri/yuzora/.yuuzu/adr/0005-native-windows-opt-in-wsl.html)：Host／Session／workspace 定義、官方 terminal、Browser 範圍與 Windows／WSL 政策。

### 官方外部來源

以下六個來源均在本次直接讀取；外部內容摘錄與來源 URL 留在 sidecar。外部資料說明協定／平台行為，本專案架構選擇由上述程式證據與需求推導。

- **W1** — [Tauri IPC](https://v2.tauri.app/concept/inter-process-communication/)：Events 與 Commands 連結 Core／Webview；支持將 transport adapter 與實際 service 分開的分析，不代表提供公網 API。
- **W2** — [RFC 8252 §§6–8](https://www.rfc-editor.org/rfc/rfc8252.html)：Native OAuth 使用外部瀏覽器；public native client 使用 PKCE。若採桌面登入，應用成熟實作。
- **W3** — [RFC 8628 §§3、5](https://www.rfc-editor.org/rfc/rfc8628.html)：device code／user code、期限、輪詢、猜碼與 remote phishing 風險。可參考其配對互動；自有 ADE 配對流程不因此自動成為標準 OAuth Device Grant。
- **W4** — [RFC 9700 §§2.2、4.14](https://www.rfc-editor.org/rfc/rfc9700.html)：public-client refresh token 使用 sender constraint 或 rotation；token 權限及 audience 應受限。
- **W5** — [OWASP WebSocket Security Cheat Sheet](https://cheatsheetseries.owasp.org/cheatsheets/WebSocket_Security_Cheat_Sheet.html)：WSS、Origin allowlist、逐訊息授權、session 撤銷、流量限制、backpressure、避免記錄完整訊息與 token。
- **W6** — [Cloudflare Durable Objects WebSockets](https://developers.cloudflare.com/durable-objects/best-practices/websockets/)：可作 WebSocket server；hibernation 需處理記憶體狀態重建；outgoing WebSocket 不會 hibernate。

## Tradeoffs / Alternatives

| 選項 | 適配程度 | 主要代價／限制 | 建議 |
|---|---|---|---|
| 每台 ADE 自建 HTTPS listener，再設定公網 ingress／tunnel | 有固定私人部署時可行 | 憑證、路由與權限仍需實作；單靠 tunnel 不會得到帳號綁定、命令授權和 UI 同步 | 不作產品預設 |
| ADE 與網頁都連公開 Relay | 符合公開網址、單 owner、多裝置 | 要維運登入、bindings、在線連線及流量；Relay 位於資料信任範圍內 | **推薦架構** |
| WebRTC 直連，加信令與 TURN | 可能降低部分流量繞路 | 多出穿透與 fallback 狀態；真實網路直連率未測 | 第一版不引入 |
| 網頁直接操作 HERDR helper／socket | 僅覆蓋部分 runtime 能力 | 瀏覽器無法直接使用目前本機 IPC；仍缺 ADE owner、workspace 與 Host 路由語意 | 不當作 ADE 的公開介面 |

### Relay 部署選型

**建議先用一個可長期保持 WSS 連線的服務，收斂第一版生命週期。** 例如單一 VPS 上的 Rust／Tokio Relay；Axum 是待實作 spike 驗證的 HTTP／WS 候選，不是現有依賴或本輪完成的選型。登入交給成熟 OAuth／OIDC 整合；不自行實作密碼與 MFA 系統。

另一候選是 Cloudflare Worker + 每個 ADE 一個 Durable Object。ADE 和網頁皆連入同一個 object，從 object 視角兩條都是 incoming WebSocket，可採 server-side hibernation。不能改成 object 主動連 ADE 又假設仍可休眠；控制權、binding version 與連線標記也不能只放記憶體。[W6]

本輪未比較實際費率、帳號方案或測量地區延遲，不宣稱哪個部署成本最低。平台決定應由後續連線 spike 與維運偏好收斂；對外協定保持一致。

## Recommendation / Decision Criteria

### 1. 連線與控制路徑

```text
自己的手機／電腦瀏覽器
  │ HTTPS 登入、裝置清單；WSS 狀態／指令
  ▼
公開 Web Control + Auth／Binding + Relay
  ▲
  │ ADE 主動建立 WSS 443；同一連線雙向通訊
  │
Yuzora ADE：Remote Access coordinator（Rust）
  ├─ 驗證 owner binding、協定版本、操作範圍、世代、控制權
  ├─ 共用 HERDR／Host service 方法與事件輸出
  ├─ 本機 HERDR runtime
  └─ 後續接既有 SSH／選擇啟用的 WSL Host runtime

桌面 UI ─ typed IPC ─ 同一控制仲裁／service ─ HERDR
```

ADE 不公開通用 `invoke(command, args)`、`eval` 或 `host_request`。網頁不能指定任意 binary、helper、路徑或 client-supplied connection owner。正式協定使用有限的 typed request union，對應已有受支援的 service 方法。登入授權與 runtime capability 檢查都要通過。[E2–E7,W5]

Rust coordinator 可以先持有本機 `HerdrManager`，並以 callback／bounded channel 將 events 送往網路 adapter。既有 Tauri command 中的 `State`、`Channel`、`AppHandle` 是 adapter 邊界，需將必要的共用 service 呼叫抽出；不能假設整份 command 註冊表能原樣掛到 WebSocket。[E1,E4]

### 2. 綁定與登入流程

1. 使用者在 ADE 啟用「網頁控制」。ADE 向服務建立有期限的 pending pairing，保留不可公開的 challenge／device secret，顯示公開配對頁與短碼／QR。
2. 使用者在公開網站登入。網站驗證登入身分後，呈現正在配對的 ADE 名稱與一次性短碼；短碼不能單獨當永久控制憑證。
3. ADE 顯示即將綁定的帳號與裝置資訊，使用者在本機確認一次。服務以原子操作寫入 `adeDeviceId → ownerAccountId`，防止兩個帳號同時搶綁。[W3]
4. ADE 取得可撤銷的 device credential，存於 OS credential store 的獨立 namespace。雲端使用內部 account ID／驗證過的 provider subject，不用 email 字串作永久 owner key。[E12]
5. 後續從自己的瀏覽器登入即可選擇已綁定 ADE；正常操作不再逐項要求配對。解除綁定立即撤銷 browser access／device authorization、關閉連線並遞增 binding version。

這是產品配對設計，不把自有 pairing endpoint 冒充 OAuth Device Authorization endpoint。若選擇用 native OAuth 登入 ADE，使用外部瀏覽器與 Authorization Code + PKCE；若身份服務已提供正式 Device Grant，再按 RFC 8628 接入。[W2,W3]

Browser 建議使用同站的 server session，cookie 設 `HttpOnly`、`Secure`、適當 `SameSite`，握手校驗 Origin，配合 CSRF 防護。OAuth provider token 留在服務端；裝置 credential 留在 ADE。登入／解除綁定的撤銷須作用到已建立的 WS，不只阻擋下次登入。[W4,W5]

### 3. 單一使用者的控制權

**一位 owner 可以有多個 browser session，但每個 terminal target 同一時間只有一個 controller。**

網頁預設 observe，點「接管」才取得可過期的 control lease。Lease 綁定 `adeDeviceId + RuntimeKey + terminalId + clientSession + generation`；驗證與仲裁由 ADE 執行。只有 controller 可輸入、resize 與呼叫控制型 scroll。[E4,E5]

桌面目前預設 takeover，因此不能只在 Relay 加鎖：桌面與網頁的 open／takeControl／resync 必須共同檢查本機仲裁狀態。桌面保留明確的「接回控制」入口；瀏覽器分頁重整、桌面切頁或事件重訂閱不可自動來回搶權。HERDR 原生 observer／controller 行為仍是執行基礎，不被自訂 lease 取代。[E5]

接管成功後原 controller 降為 observer；lease 超時、瀏覽器登出或斷線時釋放其 connector。只釋放連線，不關 terminal tab、不停止 Agent。真正的 `tab.close`／`pane.close` 屬另一個明確操作，之後才擴充。[E11,E13]

### 4. 狀態同步與斷線語意

| 狀況 | 建議行為 |
|---|---|
| 首次連線／重新登入 | 確認 ADE online、binding／protocol／capabilities，再取得當前 snapshot，建立新的 stream。 |
| Terminal frame 序號有缺口 | 依現有 first-full／resync 語意重建 connector；不要繼續套用不完整 ANSI 差量。[E6] |
| Agent／Space event 中斷 | 重新取得 snapshot 校正，不把事件流當永久完整的資料庫。 |
| 指令已送但回應遺失 | 顯示結果未知；terminal input、檔案寫入、Git 等副作用不自動重送。 |
| Relay／ADE 重啟 | 新增 connection generation，舊 request／stream／lease 一律失效，重新查狀態。 |
| 手機切背景或換網路 | 逾時釋放控制，回來後刷新 snapshot；需重新接管，不能重播離線鍵盤佇列。 |
| ADE 休眠、退出或網路不可用 | 顯示 offline／last seen；控制 API 明確失敗，不把命令存成待上線執行。 |
| Browser 持續收不完 terminal 輸出 | 每 stream 有界 buffer，超限觸發 resync／斷流；慢 client 不阻塞桌面與其他 session。 |

每個訊息帶協定版本、request ID、目標 RuntimeKey 與 stream／connection generation；網路身分由已驗證連線導出。副作用 request 的短期去重紀錄由 ADE 持有。**去重不能宣稱跨 crash 的 exactly-once**：不確定結果需由使用者檢查狀態，不自動重試。[E5–E7,W5]

連線維持不代表命令永遠有權限：每次操作檢查目前 binding、允許範圍與 lease。HTTP／WS 尺寸、速率及待處理操作數需依現有 frame 大小和實測設定，不能照抄通用 64 KB 上限造成真實 terminal frame 無法傳送。[W5]

### 5. 第一版交付範圍

| 階段 | 具體成果 | 完成的驗收條件 |
|---|---|---|
| P0：傳輸與配對 spike | 真正 ADE 主動連 WSS、網站登入、一次性配對、撤銷 | 不開入站埠可連；第二帳號不能讀取／接管已綁定 ADE；錯碼、過期與重複兌換失敗；撤銷可切斷既有連線。 |
| P1：可用的網頁控制 MVP | 本機 Named Sessions／Spaces／Agent 狀態、既有 terminal observe／接管／輸入／resize、online／offline | 網頁和桌面觀察同一真實 runtime；兩個 browser tab 加桌面不搶權；斷線不重播指令、也不終止 Agent。 |
| P2：擴充工作面 | 既有已連線 SSH／WSL Host；檔案讀取／編輯／衝突；Git read-only 優先 | hostId／generation 不跨主機混用；讀寫限已註冊 workspace；舊 revision 不能覆蓋新內容；兩端事件可校正。 |

P1 已能在既有 terminal 裡繼續和 agent 互動、執行使用者命令。按 ADR-0004 維持 Terminal 操作，不另加 Agent catalog／`agent.start` 表單。完整 Git 寫入、Database、SFTP、Browser proxy 與桌面 OS 操作不納入此第一版；它們需要各自的授權和衝突規則。[E13]

P0／P1 仍須沿用既有三平台原生 runtime 政策。先驗證本機 ADE 是為了收斂入口；不移除既有遠端 Host，也不把目前尚未實測的 SSH／WSL 網頁控制列為完成。

### 6. 預計程式接入位置

以下名稱是候選實作位置，尚未建立：

| 位置／責任 | 必要工作 |
|---|---|
| 新 `src-tauri/src/remote_access/` | 配對／device credential、outbound relay client、allowlisted dispatcher、controller lease、撤銷／shutdown。只新增本輪需要的模組。 |
| `src-tauri/src/herdr_service.rs` + 現有 `HerdrManager` | 將必要 adapter 與可共用的 service 呼叫接好，保留官方 connector、capability gate、受控生命週期。 |
| `src/lib/herdrProvider.ts`、terminal transport boundary | 區分 desktop／web transport；把公網身分與 host binary 選擇保留在受信任 ADE 端；桌面 terminal 進入共同控制仲裁。 |
| 新 `src/web/` 與獨立 Vite entry | 登入、裝置／Space 清單、Agent 狀態、terminal。重用 domain types／合適元件與 shadcn；不將 native updater、dialog、clipboard 假裝成瀏覽器能力。 |
| 雲端 Relay package／service | OAuth／OIDC session、owner binding、device registry、WS 轉送、rate limit、撤銷事件。部署位置待選，不能假設靜態網站 host 提供這些能力。 |
| `src-tauri/src/lib.rs` lifecycle + Settings | Remote Access 啟用／關閉、online 狀態、解除綁定及既有 shutdown 順序整合。 |

Host 路由目前部分由前端 `hosts` map／preferences 管理；P2 需由 ADE backend 提供受信任的已註冊 Host 目錄與 binary policy，或明確同步該目錄。不能讓網頁傳回任意 binary／Host connection owner，也不能直接把 desktop localStorage 上傳就當權威。[E2,E7,E8]

### 7. 驗證規劃

未實作前不產生「測試已通過」的錯誤印象。後續至少需要以下行為測試：

- 配對與授權：同時搶綁、錯帳號 device ID、錯 Origin、過期 session、撤銷後既有 WS、跨 stream ID 操作均被拒絕。
- 控制權：桌面＋兩個 browser sessions 的 observe／接管／resize；失去控制後已排隊的輸入被丟棄；桌面 resync 不自動搶回。
- 復原：輸入送出後立刻斷線、Relay crash、ADE restart、睡眠／喚醒、失序／重複 frame、慢 client。確認不重送副作用且 Agent 存續。
- Native regression：既有本機 terminal、SSH／WSL generation、workspace trust、App cleanup 保持契約；依實際修改範圍跑 frontend／Rust checks。
- 真實操作：手機 IME、貼上、鍵盤遮擋、桌面與手機互切、外網實測。執行 browser／E2E 前需另取得本會話明確同意；本輪只有官方文件 HTTP 讀取與靜態分析，未啟動 browser automation。

延遲、吞吐、佇列上限與費用尚無量測，P0 應記錄實際網路下的輸入到畫面延遲、重連時間及高輸出記憶體使用，再訂 SLO；不先填入無依據的效能承諾。

## Risks / Unknowns

1. **公開可登入與可以操作 ADE 是兩件事。** 服務可能有多個帳號，但每個 ADE 只有一個 owner；需要隔離所有 device／stream／request，而非只把分享按鈕藏起來。
2. **Terminal control 是實際程序權限。** Owner 能輸入 shell 命令；workspace API 的路徑限制並不會把 shell 沙箱化。配對頁與接管動作應清楚表達授予「操作此 ADE」的能力。
3. **MVP 假設 Relay 是受信任服務。** TLS 分段終止於 Relay，服務技術上可讀取轉送內容；預設不持久化 terminal output／原始指令，審計只留事件種類、裝置、時間與結果。這不等於端到端加密。[W5]
4. 若要求 Relay 無法讀取內容，需要獨立規劃裝置驗證、金鑰配對／恢復與成熟 E2EE 協定；不能只把 `wss://` 改名為 E2EE，也不在本輪發明加密協定。
5. 瀏覽器與桌面 UI 的焦點、localStorage、未儲存編輯內容不是同一份狀態。P1 不承諾遠端鏡像整個桌面；要讓網頁操作桌面目前開啟的檔案／分頁，需要額外的 UI command／event 契約，並保留 unsaved guard。[E8]
6. ADE 退出但 HERDR server 仍在，不代表 Web Control 還在線。Headless daemon／背景常駐將改變目前 ownership 與 credential lifecycle，應單獨研究並形成 ADR。[E11,E13]
7. 登入 provider、公開域名、Relay 部署環境與資料保留政策尚未指定。本文不建立外部服務、不新增 GitHub issue、不部署網址，也不聲稱已有可使用的連線。
8. `Remote Access` 是 ADE 遠端控制服務，與 ADR-0004 移除的「靜態 Preview／Dev Server 管理」不同。若將工作區服務對外代理或加入新 Agent 啟動表單，需重新核對 ADR 範圍。

## Revisit Conditions

- 使用需求擴大為多人分享控制、團隊角色或多人同時編輯。
- 需要在 ADE 關閉時繼續連線，或雲端服務不得讀取 terminal 內容。
- 第一版必須同時覆蓋所有 SSH／WSL Host、Database、Browser 或完整桌面 UI。
- HERDR 的 controller／frame／schema 契約改變，或真實手機驗證發現目前接管／resize 語意不適合。
- P0 發現長連線維運、成本或地區延遲不符合目標，需要重新比較 VPS／Durable Objects／直連方案。

## Research Metadata

- Date: 2026-09-11, Asia/Taipei.
- Mode: approved-primary-source-fallback + current-worktree architecture analysis.
- Exa: unavailable; `doctor` 回報 `exaApiKey: false`。使用者已明確同意改用官方文件核對。
- Retrieval: 本機 knowledge graph → coverage／freshness check → exact source；外部官方文件透過 HTTP 直接讀取。沒有瀏覽器自動化，也沒有呼叫外部搜尋引擎。
- Research questions: 1；外部直接來源 6；本機 evidence groups 13。
- Graph: `Users-yuuzu-HanaokaYuuzu-App-Tauri-yuzora`，2026-09-11 full index；較舊 `yuzora` alias 的 metadata 發現過期後改用同工作樹的新索引，引用檔案已核對 freshness。
- Baseline: HEAD `7370efd`；研究前工作樹已有 458 個 status entries（83 modified、336 deleted、39 untracked）。保留現有修改；未以 commit 代替工作樹事實。
- Synthesis: 本報告由 Codex 根據已讀原始碼與官方來源撰寫；沒有 Exa server-side synthesis。附帶來源不是 Exa 回應，也不捏造為 Exa 調查結果。
- Validation: 核對來源引用、檔案存在、sidecar JSON 與報告結構。deep-research validator 對 fallback 缺少 Exa synthesis 的警告屬工具格式限制；不以其通過替代架構審查或 E2E 驗收。
- Deliverables: 本 Markdown、同目錄 `findings.raw.json`，及 `docs/html/web-control-research-2026-09-11.html`。
- Implementation status: research only；未修改產品程式、未進行部署或 Git staging／commit／push。
