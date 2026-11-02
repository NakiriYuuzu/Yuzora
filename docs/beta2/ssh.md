# L4 · SSH／SFTP

beta.2 的 SSH 主機、SFTP 雙欄傳輸、tunnel 與已知主機。Rust 原生層是 `app/crates/yuzora-ssh`（JS 以 `import … from "yuzora:ssh"` 使用），畫面是 `app/defaults/mods/` 下的 L4 mods。設計依據是設計稿 p5（SFTP 畫板）與 p6（側欄 SSH 模式）。

## 範圍與檔案

| 路徑 | 內容 |
| --- | --- |
| `app/crates/yuzora-ssh/src/config.rs` | `~/.ssh/config` 解析：Include（相對、`~`、glob、條件式引入、16 層上限）、Host 萬用字元與否定、Match（`all`／`host`／`originalhost`／`user`／`localuser`）、第一個值生效、token 展開、ProxyJump／ProxyCommand 等常用選項。唯讀 |
| `hosts.rs` | 三個主機來源合併：ssh config 的具名 Host、Yuzora profiles（settings `ssh.profiles`）、herdr machines（`hc machine ls`，由 mod 傳入）。id 為 `ssh:<alias>`、`profile:<id>`、`machine:<name>` |
| `known_hosts.rs` | OpenSSH known_hosts：明文／hashed（`|1|`）、`[host]:port`、萬用字元、否定、`@revoked`；`@cert-authority` 忽略。只寫 Yuzora 自己的檔 |
| `session.rs` | 每台主機一個共用 SSH session（`Manager::session`）、host key 檢查、ProxyJump（多段）、ProxyCommand、keepalive、延遲量測 |
| `auth.rs` | 登入：ssh-agent → 私鑰檔（加密時問密語）→ keyboard-interactive → 密碼；遵守 PreferredAuthentications、IdentitiesOnly、IdentityAgent、各 `*Authentication` 開關，支援 partial success（多因子） |
| `prompt.rs` | 連線途中要問使用者的問題佇列（新金鑰、金鑰變更、密碼、密語、keyboard-interactive），任何 mod 都能 claim 並回答 |
| `sftp.rs` | 雙欄瀏覽：本機與遠端同一種 entry 形狀；新增資料夾、重新命名、刪除（資料夾遞迴）；拒絕 `/`、`~` 等根目錄與不合法名稱 |
| `transfer.rs` | 傳輸佇列：檔案或整個資料夾、同時 3 個、進度／速度／取消／重試、`.part` 續傳、保留權限與時間 |
| `edit.rs` | 開啟遠端檔案：下載到暫存資料夾並 watch，每次存檔自動上傳；主機上的檔案被別人改過就暫停並標示衝突 |
| `tunnel.rs` | `ssh -L` 本機轉送，給 L3 連遠端 DB（見下方 API） |
| `runtime.rs` | crate 自己的 tokio runtime；它回傳的 future 可在任何 executor 上 await |
| `js.rs` | `yuzora:ssh` HostModule 與 TypeScript 宣告 |
| `src/tests/` | 整合測試：測試行程內的 russh SSH 伺服器＋系統的 OpenSSH `sftp-server` |
| `yuzora-sidebar-ssh` | 側欄 SSH 模式：主機清單（LED、目標、延遲）、＋ 新增、選中主機的 4 個快速動作；也負責開啟問題對話框、更新狀態列與模式列徽章 |
| `yuzora-sftp` | tool pane：本機 ↔ 主機雙欄、→／←、拖放、傳輸佇列、編輯中的遠端檔；`mod.json` 宣告 `yuzora.pane`，L1 的「＋ 新增 pane」會列出 |
| `yuzora-ssh-prompt` | 連線問題對話框（host key 指紋、變更警告、密碼、密語、keyboard-interactive） |
| `yuzora-ssh-host` | 新增／編輯／刪除 Yuzora profile |
| `yuzora-ssh-known-hosts` | 已知主機管理 |
| `yuzora-ssh-herdr` | HERDR Session 引導（`hc machine add`） |
| `app/vendor/russh-0.62.7-yuzora`、`russh-sftp-3.0.0-yuzora` | 由 L0 從 Tauri 版複製的 vendored patches，本 lane 開始實際使用（`app/Cargo.toml` 的 patch 不再出現 unused 警告） |

## 決策

