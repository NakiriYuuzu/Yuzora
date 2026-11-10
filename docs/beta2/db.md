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
- 目錄：`databases(id)`、`objects(id, db)`、`describe(id, db, {schema, name})`；`tableInfo(jobId)`：`openTable` 開的那一次所讀的 describe（見「效能」）。
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
- 一個 pane 最多同時有三條連線，各管各的（`session::table_owner`、`session::count_owner`）：SQL 分頁的 `{owner}`、資料表的 `{owner}:table`、算總列數的 `{owner}:count`（算完就關）。資料表的連線只執行 app 自己組出來的語句（開表的 SELECT、型別查詢、寫回），SQL 分頁留在它那條連線上的東西（暫存表、`SET search_path`、`USE`，以及任何 `SET`）因此到不了資料表，資料表的設定也由 app 決定，不是留給 SQL 分頁的腳本。尚未結束的交易也留在 SQL 分頁的連線上，資料表讀不到它改的東西，但它持有的鎖仍會擋到同一個 pane 的資料表，見下面「還在的」。`release(owner)` 三條一起關。
- 開表時 `COUNT(*)` 在列讀完後、工作結束前執行，所以按「停止」也能停掉 count。count 用自己的連線（`session::count_owner`，count 完就關），不佔資料表的連線：列讀完、決定好可不可編輯（`Editing`）之後 grid 就能編輯，寫回也不用排在 count 後面。
- count 是輔助資訊：它的連線開不了（登入或資料庫有連線數上限、網路抖動）時，不改變連線的狀態（側欄不會變成「連線失敗」），改借 pane 的資料表連線、等它空閒時算，總列數照樣出現。借來的 count 永遠讓路：寫回、開表、重新整理一向 pane 的資料表連線要連線（SQL 分頁的查詢不在這裡：它在自己的連線上，不排在 count 後面）（`session::lease`，`Live::want`），count 就被叫停（`StandDown`，由 count 自己那個 task 取消自己的 statement 再放開連線），工作拿到連線；工作結束後 count 從頭再算一次（`count_between`）。使用者不會為了 count 等待。PostgreSQL 的取消若沒有被 count 的那句 statement 接住（伺服器沒回 `canceled`：count 剛好先結束，或伺服器在動作之前就先回覆了取消連線，像連線池那樣），count 的連線會被丟掉，下一個使用者開新連線（`Lease::retire`、`Conn::cancel_landed`）；接住了就留著，不用重連。
- 開表的 describe（欄位、主鍵、索引、DDL）在「開表開始的當下」就在 sidebar 的連線（META）上送出，和資料表連線（`{owner}:table`）上的 `SELECT` 同時進行，列不等它：列讀完就顯示（`complete`），`Editing` 仍是 `Loading`，describe 回來後才決定可不可編輯（主鍵、產生欄），所以「可編輯」的時間是 max(讀取, describe)。describe 沒回來時按 Stop：工作馬上結束，列保留、唯讀，原因是「could not read the table's key (stopped before it was read)」（META 連線不是工作能取消的，describe 自己會在 META 上跑完，結果丟掉）。連線是唯讀、或讀取失敗時不等 describe。
- 每次開表（含重新整理）都重新 describe，**不快取**：重新整理一定拿到表現在的主鍵，改過主鍵的表不會用舊的主鍵寫到錯的列（`sqlite_a_refresh_after_the_primary_key_moved…`）。Structure 分頁不再另外送 `describe`，改用 `tableInfo(jobId)` 取同一次的結果（`defaults/mods/yuzora-db-table/main.js` 的 `infoJob` 只認最新一次開表）；job 已被取代或不存在時 `tableInfo` 回 `NotFound`。
- 每個「停止」只停它旁邊標示的那件事。工作的停止（工具列）只取消工作當下正在跑的那句：讀列時是資料表的連線（`{owner}:table`），count 時是 count 的連線（`Job.serving`；借資料表連線的 count 是請它自己放手，`Stop::Count`），讀完之後不再碰資料表的連線，因為那上面可能正在寫回。寫回的停止（footer）只結束那次寫回。SQL 分頁的查詢在自己的連線上，有自己的語句和自己的 Stop；工作仍是一個 pane 一個（`new_job` 先 `supersede`），所以在 SQL 分頁按 Run，會取消同一個 pane 還在讀的資料表或還在算的 count，並取代它的結果集。寫回不是工作：它在等列鎖時 SQL 分頁照常執行，兩邊的 Stop 互不影響（`postgres_a_sql_tab_statement_and_a_waiting_write_back_are_stopped_apart`、`mssql_a_sql_tab_statement_and_a_waiting_write_back_are_stopped_apart`）。
- 連線還在建立時按「停止」也有效：引擎先把自己的取消機制準備好，然後在每個 statement 執行前檢查工作是否已經停止（`RowSink::cancelled`）。所以 UPDATE、DELETE 這類不回傳列的語句，不會在回報取消之後才執行。
- 讀取是串流的：引擎每 512 列（`engine::BATCH`）交給 `RowSink` 一次，grid 立即重畫，不等整個查詢結束。
- 上限 `db.maxRows`（預設 100,000，可設 100～5,000,000）。真的有列放不下時才停止讀取並標成 truncated（剛好等於上限不算）；開表會多讀一列（`LIMIT maxRows + 1`）來判斷。
- 排序在用戶端，只排已讀進來的列（排的是檢視的索引，不搬資料）。整數與數字文字逐位精確比較，不經過 f64，所以超過 2^53 的 bigint 和高精度 numeric 也排得對。NaN 排在所有數字之後（和 PostgreSQL 一樣）。PostgreSQL 的 money 依數值排序：只把數字之間的 `.`、`,` 當分隔，所以 `$1,234.56`、`1.234,56 €`、`￥1,234` 都會正確排序。查詢還在串流時就排序，之後到的列也會依排序併入。被截斷的結果要完整排序，請在 SQL 加 `ORDER BY`。
- 開表時另外跑一次 `COUNT(*)`（MSSQL 是 `COUNT_BIG`），狀態列顯示「X 列 · 顯示 a–b · N ms」。

