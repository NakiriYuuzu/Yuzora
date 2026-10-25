# Spike C：Herdr v0.9.1 多 client 實測

狀態：**[x] CLI／PTY 自動化實驗完成並清理**；Yuzora 真實 UI 提示、手機輸入與視覺驗收未執行。選定 binary `/Users/yuuzu/.local/bin/herdr`，0.9.1／protocol22。這不是在使用者 live pane 測試。

## 隔離與可重現性

- 所有 Herdr 呼叫皆經 `isolated-herdr.sh`；wrapper 清除**全部 HERDR_* env**，並指定 `/tmp/yzc-spike/{home,config,state,data,run}`。本輪 review 修正再加入 `XDG_CACHE_HOME=/tmp/yzc-spike/cache`，拒絕 symlink／非目前使用者擁有的 root，preflight 以 `realpath` 檢查 socket/session 路徑；只重驗乾淨 `session list --json`，沒有重跑量測。`sanitized-herdr-env.txt` 為空，符合預期。
- 初次 temp root 原先不存在。`preflight.json` 只有 `default=true,running=false` 的 virtual default，socket/session_dir 都在 temp root；沒有 persisted session。supervisor 核准此型態為乾淨 preflight，非要求 JSON 空陣列。
- `probe.py::preflight()` 在每次命令、stream input、PTY input 前重新 list，拒絕 temp root 外 paths、未知 session 或 running default。`routing-checks.ndjson` 記錄各次驗證。
- headless `--session companion-spike server`，以隔離 config 指定 `/bin/sh` non-login，停用版本／manifest check；`workspace create --cwd /tmp/yzc-spike/home` 建 root pane。未用 remote／machine，未連真實 socket。
- `measure.py` 是第一輪量測（需要先經 wrapper 開 server、建 w1:p1），`measure-lock.py` 是第二輪完整啟動／清理的 lock interaction 量測。重跑必須先驗證 root 不存在且 preflight 乾淨；**不得在 live server 重放 fixture 命令**。
- 第一輪 client 終止時，PTY 尚未關閉，`terminate/kill + wait(3)` 逾時；當輪程序退出後 CLI 經 wrapper 停止／刪除 session，核對無殘留。現 `measure.py` 改成 documented prefix+q detach、close PTY 再 wait；第二輪同方式完成，`lock-experiment.txt` 為空只代表沒有保存輸出，**不等於有記錄 exit 0**。第一輪量測資料仍有效，但不宣稱修改後第一輪完整腳本已重跑。

## 量測結果

| 項目 | 實際結果 | Phase 3 含意 |
|---|---|---|
| A control120×40＋B observe44×30 | 兩者都有 `terminal.frame`；A 寫 marker 後 B 收到更新。observer 的 `terminal.input`／`terminal.resize` **無 ack/error、被忽略**，pane read 無 `OBSERVER_MUST_NOT_EXECUTE`；PTY 仍40 rows×120 cols。 | view 不能依賴收到明確 input 拒絕；gateway 必須先擋 view 的寫入。observe 不取得 resize/control owner。 |
| 第三個 control，無 takeover | stdout NDJSON `terminal.closed`（見下），不是 stderr；**衝突 client 的 exit code 未記錄**，不據此推論成功／失敗。 | 映射 `owned-elsewhere`；顯示明確確認與新鮮提權，不自動 retry takeover。 |
| takeover | A 收到 `terminal.closed`，reason=`terminal attach taken over`；A process exit0，不會自動重搶。釋放 takeover stream 後小尺寸 control 可直接連上。 | 映射 `taken-over`；gateway/桌面禁止自動 reclaim。本輪未啟動 Yuzora UI，沒有驗收「已由其他視窗取得控制」toast。 |
| 同尺寸 control | `stty size`＝`40 120`，layout area120×40，無尺寸變化。 | 僅 headless 同尺寸已驗，沒有 Yuzora 桌面 client；不可宣稱桌面不重排，snapshot 也未提供實際 PTY cols。 |
| 小尺寸 control44×30 | `stty size`＝`30 44`，輸出換行/reflow；frame44×30、pane scroll.viewport_rows30；**layout snapshot rect仍120×40**。 | 不可拿 layout rect 當作已知 PTY 尺寸；direct attach resize lock 與 layout 座標分離。 |
| observe44×30 看120×40 | frame.width44/height30；ANSI 第2行 marker只到 `…XYZ_01`，尾端 `23456789_END` 被裁掉；不是 full-width資料＋client縮放。其後控制者縮為44欄時，PTY reflow 才使尾段換行可見。 | observe 尺寸由 gateway 決定，至少不小於 layout rect，但這不保證涵蓋已被 direct control 改大的 PTY；PWA 再縮放／平移，不接受手機 cols/rows。 |
| Full client PTY44×30 | transcript 有 mobile 頂列 `· spike … tab 1`／`no agents … switch`、無固定 sidebar，符合≤64 的 mobile chrome。**兩個 split panes 仍左右並排，不是 pane 自動改成單欄**；tab area44×28、各pane rect22×28，內部 stty26×19。啟動／焦點時已改 geometry，送 input 前後相同。 | 警語須涵蓋「開啟即可能改幾何」，不是只有第一次輸入。不能保證手機所有pane自動單欄；需真機可用性驗收。 |
| direct_attach_resize_locks | 第二輪 A保持80×24 control，同時full44×30啟動並輸入：layout仍改44×28／split22×28，但被鎖pane的 `stty size`一直24×80，另一pane viewport_rows26；A仍可寫 `CONTROL_STILL_WRITES`。 | Full client並不自動踢掉direct control；兩者可能同時寫。進full前必須明確關閉／協調普通 connectors，退出後恢復，不能以geometry claim當互斥授權。 |
| stopped session | observe/control均exit1、stderr為連不上`herdr-client.sock`，未自動啟動。session list仍running=false。 | 映射 `session-stopped`（先以session狀態確認）；不要照錯誤建議替使用者啟server。 |

