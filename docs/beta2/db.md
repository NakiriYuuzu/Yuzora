# L3 資料庫

> beta.4 起，Files／Git／Database／SSH 是右側 dock 的模式（`panel` 右邊的模式列，沒有 panel 的 layout 是抽屜），不再是左側欄。本文原稱「側欄」「側欄模式」的地方已改稱「右側模式」；截圖與 beta.2／3 時期的設計描述（例如圖中的左側欄）未重寫。

beta.2 的資料庫功能：連線管理、右側模式 schema 樹、資料表 pane（資料／結構／SQL）、SQL 查詢 pane、儲存格編輯與寫回。原生層是 `app/crates/yuzora-db`（JS 模組 `yuzora:db`），畫面是四個 JS mod。架構總則見 [architecture.md](architecture.md)，lane 規則見 [lanes.md](lanes.md)。

## 分層

| 層 | 位置 | 內容 |
| --- | --- | --- |
| 引擎 | `src/engine/{postgres,mssql,sqlite,tls}.rs` | 三個驅動，對上只露出 `engine::Conn`（enum dispatch，不用 trait object） |
| 連線與工作 | `src/session.rs` | 連線池（每個 pane 一條）、連線狀態、查詢工作、開表、寫回 |
| 結果集 | `src/results.rs` | 已讀進來的列、排序後的檢視、未寫回的修改、變更通知 |
| 原生元件 | `src/grid.rs`、`src/editor.rs` | `DataGrid`（GPUI `uniform_list`）、`SqlEditor`（gpui-base Editor＋SQL 上色） |
| 設定與憑證 | `src/config.rs`、`src/history.rs` | Yuzora settings 的 `db` section、keyring namespace `db`、查詢歷史 |
| 執行環境 | `src/runtime.rs` | crate 自己的 tokio runtime（2 個 worker）；HostModule 的 async 函式把工作丟過去，用 oneshot 拿結果 |
| mods | `app/defaults/mods/yuzora-sidebar-database`、`yuzora-db-connection`、`yuzora-db-table`、`yuzora-db-query` | 右側模式、連線對話框、資料表 pane、SQL pane |

gpui-shell 的 async 函式跑在 GPUI 的 background executor，不是 tokio。tokio-postgres、tiberius 都需要 tokio reactor，所以 crate 自帶 runtime，`runtime::run` 把 future 丟過去，回傳一個 `Send` 的 future 給 HostModule。

## 連線與憑證

- 設定存在 Yuzora `settings.json` 的 `db` section：`{"connections": [...], "maxRows": 100000}`。欄位見 `config::ConnectionConfig`（camelCase）。
- 密碼存在系統 keyring，key 是 `(db, <connection id>)`。設定檔裡沒有密碼。
- 取消「儲存密碼」時刪掉 keyring 裡的密碼；對話框輸入的密碼只留在這次執行的記憶體，App 重開後要在對話框重新輸入。密碼欄留空時，原本存的密碼也移到這次執行的記憶體，所以這次執行還連得上。
- **已存的密碼只送給存它時的那台伺服器**：`testConnection` 沒帶密碼時，只有 engine、host、port、user、instance、SSH tunnel 都與已存設定相同，TLS 模式不比原本弱、verify 模式下 CA 檔也沒換，才會沿用（`ConnectionConfig::password_may_follow`）；`saveConnection` 改了這些欄位又沒給新密碼時，舊密碼會從 keyring 刪掉。這樣任何 mod 都無法藉由「借用已存連線的 id、改 host」把密碼送到別的主機。對話框在這種情況下會提示重新輸入。
- `YUZORA_DB_SECRETS=memory`：密碼只存在這個 process，給隔離測試與實機驗收用，不碰真的 keyring。
- SSH tunnel：設定欄位 `ssh: {host}` 已保留，但 L4 的 `yuzora_ssh::tunnel` 還沒有，設定了 tunnel 的連線會回 `error.ssh_pending`，不會直連。

## JS API（`yuzora:db`）

完整宣告在 app home 的 `yuzora.d.ts`（由 `DECLARATIONS` 產生）。重點：