## 編輯與寫回

- 結果集的可編輯狀態是 `results::Editing`：`Loading`（開表的列還在讀）、`Editable`、`ReadOnly(原因)`；寫回狀態是 `WriteState`：`Idle`、`Writing`（可用 `stop_write` 停止）、`Failed(訊息)`。每個唯讀都有一句原因（包括 SQL 查詢的結果集，`edit.query`），每個失敗都有訊息，雙擊被擋時 footer 會說明原因。
- 寫回排在資料表的連線後面，也會被伺服器上別的連線持有的列鎖擋住（PostgreSQL、SQL Server 預設不逾時）。等待期間 footer 顯示「Writing…」加「Stop」，沒有「Discard」（修改正在寫，丟掉它可能不算數；要捨棄先 Stop，之後 footer 會出現 Discard；`ResultSet::discard_edits` 在寫回中也不動作）：Stop 結束等待，PostgreSQL／SQLite 另外叫伺服器取消那句 UPDATE，SQL Server 則是丟掉 socket（伺服器回滾）。停止後編輯仍保留，結果集說「沒有寫入任何東西」（`edit.stopped`）；只有 COMMIT 已經送出時才說「資料庫可能已經存了」（`edit.stopped_maybe`）。判斷靠各引擎在送出 COMMIT 之前一刻設起的 `CommitSent`（PostgreSQL：`tx.commit()` 之前；SQL Server：`COMMIT TRANSACTION` 批次之前；SQLite：最後一次看有沒有停止之前設起，看到停止就收回再回滾）。設在送出之前而不是之後，所以誤差只會偏向「可能已存」，不會在 COMMIT 已經進行時說沒寫。SQLite 的 COMMIT 是同步呼叫、`CommitSent` 與停止的先後可能讓訊息偏向「可能已存」一個極短的窗口，反方向不會發生。
  - 保證：不論 Stop 和語句結束的先後，停止之後（a）沒有資料被寫入，修改留在畫面上；（b）資料表的連線可用，下一次開表或寫回正常執行；（c）取消不會落在這次寫回以外的語句上。唯一的例外是 Stop 在 COMMIT 已經送出之後才到：那筆交易由伺服器決定，資料可能已寫入，所以訊息才說「可能」，重新整理就知道。這些保證的前提在下面「PostgreSQL：晚到的取消」：取消請求送到連線實際連上的那個位址，而且那端處理完才關閉連線。
  - 取消是「連線層級、打到當下正在跑的語句」的請求，送出時不知道語句還在不在；PostgreSQL 的取消還走另一條連線，伺服器收到才動作。所以「停止做完了」是伺服器處理完那個請求的時候，不是送出的時候，見下面「PostgreSQL：晚到的取消」。寫回被停止時，先通知引擎、等它做完，寫回才放開資料表的連線（`session::stop_work`），所以丟掉交易時送出的 ROLLBACK 是伺服器停止之後聽到的第一件事，取消不會落在它上面。PostgreSQL 資料表的連線在這之後仍不再使用（`Canceller::abandon`：寫回的 future 是在中途被丟掉的，不假設連線是乾淨的）。下一次開表或寫回才開新連線（SQL Server 停止時也是丟掉資料表那條 socket）；SQL 分頁在它自己的連線上，它的 `SET`、暫存表、尚未結束的交易都不受影響（資料表與 SQL 分頁共用一條連線的時候會隨之消失）。登入有連線數上限時，緊接著的下一次開表或寫回可能撞上舊連線還沒結束的那一小段時間而被拒絕一次。
  - PostgreSQL：晚到的取消。取消請求走另一條短連線，伺服器收到時對連線當下在跑的東西動作：語句剛好跑完的話，那就是下一句（下一次 Run、換表、count、寫回的 UPDATE 或 COMMIT）。驅動寫出請求就返回，所以 Stop 落在查詢結束前後 2 毫秒內時，下一次 Run 在 87／150 輪被取消（沒有任何延遲，直接走 session 量到）。協定文件（55.2.8）說伺服器「處理這個請求，然後關閉連線」；後端閒置時收到訊號，`ProcessInterrupts` 因為正在等客戶端的指令（`DoingCommandRead`）而直接忽略它（`postgres.c`）。所以只要在送下一句之前等到伺服器關閉那條取消連線，取消不是已經打到該停的語句，就是落在閒置的後端而消失。做法：請求送到這條連線實際連上的位址：連線是一個位址一個位址連的（主機名稱解析出多個位址時依序試，任何失敗換下一個，最後一個錯誤是結果，和驅動原本一樣），連上的那個位址記在這條連線的 `Target`（`ConnectSpec::addresses` 可以直接指定要試的位址，取代解析，測試用它在沒有 DNS 的情況下造出多位址的主機），取消用同一個位址和同一個 TLS 名稱，不再重新解析主機名稱（DNS 輪詢、failover 後換了位址、`localhost` 的 `::1` 與 `127.0.0.1` 是不同的伺服器時，重新解析會把取消送到別台，伺服器關閉連線不理它，Stop 就沒有作用）。`PgCanceller` 用自己的連線送請求（`cancel_query_raw`），`UntilClosed` 讓 shutdown 要等到對方關閉才算完成，`Canceller::cancel` 因此在對端關閉那條取消連線時返回（含 TLS）；連線層分不出對端是處理完才關閉，還是不理它就關閉；工具列的 Stop 沒人能等，`Canceller::cancel_in_background` 在按下的當下就把請求記為「在路上」（`PgCanceller::on_their_way`，不是等送出的 task 開始跑），`session::open_lease` 在把連線交給下一件工作前先 `Conn::settled`。等不到伺服器處理（5 秒、連不上）的請求不知道會不會到，連線就丟掉（`OnItsWay` 沒被確認就 drop → `retired`）。取捨：停止之後的下一句多等一次取消連線的來回（本機量到 2–13 毫秒），不確定時多花一次重連；只在不確定時才丟連線，已確認處理完的連線留著，所以 SQL 分頁連線上的 `SET`、暫存表不會無故消失。這一整套以連線為單位：SQL 分頁的 `{owner}`、資料表的 `{owner}:table`、count 的 `{owner}:count` 各自記自己連上的位址（`Target`）、各自計算在路上的取消、各自在被交給下一件工作前等 `settled`、各自在 5 秒沒被確認時退役（`session::open_lease` 的單位是 `(connection, database, owner)`），所以資料表的 Stop 不會等、也不會取消 SQL 分頁那條連線上的請求，反過來也一樣（`postgres_a_stop_on_the_tables_connection_is_settled_there_and_holds_back_only_the_table`）。位址對了，但對端不是處理請求的那個後端時，「對端關閉」仍會被當成處理完：中間若有負載平衡，請求轉到別的後端，後端不認得這組 pid 和密鑰就直接關閉，Stop 沒有生效，語句自己跑完；中間若有連線池在轉送之前就先關閉連線，取消可能晚到並打中下一句，也就是這個保證出現之前的行為：下一句失敗（57014），寫回則整筆回滾、畫面說失敗，不會寫一半。只有 count 讓路的那條路（`cancel_landed`）會把沒被接住的連線丟掉，其他工作沒有這道保險。Unix socket 的主機只送出、不等（驅動原本的行為）。多主機設定（`host=a,b`）一直不支援：整串被當成一個主機名稱，解析失敗，訊息和以前一樣。SSH tunnel 的本機端（`ConnectSpec::endpoint`）就是連上的位址，主機設定只用來驗證 TLS 名稱，兩者都不重新解析。
  - SQLite：取消是 `interrupt`，只對當下正在執行的 statement 有效，沒有 statement 在跑時什麼也不做，所以不會落到別處。SQLite 等檔案寫鎖時 `interrupt` 到不了，所以等鎖是寫回自己做的（`busy_timeout` 設 0，每 10 毫秒重試、看有沒有停止，最多 5 秒），並在 `BEGIN`、每句 UPDATE、COMMIT 前看有沒有在寫回要求之後停止過（`LiteCanceller.stops`，計數而不是會被下一件工作重設的旗標）。停止後鎖才放開也不會寫入，連線也不會被卡在那段等待上。
  - SQL Server：取消就是丟掉 socket（`apply_edits` 的 future 一被丟掉 socket 就跟著關），伺服器在連線結束時回滾；沒有可以晚到的取消，下一次使用開新連線。`execute` 一開始就訂閱停止的計數，之前的計數不算，所以停止落在兩句之間時對下一句沒有作用。工具列的 Stop 以前是丟進 runtime 的一個 task 才去記這個計數，task 還沒跑時下一句已經訂閱了，那一下就會落在它身上；現在在按下的當下就記（`Canceller::cancel_in_background`）。SQLite 的 `interrupt` 同樣在按下的當下呼叫。
