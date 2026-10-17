# Phase 0.5 依賴選型

狀態：**[x] 自動化選型通過；未加入產品 Cargo.toml／lock**。實驗在 `/tmp/yzc-deps`，獨立 `CARGO_TARGET_DIR`。工具：rustc/cargo 1.96.0，host `aarch64-apple-darwin`。版本、checksum、source 與完整 dependency trees 保存在 `dependency-fixtures/`；版本是本次實際解析值，不表示未來自動跟最新版。

## 選型表

| 用途 | 選擇／實際版本 | 理由／限制 |
|---|---|---|
| WebAuthn RP | **`webauthn_rp = "=0.3.0"`**，features `serde_relaxed`（保留預設 bin/serde） | RustCrypto ES256／Ed25519／RS256（另支援 ES384）；UV/UP、BE/BS、origin、rpIdHash、challenge、none attestation、signCount。MIT OR Apache-2.0，MSRV 1.86，native compile＋ES256 software authenticator 正／負向通過。|
| HTTP/WS | **axum 0.8.9，ws**；tokio 1.53.1 | router、typed extractors、WS、middleware／body limits 易稽核。MIT；比自行組 hyper 1.11.1＋tokio-tungstenite 0.29.0 少一層手工 routing/upgrade 邏輯；後兩者已是 axum 的相依，並非安全性被拒。|
| TLS | **tokio-rustls 0.26.5＋rustls 0.23.45** | 全部 `default-features=false`；前者 `ring,tls12`，後者 `ring,std,tls12`。與現有 `src-tauri/Cargo.toml:63–72` 的 ring 契約一致；不得讓 default aws-lc 回來。|
| LAN cert（後期） | **rcgen 0.14.10**，`default-features=false, features=[ring,pem]` | MIT OR Apache-2.0，MSRV 1.88。實際產生 CA，以 Python cryptography 確認 nameConstraints critical、permitted DNS 只有 `yuzora-spike.local`。依賴原始碼 `certificate.rs::write_extensions` 對該 extension 明確傳 `true`。|
| QR（後期） | **qrcode 0.14.1**，`default-features=false, features=[svg]` | MIT OR Apache-2.0，實際產生 SVG；不用 image codecs。最後發布 2024-07，成熟低變動不等於已做安全審計，升級仍人工審閱。|
| Web Push（後期候選） | **web-push-native 0.5.0**（default serialization/vapid） | MIT OR Apache-2.0，MSRV 1.87。`ece-native` 的 RFC8291 aes128gcm＋VAPID RFC8292；offline build 成功，未投遞。`jwt-simple 0.12.17` 已禁 default、用 pure-rust；`superboring 0.1.17` 是 ISC **純 Rust** compatibility API，不是 BoringSSL C bindings。Phase 7 仍需 RFC 向量／JWT claims／真機推播驗收。|

選型不是產品整合完成：axum limit/timeout/Host/Origin、WebAuthn challenge store、credentials persistence、授權、CA keychain 等仍全部屬後續 Phase。`dependency-fixtures/later-smoke.rs` 的 rcgen CA 是歷史 DNS-only API smoke，未驗 IPv4／IPv6 exclusion，不可安裝；最終 critical constraints（含全部 IP exclusion）由更新後的 LAN kit 驗證，產品 rcgen 實作須於 Phase 6 對齊並另測。

## WebAuthn 實測與 API 注意事項

`webauthn-smoke.rs` 是一次性**軟體 authenticator fixture builder**，固定測試 key 只用來簽測試資料；所有 RP 驗證皆呼叫套件，沒有自製驗證器。`webauthn-test.txt`：

