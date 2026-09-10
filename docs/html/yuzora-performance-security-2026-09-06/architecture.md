# Yuzora 分析範圍與信任邊界

分析日期：2026-09-06。基準：release/v0.0.9-beta.3，HEAD 9d0634c44e314b3dcc1c39b371ed448481b2f16a。採用當前工作目錄內容；開始時已有 QA-REPORT.md 修改、兩份 docs 刪除，本次保留。

Yuzora 是 Tauri 2 + React 19 的桌面 Agent Development Environment。前端 Zustand stores 經 src/lib/ipc.ts 與 herdrIpc.ts 到 Rust typed commands。Bridge 統籌事件，HERDR Named Session、Terminal Session、Agent Session 的 ownership 不可混用。SQLite/PG/MSSQL 是使用者選擇的資料庫，執行任意 SQL 是產品功能，不能自行當成 SQL injection。

資料流：UI action → typed IPC → ownership/trust/capability → service/actor → 本機程序、filesystem、remote SSH 或 DB → driver/parser → bounded/部分無界 cache與queue → IPC → Zustand → CodeMirror/xterm/React。

同 connection 的 DB lease 是協定與交易正確性邊界，跨 connection 可平行。PG/MSSQL query 在 helper process，SQLite 在 blocking worker。ResultSessionState 則是一個共享 registry mutex。SSH session manager 的全域 map 鎖很短；每 session handle mutex 負責 channel control，不能誤讀為所有主機傳輸都串行。

安全邊界：未信任workspace的執行權、renderer至native IPC、local selected-path capability、remote peer的host key/TLS身份、DB server回應與helper memory guard、文件/LSP markdown輸入與WebView DOM、LSP/tool binary來源、日誌/憑證儲存。

相近產品類型是桌面IDE與DB/SSH client（例如 VS Code、DBeaver）；比較用來校準信任模型，不把桌面使用者自行執行shell/SQL當成漏洞。VerifyFull應等同driver sslmode=require加上憑證/hostname驗證。此報告未實測其他產品，也不聲稱它們有或沒有相同缺陷。

MCP project yuzora 已索引；index狀態8986 nodes/47849 edges只代表graph規模，不是runtime profile。查過引用檔案coverage，已發現drift的lib.rs/herdr相關檔改讀live source。tests、vendor、docs等有索引排除，鎖定driver與必要tests直接讀取。名稱相同造成的誤配graph edges未用作runtime證據。

深查範圍：DB results/helper/actor/profile/UI、SSH/SFTP+lock依賴、frontend/editor/LSP/terminal、HERDR投影、filesystem/search、Git runner/log retrieval、logging、preview、perf sampling及主WebView設定。未做所有上游HERDR內部、WSL plugin全路徑、所有vendored driver行的逐行安全稽核；未做Windows/Linux真機、WAN/真DB負載或8小時soak。因此55項是本輪有來源依據的改善清單，不是保證找出所有可能問題。

沒有找到同repo的既有security-audit-skill輸出。此輪安全問題經獨立來源否證與結構化校對，仍應以新增回歸測試和後續聚焦審查補足覆蓋。