- Cmd+S（Ctrl+S）等於「Write back」，編輯框還開著時也一樣：先保留那一格（等於按 Enter；NULL 格沒打過字就按 ⌘S 不算修改，這點和 Enter 不同），再寫回；內容被資料庫拒絕（例如型別不合）時，原因自動展開在 footer 上方。輸入法正在組字（注音、拼音的 marked text 還沒選字）時，沒選的字不算打進儲存格的內容，不會被寫進資料庫。判斷是 `InputState::marked_text_range()` 有值，規則有兩層：⌘S 與 Enter 是輸入法的按鍵，組字中不動作、編輯框留著（組字中的 Enter 是輸入法在選字，不保留儲存格；macOS 的輸入法通常自己吃掉這顆 Enter，這是給沒吃掉它的平台的保險）；其他會把編輯框的字變成儲存格的動作（點「Write back」、點別的儲存格、點欄標題排序、編輯框失去焦點）不擋，但都經過 `commit_editing`，它是編輯框的字變成儲存格的唯一入口，只留組字範圍之外的字：已經選好的字照留，沒選的字丟掉。一個 NULL 格只組字、沒選任何字就離開，維持 NULL。去掉組字之後剩下的字才交給 `ResultSet::type_into`，所以「清空等於 NULL」的規則（下面「清空儲存格」）看的是組字範圍之外的字：數字、日期這類欄位，原值被選取後直接開始組字、沒選字就離開，剩下空字串，格子變 NULL，組字的字不會寫進去；原本是 NULL 的格子同樣只組字就離開，維持 NULL；選好的數字照留，仍是數字（`grid::tests::composing` 的 `a_number_cell_cleared_for_an_input_method_…`、`a_null_number_cell_that_was_only_composed_in_…`、`a_number_chosen_with_an_input_method_…`）。只在 gpui 的 test platform 驗證；macOS 真輸入法在滑鼠點擊時會先提交組字、丟棄，還是保留，沒有實機驗證（先提交的話，那些字已經是打進去的字，會照畫面寫）。
- footer 的訊息（寫回失敗、唯讀原因）只顯示第一行，點一下展開全文；新的寫回失敗會自動展開（伺服器的 DETAIL、HINT 行在第二行以後）。展開的訊息會依 pane 寬度換行。
- Database dock 開著時，表格 pane 只有約 110–230 px 寬：footer 的狀態文字各佔一行、太長就截斷，Discard／Write back（寫回中只有 Stop）按鈕放不下時換到下一行，只要 pane 比 footer 本身高，就完整在 pane 內，不會被切掉（110 px 寬時 footer 約 115 px 高）。展開的失敗訊息有 140 px 的高度上限、可捲，pane 放不下時它先縮，不推擠 footer，所以按鈕和「▾ 訊息」開關永遠在；代價是 pane 很矮時表格區被壓到 0，收起訊息就回來。工具列也一樣：資料表名稱和分頁放不下時換行，Run／Stop 按鈕在最後一行靠右。
- SQL Server 的連線被伺服器關掉（閒置逾時、防火牆、重啟、`KILL`）時，下一次使用才會發現：寫回的第一句（檢查未結束交易）和 metadata 查詢遇到這種斷線會自動換一條新連線重試一次（這時還沒寫入任何東西）；SQL 頁籤的語句不重試，因為答案遺失不代表語句沒執行。
- SQL Server 驅動（tiberius）讀不了 `geography`／`geometry`／`hierarchyid` 等 CLR 型別和 `sql_variant`：開這種表會以 `unsupported column type` 結束，不會再卡在載入。要看這些欄位請在 SQL 頁籤自己轉型（`col.ToString()`、`CAST(col AS nvarchar(max))`）。

