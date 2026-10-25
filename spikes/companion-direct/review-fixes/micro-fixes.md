# Phone kit micro-fixes 結案報告

已完成所有指定修正；程式與證據僅寫入 `spikes/companion-direct/**`，另依指定輸出路徑保存此報告。

## 逐項修正與證據

1. **CA 私鑰永不序列化**：`make-ca.py` 以 `write_cert("ca", ca)` 僅寫公有憑證；無 unlink/finally，leaf keys 保留。新增 leaf 簽發失敗 mock，記錄 writer 全部檔名，確認只寫 `ca.pem`。證據：`review-fixes/ca-tests.txt`、`ca-key-check.txt`。
2. **嚴格 bind**：匯出 `isAllowedBindAddress`，先驗證四段十進位、0–255、無前導零，再檢查 loopback/RFC1918；以 `import.meta.main` 隔離 server 啟動。證據：`lan/server.test.ts`、`review-fixes/lan-tests.txt`。
3. **Q7 收尾**：`RUNBOOK.md:66,69` 註明 UV required 會要求無 PIN 的金鑰設定 PIN，並以 Chrome 或 Yubico Authenticator 刪除所有測試 credential。同步將 CA 已刪除措辭改為從未落地。
4. **不可由 -O 關閉的檢查**：`probe.py`、`self-test.py` 改 explicit condition + SystemExit，保留原訊息；make-ca 原本無 assert。正常及最佳化模式 probe tests 均拒絕兩種 symlink escape。證據：`probe-tests.txt`、`probe-tests-optimized.txt`。
5. **重產與 TLS 驗證**：先刪除舊 `lan/out/`，重產 `yuzora-spike.local`。正常/最佳化 self-test 都確認合法 DNS 成功，other.local / IP 分別因 permitted/excluded subtree violation 失敗；所有 server 已停止。證據：`lan/SELFTEST.txt`、`review-fixes/lan-self-test*.txt`、`ca-generation.txt`、`ca-extensions.txt`、`listener-cleanup.txt`。

## 新 CA 指紋（拋棄式測試輸出，**不可安裝**）

`FF:2D:99:BC:CC:10:A5:06:19:92:79:39:3A:E5:EB:03:41:DA:7C:6D:BB:21:64:CE:37:1A:92:74:E8:A2:E0:77`

`ca-tests.txt` 另有單元測試 temporary CA 指紋，該 scratch 已刪除，不是以上 out CA。

**後續：**交由獨立 reviewer 驗收。手機、PIN/硬體金鑰與平台 constraints 仍待使用者實機，不可將本輪 loopback 結果視為真機通過。

