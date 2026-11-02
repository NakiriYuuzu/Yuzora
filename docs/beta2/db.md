# L3 資料庫

beta.2 的資料庫功能：連線管理、側欄 schema 樹、資料表 pane（資料／結構／SQL）、SQL 查詢 pane、儲存格編輯與寫回。原生層是 `app/crates/yuzora-db`（JS 模組 `yuzora:db`），畫面是四個 JS mod。架構總則見 [architecture.md](architecture.md)，lane 規則見 [lanes.md](lanes.md)。

## 分層

| 層 | 位置 | 內容 |
| --- | --- | --- |
| 引擎 | `src/engine/{postgres,mssql,sqlite,tls}.rs` | 三個驅動，對上只露出 `engine::Conn`（enum dispatch，不用 trait object） |
| 連線與工作 | `src/session.rs` | 連線池（每個 pane 一條）、連線狀態、查詢工作、開表、寫回 |
| 結果集 | `src/results.rs` | 已讀進來的列、排序後的檢視、未寫回的修改、變更通知 |
| 原生元件 | `src/grid.rs`、`src/editor.rs` | `DataGrid`（GPUI `uniform_list`）、`SqlEditor`（gpui-base Editor＋SQL 上色） |
| 設定與憑證 | `src/config.rs`、`src/history.rs` | Yuzora settings 的 `db` section、keyring namespace `db`、查詢歷史 |
| 執行環境 | `src/runtime.rs` | crate 自己的 tokio runtime（2 個 worker）；HostModule 的 async 函式把工作丟過去，用 oneshot 拿結果 |
| mods | `app/defaults/mods/yuzora-sidebar-database`、`yuzora-db-connection`、`yuzora-db-table`、`yuzora-db-query` | 側欄、連線對話框、資料表 pane、SQL pane |

gpui-shell 的 async 函式跑在 GPUI 的 background executor，不是 tokio。tokio-postgres、tiberius 都需要 tokio reactor，所以 crate 自帶 runtime，`runtime::run` 把 future 丟過去，回傳一個 `Send` 的 future 給 HostModule。

## 連線與憑證

- 設定存在 Yuzora `settings.json` 的 `db` section：`{"connections": [...], "maxRows": 100000}`。欄位見 `config::ConnectionConfig`（camelCase）。
- 密碼存在系統 keyring，key 是 `(db, <connection id>)`。設定檔裡沒有密碼。
- 取消「儲存密碼」時刪掉 keyring 裡的密碼；對話框輸入的密碼只留在這次執行的記憶體，App 重開後要在對話框重新輸入。
- **已存的密碼只送給存它時的那台伺服器**：`testConnection` 沒帶密碼時，只有 engine、host、port、user、instance、SSH tunnel 都與已存設定相同，TLS 模式不比原本弱、verify 模式下 CA 檔也沒換，才會沿用（`ConnectionConfig::password_may_follow`）；`saveConnection` 改了這些欄位又沒給新密碼時，舊密碼會從 keyring 刪掉。這樣任何 mod 都無法藉由「借用已存連線的 id、改 host」把密碼送到別的主機。對話框在這種情況下會提示重新輸入。
- `YUZORA_DB_SECRETS=memory`：密碼只存在這個 process，給隔離測試與實機驗收用，不碰真的 keyring。
- SSH tunnel：設定欄位 `ssh: {host}` 已保留，但 L4 的 `yuzora_ssh::tunnel` 還沒有，設定了 tunnel 的連線會回 `error.ssh_pending`，不會直連。

## JS API（`yuzora:db`）

完整宣告在 app home 的 `yuzora.d.ts`（由 `DECLARATIONS` 產生）。重點：