- 只有「開表」產生的結果集可以編輯，而且資料表必須有 primary key；沒有就停用並顯示原因（`edit.no_key`）。連線設成唯讀（SQLite 的 `readOnly`）時也會停用（`edit.read_only`）。
- 由資料庫計算的欄位不能編輯（`grid.generated`），包括產生欄、計算欄，以及 PostgreSQL 的 `GENERATED ALWAYS` identity 和 SQL Server 的 identity。否則寫回時整筆交易都會被伺服器拒絕。`describe` 的 `ColumnInfo.generated` 標示這類欄位。
- 雙擊或 Enter 編輯，⌘⌫ 設 NULL，Esc 取消。打開編輯框卻沒改內容就離開，不算修改；改回原值也等於沒改。NULL 儲存格的編輯框是空的：在裡面打過字或按了 Enter，留空就寫成空字串；只打開就離開，仍然是 NULL。
- 清空儲存格：型別沒有「空」這個值的，清空就是 NULL（`model::clears_to_null`）；格子在寫回之前就顯示 NULL，不是等寫回之後才變。數字、布林、日期與時間、uuid、json、inet 等（SQL Server：`bit`、`tinyint`～`bigint`、`real`、`float`、`decimal`、`money`、`date`、`time`、`datetime`、`datetime2`、`smalldatetime`、`datetimeoffset`、`uniqueidentifier`；PostgreSQL：`int2`～`int8`、`float4`、`float8`、`numeric`、`money`、`oid`、`bool`、`date`、`time`、`timetz`、`timestamp`、`timestamptz`、`interval`、`uuid`、`inet`、`cidr`、`macaddr`、`json`、`jsonb`）都是這樣；文字型別（`char`／`varchar`／`nvarchar`／`text` 等）仍然寫成空字串。欄位不允許 NULL 時由伺服器回它的錯誤，不改寫成別的值。以前 SQL Server 把清空的日期寫成 `1900-01-01`、數字寫成 `0`（伺服器對空字串的轉換），PostgreSQL 的非文字型別則是被伺服器拒絕。SQLite 沒有型別，行為不變。
- 編輯框只有一行：含換行的文字和二進位值不能在表格內編輯（底部會說明），請在 SQL 分頁用 `UPDATE`。
- 有未寫回的修改時，換表、重新整理或執行 SQL 會先在 pane 頂端詢問「繼續編輯」或「捨棄並繼續」。問句和按鈕會依 pane 寬度換行，兩個選項都在 pane 內。修改正在寫回時（`ResultSummary.writing`）不提供「捨棄」：寫回已經送出，修改可能照樣寫進資料庫，說「捨棄」會讓人以為沒有寫；改問「繼續等待」或「停止儲存」（`stopWrite`，修改保留，之後可以再按「捨棄並繼續」）。問的當下的修改已經寫完或被捨棄時，這一列自己消失，不會自己繼續換表。
- 寫回期間又改的儲存格保留為待寫回，不會被當成已寫入。寫回還沒結束時再按一次寫回會被拒絕（`edit.writing`），所以同一批修改不會寫兩次。
- 寫回只送到讀取這些列時的連線設定。表打開之後，如果已存的連線改成連到別處（另一台伺服器、另一個資料庫或另一個檔案），寫回會被拒絕（`edit.connection_changed`），修改仍保留為待寫回；重新整理資料表後再編輯。判斷方式和存檔時是否重連相同（`ConnectionConfig::connects_like`），在連線前後各檢查一次。
- 資料表的連線上有尚未結束的交易時，不會寫回（`edit.open_transaction`），修改仍保留為待寫回。這條連線只有 app 自己下語句，SQL 分頁的 `BEGIN` 在另一條連線上、到不了這裡，所以這一關現在是後盾：擋的是登入或伺服器預設替這條連線開的交易，以及 trigger 之類留下來的。原因是 PostgreSQL 會忽略寫回的 BEGIN，寫回的 COMMIT 會連那個交易裡別人的修改一起提交；SQL Server 的巢狀 COMMIT 只會把計數減一，之後那個交易一 ROLLBACK，寫回就跟著消失。判斷方式：PostgreSQL 看 `now() = statement_timestamp()`，SQL Server 看 `@@TRANCOUNT`，SQLite 看 autocommit。訊息說：資料表的連線上還開著一個交易、什麼都沒寫、這個 app 沒有開它（trigger 或伺服器自己的設定開的）；要關閉這個資料表再開一次：那會結束這條連線，伺服器把交易回滾。不叫人 COMMIT 或 ROLLBACK，也不叫人去 SQL 分頁收尾：那不是同一條連線，使用者沒有辦法在這條連線上下語句（`a_refusal_for_an_open_transaction_asks_only_what_a_person_can_do_on_the_tables_connection`）。
  - SQL Server 連線若自己會開交易（`IMPLICIT_TRANSACTIONS` 開著，`ANSI_DEFAULTS` 也會開），任何碰到資料表的語句，包括開表的 SELECT 和寫回前的型別查詢，都會先開一個交易，寫回的 `BEGIN` 再開一個，`COMMIT` 只減一：寫回回報成功，連線一關資料就沒了。所以寫回的第一句問 `SELECT @@TRANCOUNT, @@OPTIONS & 2`（也是偵測斷線的那一句）：有交易開著就拒絕，連線會自己開交易時用另一句話（`edit.open_implicit_transaction`：交易還開著、什麼都沒寫；連線會自己開交易，開著的那一個可能包含沒有人提交的修改；關閉這個資料表再開一次，連線結束、伺服器連同裡面的東西一起回滾）；沒有交易開著就在寫回期間 `SET IMPLICIT_TRANSACTIONS OFF`，不論成功、拒絕、key 對不到或伺服器出錯，結束時都設回寫回之前這條連線原來的狀態（這條連線上沒有 SQL 分頁的設定：資料表有自己的連線）。`COMMIT` 和 `SELECT @@TRANCOUNT` 在同一批，不是 0 就當失敗並回滾（`edit.not_committed`）：不拿「沒有收到錯誤」當成功。這是第二道防線：拿掉 `SET IMPLICIT_TRANSACTIONS OFF` 時，是它讓結果從靜默遺失變成明確失敗（mutation 實測）；它自己沒有常駐的獨立測試，也不是每個保證都做過 mutation：這一項拿掉之後 mssql 的測試全綠（lib 12、engines 22、writeback 18，r3 審查實測）。原因是找不到走得到它的情形：寫回的 UPDATE 是 RPC，會讓計數不歸零的 trigger，在 UPDATE 回來時伺服器自己的檢查（Msg 266）就先擋下了，而 COMMIT 之前連線的計數已經由第一句（`@@TRANCOUNT`）確認過是 0、中間沒有別人在這條連線上下語句。試過 trigger 裡 `BEGIN TRANSACTION`、`EXEC('BEGIN TRANSACTION')`、`COMMIT` 後兩次 `BEGIN`，都是 Msg 266；trigger 裡 `SET IMPLICIT_TRANSACTIONS ON` 出了 trigger 就還原，計數不變（SQL Server 16，sqlcmd 實測）。這一關留著當最後的防線，不假裝它有測試。
  - SQL 分頁自己設了 `IMPLICIT_TRANSACTIONS`（或 `ANSI_DEFAULTS`）只留在它那條連線上，不再讓資料表的寫回被拒絕，也不再需要靠「SQL 分頁的任何語句都會取代 pane 的結果」（`session::supersede`）這個論證：資料表看不到 SQL 分頁設的任何東西（`mssql_what_the_sql_tab_sets_does_not_change_what_the_table_reads_and_saves` 逐一試過 20 種狀態）。代價見下面「資料表與 SQL 分頁分開連線」。
  - 走得到的路徑是換了一條新連線：讀取被 `maxRows` 截斷、讀取被 Stop（`Flow::Enough` 丟掉 socket），或連線閒置被伺服器關掉（`apply_edits` 的第一句失敗後換新連線）。新連線帶的只有伺服器層級的預設；伺服器預設開著 implicit（`sp_configure 'user options'` 含 2）時，寫回當下 `@@TRANCOUNT` 為 0 而 implicit 開著，修正前就是靜默遺失，現在照上面的做法存成功（那條新連線本來就沒有被預設開過交易）。伺服器預設本身（`sp_configure` 是整台伺服器的設定，fixture 與其他 lane 共用，沒有動）用測試模擬：在資料表的連線上先 `SET IMPLICIT_TRANSACTIONS ON`（或 `ANSI_DEFAULTS ON`）。
  - 同樣的伺服器預設加上沒有被截斷的讀取：每次開表都會留下一個交易，每次寫回都被拒絕，而資料表的連線上沒有 SQL 分頁能去收尾（共用連線時，使用者還可以去 SQL 分頁 `COMMIT` 加 `SET IMPLICIT_TRANSACTIONS OFF`；分開之後這條路不見了，所以不能留著不處理）。做法：開表的 SELECT 前面在同一批加上 `SET IMPLICIT_TRANSACTIONS OFF; `（`engine::table_prelude`，只有 SQL Server；同一個 TDS 請求，沒有多一次往返），這條連線是 app 的，由 app 決定它不開交易。測試 `mssql_a_table_connection_that_opens_transactions_by_itself_still_saves`：在資料表的連線上先把 `IMPLICIT_TRANSACTIONS` 或 `ANSI_DEFAULTS` 開起來，開表、改、寫回都成功，連線上沒有留下交易；拿掉前綴，這個測試變紅，訊息就是「交易還開著」。
  - PostgreSQL 與 SQLite 沒有對應的模式：兩者的連線不會自己留在交易裡（PostgreSQL 的 psql autocommit 是客戶端的設定；SQLite 只有 `BEGIN`）。使用者在 SQL 分頁自己的 `BEGIN` 在另一條連線上，不會讓資料表的連線落在交易內（它持有的鎖仍會擋到資料表，見下面「還在的」）；寫回回報成功後另一條連線讀得到、資料表的連線不在交易內，有測試（`sqlite_`／`postgres_a_write_back_leaves_no_transaction_open_and_another_connection_reads_it`）。
  - 資料表與 SQL 分頁分開連線（`session::table_owner`）。前提：資料表讀寫用的 session 設定要由 app 決定，不能留給 SQL 分頁的腳本去改。以前兩者共用一條連線，所以腳本設了什麼就影響資料表讀到什麼、存進什麼：SQL Server 的 `SET ANSI_WARNINGS OFF` 讓過長的文字（`abcdefgh` 進 `nvarchar(5)`）被靜默截成 `abcde`、溢位的數字（300 進 `tinyint`）被存成 `NULL`，而不是被伺服器拒絕（Msg 2628、8115），`SET ROWCOUNT 1` 讓只讀到 1 列，`SET TEXTSIZE 8` 把 5000 字元的儲存格讀成 8 個，PostgreSQL 的 `client_encoding LATIN1` 讓 5 個字存成 10 個位元組，`datestyle 'German, DMY'` 讓 `timestamptz` 差 14 小時，`array_nulls off` 把 NULL 元素存成字串 `'NULL'`，`default_transaction_read_only on` 讓所有寫回被拒絕。一個一個設定去釘（以前 PostgreSQL 的 `read_table` 重設 `datestyle` 與 `extra_float_digits`）永遠漏掉下一個沒想到的。
  - 三個做法：(1) 資料表自己一條連線（採用）；(2) 每次開表或寫回前重設連線（`RESET ALL`／`DISCARD ALL`）；(3) 列出要釘的設定。(2) 不採用：會一併毀掉 SQL 分頁要保留的暫存表、`search_path`、`USE`，也沒有辦法在不丟掉尚未結束的交易下做；SQL Server 沒有可以在使用者的語句之間呼叫的等同做法（`sp_reset_connection` 是連線池內部用的，同樣會清掉暫存表）。(3) 不採用：清單永遠不完整，每次伺服器版本多一個設定就多一個洞。(1) 的代價是：用過 SQL 分頁又開了表的 pane 比以前多一條連線（只開表、沒用 SQL 分頁的 pane 連線數不變：開表原本就用一條 pane 的連線，現在是資料表那條）；有連線數上限的登入要把這一條算進去，count 那條仍然算完就關。只開表的 pane：延遲 25 毫秒的中繼上量到的交換次數與連線數，改前改後相同（PostgreSQL 冷 45／暖 25，SQL Server 冷 30／暖 18）。用過 SQL 分頁又開表的 pane，會多付一次連線建立（`perf_phases_of_an_open` 量到的 connect：PostgreSQL 11 次交換、約 0.5 秒，SQL Server 6 次、約 0.35 秒，都是延遲 25 毫秒下）。連線數上限很緊的登入（PostgreSQL `CONNECTION LIMIT 2`：側欄一條、資料表一條）上，開著資料表的 pane 再用 SQL 分頁會因為沒有第三條連線而失敗：SQL 分頁顯示伺服器的錯誤（`too many connections for role … (53300)`），側欄的連線狀態也跟著顯示失敗（直到下一次連線成功），SQL 分頁的語句會取代 pane 的結果，資料表要再開一次（它的連線還在）才能繼續；測試 `postgres_a_sql_tab_beside_a_table_with_no_connection_to_spare_says_why_and_leaves_the_table_alone`。反方向也會發生：資料表的連線退役之後（寫回被 Stop、伺服器閒置斷線、網路中斷），若 SQL 分頁在這之後連上，名額就被它佔走，同一個 pane 再開表連續回 `too many connections`（r3 審查實測，上限 2），只有關掉 pane（`session::release_owner`，三條一起關）才恢復，再開一個 pane 就開得了。錯誤訊息是伺服器的，沒有說名額是被同一個 pane 的 SQL 分頁佔走的。
  - 還在的：伺服器或登入層級的預設，是任何用這個登入連線的客戶端都會看到的（app 沒有釘的設定，例如 SQL Server `ARITHABORT` 的伺服器預設）；trigger 若呼叫 `set_config(…, false)` 還是可能改到資料表的連線（少見，且是伺服器端的程式在改）；資料表看不到 SQL 分頁尚未提交的資料（以前共用連線時看得到），而且 SQL 分頁開著的交易會用鎖擋住同一個 pane 的資料表（以前共用連線，自己的鎖擋不到自己）：SQL Server 的預設 `LOCK_TIMEOUT` 是 -1，被擋的讀取（SQL 分頁的 `BEGIN TRANSACTION; UPDATE …` 沒提交，再回 Data 按 Run）或寫回會一直等，畫面停在 Loading／Writing，r3 審查量到 8 秒後仍在等、伺服器上有一個被擋住的請求，必須按 Stop 才會放開，之後到 SQL 分頁 `COMMIT` 或 `ROLLBACK` 再開表就正常；SQLite 的資料表寫回約 5 秒後回 `database is locked`，只讀的 `BEGIN; SELECT …` 沒結束也會這樣；PostgreSQL 的讀取不受影響（強鎖如沒提交的 `ALTER TABLE` 除外），寫回改到交易改過的那一列時會等，直到按 Stop 或交易結束（推論，機制與 `postgres_a_write_back_waiting_on_a_row_lock_can_be_stopped` 相同）。以前的行為是寫回被立刻拒絕、並叫人去 SQL 分頁收尾。這一條沒有程式處理，只寫在出貨指南（`app/defaults/AGENTS.md`）；要處理的話，可以在資料表連線的讀取加有限的 `LOCK_TIMEOUT`，或讓 SQL 分頁在資料表工作進行中仍能執行。