1. **自己實作 SSH，不靠 OpenSSH 子行程。** SFTP、tunnel、延遲與 host key 提示都需要一條長駐、可共用的連線；用 russh（vendored patch）在 Rust 層做。終端機例外：「新終端機」仍交給 herdr-core（machine）或本機 OpenSSH（`ssh <alias>`），與使用者自己打指令的行為一致。
2. **known_hosts：讀 `~/.ssh`，只寫 Yuzora 自己的檔。** 檢查時合併 Yuzora 的 `<app home>/yuzora/known_hosts` 與 ssh config 指定的 `UserKnownHostsFile`／`GlobalKnownHostsFile`（預設 `~/.ssh/known_hosts{,2}`、`/etc/ssh/ssh_known_hosts{,2}`），語意同 OpenSSH：任何一行相符即信任、同演算法不同金鑰為「已變更」、`@revoked` 一律拒絕。新金鑰或使用者接受的變更只寫進 Yuzora 的檔（同主機同演算法的舊行會被取代）。`~/.ssh` 永遠不寫；已知主機管理只能刪 Yuzora 的項目，`~/.ssh` 的項目唯讀並提示 `ssh-keygen -R`。
3. **StrictHostKeyChecking 照 ssh config。** `ask`（預設）問使用者；`yes` 拒絕未知金鑰；`no`／`accept-new` 自動記錄新金鑰。金鑰變更一律要使用者明確接受（沒人能問時直接拒絕）。
4. **連線途中的問題用佇列，不用兩段式連線。** keyboard-interactive（例如 OTP）的問題綁在同一條連線上，無法「失敗後重連再帶答案」。所以 connect 時 Rust 端註冊問題並等待（180 秒逾時），mod 端輪詢 `prompts()`，用 `claimPrompt()` 保證同一題只開一個對話框；對話框不回答就關閉會取消該題。
5. **密碼與密語只在使用者勾選「記住」時存進系統鑰匙圈**（`yuzora_core::secrets`，namespace `ssh`，id `password:<host id>`、`passphrase:<key path>`）。設定檔只存 profile 的非機密欄位。
6. **非互動連線（給其他 lane 的 tunnel）不問人。** 主機金鑰未信任或需要密碼時立即失敗（`host-key-unknown`、`needs-input`），訊息請使用者先從 SSH 側欄連一次。這避免 L3 的資料庫連線卡在一個沒人看得到的問題上。
7. **續傳以 `.part` 檔大小為準**（同 `sftp reget`／`reput`）：傳輸寫到 `<目的>.part`，完成才 rename；失敗或取消後，重試（或同一個傳輸再來一次）會從 `.part` 的大小接續，UI 顯示「續傳」。`.part` 比來源大時重新開始。
8. **russh 用 `ring` 而非預設的 `aws-lc-rs`**：`ring` 已在 app 的 lock 中，且不需要 cmake。`rsa` feature 保留（RSA 金鑰與只有 RSA 的主機）；其 RUSTSEC-2023-0071 已在 root `deny.toml`／`.cargo/audit.toml` 以「app: russh rsa」接受，本 lane 沒有新增任何例外。app workspace 目前沒有 cargo-deny／audit 關卡。

## `yuzora:ssh`（JS）

完整宣告在 app home 的 `yuzora.d.ts`（來源：`app/crates/yuzora-ssh/src/js.rs` 的 `DECLARATIONS`）。重點：

- 主機：`hosts(machines?)`、`hostStates()`（只回有 session 或錯誤的主機，給側欄輪詢用，不重讀設定）、`connect(host)`、`disconnect(host)`、`ping(host)`、`saveProfile(p, password?)`、`removeProfile(id)`、`forgetPassword(host)`、`sshArgv(host)`。
- 問題：`prompts()`、`claimPrompt(id)`、`answerPrompt(id, answer)`。
- SFTP：`list(side, path)`（side `null` 是本機）、`mkdir`、`rename`、`remove`、`localHome()`。
- 傳輸：`transfer({host, direction, from, toDir, preserve})`、`transfers()`、`cancelTransfer(id)`、`retryTransfer(id)`、`clearTransfers()`。
- 編輯：`openRemote(host, path)`、`edits()`、`stopEdit(id)`、`overwriteEdit(id)`。
- 已知主機：`knownHosts()`、`removeKnownHost(file, line, raw)`。
- tunnel：`openTunnel(host, remoteHost, remotePort, localPort?)`、`tunnels()`、`closeTunnel(id)`。
- 狀態：`status()` 回傳 `{text, tone}`，側欄與 SFTP pane 以 `setShared("yuzora.status.ssh", …)` 發布；`yuzora.badge.ssh` 是進行中＋排隊中的傳輸數（沒有時為 `null`）。
- 錯誤以 `"[code] message"` 拋出，code 見 `ErrorKind::code`（`host-key-unknown`、`needs-input`、`auth-failed`…），message 依語系。