- 連線：`connections()`（含 `status`）、`saveConnection(config, password?)`、`deleteConnection(id)`、`testConnection(config, password?)`、`connect(id, db?)`、`disconnect(id)`。
- 目錄：`databases(id)`、`objects(id, db)`、`describe(id, db, {schema, name})`。
- 查詢：`query(id, db, owner, sql) → jobId`、`openTable(id, db, owner, table, order?) → jobId`、`job(jobId, seen?)`（長輪詢：版本超過 `seen` 或 250 ms 後回傳）、`cancel(jobId)`、`result(resultId)`。
- 生命週期：`release(owner)` 關掉 owner（pane key）的連線、工作、結果集、grid 與 editor。pane 的 `close()` 必須呼叫。
- 歷史：`history(id)`、`clearHistory(id)`；存在 app home 的 `db-history.json`，每個連線最多 200 筆、每筆最多 20,000 字，只存 SQL 不存結果。檔案權限是 0600（只有擁有者能讀）。含 `password`、`passwd`、`pwd=`、`secret`、`identified by`，或含帶 userinfo 的 URL（`postgresql://user:pass@host`，也包括只放 token 的 `https://token@host`）的 SQL，整段都不記錄（`history::carries_credentials`）。這個範圍刻意放寬，所以查 `password_hash` 欄位的查詢也不會進歷史。
- 元件：`DataGrid.new(id, {result, colors})`、`SqlEditor.new(id, {engine, value, readonly})`。注意是 `X.new(...)`，不是 `new X(...)`。
- 編輯器：`editorText(id)`（全文、選取、游標所在的句子；句子在哪裡結束要看 session 時是 `null`，見 PostgreSQL 一節）、`setEditorText(id, text)`、`editorAction(id)`（等 ⌘↵，回傳 `"run"`）。

### 共用狀態

| key | 寫入者 | 內容 |
| --- | --- | --- |
| `yuzora.status.db` | 右側模式 | `{text, tone}`，例如 `PostgreSQL 16 · local-pg`；連線中 `working`、失敗 `blocked` |
| `yuzora.badge.database` | 右側模式 | 已連線的連線數，沒有時 `null` |
| `yuzora.db.active` | 右側模式 | 右側模式正在看的 `{connection, database}` |
| `yuzora.db.open` | 右側模式 | `{key, target, seq}`：要求某個資料表 pane 換表 |
| `yuzora.db.tabPanes` | 資料表 pane | `{tab: key}`：每個 tab 的資料表 pane（右側模式點表時重用） |

## 查詢與結果集

- 一個工作（job）跑一段 SQL，可以有多個 statement，每個 statement 一個結果集（有列的、或只有 affected 的）。
- 一個 pane（owner）只顯示最新的工作：開新工作時，同一個 owner 還在跑的工作會被取消，舊工作和它的結果集一起釋放。`release(owner)` 也會先取消，再關連線。
- 開表時 `COUNT(*)` 在列讀完後、工作結束前執行，所以按「停止」也能停掉 count。count 用自己的連線（`session::count_owner`，count 完就關），不佔 pane 的連線：列讀完、決定好可不可編輯（`Editing`）之後 grid 就能編輯，寫回也不用排在 count 後面。
- count 是輔助資訊：它的連線開不了（登入或資料庫有連線數上限、網路抖動）時，不改變連線的狀態（側欄不會變成「連線失敗」），改借 pane 自己的連線、等它空閒時算，總列數照樣出現。借來的 count 永遠讓路：寫回、SQL 分頁的查詢、開表、重新整理一向 pane 的連線要連線（`session::lease`，`Live::want`），count 就被叫停（`StandDown`，由 count 自己那個 task 取消自己的 statement 再放開連線），工作拿到連線；工作結束後 count 從頭再算一次（`count_between`）。使用者不會為了 count 等待。PostgreSQL 的取消若沒有被 count 的那句 statement 接住（伺服器沒回 `canceled`，請求可能還在路上，之後落在別的語句上），count 的連線會被丟掉，下一個使用者開新連線（`Lease::retire`、`Conn::cancel_landed`）；接住了就留著，不用重連。
- 每個「停止」只停它旁邊標示的那件事。工作的停止（工具列）只取消工作當下正在跑的那句：讀列時是 pane 的連線，count 時是 count 的連線（`Job.serving`；借 pane 連線的 count 是請它自己放手，`Stop::Count`），讀完之後不再碰 pane 的連線，因為那上面可能正在寫回。寫回的停止（footer）只結束那次寫回。
- 連線還在建立時按「停止」也有效：引擎先把自己的取消機制準備好，然後在每個 statement 執行前檢查工作是否已經停止（`RowSink::cancelled`）。所以 UPDATE、DELETE 這類不回傳列的語句，不會在回報取消之後才執行。
- 讀取是串流的：引擎每 512 列（`engine::BATCH`）交給 `RowSink` 一次，grid 立即重畫，不等整個查詢結束。
- 上限 `db.maxRows`（預設 100,000，可設 100～5,000,000）。真的有列放不下時才停止讀取並標成 truncated（剛好等於上限不算）；開表會多讀一列（`LIMIT maxRows + 1`）來判斷。
- 排序在用戶端，只排已讀進來的列（排的是檢視的索引，不搬資料）。整數與數字文字逐位精確比較，不經過 f64，所以超過 2^53 的 bigint 和高精度 numeric 也排得對。NaN 排在所有數字之後（和 PostgreSQL 一樣）。PostgreSQL 的 money 依數值排序：只把數字之間的 `.`、`,` 當分隔，所以 `$1,234.56`、`1.234,56 €`、`￥1,234` 都會正確排序。查詢還在串流時就排序，之後到的列也會依排序併入。被截斷的結果要完整排序，請在 SQL 加 `ORDER BY`。
- 開表時另外跑一次 `COUNT(*)`（MSSQL 是 `COUNT_BIG`），狀態列顯示「X 列 · 顯示 a–b · N ms」。

