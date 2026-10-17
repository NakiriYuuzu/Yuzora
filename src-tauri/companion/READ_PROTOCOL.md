# Companion WS v1 — Phase 3B-1 read / 3B-2 write paths

範圍只含本機 owning host；所有方法仍走既有 registry scope。3B-2 已加入 Control／Full／input／agent／audit；`push.subscribe` 留 Phase 7。沒有 raw Herdr request forwarding，也不啟動 server/session。

## 方法

先完成 cookie＋首 frame binding；`session.ready` 後才可呼叫：

```json
{"id":1,"method":"sessions.list","params":{}}
{"id":1,"result":{"sessions":[{"name":"work","default":true}]}}

{"id":2,"method":"snapshot.get","params":{"session":"work"}}
{"id":2,"result":{"session":"work","workspaces":[{"id":"w","label":"Work","status":"unknown"}],"tabs":[{"id":"w:t","workspaceId":"w","label":"Tab"}],"panes":[{"id":"w:p","tabId":"w:t","workspaceId":"w","status":"working","label":null,"layout":{"cols":120,"rows":40}}],"agents":[{"paneId":"w:p","name":"Agent","agent":"agent-kind","status":"working"}]}}

{"id":3,"method":"events.subscribe","params":{"session":"work"}}
{"id":3,"result":{"stream":1,"subscribed":true}}
{"event":"agent.status","data":{"session":"work","pane":"w:p","workspaceId":"w","status":"blocked"},"stream":1}
{"event":"pane.exited","data":{"session":"work","pane":"w:p","workspaceId":"w"},"stream":1}

{"id":4,"method":"terminal.observe","params":{"session":"work","target":"w:p","cols":1,"rows":1}}
{"id":4,"result":{"stream":2,"cols":120,"rows":40}}

{"id":5,"method":"stream.close","params":{"stream":2}}
{"id":5,"result":{"stream":2,"closed":true}}
```

- Sessions 是 `init.sessionAllowlist ∩ list_sessions.running`。不在 allowlist：`session-not-allowed`；已停止／不存在：`session-stopped`。錯誤不含 raw stderr、socket path 或啟動建議。Host 的 `live` alias 不得越過 exact named-session allowlist。
- Snapshot 是封閉 camelCase projection；不轉發 cwd/path、commands、tokens、state labels、raw terminal titles 或任意後端欄位。未識別狀態為 `unknown`；缺欄位為 null／空陣列。
- 每 peer／session 僅一個 events subscription；重複成功訂閱回同一 stream（尚在 opening 時回 `busy`）。只傳 agent status change 與 pane exited，其他 Herdr event kinds 一律不轉發。
- Events、observe、control、full 共用每 WS **8 streams**。stream ID 是該 WS 的非零 u32；跨 peer 不可使用，重連失效、不重播。`stream.close` 可關閉兩種 stream；未知 ID 回 `stream-not-found`。
- `hello.result.capabilities` 宣告17個已實作方法與limits（不含push.subscribe）；授權仍以各方法scope即時判定。Client binary只接受本peer已開Control/Full stream，見下節，不是View輸入通道。

## Snapshot chunk 協定：base64-json-v1

正規化 snapshot JSON UTF-8 **≤48 KiB** 直接回傳；超過時：

```json
{"id":2,"result":{"chunked":true,"encoding":"base64-json-v1","bytes":60000,"chunks":2}}
{"event":"snapshot.chunk","data":{"id":2,"index":0,"data":"BASE64..."}}
{"event":"snapshot.chunk","data":{"id":2,"index":1,"data":"BASE64..."}}
```

每 chunk 是原始 JSON UTF-8 bytes 的最多 **36 KiB** 片段，以 standard padded base64 編碼（data ≤48 KiB；完整 envelope <64 KiB）。以 request ID 關聯；index 從 0 連續到 chunks−1。先逐片 base64 decode、串接，再核對 bytes、解析 UTF-8 JSON；**不要逐片做 UTF-8 解碼**。同一 peer 的並行 request IDs 必須由 client 保持唯一。中斷／撤銷時丟棄未完成結果，重連重新 request。