## Tunnel API（給 L3）

```rust
use yuzora_ssh::tunnel::{self, TunnelSpec};

// host 可以是 ssh config alias、host id（"ssh:lab-01"、"profile:…"、"machine:…"）或 "user@host:port"。
let t = tunnel::open(TunnelSpec::new("lab-01", "127.0.0.1", 5432)).await?;   // 任何 executor
// let t = tunnel::open_blocking(spec)?;                                      // 一般執行緒
let port = t.local_port();            // 連 127.0.0.1:port
t.close();                            // 或 drop
```

- `TunnelSpec::local_port(n)` 指定本機埠（預設 0＝自動），`interactive(true)` 允許第一次連線問使用者（預設不問，見決策 6）。
- 同一主機的 session 與 SFTP、其他 tunnel 共用；session 斷掉後，下一個經過 tunnel 的連線會自動以非互動方式重連。
- `tunnel::list()` 列出所有 tunnel 的連線數與流量；主機被 `disconnect` 時其 tunnel 一併關閉，`Tunnel::is_open()` 變成 false。
- 錯誤是 `yuzora_ssh::SshError`（`kind: ErrorKind`、`message`）。

## 測試

```bash
export PATH="$HOME/.local/zig-0.16.0:$PATH"
export CARGO_TARGET_DIR=/Users/yuuzu/HanaokaYuuzu/App/Tauri/.yuzora-lanes/_targets/slot-3-app
cd app && cargo test --locked -p yuzora-ssh
```

- 單元測試：config、known_hosts、hosts、prompt、runtime、sftp 路徑、傳輸計畫、i18n 兩語系 key 一致、HostModule 宣告一致。
- 整合測試（`src/tests/`，Unix）：測試行程內啟動 russh SSH 伺服器（密碼、公鑰、keyboard-interactive、direct-tcpip），SFTP 交給系統的 OpenSSH `sftp-server`（以 `-d` 指到暫存資料夾）。涵蓋首次連線確認並記錄金鑰、金鑰變更拒絕／接受、加密私鑰問密語（含輸錯重問）、keyboard-interactive 兩題、tunnel 轉送與關閉、ProxyJump、SFTP 瀏覽與增刪改、上傳續傳＋保留權限時間、資料夾上傳、取消後重試、下載續傳與資料夾下載、遠端檔編輯上傳與衝突。
- 機器上沒有 `sftp-server` 時，SFTP 相關整合測試印出 skipped 後通過（macOS 一定有；Ubuntu 需 `openssh-sftp-server`）。
- 所有測試只用暫存目錄與記憶體憑證庫，不讀寫 `~/.ssh`、系統鑰匙圈或 app home。

## 待辦與已知限制

- 後續整合（合併後由總指揮串接）：
  - L1：狀態列讀 `shared("yuzora.status.ssh")`、模式列讀 `shared("yuzora.badge.ssh")`。問題對話框目前由 SSH 側欄與 SFTP pane 輪詢開啟；若其他 lane 以互動模式連線（例如 L3 想在 DB 側欄內觸發），需要一個常駐的地方（L1 外殼）呼叫 `prompts()`／`claimPrompt()` 開啟 `yuzora-ssh-prompt`。
  - L3：`yuzora_ssh::tunnel` 連遠端 DB（L3 crate 加 `yuzora-ssh` 依賴）。
  - L5：「開啟遠端檔案」目前用系統預設程式開啟暫存檔；L5 編輯器合併後可改成開 L5 的編輯器 pane（`openRemote` 回傳的 `local` 路徑不變）。
- 未支援：`Match exec`／`localnetwork`／`tagged` 等條件（視為不成立）、`CertificateFile`／host 憑證（`@cert-authority`）、`CanonicalizeHostname`、`ControlMaster`、`${ENV}` 展開、SFTP 斷點續傳的內容比對（僅以大小判斷，同 OpenSSH）。
- ProxyJump 的第一段照它自己的設定（包含它自己的 ProxyJump），後續各段忽略自己的 ProxyJump／ProxyCommand（同 OpenSSH `-J`）。
- Windows：agent 走 `\\.\pipe\openssh-ssh-agent`／Pageant；整合測試只在 Unix 跑，Windows 未實機驗證。