## 編輯與寫回

- 結果集的可編輯狀態是 `results::Editing`：`Loading`（開表的列還在讀）、`Editable`、`ReadOnly(原因)`；寫回狀態是 `WriteState`：`Idle`、`Writing`（可用 `stop_write` 停止）、`Failed(訊息)`。每個唯讀都有一句原因（包括 SQL 查詢的結果集，`edit.query`），每個失敗都有訊息，雙擊被擋時 footer 會說明原因。
- 寫回排在 pane 的連線後面，也會被伺服器上別的連線持有的列鎖擋住（PostgreSQL、SQL Server 預設不逾時）。等待期間 footer 顯示「Writing…」加「Stop」，沒有「Discard」（修改正在寫，丟掉它可能不算數；要捨棄先 Stop，之後 footer 會出現 Discard；`ResultSet::discard_edits` 在寫回中也不動作）：Stop 結束等待，PostgreSQL／SQLite 另外叫伺服器取消那句 UPDATE，SQL Server 則是丟掉 socket（伺服器回滾）。停止後編輯仍保留，結果集說「沒有寫入任何東西」（`edit.stopped`）；只有 COMMIT 已經送出時才說「資料庫可能已經存了」（`edit.stopped_maybe`）。判斷靠各引擎在送出 COMMIT 之前一刻設起的 `CommitSent`（PostgreSQL：`tx.commit()` 之前；SQL Server：`COMMIT TRANSACTION` 批次之前；SQLite：最後一次看有沒有停止之前設起，看到停止就收回再回滾）。設在送出之前而不是之後，所以誤差只會偏向「可能已存」，不會在 COMMIT 已經進行時說沒寫。SQLite 的 COMMIT 是同步呼叫、`CommitSent` 與停止的先後可能讓訊息偏向「可能已存」一個極短的窗口，反方向不會發生。
  - 保證：不論 Stop 和語句結束的先後，停止之後（a）沒有資料被寫入，修改留在畫面上；（b）pane 的連線可用，下一句正常執行；（c）取消不會落在這次寫回以外的語句上。唯一的例外是 Stop 在 COMMIT 已經送出之後才到：那筆交易由伺服器決定，資料可能已寫入，所以訊息才說「可能」，重新整理就知道。
  - 取消是「連線層級、打到當下正在跑的語句」的請求，送出時不知道語句還在不在。所以停止時先通知引擎、寫回還握著 pane 的連線，之後才放開（`session::stop_work`：先放開的話，排在後面的工作會搶到連線）；而且 PostgreSQL 的連線在這之後不再使用（`Canceller::abandon`：取消請求可能還在路上，之後會落在收尾的 ROLLBACK 或下一句上，讓交易卡在 aborted 狀態，這個 pane 之後每一句都是 25P02）。下一次使用才開新連線，所以這個 pane 的 SQL 分頁在同一條連線上做的 `SET`、暫存表會隨之消失（和 SQL Server 停止時一樣）。登入有連線數上限時，緊接著的下一句可能撞上舊連線還沒結束的那一小段時間而被拒絕一次。
  - SQLite：取消是 `interrupt`，只對當下正在執行的 statement 有效，沒有 statement 在跑時什麼也不做，所以不會落到別處。SQLite 等檔案寫鎖時 `interrupt` 到不了，所以等鎖是寫回自己做的（`busy_timeout` 設 0，每 10 毫秒重試、看有沒有停止，最多 5 秒），並在 `BEGIN`、每句 UPDATE、COMMIT 前看有沒有在寫回要求之後停止過（`LiteCanceller.stops`，計數而不是會被下一件工作重設的旗標）。停止後鎖才放開也不會寫入，連線也不會被卡在那段等待上。
  - SQL Server：取消就是丟掉 socket（`apply_edits` 的 future 一被丟掉 socket 就跟著關），伺服器在連線結束時回滾；沒有可以晚到的取消，下一次使用開新連線。
