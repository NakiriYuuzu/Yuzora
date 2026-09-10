# 安全性審查結果

本輪確認一項 HIGH：PostgreSQL VerifyFull 仍可能允許降級明文並傳送密碼。其餘 CSP、trace保留、資源預算等列在主報告為加固建議，不當成已確認安全漏洞。完整效能/記憶體/平行改善清單見 observations.json 與上層 HTML。

2026-09-06 文件更新：已接受的 55 項改善與多 database／schema 能力，已拆成工程工作包、交付相依、可先 mock 的契約、有效檔案 writer 及聯合驗收。詳見 [完整實作與平行分工文件](EXECUTION-PLAN.md)、[結構化 DAG](execution-plan.json) 與[主報告](../yuzora-performance-security-2026-09-06.html#execution)。所有產品工作仍為 planned；本次文件更新不代表漏洞已修復、效能改善已實作或完整測試已通過。原稽核 observations.json 保留原證據，採用方案以 execution-strategy.json 的決策與 acceptedOverrides 為準。

| 嚴重度 | 問題 | 證據 |
|---|---|---|
| HIGH | PostgreSQL encrypted modes未強制TLS | src-tauri/src/db_service.rs:2094、2112；tokio-postgres 0.7.18預設Prefer |

使用者以VerifyFull發起密碼連線時，能控制路徑或冒充endpoint的攻擊者，回覆SSLRequest=N並要求AuthenticationCleartextPassword，即可在未驗證server身份前取得密碼。本機loopback測試使用現有Yuzora helper與合成假密碼，觀察到plaintext_startup_after_N=true、password_message_tag=p、fake_password_sent_in_cleartext=true。沒有使用真實密碼或外部資料庫。

動態證據由資料庫分析代理執行一次；另一代理獨立核對production opener→helper→policy→Config→鎖定driver的完整資料流。不是兩次獨立PoC。結構化findings.json已通過skill schema校驗。

修復：兩個加密mode明確設定SslMode::Require，唯有經授權InsecurePlaintext設定Disable。保持VerifyFull原有rustls CA/hostname驗證與trust-server-cert獨立ack。回歸測試必須在SSLRequest後回N，證明不再有StartupMessage/PasswordMessage；現有測試讀第一包後斷線不足以涵蓋。

相近桌面IDE/DB client允許使用者主動執行SQL/工具是預期行為，本次沒有把此設計當安全漏洞。driver的Require與Prefer差別才是此問題的明確邊界；未對第三方應用進行現場測試。

已有值得保留的保護：SSH host-key change/corrupt pin fail-closed與新key challenge；selected/workspace/download capability與pinned directory；DB exact owner/generation fencing、field/row/session/registry budgets與helper guard；LSP framing caps和managed-server驗證；Markdown/LSP內容DOMPurify；Preview token/allowlist/CSP；Git literal pathspec與process-tree termination；updater簽章。

加固建議：主WebView CSP、LSP trace資料最小化與rotation、SFTP exclusive temp/跨session destination ownership、整app byte budgets與取消。它們的嚴重度不能直接套用到此HIGH漏洞上。