Gateway 正規化結果上限 **2 MiB**；另受現有 Host **1 MiB raw NDJSON**／IPC complexity bounds 約束。兩層 size rejection 都映射 `payload-too-large`，不是 slow-consumer 1013。大型 observe frame 超過 65532 ANSI bytes（另加4-byte header）也只關該 stream，reason=`payload-too-large`。這不承諾所有 2 MiB raw Herdr response 都能通過 Host。

## Observe binary、尺寸與停止

Server binary message：`u32 BE streamId ‖ ANSI bytes`。每個 Herdr ANSI frame 保留一筆 message，不做無條件丟幀或 replay。open reply 先於該 stream 的資料。Client 可依回傳 cols/rows 設 xterm，再縮放／平移。

`cols/rows` 即使由 client 指定也完全忽略。target 可用該 snapshot 的 pane ID 或 terminal ID，gateway 解析為 pane ID，取官方 snapshot 中該 pane layout rect 的正數 u16 width/height（重複 rect 取各維最大）。缺有效 rect 回 `geometry-unavailable`，不猜手機尺寸。

**Herdr 0.9.1 無找到 actual PTY cols/rows 官方來源。**逐項檢閱 Spike C `schema.txt`：`PaneLayoutRect` 是 layout 幾何；`PaneScrollInfo.viewport_rows` 只有捲動 viewport；`PaneGraphicsPlacementParams.grid_cols/grid_rows` 是 graphics placement，`PaneReadResult` 是文字內容而非 PTY 尺寸。observe frames 固定為 request 尺寸，小 request 會裁切（`observe-b.decoded.txt`）；`geometry-small.txt` rect 120×40 但 `pane-small.txt` stty 30×44，證明 rect ≠ PTY。此策略至少覆蓋 layout rect，**不保證覆蓋被其他 direct client 放大的 PTY**。

`terminal.closed` 三筆精確 0.9.1 fixture 在 `tests/fixtures/herdr-0.9.1-closed.json`（來源 Spike C control-conflict/control-a/observe-b）：owner conflict → `owned-elsewhere`；`terminal attach taken over` → `taken-over`；`server is shutting down` → `session-stopped`；其餘 → `stream-closed`。不自動 takeover／retry／start。

**TOCTOU 限制（parent 核准保留）：**最後一次 running check 與 connector 實際 connect 之間仍可能停止。Host 在 spawn 後即回成功，稍後 stderr callback 無 typed session state；此 race 安全關 stream，但回 generic `stream-closed`，不靠 stderr 猜狀態、不曝露啟動建議。PWA 應重新呼叫 `sessions.list` 取得最新狀態，不自動重連／啟動。callback 後有界 re-list 列後續改善，非本輪已實作。

## 背壓、ownership、撤銷

- 保留 WS global16／device2、每 WS outbound4 MiB（queued＋in-flight、frame overhead、library buffer reservation）、RPC queue256、20s stalled writer→1013。
- 每 stream ≤256 queued＋in-flight frames，另 **384 KiB** stream byte budget；8 streams 最多3 MiB；stream sinks 與 chunked snapshots 共用 bulk semaphore，但不能消耗獨立 **128 KiB reply／ping headroom**。小型 replies／stream.closed／ping 先用 headroom、不足才用 bulk；兩者加 library reservation 仍≤4 MiB。snapshot 先入列再被 streams 補滿，亦不會吃掉這段預留。
- Observe／events overflow：丟棄該已關閉 stream 的待送資料、release resource，送 `stream.closed{stream,reason:"slow-consumer"}`；其他 stream 與 WS 留存。全部 TCP 都無法寫入時仍受既有 socket 20s／1013 界限，close frame 為 best-effort。
- 每peer仍≤2 pending read results；新增每device≤2 active read calls（含orphan，job結束才歸還permit）。gateway-owned JoinSet分隔global read4／write2／cleanup2 slots；每device write1，peer write queue16，read orphan不佔write／cleanup slots。超額回busy，不排無界工作。128 resource permits界定active/pending資源與cleanup waiters；Drop不在reactor join Host threads。
- Peer 以 select 持有 pending results、stream queues 與 RAII leases；callback 及每次 WS write 前查失效 watch。4401／socket close／shutdown 立即停止新的輸出、清除排隊資料；已交給 OS 的 bytes 無法撤回，不能聲稱撤銷會抹除已送出的 terminal 内容。
- **取消 ≠ 終止執行中的同步 Host call**（parent 明確核准）：call 被 orphan，回來後不發布任何結果／frame，成功取得的 connector/subscription 以 RAII 排入 bounded release。Host 15s CLI／5s socket timeout 是每次操作，串行 discovery 可能更久；cleanup 使用獨立2-slot executor。shutdown保留500ms server／1s main join，最終 executable exit不等待blocking thread。
- Host 內部 stderr reader 尚無 join handle；companion 不新增 detached task，但不能宣稱 Host 全 thread 已 join。fake PID／socket EOF 測試證明本機正常與 orphan 結束路徑最終回收，不替代所有OS／不合作binary的程序終止證明。後續另案：Host cancellable async API、保存並 join stderr reader。