- Cmd+S（Ctrl+S）等於「Write back」，編輯框還開著時也一樣：先保留那一格（等於按 Enter；NULL 格沒打過字就按 ⌘S 不算修改，這點和 Enter 不同），再寫回；內容被資料庫拒絕（例如型別不合）時，原因自動展開在 footer 上方。輸入法正在組字（注音、拼音的 marked text 還沒選字）時，⌘S 不動作、編輯框留著，留給輸入法：沒選的字不算打進儲存格的內容，不能被寫進資料庫。判斷是 `InputState::marked_text_range()` 有值。Enter 也一樣，組字中的 Enter 是輸入法在選字，不保留儲存格（macOS 的輸入法通常自己吃掉這顆 Enter，這是給沒吃掉它的平台的保險）。
- footer 的訊息（寫回失敗、唯讀原因）只顯示第一行，點一下展開全文；新的寫回失敗會自動展開（伺服器的 DETAIL、HINT 行在第二行以後）。展開的訊息會依 pane 寬度換行。
- Database dock 開著時，表格 pane 只有約 110–230 px 寬：footer 的狀態文字各佔一行、太長就截斷，Discard／Write back（寫回中只有 Stop）按鈕放不下時換到下一行，只要 pane 比 footer 本身高，就完整在 pane 內，不會被切掉（110 px 寬時 footer 約 115 px 高）。展開的失敗訊息有 140 px 的高度上限、可捲，pane 放不下時它先縮，不推擠 footer，所以按鈕和「▾ 訊息」開關永遠在；代價是 pane 很矮時表格區被壓到 0，收起訊息就回來。工具列也一樣：資料表名稱和分頁放不下時換行，Run／Stop 按鈕在最後一行靠右。
- SQL Server 的連線被伺服器關掉（閒置逾時、防火牆、重啟、`KILL`）時，下一次使用才會發現：寫回的第一句（檢查未結束交易）和 metadata 查詢遇到這種斷線會自動換一條新連線重試一次（這時還沒寫入任何東西）；SQL 頁籤的語句不重試，因為答案遺失不代表語句沒執行。
- SQL Server 驅動（tiberius）讀不了 `geography`／`geometry`／`hierarchyid` 等 CLR 型別和 `sql_variant`：開這種表會以 `unsupported column type` 結束，不會再卡在載入。要看這些欄位請在 SQL 頁籤自己轉型（`col.ToString()`、`CAST(col AS nvarchar(max))`）。

