# Companion Phase 0 真機 spike 操作手冊（zh-TW）

這是**拋棄式能力測試，不是產品 gateway**；不提供 Herdr、RPC、配對、cookie 或伺服器端 WebAuthn 驗證。全部手機項目：**（待使用者實機）**。代理只跑 loopback 自測，沒有執行 Tailscale／LAN listener／瀏覽器。

## 共通準備

1. 使用隔離測試機及測試 passkey；先確認公司網路政策允許 Tailscale。安裝 Bun、uv；不須安裝專案依賴。
2. 在本 worktree 根目錄執行以下指令。需要長駐的 server 各開一個終端；記錄 PID，最後 Ctrl-C 停止。不要放入產品 bundle。
3. 手機記錄機型、OS、瀏覽器版本、時間、網址／RP ID；每個 API 按鈕須由本人點擊。結果欄位用「通過／失敗＋原始錯誤」，不能以桌面 curl 代替手機。
4. 原生 HTML buttons 刻意不用產品 shadcn／React：此拋棄式 probe 要排除產品 UI、bundler、第三方 script 的干擾；不是新的共用 UI primitive。
5. WebAuthn `create/get` 只證明瀏覽器願意執行，**不證明 server 驗簽、硬體不可匯出或人身身分**。UV 也可為裝置 PIN；BE/BS 顯示是未經伺服器驗證的診斷值。

## Spike A：Direct · Tailscale

1. 桌面、iPhone／Android 登入預期的 Tailscale 帳號；管理後台 DNS 啟用 MagicDNS 與 HTTPS Certificates。公開憑證的機器名稱可能出現在 Certificate Transparency，使用不含私密資訊的 hostname。
2. 先保存既有設定，確認 8443 沒有其他 handler。以下指令全部由**使用者**執行：
   ```sh
   mkdir -p spikes/companion-direct/tailscale/out
   tailscale status --json > spikes/companion-direct/tailscale/out/status-before.json
   tailscale serve status --json > spikes/companion-direct/tailscale/out/serve-before.json
   tailscale funnel status
   ```
   預期 Funnel 沒有公開此 port；若已啟用／8443 被佔用，**停止，不覆蓋、不自動重設其他服務**。管理後台 ACL／Grants 限制僅預期手機可達此 port。
3. 終端 A：
   ```sh
   bun spikes/companion-direct/tailscale/server.ts
   ```
   固定只 listen `127.0.0.1:47110`，不允許改成 wildcard／LAN 位址。
4. 終端 B：
   ```sh
   tailscale serve --bg --https=8443 http://127.0.0.1:47110
   tailscale serve status --json > spikes/companion-direct/tailscale/out/serve-active.json
   tailscale funnel status
   tailscale status --json > spikes/companion-direct/tailscale/out/status-active.json
   ```
   使用 Serve 印出的 `https://<device>.<tailnet>.ts.net:8443/`；不要換 IP。確認 HTTPS 無警告、Funnel 未公開。若工具要求管理後台啟用 HTTPS，完成後再跑。
5. iPhone（iOS 17+）Safari 開網址 → 分享 →「加入主畫面」→ 從主畫面啟動。Android Chrome 開同網址 → 選單「安裝應用程式」／「加入主畫面」。普通分頁和獨立 PWA 分別記錄。
6. 依序按 Secure context（true；RP ID＝hostname，不含 port）、Register SW（同 origin scope `/`）、Create platform passkey（出現本人驗證；儲存測試 credential）、Get platform passkey（再驗證，UP/UV=true）；保存 create 的 UP/UV/BE/BS、authenticatorAttachment、transports。重新載入後可用 discoverable credential；若 preferred 未建立 discoverable，保留頁面先完成 get，並另記重載限制。
7. 按 Notification permission：iOS 請在**已安裝主畫面 PWA**操作；預期可詢問並取得 granted，並非已證明推播投遞。使用者主動拒絕與平台不支援須分開記。
8. 按 Who am I：預期 `Tailscale-User-Login`＝預期登入者、Name 可能為 RFC 2047 編碼，Host＝公開 hostname:8443。GET fetch 通常沒有 Origin，`null` 是正常；後續 mutation／WS 才強制 Origin。保存原始 header 字串，不能先假設已解碼。
9. 負向測試：以 tagged device（若手機不能直接 tagged，可用另一測試節點）以及被 share 的不同使用者重做 Who am I。預期 tagged 無 login、share login 不同。**本 kit 只 echo，不會真的回 403**；記錄「依未來 gateway 的 required login 規則應拒絕」。若要宣稱實際拒絕，須等 Phase 1 guard 完成，不能把本 kit 當認證測試。不得為了測試把 Funnel 打開。
10. 收尾（即使失敗也執行）：
    ```sh
    tailscale serve --https=8443 off
    tailscale serve status --json > spikes/companion-direct/tailscale/out/serve-after.json
    tailscale status --json > spikes/companion-direct/tailscale/out/status-after.json
    tailscale funnel status
    ```
    Ctrl-C 停止終端 A。只關本次 port，不用 `serve reset`。移除測試 PWA、網站資料及測試 passkey（手機密碼／passkey 設定）。