## Process environment

同步 `main` 在任何 runtime/thread 建立前刪除所有繼承的 `HERDR_*`；保留 production HOME/XDG。Library LocalHerdr 遇未清 env 時 fail-closed，不在多執行緒中改全域 env。Host 只對子程序有意加入選定的 `HERDR_SESSION`。測試 launcher 清全部繼承 HERDR_*；gateway subprocess 測試另注入非真實路由的 poison values、隔離 HOME/XDG，驗 fake binary 僅收到合法 HERDR_SESSION。

## 3B-2：Control / Full Session access

| RPC | params / 行為 |
|---|---|
| `terminal.control` | `{session,target,takeover?:false,confirmResize:true,cols?,rows?}`。client尺寸仍忽略；0.9.1沒有actual PTY來源，因此無confirmResize必回`size-confirmation-required`。確認文字必須說「桌面pane可能reflow，release不恢復尺寸」。確認後使用該pane layout rect。takeover須Control elevation≤120秒；否則`fresh-elevation-required`，不自動重試／搶回。 |
| `terminal.input` | `{stream,text}`或`{stream,bytesB64}`，恰一格式、decoded≤4096 bytes。成功回`{stream,bytes}`。 |
| `terminal.scroll` | `{stream,direction:"up"|"down",lines:1..1000}`，Control。 |
| `terminal.release` | `{stream}`，即時失效、清未執行input、排入owned cleanup；不restore PTY；僅已announced才送control.state(open:false)。 |
| `agent.prompt` | `{session,target,text}`，UTF-8≤16384 bytes；target解析為allowlisted running session內的agent pane id。 |
| `pane.sendKeys` | `{session,target,keys[]}`，1..32 keys：Enter、Escape、Tab、BackTab、ArrowUp/Down/Left/Right、Ctrl-C、Ctrl-D。 |
| `full.open` | `{session,cols,rows,confirmResize:true}`，cols20..400／rows8..200。Full policy＋boolean grant＋≤120秒fresh elevation；每device最多1個（含opening／cleanup）。先實際release本peer同session的control、開本機official client PTY成功後，才送admin full.state(open:true)。 |
| `full.input` | 同terminal.input格式／4096-byte上限；只能使用本peer Full stream token，不能用Control token。 |
| `full.resize` | `{stream,cols,rows,confirmResize:true}`，與open同尺寸界限；每次確認幾何改變。 |
| `full.close` | `{stream}`，釋放official client；僅已announced才送full.state(open:false)。 |

Open回`{stream,cols,rows,until,target}`（Full target=null）；streamId是WS-local不可跨peer／重連使用的token。`stream.close`只關observe/events；write stream請用對應release/close。release/close是收緊，僅需仍有效的View登入＋本peer正確stream type，不依賴已過期elevation或audit queue空間。