- `registration valid: Ok(())`（ES256、attestation none）；`authentication valid: Ok(true)`。
- 錯誤 origin → `OriginMismatch`；錯誤 rpIdHash → `RpIdHashMismatch`；缺 UV → `UserNotVerified`；缺 UP → `UserNotPresent`／`AuthenticatorData(UserNotPresent)`。
- challenge 不符 → `ChallengeMismatch`；counter 10→9 → `SignatureCounter`；錯誤 signature → `AssertionSignature`。
- BE false→true → `BackupEligible`；BS=true/BE=false → parse 拒絕／`AuthenticatorData(BackupWithoutEligibility)`。
- API 支援 EdDSA/RS256 的證據為套件 README／typed key／RustCrypto dependency；本輪**未另外執行 EdDSA/RS256 authenticator cases**，Phase 2 補足。
- 必須傳 `allowed_origins: &["https://<host>:<port>"]`；空 origins 的 default 不含自訂 port，不可誤用寬鬆 origin。
- `passkey()` 預設 residentKey required、TTL 5 分，Phase 2 明確改 preferred、120000 ms；UV 保持 required。預設 extensions 含 credProtect，若產品不啟用需明確清掉，不能隱性要求 kit 沒有測的 extension。
- `Registration::from_json_custom` 是**扁平自訂 wire shape**，不是 browser `toJSON()` shape。本次最初誤用而 parse 失敗，改用 `RegistrationRelaxed`／`DiscoverableAuthenticationRelaxed64` 的 serde parser 後成功；不要自行拆 CBOR。
- verifier consume server state 支援一次性語意，但 gateway 還要保證 challenge lookup/take、TTL、origin/RP 綁定、credential ownership、nonzero counter 保存及鎖定、Companion login session／elevation／replay 政策。

### P1-4：passkey 政策的選型限制

`webauthn_rp 0.3.0` 的 attestation 僅支援 none／self（surrogate），**不支援 x5c attestation 憑證鏈驗證**。UV required 不等於硬體綁定；BE=0 也不是經 attestation 證明的不可匯出性。若使用者政策要求 x5c 鏈驗證或 AAGUID 硬體證明，必須重新選型，不能自行補寫 verifier。O1–O4 尚待使用者裁決，見 ADR-0008（2026-10-17 由 0007 改號）；Phase 2 才依裁決設定各 scope 的 `backup_requirement`。

### 128-bit challenge 修訂（計畫作者核准）

套件 `request.rs::Challenge(u128)` 僅由 `rand::random()` 建立（rand 0.9 thread_rng，CSPRNG），固定 **16 bytes**，不能配置32 bytes。WebAuthn L3 §13.4.3 **建議（SHOULD）**至少16 bytes；supervisor／計畫作者於本輪核准把 Phase 2.2 改成 **128-bit、單次、2 分鐘 TTL**。配對 secret 與 Companion login session id 仍32 bytes。未 fork、未弱化 UV、未 hand-roll WebAuthn。

### 維護／供應鏈

crates.io 實際 metadata：webauthn_rp 最後發布 **2025-04-03**；README 宣稱持續維護，但本輪不能獨立證明近期 maintainer 活躍，不能把這點寫成已驗證。單一作者 Zack Newman、自架 `git.philomathiclife.com`（非 GitHub）、使用量較少，構成殘餘供應鏈風險。選擇經 supervisor 核准，對策：精確 pin `=0.3.0`、保存 Cargo.lock registry checksum、升級人工審閱 diff、Phase 2/8 獨立安全審查；不宣稱 audited。工具鏈1.96符合其1.86 MSRV。

### 拒絕／未選候選

- `webauthn-rs 0.5.4`／core 0.5.4：core Cargo.toml 明列 openssl、openssl-sys，不符合硬性依賴門檻（新系列 metadata 0.6.1-dev 不代表可移除此依賴）。
- `passkey-auth 0.1.3`：純 Rust ES256/EdDSA、none、UV（需 opt-in）可用，但 source 的 auth-data 只提供 UP/UV helper，credentials/outcome 無 BE/BS 語意與一致性驗證；也不支援 RS256，因此不選，不在 gateway 手刻補全。
- `passkey 0.5.0`（1Password）：client/authenticator，不是 RP verifier，可供後續測試工具選擇。
- `web-push 0.11.0`：預設 isahc，另有 hyper-tls 路徑，不符合預設 pure-Rust TLS 方向；本輪不推薦，不宣稱已對所有 feature combinations 排除 OpenSSL。其 `ece 2.x` 預設 backend 可能引入 OpenSSL，**尚未驗證**，須另以 `cargo tree -e features` 確認，不能視為已證實的拒絕理由。