- 只有「開表」產生的結果集可以編輯，而且資料表必須有 primary key；沒有就停用並顯示原因（`edit.no_key`）。連線設成唯讀（SQLite 的 `readOnly`）時也會停用（`edit.read_only`）。
- 由資料庫計算的欄位不能編輯（`grid.generated`），包括產生欄、計算欄，以及 PostgreSQL 的 `GENERATED ALWAYS` identity 和 SQL Server 的 identity。否則寫回時整筆交易都會被伺服器拒絕。`describe` 的 `ColumnInfo.generated` 標示這類欄位。
- 雙擊或 Enter 編輯，⌘⌫ 設 NULL，Esc 取消。打開編輯框卻沒改內容就離開，不算修改；改回原值也等於沒改。NULL 儲存格的編輯框是空的：在裡面打過字或按了 Enter，留空就寫成空字串；只打開就離開，仍然是 NULL。
- 編輯框只有一行：含換行的文字和二進位值不能在表格內編輯（底部會說明），請在 SQL 分頁用 `UPDATE`。
- 有未寫回的修改時，換表、重新整理或執行 SQL 會先在 pane 頂端詢問「繼續編輯」或「捨棄並繼續」。問句和按鈕會依 pane 寬度換行，兩個選項都在 pane 內。修改正在寫回時（`ResultSummary.writing`）不提供「捨棄」：寫回已經送出，修改可能照樣寫進資料庫，說「捨棄」會讓人以為沒有寫；改問「繼續等待」或「停止儲存」（`stopWrite`，修改保留，之後可以再按「捨棄並繼續」）。問的當下的修改已經寫完或被捨棄時，這一列自己消失，不會自己繼續換表。
- 寫回期間又改的儲存格保留為待寫回，不會被當成已寫入。寫回還沒結束時再按一次寫回會被拒絕（`edit.writing`），所以同一批修改不會寫兩次。
- 寫回只送到讀取這些列時的連線設定。表打開之後，如果已存的連線改成連到別處（另一台伺服器、另一個資料庫或另一個檔案），寫回會被拒絕（`edit.connection_changed`），修改仍保留為待寫回；重新整理資料表後再編輯。判斷方式和存檔時是否重連相同（`ConnectionConfig::connects_like`），在連線前後各檢查一次。
- SQL 分頁在同一條連線上留著尚未結束的交易時，不會寫回（`edit.open_transaction`），修改仍保留為待寫回。原因是 PostgreSQL 會忽略寫回的 BEGIN，寫回的 COMMIT 會連分頁自己的修改一起提交；SQL Server 的巢狀 COMMIT 只會把計數減一，之後分頁一 ROLLBACK，寫回就跟著消失。判斷方式：PostgreSQL 看 `now() = statement_timestamp()`，SQL Server 看 `@@TRANCOUNT`，SQLite 看 autocommit。
- 底部顯示「N 個儲存格已修改 · 捨棄 · 寫回」。寫回在一個交易內，每列用 primary key 定位，每列影響的列數必須剛好是 1，否則整筆 rollback 並顯示錯誤。key 是 NULL 的列拒絕寫回。
- PostgreSQL 用文字格式參數寫回（server 依欄位型別解析），所以 numeric、interval、enum、array 等都能原文 round-trip。
- 數字儲存格改成同一類的數字時，寫回的仍是數字：整數儲存格要能讀成 i64，浮點儲存格要能讀成有限的 f64（`Cell::edited`）。其他情況都以文字寫回，由伺服器轉換或拒絕。這是因為 SQLite 沒有型別（或 BLOB affinity）的欄位不會轉換型別，用文字寫回會把整數 5 變成文字 `'7'`。整數儲存格改成 `7.5` 仍然送文字，因為 SQL Server 收到 float 參數時，會在 int 欄位把它靜默截斷成 7。
- 日期文字不依登入語言。SQL Server 的 `datetime`／`smalldatetime` 從文字讀日期時依 session 的 `DATEFORMAT`：British、German、French 等登入是日先月後，`2024-01-06` 會被讀成 6 月 1 日，13 日則報超出範圍，`datetime` 當主鍵時找不到列。寫回時，年在前的文字（`2024-01-06 10:00`、`2024/1/6`）送往這兩種型別的欄位（SET 與 WHERE 都是）前先改成 `2024-01-06T10:00:00`，這種寫法在任何 `DATEFORMAT` 下讀法都相同（`day_order_columns` 從系統目錄找出這類欄位，每次寫回多一次往返；別的欄位，包括文字欄，原文不動）。其他形狀（`13/01/2024`）是使用者自己的寫法，仍由 session 依登入語言讀。`date`、`datetime2`、`datetimeoffset`、`time`、`money`、`decimal`、`float` 不受 `DATEFORMAT` 與語言影響（六種 `DATEFORMAT`、九種語言實測）。
- PostgreSQL 連線後固定 `SET datestyle = 'ISO'`（日月順序維持原設定）與 `SET extra_float_digits = 3`：格子顯示的文字要能原樣寫回。非 ISO 樣式把時區印成縮寫，`Asia/Taipei` 印 `CST`，而 `CST` 讀回是 -06:00，`timestamptz` 主鍵找不到列，改了顯示的文字則會存成差 14 小時的時間；`extra_float_digits = 0` 會少印 float 的位數，浮點主鍵同樣找不到列。副作用是這條連線上 SQL 分頁的日期也印成 ISO。`IntervalStyle` 和 `TimeZone` 不需要固定：interval 與 timestamptz 的 ISO 輸出在任何樣式下都能讀回。