**Host API裁決：**Host無pane.send_keys封閉feature，經parent核准RPC `pane.sendKeys`映射到`HerdrFeatureRequest::AgentSendKeys`／Herdr `agent.send_keys`。0.9.1只接受有agent的pane，因此兩個agent methods都檢查snapshot agents membership，否則`no-agent`；非snapshot target回`target-not-found`。keys映射為enter/esc/tab/shift+tab/up/down/left/right/ctrl+c/ctrl+d；Herdr pane.send_keys／pane.send_input／pane.send_text仍不可達。明確mapping見`herdr.rs::METHOD_MAPPING`，測試逐項走102個schema request methods（HERDR 0.9.5；0.9.2–0.9.5 相同，2026-10-17 由 0.9.1 的103個換成）；額外API只有Host compatibility ping、session.snapshot、events.subscribe、agent.prompt、agent.send_keys。完整client是高權限互動PTY，不把schema deny list誤當成PTY內所有shell動作的限制。

### 授權、queue與期限

- 每個新增寫入皆以with_authorized(Control|Full)同步try_send到peer-owned16-slot queue；單peer一次只送一個job到gateway-owned blocking executor。所有Herdr facade呼叫前再查peer invalidation／stop及同一stream不可復用的validity state（epoch）；control/full既有stream另查immutable deadline。無detached task／無retry。
- Control stream deadline=min(elevation,grant,login absolute,enableUntil)。Full elevation validity改為≤policy.elevationMaxMin（最多15分鐘）、absolute與enableUntil；freshness另存full_at，只在open查≤2分鐘（排队到實際呼叫前再查）。已開stream保留當時deadline；再提權不自動延長它，2分鐘後仍可輸入，最長剩餘13–15分鐘，視開啟時間而定。Full grant是boolean，沒有捏造until。
- Peer select持有Clock::until timer（每秒重驗wall-clock跳動）；自然expiry無需下一RPC，主動invalidate／release，WS `stream.closed{stream,reason:"elevation-expired"}`。4401、shutdown、socket close亦清所有資源。新增write的grant變更仍沿用identity失效watch。
- Binary input：`u32 BE streamId || raw bytes`，先驗本peer stream存在且type=Control/Full，再with_authorized相應scope；短header／0／從未發出的ID／仍在map內的observe/events token→1003。`0 < id < next_id`且已不在map的token一律回`{id:null,stream,error:{code:"stream-closed",message:"stream-closed"}}`，不關WS、不寫Herdr；包含已關的observe/events，刻意不保存無界歷史類型。合法token的scope/size錯誤回`{id:null,stream,error}`；成功回`{id:null,result:{stream,bytes}}`。既有50message/s與64KiB transport上限不變，input另≤4KiB。
- 所有mutation以`started.store(true, SeqCst)`之後的最後一次validity check為保守邊界；取消側先SeqCst失效、再load started。最後check未通過但started已設，仍回unknown（可能其實未送），絕不把可能已送出誤報dropped。watchdog亦會失效該job，含沒有stream的prompt／keys，防止discovery逾時後才遲到執行。尚未開始的input取消：`input-dropped`；已開始且結果未交付：`input-outcome-unknown`，**不代表沒執行**。內容無關的錯誤在close前僅best-effort送出；4401／斷網未收到error，也必須視為結果未知，重新檢查pane，使用者自行決定，不自動重送。已執行的OS bytes不能撤回。
- Parent核准的實際boundary是**Host facade呼叫前**。Host同步方法內尚有discovery＋IO，無cancellable public API；feature socket timeout可達135秒（不是read階段的5秒），serial CLI discovery亦可能超過單次15秒。進Host後的撤銷無法中止；late open資源由Lease回收。cleanup2 slots避免read orphan阻塞release，但仍不能承諾Host卡住時立即完成物理release；state(false)代表gateway邏輯所有權已失效。

**取消／10秒watchdog／4401對照**（`stream`始終存在於非同步write error；非stream mutation為null）：