- 底部顯示「N 個儲存格已修改 · 捨棄 · 寫回」。寫回在一個交易內，每列用 primary key 定位，每列影響的列數必須剛好是 1，否則整筆 rollback 並顯示錯誤。key 是 NULL 的列拒絕寫回。
- PostgreSQL 用文字格式參數寫回（server 依欄位型別解析），所以 numeric、interval、enum、array 等都能原文 round-trip。
- 數字儲存格改成同一類的數字時，寫回的仍是數字：整數儲存格要能讀成 i64，浮點儲存格要能讀成有限的 f64（`Cell::edited`）。其他情況都以文字寫回，由伺服器轉換或拒絕。這是因為 SQLite 沒有型別（或 BLOB affinity）的欄位不會轉換型別，用文字寫回會把整數 5 變成文字 `'7'`。整數儲存格改成 `7.5` 仍然送文字，因為 SQL Server 收到 float 參數時，會在 int 欄位把它靜默截斷成 7。
- 日期文字不依登入語言。SQL Server 的 `datetime`／`smalldatetime` 從文字讀日期時依 session 的 `DATEFORMAT`：British、German、French 等登入是日先月後，`2024-01-06` 會被讀成 6 月 1 日，13 日則報超出範圍，`datetime` 當主鍵時找不到列；年在前但伺服器照自己的規則讀的寫法（`2:30 PM`、`9:5`、`10:00:00:5`）同樣會被對調而且畫面顯示存檔成功；時間寫在日期前面（`10:00 2024-01-06`）也一樣。所以規則不是「列出認得的寫法，其餘交給伺服器」，也不是「只看開頭」，而是問：文字裡的任何位置有沒有年在前的日期（`engine/date_text.rs` 的 `classify`）。有，就由用戶端完整解析，解析不了就拒絕，不交給伺服器猜；沒有，才交給 session。結果只有兩種：存進去的就是使用者的意思，或送出前被拒絕。送往這兩種型別的欄位（SET 與 WHERE 都是）才適用。
  - 封閉的表，每段文字落在恰好一列：
    | 文字 | 處理 | 為什麼 |
    | --- | --- | --- |
    | 裡面沒有年在前的日期（`1/6/2024`、`Jan 6 2024`、`10:00`、`2024 Jan 6`、`240106`） | Session：原樣送出 | 人自己的日期寫法本來就由登入語言讀；月份名稱或沒有日的年，六種 `DATEFORMAT` 讀法一致 |
    | 有年在前的日期，而且只有那個日期、它前面或後面的時間、時間後面緊貼的時區 | Client：改寫成 `YYYY-MM-DDThh:mm:ss[.f][zone]` 送出 | 這個寫法在任何 `DATEFORMAT`、任何語言下讀法相同；時區交給伺服器接受或拒絕 |
    | 有年在前的日期，而且有任何別的東西 | 拒絕（`edit.bad_date`） | 沒有確定的讀法 |
  - 「年在前的日期」是：四位數字（前後不貼著別的數字），接著可有空白、`-`、`/` 或 `.`，再接一個數字（伺服器連 `2024- 01- 06` 都讀，日先月後的登入會讀反）；或八位數字（`20240106`）。
  - 認得的：`2024-01-06`、`2024/1/6`、`2024.01.06`（兩個分隔符要一樣、中間不能有空白）、`20240106`，時間可以在日期前面或後面，用 `T` 或空白接；時間是 24 小時制 `H:MM[:SS[.f]]`（小數 1～9 位，分和秒都是兩位），或 12 小時制加 AM／PM（`2:30 PM`、`10AM`、`12:00 AM` 是 0 點）；時間後面可緊貼 `Z` 或 `±hh:mm`（12 小時制不行）；日期必須是真的日曆日期。轉成 `2024-01-06T14:30:00` 送出（有 `Z` 就帶著）。ISO 8601 的 `T`、`Z`、小數秒因此都存得進去（上一輪的修正把 `Z` 擋掉，這一輪還原）。
  - 時區偏移（`+08:00`、`+00:00`）原樣交給伺服器，`datetime`／`smalldatetime` 沒有放偏移的地方，伺服器自己拒絕（Msg 241，六種語言下都一樣）：不換算成別的時間，也不假裝存了。`Z` 伺服器接受，當成沒有時區的那個時間。
  - 不認得的一律拒絕，在送出任何語句之前（`edit.bad_date`，說明要怎麼寫）：有年在前日期的文字裡，只有一位數的分或秒（`9:5`、`20240106 9:5`）、冒號後的毫秒（SQL Server 把 `10:00:00:5` 讀成 5 毫秒，不是 0.5 秒）、日期分隔符旁的空白、第二個日期、月份名稱（`2024 Jan 6`）、24:00、0 點或 13 點加 AM／PM、不存在的日期、時區沒有貼著時間。小數秒超過 3 位的 `datetime` 由伺服器拒絕（Msg 241），不會被進位。`20240106 9:5` 伺服器其實讀成 9:05，仍然拒絕：這個寫法不是 ISO，先前幾輪決定不替它選一個確定的讀法。
  - 完全沒有年在前日期的（`13/01/2024`、`1/6/2024`、`2024 Jan 6`）是使用者自己的寫法，仍由 session 依登入語言讀。
  - 欄位型別由伺服器回答：對寫入的欄位做 `SELECT TOP (0)`，看回傳的 metadata 型別（`date_columns`）。問的對象就是 UPDATE 指名的那張表，所以暫存表、view、別名型別（`CREATE TYPE … FROM datetime`）一視同仁，不會因為系統目錄查不到而退回猜測；只有真的寫了年在前的文字才問，一般的寫回沒有這一次往返。`date`、`datetime2`、`datetimeoffset`、`time`、`money`、`decimal`、`float` 不受 `DATEFORMAT` 與語言影響（六種 `DATEFORMAT`、九種語言實測），文字欄與文字主鍵裡看起來像日期的文字原樣不動。
  - 進位（`datetime` 約 3.33 毫秒、`smalldatetime` 到分鐘）由伺服器做，SET 與 WHERE 送出同一種寫法，所以寫回之後格子還留著打的字，下一次寫回的 WHERE 進位後仍然對到同一列。
  - 沒有做「寫回後重讀那一列來顯示」：格子在寫回之後顯示的是使用者打的字，不是伺服器存的值，所以進位（`11:00:00.001` 存成 `.000`）、`numeric(10,2)` 的 5.555 存成 5.56、`timestamp(0)` 的進位都要重新整理才看得到（PostgreSQL 的 numeric／timestamp(0) 主鍵之後會 `matched 0 rows`，有訊息不是靜默）。不做的理由：SQL Server 不帶 `INTO` 的 `OUTPUT inserted.*` 在有 trigger 的表上被伺服器拒絕（Msg 334），要改成寫回後用新 key 再 SELECT；PostgreSQL 的 `RETURNING` 要轉 `::text` 才和格子讀取路徑印出同樣的文字，而 `bool` 轉 `::text` 是 `true`、格子上是 `t`，每個型別都得對照；重讀出錯會讓每一次存檔的結果都不對，爆炸半徑是所有使用者的所有寫回。這一輪靠「年在前一律由用戶端解析」把靜默錯誤關在源頭，重讀留作之後的工作（SQLite 與 SQL Server 用同一個讀取路徑，容易；PostgreSQL 要逐型別對照）。