## 依賴樹與授權證據

實際執行（四 target，包含題目要求的三個）：

```sh
cargo tree --locked --manifest-path /tmp/yzc-deps/Cargo.toml --target <target>
cargo tree --locked --manifest-path /tmp/yzc-deps/Cargo.toml --target <target> -i openssl
cargo tree --locked --manifest-path /tmp/yzc-deps/Cargo.toml --target <target> -i openssl-sys
# 額外：-i aws-lc-rs、-i aws-lc-sys
```

`aarch64-apple-darwin`、`x86_64-pc-windows-msvc`、`x86_64-unknown-linux-gnu`、`aarch64-unknown-linux-gnu` 全部回 **did not match any packages**（Cargo exit101 是預期的無匹配結果；完整 tree 指令均成功）。`rustls-features.txt` 只有 ring/std/tls12 provider features。

`licenses.txt` 列出完整 resolution metadata（包含其他平台）；所有第三方套件可選 MIT／Apache／ISC／BSD 等 permissive license；r-efi 的 LGPL 是 OR 選項，可選 MIT/Apache，並非強制 LGPL。另含 **Unicode-3.0**（icu_*、zerovec、yoke 等），屬寬鬆授權但出貨須附授權／copyright 聲明；不能只列 MIT／Apache。scratch 根 crate 的 license=None 不會出貨。這是技術授權盤點，不代替法務審查。

### 已知安全公告與 Phase 2 CI

既有 tree 含 `rsa 0.9.x`（本次解析 0.9.10），經 `webauthn_rp` 與 `superboring` 引入。**RUSTSEC-2023-0071（Marvin）**涉及 RSA 私鑰操作的時序側通道；此 RP 僅使用 RSA 公鑰驗簽，VAPID 使用 ES256，不使用 RSA 私鑰解密／簽章，因此目前用途不走受影響操作。這是用途層影響評估，不是宣稱套件已修復或 audit 無告警。

本輪未執行 `cargo audit`／`cargo deny`；將於 **Phase 2 CI** 加入，並為此公告建立文件化例外：固定 advisory ID、上述驗簽限定／VAPID ES256 理由、依賴版本、負責人與複查期限；若新增 RSA 私鑰操作須撤銷例外並重新評估。授權檢查須包含 Unicode-3.0 attribution。

**只有 aarch64-apple-darwin 實際 compile/run**。Windows/Linux 僅 target-aware trees，不能宣稱跨平台編譯成功；Phase 1 CI 必須補 native checks。

## 重現

在新的 `/tmp` scratch 目錄還原，不要建成產品 crate：

```sh
mkdir -p /tmp/yzc-deps-review/src/bin
cp spikes/companion-direct/dependency-fixtures/Cargo.toml.example /tmp/yzc-deps-review/Cargo.toml
cp spikes/companion-direct/dependency-fixtures/Cargo.lock.example /tmp/yzc-deps-review/Cargo.lock
cp spikes/companion-direct/dependency-fixtures/webauthn-smoke.rs /tmp/yzc-deps-review/src/main.rs
cp spikes/companion-direct/dependency-fixtures/later-smoke.rs /tmp/yzc-deps-review/src/bin/later.rs
CARGO_TARGET_DIR=/tmp/yzc-deps-review/target cargo run --locked --manifest-path /tmp/yzc-deps-review/Cargo.toml --bin yzc-dependency-spike
# later-smoke 的 DER 輸出目前固定 /tmp/yzc-deps/rcgen-ca.der；重跑前先建立該 scratch 目錄
mkdir -p /tmp/yzc-deps
CARGO_TARGET_DIR=/tmp/yzc-deps-review/target cargo run --locked --manifest-path /tmp/yzc-deps-review/Cargo.toml --bin later
```

後期 smoke 只建立加密 push Request，**沒有發網路請求或 listener**。原始碼／lock 為重現用副本，私鑰未提交。