### 精確錯誤 fixture

`control-conflict.ndjson`：
```json
{"reason":"terminal attach failed: terminal term_65c7a07b973731 already has an attached client; retry with --takeover","type":"terminal.closed"}
```
`control-a.ndjson` 最後一行：
```json
{"reason":"terminal attach taken over","type":"terminal.closed"}
```
已連線 observer 的 `observe-b.ndjson` 最後一行：
```json
{"reason":"server is shutting down","type":"terminal.closed"}
```
映射為 `session-stopped`；這與停止後新 attach 的 stderr 是兩種情境。Phase 3 應 pin Herdr 0.9.1 reason fixtures：衝突 → `owned-elsewhere`、接管 → `taken-over`、上述 shutdown → `session-stopped`；attach 前仍先以 `session list` 的 `running` 判斷。

stop 後新 attach 的 observe／control stderr 相同：
```text
herdr: failed to connect to server: No such file or directory (os error 2)
Is herdr server running? Start it with `herdr server`.
Socket path: /tmp/yzc-spike/config/herdr/sessions/companion-spike/herdr-client.sock
```

### Phase 3 implications（既有證據，不是新增量測）

- **P1-2：**目前官方 snapshot 只提供 layout rect 與 `scroll.viewport_rows`，沒有實際 PTY cols；observe frame 固定為請求尺寸，也不能倒推 PTY cols。Phase 3 須先確認是否有其他官方來源；找不到就保留此限制。gateway 決定 observe/control 尺寸，忽略 client cols/rows；observe 至少取 layout rect，但不可宣稱它等於 PTY 尺寸。control 優先不送尺寸；若 API 必填且無已知 PTY 尺寸，不得猜測為無副作用，需明確確認改尺寸與恢復策略。
- **P1-2：**control 釋放後 PTY **不會自動恢復**；`geometry-before-full.txt` 的 rect 為 60×40、viewport_rows 仍 30。須驗收桌面 connector 下一次 resize 的恢復行為，不把 release 當 restore。
- **P1-3：**Full Session access 使用的官方 full client 不排除 direct control，開啟時就會改 tab 幾何；gateway 在 `full.open` 前須釋放同一 named Session 自己的 control streams，通知桌面依 ADR-0004 暫停一般 pane connectors，關閉後協調恢復。這是 Yuzora 的協調責任，不是 Herdr 互斥保證。
- **P1-3：**≤64 欄只改 mobile chrome；BSP panes 仍左右並排。不得承諾手機單欄，應警告「開啟即改變桌面該 tab 幾何」。

### 尚不能從本輪推論

- full client 的「單欄」只確認 mobile **chrome**（PTY44欄）；BSP splits沒有折疊，勿把計畫文字理解成所有pane單欄。`full-client-44x30.screen.txt` 是 pyte44×30 非瀏覽器回放；已濾掉 pyte 不支援的 terminal status query，原始 ANSI 完整保留。
- 沒有真實桌面client與手機觸控，因此不宣稱 Yuzora takeover UI／重連UX／IME已驗收。
- 首次 `api snapshot --json` 得 `usage: herdr api snapshot`，改成無flag後成功。CLI version差異不要以猜測覆蓋。