## 引擎

三個引擎對上都是 `engine::Conn`，`execute(sql, &mut dyn RowSink)` 以每批 512 列交出結果；`RowSink::rows` 回 `false` 時引擎停止讀取。引擎自己產生的錯誤訊息走 crate 的 i18n（`engine.*`），伺服器回的訊息照原文顯示，並保留 SQLSTATE／Msg 編號、DETAIL、HINT、POSITION。

### PostgreSQL

- tokio-postgres 0.7＋tokio-postgres-rustls，rustls 用 ring provider，不呼叫 `install_default`。
- 沒指定資料庫時依序試 `postgres`、與使用者同名的資料庫，只有遇到 SQLSTATE `3D000`、`42501`、`55000` 才換下一個。
- 欄位型別用 `prepare` 取得；值用 `simple_query_raw` 讀文字格式，所以 numeric、interval、inet、enum、array、JSON 大整數都照伺服器的原文顯示。
- 取消用 cancel token，連線用的 TLS connector 也用在 cancel 上。工作（查詢、讀列、count）自己看得到結果的取消，連線在取消後可以繼續使用；看不到結果的（寫回被停止、借 pane 連線的 count 沒被接住）連線就不再使用，見「編輯與寫回」。
- 切句會照 session 的 `standard_conforming_strings`。設定為 off 時，一般字串裡的 `\'` 也是跳脫，所以字串裡的 `;` 不會被當成分隔，不會把字串的一部分當成另一句執行。腳本含 `\` 時才用 `SHOW` 查詢這個設定；執行到提到這個設定的語句，或 `RESET`、`DISCARD`、結束交易的語句（`COMMIT`、`ROLLBACK`…）之後會再查一次，模式變了就重新切剩下的腳本。交易已失敗時查不到，就沿用上次的結果。編輯器的「游標所在的句子」查不到 session，所以腳本含 `\` 時兩種模式都切一次：切出的句子不同，就不從游標執行，查詢 pane 會提示先選取要執行的句子（`sql::runnable_statement_at`）。
- schema 樹會列出 trigger（`pg_trigger`，不含外鍵等約束內部用的 trigger），說明欄是所屬的表。
- TLS 照 libpq 的語意：

| 模式 | 行為 |
| --- | --- |
| `disable` | 不用 TLS |
| `prefer` | 伺服器支援就用 TLS，不驗憑證 |
| `require` | 一定要 TLS，不驗憑證鏈（仍驗 handshake 簽章） |
| `verify-ca` | 驗憑證鏈，不驗主機名稱；只信任連線設定的 CA 檔，所以 CA 檔必填 |
| `verify-full` | 驗憑證鏈與主機名稱；信任 webpki roots 加上連線設定的 CA 檔 |

  CA 檔是 PEM，可以有多張，`~` 會展開。`verify-ca` 不使用公開根憑證：不驗主機名稱又信任公開 CA，等於任何網域的公開憑證都能通過。
- rustls 不接受本身是 CA 的伺服器憑證（`openssl req -x509` 自簽的通常就是這種），也不接受 X.509 v1 憑證；遇到時錯誤訊息會說明，請改用由 CA 簽發的伺服器憑證，或改用 `require`。

### SQL Server

- 使用 Yuzora 的 tiberius 0.12.3 patch（`app/vendor/tiberius-0.12.3-yuzora`）。
- 只在單獨一行的 `GO` 切 batch，後面可以接 `--` 或 `/* */` 註解；`GO 5` 這種帶次數的寫法不支援。一個 batch 可以有多個結果集。
- 一個 batch 的 affected 是所有非結果集 DONE 計數的總和，可能包含 trigger 或 procedure 內部的筆數。
- 寫回用 `UPDATE …; SELECT ROWCOUNT_BIG();` 確認每列剛好影響一列，不受 trigger 影響。
- schema 樹會列出 DML trigger（`sys.objects` 的 `TR`、`TA`），說明欄是所屬的表。
- 資料表 pane 產生的 SELECT、COUNT 與寫回都用三段式名稱 `[db].[schema].[table]`，db 是連線開啟時的 `DB_NAME()`（`Conn::database`）。所以 SQL 分頁在同一條連線上 `USE` 別的資料庫，也不會讀到或改到那邊同名的表。
- tiberius 沒有 attention API，所以取消（包括達到列數上限提早停止）的做法是斷開 socket，下一次使用時重新連線；之前在連線上用 `USE` 切換的資料庫狀態會遺失。
- TLS 對應：

| 模式 | 行為 |
| --- | --- |
| `disable` | `NotSupported` |
| `prefer` | `Off`：只加密登入。tiberius 在 `On` 遇到不強制加密的伺服器會 panic |
| `require` | `Required`，信任伺服器憑證 |
| `verify-ca`／`verify-full` | `Required`，有 CA 檔就用 `trust_cert_ca`。tiberius 固定會檢查主機名稱，所以 `verify-ca` 實際上等同 `verify-full` |

- named instance 一定要指定 port（沒有 SQL Browser）。
- `smalldatetime` 與 `datetime` 一律顯示成 `datetime`，因為 tiberius 分不出 nullable 的兩者。

### SQLite

- rusqlite（bundled）。所有工作都在 `spawn_blocking` 中執行，列以 channel 送回；取消用 `InterruptHandle`。
- 檔案必須已經存在，不會自動建立。`read_only` 用 `SQLITE_OPEN_READ_ONLY` 開啟，`busy_timeout` 是 5 秒。
- schema 樹只列 `main` 資料庫的物件。外鍵名稱是合成的（`fk_<id>`）。
- 切句時會把 `CREATE TRIGGER … BEGIN … END;` 當成一整句。

### DDL

結構分頁的 DDL 是用系統目錄重建的近似結果：PostgreSQL 與 SQL Server 不含 storage options、partitioning、owner。view、procedure、function 取自資料庫本身保存的定義（`pg_get_viewdef`、`OBJECT_DEFINITION`），SQLite 取自 `sqlite_schema.sql`。

## 效能

量測方式：debug build 的測試 binary，`cargo test -p yuzora-db --test engines -- --ignored --nocapture --test-threads=1`。fixture 是 `tests/database/docker-compose.yml`。資料經過 `RowSink` 收集，這也是 grid 讀資料的同一條路徑。

| 引擎 | 讀取量 | 第一批 | 總時間 | 吞吐 | 列資料記憶體 |
| --- | --- | --- | --- | --- | --- |
| PostgreSQL | 從 1,000,000 列的 `generate_series` 讀到約 100k 列後停止 | 38 ms | 148–162 ms | 62 萬–67 萬列／秒 | 約 15.5 MiB（161 B／列） |
| SQL Server | 約 100k 列後停止 | 4.7–5.1 ms | 590–594 ms | 約 17 萬列／秒 | 約 13.5 MiB（140 B／列） |
| SQLite | 1,000,000 列 | 0.4 ms | 439–478 ms | 209 萬–228 萬列／秒 | 約 101 MiB（105 B／列） |

grid 只繪製看得到的列（`uniform_list`），所以畫面成本與結果大小無關。預設上限是 100,000 列。PostgreSQL 一列約 161 B，所以一個裝滿的結果集大約 16 MB。

開表與 SQL 分頁從「使用者動作」到「第一批列、可編輯、總列數、Stop 結束」的時間，用 `crates/yuzora-db/tests/perf_open_table.rs` 量（全部 `#[ignore]`，每個樣本即時印一行 `PERF`）。app 與 fixture 之間放一個延遲轉送：每一塊資料在轉送裡等固定的單向延遲再放行，所以 pipeline 仍是 pipeline；沒有頻寬上限、掉包或抖動。轉送也記下每條連線的方向轉折，所以往返次數是量到的。量測用 `--config 'profile.dev.package.yuzora-db.opt-level=3'`（只重建這個 crate；依賴維持專案設定的 opt-level 2），絕對數字不能和 CI 的 opt-level 0 比。

