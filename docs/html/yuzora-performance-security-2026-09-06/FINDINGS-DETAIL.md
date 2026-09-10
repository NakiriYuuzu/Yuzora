# SEC01：PostgreSQL TLS downgrade

目標行為：VerifyFull需加密且驗證server；EncryptedTrustServerCert需明確確認且仍然加密；InsecurePlaintext需明確例外。

完整資料流：

1. src-tauri/src/db_profiles.rs:2389，ProductionDatabaseConnectionOpener.open選擇PG配置，經db_service.rs:3573的open_unregistered傳遞transport mode。
2. src-tauri/src/db_query_worker.rs:589，NetworkQueryWorker.spawn_postgres傳送ConnectPostgres給helper。
3. src-tauri/src/db_service.rs:3204，query_worker_loop接收mode/endpoint/password；:3216呼叫pg_open，再到:2071的pg_open_with_timeout。
4. :2082檢查transport policy；:2094建Config並附password，沒有設定ssl_mode。
5. :2112呼叫cfg.connect(rustls connector)。Cargo.lock:6729鎖定tokio-postgres 0.7.18；該版本config.rs:255的default為Prefer。
6. 鎖定driver connect_tls.rs:41在SSLRequest回N且非Require時返回Raw，不呼叫TLS verifier；connect_raw.rs:173的AuthenticationCleartextPassword分支可在該raw stream傳送PasswordMessage。

安全重現方式：loopback-only假server，helper使用YUZORA_DB_QUERY_WORKER=1；送length-prefixed ConnectPostgres JSON，host=127.0.0.1、port=fixture ephemeral port、transportMode=verifyFull、insecureException=null、trustServerCertAcknowledged=false，帳密全為合成fixture。server收到SSLRequest後回ASCII N，再送PostgreSQL AuthenticationCleartextPassword（R、length=8、auth code=3）。檢查是否出現p PasswordMessage以及是否等於合成sentinel；關閉socket並kill/reap helper。

已觀察：plaintext_startup_after_N=true；password_message_tag=p；fake_password_sent_in_cleartext=true。此為分析代理回報的單次本機動態測試，並由不同代理独立驗證來源與協議行為。未測外部MITM部署、未讀真實credentials。

實際攻擊者需控制使用者所連PG endpoint或主動介入路徑；能取得該次使用者輸入的密碼。此問題不是無需互動的RCE，也不證明資料庫權限本身被繞過。

防護否證：CA/hostname verifier不會執行，因driver在N已返回Raw；mode acknowledgement只控制是否允許指定mode，沒有強制底層TLS；現有plain-server測試只讀首包即關socket。helper memory sandbox無法修正傳輸協議選擇。

修法以明確driver Require語意為基準，不依賴connector是否存在推導TLS必需。沒有實測其他IDE/DB client，不能以比較產品作肯定或否定證據。