| Method | started=false | started=true |
|---|---|---|
| `terminal.input`、`full.input`（含binary） | `input-dropped` | `input-outcome-unknown` |
| `agent.prompt`、`pane.sendKeys` | `write-dropped` | `write-outcome-unknown` |
| `terminal.scroll`、`full.resize` | `write-dropped` | `write-outcome-unknown` |
| `terminal.control`、`full.open` | `write-dropped` | `write-outcome-unknown` |
| `terminal.release`、`full.close` | 不走write job：同步logical close ACK | 不等physical cleanup；不是mutation delivery receipt |

input名稱為相容性保留；其餘統一write-*。**PWA不得自動重試任何mutation**（包括dropped、unknown、Host error、缺ACK與斷線）；unknown必須先由使用者核對pane。只有明確的read busy可依read規則重試。成功開啟後的audit-unavailable／Host內容層錯誤只回該次write error，stream保持開啟；expiry／takeover／watchdog等必要關閉一律state.close→WS stream.closed，不靜默拆stream。

### Audit / 桌面事件

Admin：`audit{deviceId,peerId,scope,action,session,target,summary,at,reason?,outcome?}`；peerId與session.opened一致，無cookie或binding。prompt summary=`{prefix:前200 Unicode chars,length:char count,bytes:UTF-8 length}`；keys只記數量，**不記按鍵值**。input按stream每10秒聚合`{bytes}`，close時flush尾桶；bytes是已接納的寫入嘗試量，**不是delivery receipt**。第一筆先reserve admin queue slot保證尾桶可送；**分開的semaphores：input buckets最多12個（optional）、open announcements最多4個（mandatory），合計仍占48 ordinary slots之內**，input超額改為該次同步try_emit(bytes)，失敗不呼叫Host mutation。即使已有桶，ordinary audit容量耗盡仍拒絕新input。取消另記input.cancelled／write.cancelled與outcome=dropped/unknown。

- 新open/takeover/input/prompt/keys/scroll/resize使用非阻塞try_emit／reserve，full/closed queue→`audit-unavailable`且不呼叫Host mutation（read discovery可能已做）。open在同一步audit admission額外預留state(true)的OwnedPermit，計入獨立4個announcement permits，不與12個input buckets競爭；預留失敗在Host open前拒絕，成功則不因Host期間queue填滿而丟失announcement。Host失敗釋放permit、不送state。audit是accepted attempt，不宣稱Host已成功；入列不等於desktop持久化。
- release／自然expiry／revoke／teardown優先失效及釋放，**不因audit滿而阻擋**。相應audit＋已announced的state(false)失敗時按stream保留latest bounded bundle（總64，僅收緊可使用）；maintenance重送。同device有任何pending bundle時拒絕新open（audit-unavailable），Host前reserve announcement時亦重查。admission後才出現的其他stream deferred bundle不阻擋已預留announcement：不同opaque stream key不會覆蓋彼此，同一stream只有成功入列true後才可能false，seq仍單調。覆蓋／超額的事件數都累計`audit.dropped`的`lost`及其子集`lostTightening`，不能假裝完整稽核。入列不等於desktop持久化；stdout故障仍讓gateway shutdown。
- `control.state{deviceId,peerId,stream,seq,session,target,takeover,open}`；`full.state{deviceId,peerId,stream,seq,session,open}`。`stream`是gateway生命週期不重用的opaque字串，**不是WS-local u32 token**；seq以每stream單調遞增（open:true=1、open:false=2）。desktop以**(peerId,stream)**記錄並只套用更新seq，再按session聚合協調Full使用者／connectors，不能以device/session覆蓋其他peer。target為解析後pane ID。Host open成功且stream未關才enqueue state(true)，enqueue成功才設announced；open失敗不送state，未announced不送state(false)。takeover逐事件通知；Full成功後通知暫停一般connectors或明確警告，這不是開啟前的desktop握手，不能保證暫停先於PTY改幾何。gateway沒有Herdr全局互斥，關閉後恢復仍須桌面驗收。
- admin queue維持64 slots；ordinary audit／open announcements最多48 slots（含reservations及in-flight），**至少16 slots不被這些寫入稽核／open reservations占用**，留給pairing、device revocation、session lifecycle及收緊事件；不是無限可靠通知保證。
- rejected-write不進deferred、不保留內容／逐筆reason；立即計入audit.dropped.rejectedOmitted（刻意省略的詳細audit，不算lost），另按peer計數、最多每10秒在下一次rejection嘗試送`audit(action="write.rejected",summary={count})`。summary只是合併提示，不是額外遺失數，也不代表逐筆audit已完整。stalled／cancelled通知若queue滿只累計lost，不增加lostTightening、不占收緊bundle。
- `audit.dropped{lost,lostTightening,rejectedOmitted,since}`：lost是本稽核管線實際未送出的事件數（含普通stalled／cancelled／open.failed與收緊事件）；lostTightening是其中release／expiry／revoke／state(false)收緊／生命週期bundle的事件數子集，**桌面須以lostTightening > 0醒目提示安全相關稽核缺口**，不可與lost重複相加。rejectedOmitted僅表示刻意count-only的拒絕；write.rejected summary亦不得再累加。since為本批最早累計時間；成功入列後重置三個計數。這不是所有admin事件的可靠投遞保證。
- 從未announced且未開始Host open（或Host open已失敗）的write stream移除時，只送ordinary `audit(action="open.failed",reason)`；queue滿只計lost，絕不送terminal.release／full.close bundle、進deferred或觸發same-device pending guard。Host open開始前先保守標記leased（Host可能持有lease），僅Host回Err才清除；in-flight或已取得lease但未announced仍走收緊audit，不送state(false)。
- **stream.closed只在WS，沒有admin同名事件**。Desktop透過audit reason/outcome與state(false)得知失效；expiry action=`expiry`，4401失效action=`revoke`（reason=session-invalid，不區分logout／grant變更等具體起因）；其他release保留terminal.release/full.close。手機已斷線不補送stream.closed。

