# pi-acp contract fixtures（P1）

`.yuuzu/specs/pi-sdk-adapter-migration.html` P1 的產物：錄下**現行社群 pi-acp** 的
ACP wire 對話作為 contract 基線，供 P2 自製 builtin adapter 做 parity 比對，以及
日後升 pi／pi-acp 的回歸閘。

## 檔案

- `driver.ts` — 最小 raw-stdio ACP client（capture 全量 wire、不經 SDK；client
  參數逐字鏡射 `src/agent/acpConnection.ts`）。
- `record.ts` — 錄音器（情境：initialize／session-prompt-write／session-cancel／
  session-load）。
- `contract.ts` — 錄音 → 結構簽名序列（忽略 id／時間／文字內容）；不變量；比對。
- `check.ts`／`compare.ts` — CLI。
- `recordings/` — 已提交的基線錄音（見各檔 meta 首行的版本資訊）。

## 用法

```sh
# 重錄基線（會真的呼叫模型；自動切 fast model＋最低 thinking）
bun fixtures/pi-acp-contract/record.ts

# 驗證錄音健全性
bun fixtures/pi-acp-contract/check.ts fixtures/pi-acp-contract/recordings/*.jsonl

# P2 parity：對 builtin adapter 重錄後與基線比對（adapter 路徑要絕對——
# record/probe 的 spawn cwd 是暫存 workspace）
bun fixtures/pi-acp-contract/record.ts --command "node $PWD/adapters/yuzora-pi-acp/dist/index.mjs" --out /tmp/candidate
bun fixtures/pi-acp-contract/compare.ts \
  fixtures/pi-acp-contract/recordings/session-prompt-write.jsonl \
  /tmp/candidate/session-prompt-write.jsonl
```

## 多 session 驗證（P2 gate）

```sh
PI_ACP_PI_COMMAND="$HOME/.local/bin/pi-no-question" \
  bun fixtures/pi-acp-contract/multi-session.ts --command "node $PWD/adapters/yuzora-pi-acp/dist/index.mjs"
```

同 cwd 兩 session 並行寫檔互不干擾＋事件 sessionId 分流＋異 cwd 隔離，10 項斷言。

## Elicitation 全鏈路驗證（P3 gate）

```sh
PI_ACP_PI_COMMAND="$HOME/.local/bin/pi-no-question" \
  bun fixtures/pi-acp-contract/elicitation-probe.ts --command "node $PWD/adapters/yuzora-pi-acp/dist/index.mjs"
```

以 project-local extension（`<cwd>/.pi/extensions`，pi 會自動載入）做決定性觸發：
select／confirm／input／editor（multiline `_meta`＋prefill default）accept、
decline、timeout（driver 故意不回應→adapter 端 default 收斂）共 6 項斷言。
driver 可設 `onElicitation`（回 `undefined`＝不回應）。

## Question 真答題＋custom fail-fast 驗證（P4 gate）

```sh
bun fixtures/pi-acp-contract/question-probe.ts \
  --command "node $PWD/adapters/yuzora-pi-acp/dist/index.mjs"
```

需要使用者環境已裝 `@vanillagreen/pi-questions`（pi settings packages）。
**不要**設 `PI_ACP_PI_COMMAND` wrapper——question tool 必須存在（builtin adapter
本來就不讀該 env；wrapper 只影響社群 pi-acp）。8 項斷言：question 兩題（單選＋
multiple:true）→ 恰一個多題 form elicitation（單選 oneOf／多選 array multiselect
／`q<i>custom` 自由文字欄）→ accept 值全數回到 tool result（以 `"answers"` 鍵
鑑別，避開 schema/rawInput 假陽性）；decline 收斂；未知 custom（project-local
extension 觸發）即時 reject＋end_turn 不 hang。

注意：driver 的 spawn cwd 是暫存 workspace——`--command` 的 adapter 路徑要用
**絕對路徑**（上例以 `$PWD` 展開）。

## 跨 runtime 續聊＋並存驗證（P5 gate）

```sh
PI_ACP_PI_COMMAND="$HOME/.local/bin/pi-no-question" \
  bun fixtures/pi-acp-contract/cross-runtime.ts \
  --builtin "node $PWD/adapters/yuzora-pi-acp/dist/index.mjs" \
  --community "bunx pi-acp@0.0.31"
```

11 項斷言：builtin ⇄ community 雙向 new→load→replay 歷史→續 prompt（兩邊共用
pi session store）；同 cwd 各一 session 並行（V4）——sessionId 相異、事件依
sessionId 分流無串音、文字級無 cross-marker。斷言刻意不依賴「模型執行 write」
與「後端不 retry」（fast model 服從度與 router 並行限流都會 flaky——那不是
runtime 隔離的驗證對象）。歷史相容關鍵：host 對 cwd 做 realpath（macOS `/var`
symlink 會讓 project 歸屬分裂，跨 runtime 互找不到 session）。

## Soak 修復驗證（config/steering/commands）

```sh
bun fixtures/pi-acp-contract/config-probe.ts \
  --command "node $PWD/adapters/yuzora-pi-acp/dist/index.mjs"
```

10 項斷言（2026-07-22 soak 回饋）：模型切換 wire 生效、thinking `max`（SDK
clamp 語意——切到支援 max 的模型後 effective=max、不假拒絕）、`usage_update`
（context tokens/window＋cost）、steering（turn 進行中送第二句→插進現行 turn、
兩個 request 都收斂）、內建 slash commands（availableCommands 併入＋`/session`
`/name` 實測）。**內建 set model/thinking 會寫 pi 全域 settings——probe 自帶
備份還原護欄；任何臨時 debug 腳本也必須帶**（教訓 ×2）。

## 比對器豁免（記錄在案）

- **floating**：`available_commands_update` 為 async best-effort 廣播（pi-acp 以
  setTimeout 0 fire-and-forget），位置不構成契約——抽出後只驗出現次數相等。
- **chunk-slide**：startup banner（agent_message_chunk）相對 info/current_mode 的
  落點是跨行程時序，分歧點若任一側為純 chunk 允許跳過續比（上限 4、列入報告）。
- **additive**：`usage_update` 為 builtin adapter 對社群基線的增量通知（context/
  pricing 通道；社群 0.0.31 沒有），比對前自兩側剔除、列入報告。

## 注意

- **pi settings 護欄**：pi 的 setModel/setThinkingLevel 會寫入全域
  `~/.pi/agent/settings.json`。record.ts 錄音前備份、結束（含失敗）還原；tuner
  刻意只動 thinking、不動 model（動態 provider 的模型存進 settings 後，新
  session 可能解析失敗直接 auth 錯）。

- 比對是**結構級**（事件序＋欄位形狀），模型輸出內容刻意不比；chunk 洪流會
  collapse 成單一事件。
- 錄音時 `PI_ACP_PI_COMMAND` wrapper（排除 question tool）照常生效——基線反映
  生產環境設定；meta 記錄了當時的 env。
- 錄音的 cwd 是 `/tmp` 暫存 workspace；session 檔會落在使用者的 pi session
  store（`~/.pi/agent/sessions`，掛在該暫存 cwd 的 project 底下，可自行清理）。
- 此目錄不進 vitest 單元套件、也不在 app 的 tsconfig 內（與 `fixtures/gen-*.ts`
  慣例一致），以 `bun` 直接執行。