```acceptance-report
{
  "criteriaSatisfied": [
    {
      "id": "criterion-1",
      "status": "satisfied",
      "evidence": "只修改 spikes/companion-direct/**（另寫指定報告）；完成四項 micro-fixes 與拋棄式 CA 重產，未碰其他 workers 範圍。"
    },
    {
      "id": "criterion-2",
      "status": "satisfied",
      "evidence": "回歸測試、正常/最佳化測試、TLS 正負例、CA fingerprint、權限、listener cleanup 與空 index 證據均已保存。"
    }
  ],
  "changedFiles": [
    "spikes/companion-direct/lan/make-ca.py",
    "spikes/companion-direct/lan/make-ca.test.py",
    "spikes/companion-direct/lan/server.ts",
    "spikes/companion-direct/lan/server.test.ts",
    "spikes/companion-direct/lan/self-test.py",
    "spikes/companion-direct/herdr-multiclient/probe.py",
    "spikes/companion-direct/herdr-multiclient/probe.test.py",
    "spikes/companion-direct/RUNBOOK.md",
    "spikes/companion-direct/lan/SELFTEST.txt",
    "spikes/companion-direct/review-fixes/ca-tests.txt",
    "spikes/companion-direct/review-fixes/probe-tests.txt",
    "spikes/companion-direct/review-fixes/probe-tests-optimized.txt",
    "spikes/companion-direct/review-fixes/lan-tests.txt",
    "spikes/companion-direct/review-fixes/page-tests.txt",
    "spikes/companion-direct/review-fixes/ca-generation.txt",
    "spikes/companion-direct/review-fixes/ca-key-check.txt",
    "spikes/companion-direct/review-fixes/ca-extensions.txt",
    "spikes/companion-direct/review-fixes/lan-self-test.txt",
    "spikes/companion-direct/review-fixes/lan-self-test-optimized.txt",
    "spikes/companion-direct/review-fixes/listener-cleanup.txt",
    "spikes/companion-direct/review-fixes/staged-files.txt",
    "spikes/companion-direct/review-fixes/micro-fixes.md",
    "spikes/companion-direct/lan/out/ (ignored throwaway artifacts regenerated)"
  ],
  "testsAddedOrUpdated": [
    "spikes/companion-direct/lan/make-ca.test.py",
    "spikes/companion-direct/lan/server.test.ts",
    "spikes/companion-direct/herdr-multiclient/probe.test.py"
  ],
  "commandsRun": [
    {
      "command": "uv run --with cryptography python spikes/companion-direct/lan/make-ca.test.py",
      "result": "passed",
      "summary": "3 tests OK；模擬 leaf 簽發失敗時 writer 僅記錄 ca.pem，從未寫入 ca.key；成功案例保留 leaf keys。"
    },
    {
      "command": "uv run python spikes/companion-direct/herdr-multiclient/probe.test.py",
      "result": "passed",
      "summary": "1 test OK；兩個 routing 欄位的 symlink escape 均拒絕，SystemExit 保留原始 session 訊息。"
    },
    {
      "command": "PYTHONOPTIMIZE=1 uv run python spikes/companion-direct/herdr-multiclient/probe.test.py",
      "result": "passed",
      "summary": "1 test OK；最佳化模式仍拒絕 escapes，subprocess 完全 mock，未呼叫 Herdr。"
    },
    {
      "command": "bun test spikes/companion-direct/lan",
      "result": "passed",
      "summary": "2 pass，39 expect；8 個合法值及 31 個無效值；匯入 validator 不啟動 listener。"
    },
    {
      "command": "bun test spikes/companion-direct/tailscale/page.test.ts",
      "result": "passed",
      "summary": "5 pass，28 expect；純 Bun mock，未啟動瀏覽器。"
    },
    {
      "command": "rm -rf spikes/companion-direct/lan/out && uv run --with cryptography python spikes/companion-direct/lan/make-ca.py yuzora-spike",
      "result": "passed",
      "summary": "舊 out 已先刪除；重新產生 yuzora-spike.local 拋棄式 CA 與 leaf。"
    },
    {
      "command": "uv run python spikes/companion-direct/lan/self-test.py",
      "result": "passed",
      "summary": "正例 verify/curl exit 0；other.local 與 IP 分別 error 47/48，兩者 curl exit 60；三個 loopback servers 正常停止。"
    },
    {
      "command": "PYTHONOPTIMIZE=1 uv run python spikes/companion-direct/lan/self-test.py",
      "result": "passed",
      "summary": "同樣通過；輸出另存 lan-self-test-optimized.txt，SELFTEST.txt 為本次結果。"
    },
    {
      "command": "test ! -e spikes/companion-direct/lan/out/ca.key",
      "result": "passed",
      "summary": "通過。"
    },
    {
      "command": "rg -n \"ca.key\" spikes/companion-direct/lan/make-ca.py",
      "result": "passed",
      "summary": "只有第 98 行說明訊息，沒有 ca.key 寫入路徑。"
    },
    {
      "command": "rg -n '\\bassert\\b' spikes/companion-direct/lan/make-ca.py spikes/companion-direct/lan/self-test.py spikes/companion-direct/herdr-multiclient/probe.py",
      "result": "passed",
      "summary": "零命中（預期 rg exit 1）；三個檔案沒有可被 -O 移除的 assert。"
    },
    {
      "command": "uv run --with cryptography python - (CA extensions / artifact permissions check)",
      "result": "passed",
      "summary": "critical nameConstraints：唯一 DNS yuzora-spike.local，排除全部 IPv4/IPv6；所有 artifacts 0600，ca.key 不存在。"
    },
    {
      "command": "curl --noproxy '*' --silent --show-error --max-time 2 http://127.0.0.1:{47110,47443}/ (逐 port 執行)",
      "result": "passed",
      "summary": "兩個 port 均 exit 7，拒絕連線。"
    },
    {
      "command": "ps -axo pid=,comm=,args=",
      "result": "passed",
      "summary": "沒有 kit Bun server process。"
    },
    {
      "command": "git diff --cached --name-only",
      "result": "passed",
      "summary": "空；staged-files.txt 為空檔。"
    },
    {
      "command": "git diff --check -- spikes/companion-direct",
      "result": "passed",
      "summary": "exit 0；此 kit 在 worktree 為 untracked，故這不是完整 untracked patch 檢查。"
    }
  ],
  "validationOutput": [
    "make-ca 3 tests OK；probe 正常及 PYTHONOPTIMIZE=1 各 1 test OK；Bun LAN 2 pass；Bun page 5 pass。",
    "TLS 合法 DNS OK；other.local: permitted subtree violation；127.0.0.1: excluded subtree violation。",
    "新 CA SHA-256: FF:2D:99:BC:CC:10:A5:06:19:92:79:39:3A:E5:EB:03:41:DA:7C:6D:BB:21:64:CE:37:1A:92:74:E8:A2:E0:77（只供拋棄式輸出識別，不可安裝）。",
    "curl 47110/47443 refused；ps 無 kit Bun；Git index 為空。"
  ],
  "residualRisks": [
    "未做手機、硬體金鑰、瀏覽器或平台 trust-store 驗收；loopback 不代表手機 constraints 支援。",
    "out/ 保留測試 leaf 私鑰；不可安裝本次 CA，實機測試前須刪除 out/ 並以使用者實際名稱重產。",
    "required reviewer gate 尚待獨立 reviewer 確認。"
  ],
  "noStagedFiles": true,
  "diffSummary": "CA 私鑰不再序列化或落地；移除 unlink/finally。匯出嚴格 bind validator 並用 import.meta.main 防止測試匯入啟動 server。補 Q7 金鑰 credential/PIN 說明及更新 CA 敘述。安全 assert 改 SystemExit 並保留訊息。重產 out 與測試證據。",
  "reviewFindings": [
    "自查無 blocker；未代替 required independent reviewer。"
  ],
  "manualNotes": "未執行 cargo、瀏覽器、自動化 E2E、Tailscale、Herdr 或 wrapper，Git 全程唯讀且無 staging。codebase-memory/memory 工具未提供，直接讀取指定檔案。既有 kit 為 untracked，changedFiles 是本輪實際寫入清單，而非整個 Git status。"
}
```