- 連線：`connections()`（含 `status`）、`saveConnection(config, password?)`、`deleteConnection(id)`、`testConnection(config, password?)`、`connect(id, db?)`、`disconnect(id)`。
- 目錄：`databases(id)`、`objects(id, db)`、`describe(id, db, {schema, name})`。
- 查詢：`query(id, db, owner, sql) → jobId`、`openTable(id, db, owner, table, order?) → jobId`、`job(jobId, seen?)`（長輪詢：版本超過 `seen` 或 250 ms 後回傳）、`cancel(jobId)`、`result(resultId)`。
- 生命週期：`release(owner)` 關掉 owner（pane key）的連線、工作、結果集、grid 與 editor。pane 的 `close()` 必須呼叫。
- 歷史：`history(id)`、`clearHistory(id)`；存在 app home 的 `db-history.json`，每個連線最多 200 筆、每筆最多 20,000 字，只存 SQL 不存結果。
- 元件：`DataGrid.new(id, {result, colors})`、`SqlEditor.new(id, {engine, value, readonly})`。注意是 `X.new(...)`，不是 `new X(...)`。
- 編輯器：`editorText(id)`（全文、選取、游標所在的句子）、`setEditorText(id, text)`、`editorAction(id)`（等 ⌘↵，回傳 `"run"`）。

### 共用狀態

| key | 寫入者 | 內容 |
| --- | --- | --- |
| `yuzora.status.db` | 側欄 | `{text, tone}`，例如 `PostgreSQL 16 · local-pg`；連線中 `working`、失敗 `blocked` |
| `yuzora.badge.database` | 側欄 | 已連線的連線數，沒有時 `null` |
| `yuzora.db.active` | 側欄 | 側欄正在看的 `{connection, database}` |
| `yuzora.db.open` | 側欄 | `{key, target, seq}`：要求某個資料表 pane 換表 |
| `yuzora.db.tabPanes` | 資料表 pane | `{tab: key}`：每個 tab 的資料表 pane（側欄點表時重用） |

## 查詢與結果集

- 一個工作（job）跑一段 SQL，可以有多個 statement，每個 statement 一個結果集（有列的、或只有 affected 的）。
- 讀取是串流的：引擎每 512 列（`engine::BATCH`）交給 `RowSink` 一次，grid 立即重畫，不等整個查詢結束。
- 上限 `db.maxRows`（預設 100,000，可設 100～5,000,000）。超過時停止讀取並標成 truncated，畫面顯示「已載入 N 列（上限）」。
- 排序在用戶端，只排已讀進來的列（排的是檢視的索引，不搬資料）。被截斷的結果要完整排序，請在 SQL 加 `ORDER BY`。
- 開表時另外跑一次 `COUNT(*)`（MSSQL 是 `COUNT_BIG`），狀態列顯示「X 列 · 顯示 a–b · N ms」。

## 編輯與寫回

- 只有「開表」產生的結果集可以編輯，而且資料表必須有 primary key；沒有就停用並顯示原因（`edit.no_key`）。
- 雙擊或 Enter 編輯，⌘⌫ 設 NULL，Esc 取消。改回原值等於沒改。
- 底部顯示「N 個儲存格已修改 · 捨棄 · 寫回」。寫回在一個交易內，每列用 primary key 定位，每列影響的列數必須剛好是 1，否則整筆 rollback 並顯示錯誤。key 是 NULL 的列拒絕寫回。
- PostgreSQL 用文字格式參數寫回（server 依欄位型別解析），所以 numeric、interval、enum、array 等都能原文 round-trip。

## 引擎

（引擎細節、TLS 對應、效能數字：見下一節，隨引擎實作補上。）

## 已知限制

- SSH tunnel 等 L4。
- SQL Server 的 named instance 需要指定 port（沒有用 SQL Browser）。
- 排序只排已載入的列。
- 「其他物件」（function、procedure、sequence、trigger）只列在 schema 樹，還不能開啟定義。

## 後續整合

- L4：`yuzora_ssh::tunnel(host) -> (local_host, local_port)` 一類的 API；L3 在 `session::spec_for` 把 `ConnectSpec::endpoint` 換成 tunnel 的本機端點，`refuse_tunnel` 拿掉即可。
- L6：`db.maxRows` 的設定頁（目前只能手改 `settings.json`）。