11. 將 status／serve JSON 去識別化後交回作 Phase 4 fixtures：保留 `Self.DNSName`、`Self.UserID`、`User`／`LoginName`、`CertDomains`、`BackendState` 與 serve handler 結構。不要提交原始 tailnet 裝置、使用者資訊；`out/` 已忽略。

| 路徑／平台 | 版本／機型 | secure | 安裝／standalone | SW | create／UV | create BE/BS | attachment | transports | get／UV | 通知 | identity 原始值／錯誤 |
|---|---|---|---|---|---|---|---|---|---|---|---|
| Tailscale／iPhone Safari | | | | | | | | | | | |
| Tailscale／iPhone 主畫面 | | | | | | | | | | | |
| Tailscale／Android Chrome | | | | | | | | | | | |
| Tailscale／Android standalone | | | | | | | | | | | |
| tagged device（應拒絕訊號） | | | | | | | | | | | |
| share user（應拒絕訊號） | | | | | | | | | | | |

### Q7：passkey 綁定政策實機測試（裁決前的證據）

在 Spike A 第6步後、收尾前執行；LAN 也以自己的 RP 重做。先不要把 O2 建議當成已裁決政策。

1. iPhone **Safari 分頁**與**主畫面 standalone PWA**分別按 Create platform passkey → Get platform passkey，保存 create BE/BS（及 get 的 UP/UV/BE/BS）、attachment、transports；Android 的 Chrome 分頁／standalone 同樣記錄。不要以平台名稱推定 BE=0 或 BE=1。
2. 有 FIDO2 硬體安全金鑰時，分別以 **NFC／USB-C** 按 Create security key → Get security key；後者只以該頁剛註冊的 cross-platform credential ID／transports 呼叫 allowCredentials，不會拿平台 passkey 冒充。iPhone Safari 分頁、iPhone standalone、Android Chrome 分頁、Android standalone 均測 register＋login，記錄 PIN／UV 提示、成功或錯誤原文；安全金鑰需支援 UV；`userVerification: required` 會要求尚未設定 PIN 的金鑰先設定 PIN。重載會清掉目標 ID，需重新註冊測試 credential，勿將 discoverable get 當成已選中該金鑰。
3. `getAuthenticatorData()` 或 `getTransports()` 不支援時欄位為 null＝未知，**不是 BE=0**。本 kit 不做 server 驗簽，不證明硬體不可匯出；記錄實際 attachment，而非只記請求值。
4. **若沒有安全金鑰**，填「無金鑰，未測」，不標不支援或通過；回報使用者是否願意取得金鑰，再裁決 O1／O2／O3。未測 standalone 硬體登入前，不宣稱 O2 體驗已可用。
5. 收尾時刪除硬體安全金鑰上的所有測試 credential（包含重複註冊）：Chrome → Settings → Privacy → Security keys → Manage sign-in data，或使用 Yubico Authenticator。`residentKey: preferred` 可能佔用 discoverable credential slot；只清除手機 passkey／網站資料不會清掉金鑰上的憑證。

| 平台／模式 | credential／介面 | register 結果 | create BE/BS | attachment | transports | login／UP/UV/BE/BS | 原始錯誤／無金鑰 |
|---|---|---|---|---|---|---|---|
| iPhone Safari 分頁 | platform／NFC／USB-C（各一列） | | | | | | |
| iPhone standalone PWA | platform／NFC／USB-C（各一列） | | | | | | |
| Android Chrome 分頁 | platform／NFC／USB-C（各一列） | | | | | | |
| Android standalone PWA | platform／NFC／USB-C（各一列） | | | | | | |