- PostgreSQL：格子顯示伺服器印出來的文字，寫回再把它當文字送回去，所以資料表的連線印出來的必須是讀得回去的：連線建立時 `SET datestyle = 'ISO'`（日月順序維持原設定）與 `SET extra_float_digits = 3`（`PRINT_AS_READ`）。非 ISO 樣式把時區印成縮寫，`Asia/Taipei` 印 `CST`，而 `CST` 讀回是 -06:00，`timestamptz` 主鍵找不到列，改了顯示的文字則會存成差 14 小時的時間；`extra_float_digits = 0` 會少印 float 的位數，浮點主鍵同樣找不到列。`IntervalStyle` 和 `TimeZone` 不需要固定：interval 與 timestamptz 的 ISO 輸出在任何樣式下都能讀回。
  - 使用者的 SQL 分頁可以把這兩個設定改掉（`RESET ALL`、`DISCARD ALL`、`SET datestyle`、`SELECT set_config(…, false)`，或呼叫改設定的函式），所以資料表不在 SQL 分頁那條連線上讀寫：資料表有自己的連線（`session::table_owner`），資料表的連線建立時設一次 `PRINT_AS_READ`（在 `PgConn::ready`，由 `ConnectSpec::for_table` 決定；`session::open_lease` 對 `{owner}:table` 這個 owner 設它，取代被退役連線的新連線也一樣；SQL 分頁與 count 的連線不設，見下面「SQL 分頁看到的」），之後只有 app 自己的語句在上面跑。測試 `postgres_only_the_connection_a_table_is_read_and_written_on_is_opened_with_the_settings_a_value_is_read_and_written_by`。以前的做法是把 SQL 分頁那條連線標成「設定可能被動過」，下一次開表前重設（`unpinned`／`read_table`），因為從語句文字判斷會漏（函式裡的 `set_config` 看不出來）；現在不必判斷，那套機制整個拿掉。測試 `postgres_what_the_sql_tab_sets_does_not_change_what_the_table_reads_and_saves` 在 SQL 分頁設 16 種東西（`datestyle`、`client_encoding`、`array_nulls`、`default_transaction_read_only`、`search_path`、`TimeZone`、`IntervalStyle`、`bytea_output`、`extra_float_digits`、`RESET ALL`、`DISCARD ALL`、開著的交易、先設 `datestyle` 再因 `1/0` 失敗的腳本等），每一種之後資料表讀到的、存進去的都和沒設時相同。
  - 為什麼不是寫回交易開頭的 `SET LOCAL`：錯在「讀」，印出來的文字已經是 `CST`，寫回時才設 ISO 救不了；錯的是共用連線這個前提，不是某一個設定。
  - SQL 分頁看到的：日期格式與浮點位數是資料庫或角色自己的設定，和 psql 一致（資料庫設 `datestyle = 'SQL, DMY'` 時，SQL 分頁的 timestamp 印 `06/01/2024 10:00:00`），不被開表改掉，也不被 app 釘成 ISO。這兩個設定只在資料表的連線上，因為只有那條連線把讀回的文字再寫回去（SQL 分頁的結果是唯讀的）。count 的連線也不設：`count(*)` 是整數。count 在連線數用完時借資料表的連線算，那時用的是資料表的設定，不影響數字。