## 清理證據

兩輪皆 `session stop` → `session delete companion-spike`；`sessions-after-delete.txt`／`lock-sessions-after-delete.txt` 只剩未執行 virtual default。`cleanup-processes.txt` 以 ps命令＋每個Herdr process環境檢查 isolated HOME/XDG，找不到spike PID；`/tmp/yzc-spike` 已移除。未 kill 其他 Herdr。

`live-config-before.txt` vs `live-config-after.txt`：config.toml、session.json、socket、sessions目錄等 metadata沒變；**herdr-server.log 因同時活躍的使用者 session增長**（total也改變），不能把整個ls宣稱byte-identical。本輪所有routing證據均指向temp root；沒有對真實Herdr送命令或寫檔。ls對比不是完整內容hash證明。

## Fixtures 索引

- 隔離／schema：`preflight.json`, `sanitized-herdr-env.txt`, `routing-checks.ndjson`, `help.txt`, `status.txt`, `schema.txt`, `default-config.txt`。
- 基線：`server.ndjson`, `server.stderr.txt`, `reload-config.txt`, `workspace-create.txt`, `snapshot-created.txt`。
- A/B/control：`control-a.ndjson`, `observe-b.ndjson`, `control-conflict.ndjson`, `control-takeover.ndjson`, `control-small.ndjson`，各自stderr；`*.decoded.txt` 為base64解碼ANSI的repr。
- 尺寸：`geometry-current.txt`, `geometry-after-takeover.txt`, `geometry-small.txt`, `pane-small.txt`, `pane-after-observer-input.txt`。
- Full：`split.txt`, `geometry-before-full.txt`, `geometry-full-before-input.txt`, `geometry-full-after-input.txt`, `pane-full.txt`, `full-client-44x30.ansi`, `full-client-44x30.screen.txt`。
- Resize lock：`lock-control.ndjson`, `lock-full-client.ansi`, `lock-full-before-input.txt`, `lock-full-after-input.txt`, `lock-pane-after-input.txt`, `lock-pane-after-control.txt`。
- 停止／清理：`session-stop.txt`, `observe-stopped.txt`, `control-stopped.txt`, `sessions-after-stopped-attempts.txt`, `session-delete.txt`, `sessions-after-delete.txt`，第二輪`lock-session-*`，`cleanup-processes.txt`, `live-config-{before,after}.txt`。

fixture只含隔離測試pane文字，不含使用者terminal內容；在終端檢閱不可信ANSI時，使用escaped/decoded文字檔，勿直接cat原始ANSI。

## 2026-10-17：HERDR 0.9.5 重跑（Phase 3C）

- binary：`src-tauri/resources/herdr/macos-aarch64/herdr`，由 `prepare-herdr-resources.ts` 依 `herdr-runtime.json` 下載，版本 0.9.5，sha256 `fb77c223…`，和 GitHub release asset digest 一致。
- 方式：`isolated-herdr.sh` 加上 `YZC_SPIKE_BIN` 參數（預設仍是 `~/.local/bin/herdr`），`probe.py` 加上 `YZC_SPIKE_FIX`，結果輸出到 `fixtures-0.9.5/`，**0.9.1 的 `fixtures/` 原樣保留**。
- 流程：先確認 `/tmp/yzc-spike` 不存在，再用 driver 依序跑 round 1（bootstrap＋`measure.py`）與 round 2（`measure-lock.py`）。結束後 session 已停止並刪除，沒有殘留程序，`/tmp/yzc-spike` 已移除。全程沒有連到使用者的 live session。
- 結果與 0.9.1 **完全一致**：
  - 衝突時回 `already has an attached client; retry with --takeover`，原 owner 收到 `terminal attach taken over`，並以 exit 0 退出。
  - observer 送出的 input 不會執行（pane 內沒有 `OBSERVER_MUST_NOT_EXECUTE` 的輸出行）。
  - `stty` 依序為 `40 120`，接著小尺寸 control 為 `30 44`。
  - full client 在 44×30 下，兩個 split pane 仍各為 22×28 並排，內部 stty `26 19`。
  - resize lock 下，direct control 鎖住的 PTY 保持 `24 80`，而且仍可寫入（`CONTROL_STILL_WRITES`）。
  - session 停止後，observe／control 都 exit 1，不會自動啟動 server。
- 另外驗證：0.9.3、0.9.4、0.9.5 的 `api schema --json` 完整 JSON（request／response／event／subscription）零差異，`session list --json` 的格式也相同。
