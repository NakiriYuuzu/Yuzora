# Spike N1：內嵌 Tailscale（tsnet）取代 `tailscale serve`

2026-10-18。拋棄式能力測試，不是產品程式碼。目的：Mac 不安裝 Tailscale，也能讓手機經 tailnet 連到 Companion gateway。

## 做法

`main.go` 是一個 Go sidecar（tailscale.com v1.104.1 的 `tsnet`）：

- 自己成為 tailnet 節點（`yuzora-spike`），在 tailnet 的 `:8443` 終止 TLS，憑證由 Tailscale 簽發的 ts.net 憑證（`LocalClient.GetCertificate`）。
- 每個連線用 `LocalClient.WhoIs(RemoteAddr)` 查出對方帳號與節點；刪掉 client 送來的 `Tailscale-*` header，再注入 `Tailscale-User-Login`／`Tailscale-User-Name`（非 ASCII 以 RFC 2047 編碼），tagged 節點不注入。行為比照 Serve，所以 gateway 不用改。
- 反向代理到 `127.0.0.1:47110`（含 WebSocket）。
- `-selftest`：節點經 tailnet 連自己的 ts.net 名稱，以系統根憑證驗證，不需要手機就能測憑證與 WhoIs。
- 驗收 app 端用一個假 `tailscale` CLI（`/tmp/yz-tsnet-ts/tailscale`），`status --json` 回傳 tsnet 節點自己的 `ipnstate.Status`（sidecar 每 3 秒寫一次）。現有 probe 能直接解析，`serve` 只記錄、不做事。

## 結果

| 項目 | 結果 |
|---|---|
| Binary（`CGO_ENABLED=0 -trimpath -ldflags "-s -w"`） | macOS arm64 19.6 MB（gzip 7.6 MB）；Windows x64 21.5 MB（8.3 MB）；Linux x64 21.2 MB（8.3 MB）。三平台純 Go 交叉編譯，不需要 C toolchain。對照：現有 companion sidecar 20 MB、隨附 HERDR 22 MB。 |
| Go 版本 | tailscale v1.104.1 要求 Go 1.27（`go.mod` 被升到 1.27.1，toolchain 自動下載 go1.27.2）。CI 要多一套 Go。 |
| 登入 | 不需要 Tailscale App。啟動 1.6 秒拿到 AuthURL；使用者授權後 Starting→Running 約 0.3 秒。 |
| 重啟 | 狀態存在 `-dir`（`tailscaled.state`、`certs/`，權限 0600／0700），重啟 65 ms 回到 Running，不必重新登入，名稱不變。 |
| HTTPS capability | 管理後台按下 Enable HTTPS 後，節點的 `CapMap["https"]` 與 `CertDomains` 隨 netmap 推送出現，不用重啟。未開時 `GetCertificate` 回 `your Tailscale account does not support getting TLS certs`。 |
| 第一次簽憑證 | 約 35 秒（Let's Encrypt，中繼 `YE1`，90 天）。之後同一張憑證從 `certs/` 讀，握手 373 ms 內完成。 |
| WhoIs | 自連：本節點、`tagged=false`、14 ms。iPhone：節點為使用者的 iPhone、同一帳號、`tagged=false`、17 ms。真正依連線來源驗證 peer。 |
| 端到端（真 iPhone，Mac 未裝 Tailscale） | Safari 掃 QR → 配對 → 桌面核准 → 建立 passkey → 登入，全部經 tsnet 節點完成，看得到 Agent。passkey 為 iCloud 鑰匙圈（BE=1、BS=1），依 O2 只能檢視，無法控制（預期）。 |

## 發現

1. **第一次簽憑證比讀取逾時長**：sidecar 的 `ReadHeaderTimeout` 是 10 秒，TLS 握手在逾時內，第一次簽發約 35 秒，所以第一個連線被斷（client 收到 EOF）。產品版要在啟用時先用 `CertPair` 簽好，並在設定頁顯示「正在取得憑證」。
2. **產品 bug：節點換了就卡死**（與 tsnet 無關，現有 T2 程式碼）。`settings.applied` 記錄的 host 與目前節點不同時，`companion_enable` 回 `companion-tailscale-remove-first`，`companion_tailscale_remove` 又因 host 不同回 `companion-tailscale-changed` 並保留紀錄，UI 沒有出口。改機器名稱、切換帳號或 profile、重灌、改用內嵌節點都會遇到。已修：節點不同時可以「忘記這筆紀錄」（`companion_tailscale_forget`），確認對話框列出要在舊節點執行的 `tailscale serve --https=8443 --set-path=/ off`。
3. **ACL 預設全通**：同一 tailnet 的其他裝置也連得到 `:8443`。文件要建議用 ACL／Grants 限制只有手機能連。
4. **O2 讓真手機無法控制**：iPhone 的 passkey 一定是 iCloud 鑰匙圈（BE=1）。不帶硬體金鑰的一般使用者，在手機上永遠只能檢視。O2 仍是 provisional，需要使用者裁決。
5. **sidecar 到 gateway 這一段仍可被本機程序偽造 header**（與 Serve 相同）。tsnet 的 WhoIs 本身是真的 peer 驗證，但要讓 gateway 信任它，產品設計要讓 WhoIs 結果經不可偽造的通道送達：把 tsnet 放進 gateway 同一個行程（libtailscale／Go 改寫該層），或 sidecar 與 gateway 改走只有本使用者可讀寫的 unix socket／named pipe 並帶 per-run secret。

## 尚未驗證

- Windows／Linux 實機執行（只驗證了交叉編譯）。
- 與系統版 Tailscale 同時存在（這台 Mac 沒裝）。tsnet 是 userspace，不建 TUN，預期可共存。
- 節點金鑰到期（預設 180 天）後的重新登入流程；登出（`-logout`）後節點是否從管理後台消失。
- 狀態目錄改放 OS keychain（tsnet 支援自訂 `ipn.StateStore`）。
- 公司網路（Spike A 的公司網路項目仍待實測）。

## 執行方式

```sh
cd spikes/companion-tsnet
CGO_ENABLED=0 go build -trimpath -ldflags "-s -w" -o bin/yzts-darwin-arm64 .
bin/yzts-darwin-arm64 -dir /tmp/yzts/state -status-out /tmp/yzts/status.json -selftest 15s
```

第一次執行會輸出 `login-required` 事件與 AuthURL；事件為 stdout 的 JSON lines，tsnet 自己的 log 在 `<dir>/tsnet.log`。狀態目錄含節點私鑰與憑證私鑰，結束後刪除，並到管理後台移除節點。