## 引擎

三個引擎對上都是 `engine::Conn`，`execute(sql, &mut dyn RowSink)` 以每批 512 列交出結果；`RowSink::rows` 回 `false` 時引擎停止讀取。引擎自己產生的錯誤訊息走 crate 的 i18n（`engine.*`），伺服器回的訊息照原文顯示，並保留 SQLSTATE／Msg 編號、DETAIL、HINT、POSITION。

### PostgreSQL

- tokio-postgres 0.7＋tokio-postgres-rustls，rustls 用 ring provider，不呼叫 `install_default`。
- 沒指定資料庫時依序試 `postgres`、與使用者同名的資料庫，只有遇到 SQLSTATE `3D000`、`42501`、`55000` 才換下一個。
- 欄位型別用 `prepare` 取得；值用 `simple_query_raw` 讀文字格式，所以 numeric、interval、inet、enum、array、JSON 大整數都照伺服器的原文顯示。
- 取消用 cancel token 的資料（`cancel_query_raw`，在我們自己開的連線上送，連線用的 TLS connector 也用在 cancel 上），並等伺服器關閉那條連線才算完成。工作（查詢、讀列、count）自己看得到結果的取消，連線在取消後可以繼續使用；寫回被停止、或請求沒有被確認處理的，連線就不再使用，見「編輯與寫回」。
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
- 資料表 pane 產生的 SELECT、COUNT 與寫回都用三段式名稱 `[db].[schema].[table]`，db 是連線開啟時的 `DB_NAME()`（`Conn::database`）。資料表與 SQL 分頁分開連線之後，SQL 分頁的 `USE` 本來就到不了資料表的連線；三段式名稱留著，是第二層保護。
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