**停止條件：**任一目標平台的 Tailscale secure context／passkey 不通過，停止 Phase 1 並回到規劃，不能降級成 HTTP／token-only。

## Spike B：Direct · LAN

1. 同一私有網路，關閉 AP client isolation；桌面選定一個具體 RFC1918 IPv4。名稱應為穩定單一 DNS label。macOS：
   ```sh
   scutil --get LocalHostName
   # 192.168.1.20 必須換成本次明確選定的桌面 RFC1918 IPv4；不自動偵測。
   uv run --with cryptography python spikes/companion-direct/lan/make-ca.py --counterexample-ip 192.168.1.20
   # Windows/Linux 必須明給 name（不含 .local）：
   # uv run --with cryptography python spikes/companion-direct/lan/make-ca.py my-test-host --counterexample-ip 192.168.1.20
   ```
   `out/` 非空會拒絕覆寫。**舊版沒有 IP exclusion 的 out 已刪除，舊 CA 絕不可安裝；若曾安裝，先移除。**若已有代理產生的 `yuzora-spike.local` 測試材料，先停止測試、明確刪除整個 `spikes/companion-direct/lan/out/` 再生成本機名稱，**不得安裝代理測試 CA 當正式 CA**。每次重跑都會生成全新的 CA／指紋，不能沿用舊 profile。CA 私鑰只在記憶體中簽發合法 leaf、other.local、127.0.0.1 與選填的 phone-ip leaf，**從未序列化或寫入 `ca.key`**；保留的 leaf 私鑰限測試、0600，不能傳到手機或入 Git。沒提供 `--counterexample-ip` 就沒有手機 IP leaf，應記「手機 IP 反例未測」。
2. 本機先驗證（只 loopback，不改信任庫）：
   ```sh
   uv run python spikes/companion-direct/lan/self-test.py
   ```
   預期合法 `.local` leaf 成功；`other.local` 在 openssl verify 出現 `permitted subtree violation`；SAN 與 URL 均為 `127.0.0.1` 的 IP leaf 出現 `excluded subtree violation`。兩個反例 curl 都 exit 60。`SELFTEST.txt` 記錄 verifier 版本／backend 與各結果；不能替代手機。另以 `test ! -e spikes/companion-direct/lan/out/ca.key` 確認 CA key 未落地。
3. 用 AirDrop／Nearby Share／USB 傳 `out/ca.mobileconfig`（iOS）或 `out/ca.crt`（Android），**不要以 HTTP 提供下載，不傳任何 .key**。以另一受信任管道比較 make-ca 輸出的 **CA SHA-256 指紋**。可重新列印：
   ```sh
   openssl x509 -in spikes/companion-direct/lan/out/ca.pem -noout -fingerprint -sha256
   ```
   不要拿 leaf 指紋比 CA。若手機 UI 不顯示 SHA-256，可透過 USB 回拷同一檔，在受信任桌面比對 DER 憑證指紋；仍需確認手機已安裝的 subject／serial，將 UI 無法直接比對列為限制。
4. iOS：設定 → 已下載描述檔（或 一般 → VPN 與裝置管理）→ 安裝；接著 一般 → 關於本機 → 憑證信任設定 → 對該 CA 開啟「完全信任」。Android：設定 → 安全性 → 加密與憑證 → 安裝 CA 憑證（實際路徑依廠牌）；接受系統的網路監控警示只限測試 CA。裝置管理政策禁止安裝時記為不支援，不繞過。
5. **以下 LAN listener 只由使用者自行啟動**（將 IP 換成本機選定介面）：
   ```sh
   bun spikes/companion-direct/lan/server.ts --bind 192.168.1.20
   ```
   防火牆只允許 Private LAN／本測試手機、TCP 47443。禁止 0.0.0.0、公網 port forwarding／tunnel；測試完移除本次規則。手機開 `https://<name>.local:47443/`，無 TLS 警告才繼續，不按「略過憑證警告」。