### 大型回覆與負載

所有RPC reply batch先檢查完整queue capacity，再一次reserve全部bytes、split permits。每peer最多1個chunked snapshot queued/in-flight；空間不足→可重試busy，**不是1013**，沒有半份snapshot。Control/Full replies也走batch；連固定busy reply都放不下時1008 reply-queue-full。真正WS writer停滯20秒仍1013（不是用1013表示RPC拒絕）。

init.herdrBinary必須absolute；不接受裸檔名或relative path，否則invalid-herdr-binary。Fake fixture sentinel由平台temp path構成絕對不存在路徑；所有本輪Cargo／test subprocess皆透過不分大小寫清HERDR_*的launcher。

### 發佈 blocker：Host pipe / PTY 沒有 write timeout

**Parent明確接受為本輪殘留，但阻擋Phase4／產品發佈。** Host `send_control`持stdin mutex做write_all/flush；native_client_input也持input mutex阻塞寫PTY。兩者沒有timeout，release又先取得相同mutex（connector還會先寫release），因此不合作的Herdr可令**物理release永久等待**。135秒只屬feature API，不能套到這些pipe／PTY呼叫。

Companion緩解：每個write job有10秒async watchdog；逾時一次audit(action=herdr-io-stalled, summary.phase=write)，將對應stream失效並送stream.closed(reason=stream-stalled)，清queued input；已開始的input回input-outcome-unknown，其餘write回write-outcome-unknown；未開始回對應dropped。此peer的新input拒絕stream-stalled、新open回busy；其他peer同device仍受被佔住的write permit限制。late result不再送第二個ACK／不重播，late資源仍cleanup。release jobs（含排隊）也有10秒觀測timeout，一次相同audit、phase=release；若queue滿只計入audit.dropped，deferred專留給收緊bundle。

這些timeout**只觀測，不取消blocking thread**：task、device/global slot、resource/full permit保留到Host返回或gateway process結束；read slots及cleanup admission獨立，不被write slot鎖住，但cleanup2 slots本身可被不合作release佔滿。logical invalidation有期限；physical release無期限保證。**發佈前須另案修Host的bounded/cancellable connector與native client input/release，驗證永久不讀的fake仍能在期限內kill/reap且permit回收。**本輪不修改Host，也不以正常fake的回收測試冒充此條件已滿足。