## CI 驗什麼

PostgreSQL 與 SQL Server 的測試在引擎沒被選到（`YUZORA_DATABASE_TEST_ENGINES` 沒列）時會跳過並算通過，所以 `app-ci.yml` 的 `database-integration` job（納入 `App (build · test)`）在 Linux x86-64 起 `tests/database/docker-compose.yml`（含 `mssql` profile），以 `YUZORA_DATABASE_TEST_ENGINES=sqlite,postgres,mssql` 跑 `cargo test --locked -p yuzora-db`：寫回、停止、catalog 與 grid 讀取都對真的 PostgreSQL 16 與 SQL Server 2022 執行。該 job 另設 `YUZORA_DATABASE_REQUIRE_ENGINES=postgres,mssql`：被要求的引擎不在 `TEST_ENGINES` 裡時，測試直接失敗而不是跳過（`tests/common/mod.rs`）；fixture 連不上則測試本身的連線失敗。`#[ignore]` 的效能量測不在 CI 跑。本機不設這兩個變數時行為不變。

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

環境變數：`YUZORA_PERF_ENGINES`、`YUZORA_PERF_DELAYS`（單向毫秒，預設 `0,25`）、`YUZORA_PERF_TABLES`（`small,100k,1m,widecols,widetext`）、`YUZORA_PERF_MODES`（`cold,warm`，加 `cold-alone,warm-alone` 可去掉 pane 自己要 description 的那一步；現在它只是 `tableInfo`，不送任何東西，所以兩種一樣）、`YUZORA_PERF_COLD`／`YUZORA_PERF_WARM`（樣本數）、`YUZORA_PERF_KEEP=1`（保留 fixture 資料庫 `yz_speed_pg`／`yz_speed_ms` 給下一次，預設結束時刪除）。其他測試：`perf_calibrate_the_delay_line`（先跑，確認轉送的延遲與往返次數）、`perf_phases_of_an_open`（連線、describe、SELECT、count 各自的時間）、`perf_sql_tab`（SQL 分頁與 Stop）、`perf_relay_overhead_on_a_big_result`。

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