6. `.local` 預檢：macOS `dns-sd -G v4 <name>.local`（Ctrl-C 停止）；Linux 使用 `avahi-resolve-host-name <name>.local` 並確認 avahi-daemon／UDP5353 可用；Windows 的 resolver 支援不等於有 responder，須確認 Bonjour／其他已安裝 mDNS responder 是否發布本機名稱。不自動安裝或改 hostname；無 responder 記為不支援，**不改用 IP 作 RP ID**。
7. 依 Spike A 第5–7步測 PWA、SW、create/get；RP ID 必須為 `<name>.local`。Tailscale 與 LAN 是不同 RP，需分別建立測試 credential，不能拿同一 passkey 跨 origin。
8. 真機 nameConstraints 反例：Ctrl-C 正例 server，再執行
   ```sh
   bun spikes/companion-direct/lan/server.ts --bind 192.168.1.20 --counterexample
   ```
   手機必須解析 `other.local` 到**同一個測試 IP**，且 URL 用 `https://other.local:47443/` 才能排除單純 hostname mismatch。若名稱已被其他主機使用，停止此反例。
   - macOS 可在另一終端暫時代理發布：`dns-sd -P 'YZ constraint negative' _https._tcp local. 47443 other.local 192.168.1.20`，以 `dns-sd -G v4 other.local` 核對；兩個命令最後均 Ctrl-C。
   - Linux 可由使用者執行 `avahi-publish-address other.local 192.168.1.20`，最後 Ctrl-C。
   - Windows 沒有可用 responder 時記錄反例解析未完成，不能以 `https://<name>.local` 上的錯誤 leaf 冒充 nameConstraints 測試。
   預期手機 TLS 拒絕，不允許略過。若手機信任成功，記為該平台 LAN 不支援／需安全審查。
9. 真機 **IP-SAN 反例**：先確認第5步合法 `.local` leaf 可連（證明同一 CA 已信任），停止 other.local server，再由使用者啟動：
   ```sh
   bun spikes/companion-direct/lan/server.ts --bind 192.168.1.20 --phone-ip-counterexample
   ```
   手機開 `https://192.168.1.20:47443/`（必須換成產生時的 `--counterexample-ip`）。此 leaf SAN 與網址 IP 相符，其他簽發參數與合法 leaf 相同；在同一 CA 正例已成功且服務可達的前提下，**TLS 憑證拒絕**才能支持 IP exclusion（不是 hostname mismatch）。保存手機錯誤原文，逾時／連不上不算成功反例；若信任成功，停止並記為 LAN 不支援／需安全審查。不允許略過警告。`--ip-counterexample` 是 SAN=127.0.0.1 的代理 loopback 自測，**不能**用桌面 LAN IP 連它冒充手機 IP exclusion；未執行有效手機 IP 反例則明記「手機 IP 反例未測」。測完由使用者 Ctrl-C 停止 server。
10. 收尾：Ctrl-C 全部 server／臨時 mDNS publisher，移除本次防火牆例外；手機移除測試 CA 描述檔／使用者 CA、PWA、網站資料與 passkey；刪除整個 out（`ca.key` 從未建立，仍須清理 leaf 私鑰）。不要移除其他 CA。

`nameConstraints` 的 DNS 值是 **DNS subtree**，`<name>.local` 同時允許其子網域，不是 exact-host 限制。同一 critical extension 另排除全部 IPv4 `0.0.0.0/0` 與 IPv6 `::/0`；產品仍須嚴格 leaf SAN／Host／Origin／RP ID。手機是否對自行安裝的 root 執行 constraints，必須以反例實證。

| 桌面／手機 | 名稱解析 | CA 安裝／SHA256 比對 | TLS 正例 | other.local 拒絕原因 | IP-SAN 拒絕原因／未測 | secure | SW／安裝 | create/get／UV | create BE/BS | attachment | transports | 結論／錯誤 |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| macOS／iPhone | | | | | | | | | | | | |
| macOS／Android | | | | | | | | | | | | |
| Windows／iPhone | | | | | | | | | | | | |
| Windows／Android | | | | | | | | | | | | |
| Linux／iPhone | | | | | | | | | | | | |
| Linux／Android | | | | | | | | | | | | |

LAN 平台失敗只停用該平台 LAN，不提供較弱替代。填完後由 reviewer 將結果加入 living plan Amendments 與 ADR-0008（原 0007）；目前全部待驗證。（2026-10-17：LAN 延後，本矩陣暫停）

## Spike C 與依賴

已執行的非瀏覽器證據：`herdr-multiclient/REPORT.md`、`DEPENDENCIES.md`。**不要對 live Herdr 重跑**；只能透過 isolated-herdr.sh，並遵守該報告的隔離 preflight。桌面接管提示與真實手機操作仍待使用者驗收。