```bash
YUZORA_DATABASE_TEST_ENGINES=postgres,mssql YUZORA_PERF_KEEP=1 \
  cargo test --locked -p yuzora-db --config 'profile.dev.package.yuzora-db.opt-level=3' \
  --test perf_open_table -- --ignored --nocapture --test-threads=1 perf_open_table
```

環境變數：`YUZORA_PERF_ENGINES`、`YUZORA_PERF_DELAYS`（單向毫秒，預設 `0,25`）、`YUZORA_PERF_TABLES`（`small,100k,1m,widecols,widetext`）、`YUZORA_PERF_MODES`（`cold,warm`，加 `cold-alone,warm-alone` 可去掉 pane 自己另外送的 `describe`）、`YUZORA_PERF_COLD`／`YUZORA_PERF_WARM`（樣本數）、`YUZORA_PERF_KEEP=1`（保留 fixture 資料庫 `yz_speed_pg`／`yz_speed_ms` 給下一次，預設結束時刪除）。其他測試：`perf_calibrate_the_delay_line`（先跑，確認轉送的延遲與往返次數）、`perf_phases_of_an_open`（連線、describe、SELECT、count 各自的時間）、`perf_sql_tab`（SQL 分頁與 Stop）、`perf_relay_overhead_on_a_big_result`。

## 實機畫面

隔離的 app home 與 `HC_SESSION`，`herdr-app shot` 截圖。資料是一個 SQLite 檔：customers 120,000 列、orders 30,000 列（含 FK 與兩個索引）、沒有 PK 的 audit_log、一個 view、一個 trigger。

- [l3-db-data.png](evidence/l3-db-data.png)：右側模式的連線與 schema 樹，orders 的資料分頁。
- [l3-db-structure.png](evidence/l3-db-structure.png)：結構分頁，含欄位、PK、索引、FK 與 DDL。
- [l3-db-query.png](evidence/l3-db-query.png)：SQL pane 一次執行四句，每個結果集一個分頁。
- [l3-db-connection.png](evidence/l3-db-connection.png)：新增連線對話框。

儲存格編輯與寫回沒辦法用 `herdr-app do` 操作，所以沒有截圖。這部分由 `tests/session.rs` 與 `tests/engines.rs` 對真實資料庫驗證。

## 已知限制

- SSH tunnel：等 L4。
- SQL Server：named instance 需要指定 port；取消會斷線重連；`GO n` 不支援。
- 排序只排已載入的列。
- 「其他物件」（function、procedure、sequence、trigger）只列在 schema 樹，還不能開啟定義。
- 含換行的文字與二進位值不能在表格內編輯。
- `yuzora.status.db`、`yuzora.badge.database` 是共用狀態，會寫進 `shared.json`。App 重開後，在右側模式的資料庫模式重新繪製之前，狀態列可能還顯示上一次的值。

## 後續整合

- L4：#190 提供 `yuzora_ssh::tunnel::open(TunnelSpec::new(ssh_host, host, port))`，回傳的 `Tunnel` 在 drop 時關閉。合併後，L3 在 `session::spec_for` 開 tunnel、把 `ConnectSpec::endpoint` 設成 `127.0.0.1:local_port`，並讓 `Tunnel` 跟連線池的那條連線同生命週期，然後拿掉 `refuse_tunnel`。兩個引擎驗證憑證時都已經用設定的 host，不用 endpoint，所以經過 tunnel 時 verify-full 不受影響。
- L6：`db.maxRows` 的設定頁（目前只能手改 `settings.json`）。
- L1：狀態列讀 `yuzora.status.db`，模式列讀 `yuzora.badge.database`。pane 標題目前顯示 mod 名稱（`yuzora-db-table`），應改成資料表或查詢名稱。
