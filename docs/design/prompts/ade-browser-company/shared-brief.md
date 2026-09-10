# 共用設計 Brief：Yuzora ADE × The Browser Company

## 任務與授權範圍
重新設計 Yuzora 的整體 UI/UX，讓資訊架構、操作入口與日常工作流程更像 Agent Development Environment，同時延續目前專案中的 The Browser Company / Arc / Dia 風格。
本套 prompts 用於設計、規格、原型與實作交接；不代表已授權修改產品 UI、執行 Git 寫入或控制 live Herdr。
已提供的需求與方向直接使用；可逆的設計選擇自行作出並註明理由。只有缺少必要輸入、權限或存在真正的領域衝突時才詢問。
輸出繁體中文；技術詞保留 English；UI 文案提供 zh-TW，規格附對應 en。

## 產品基準
- Repo：/Users/yuuzu/HanaokaYuuzu/App/Tauri/yuzora
- 研究基準：2026-09-06，Yuzora 0.0.9-beta.3，HEAD 9d0634c44e314b3dcc1c39b371ed448481b2f16a。開始新工作時重新確認 HEAD 和 dirty state。
- Herdr：研究當時官方／bundled 為 0.8.2 / protocol 20；實際執行能力以選定 binary、server schema、platform、session capabilities 為準。
- 技術：Tauri 2、React 19、TypeScript、Zustand、Tailwind 4、shadcn/ui radix-nova、Lucide、CodeMirror 6、xterm。
- 核心：Space、Named Session、Agents、Attention、terminal panes；輔助：Editor/LSP、Git、Database、SSH/SFTP、Preview、Settings。
- 現行已具備 Space/Agent/Terminal、read-only Inspector、observe/control、capability gating。跨 Named Session Attention、完整 agent prompt/wait/attach/explain、worktree lifecycle、通用 Plugins 管理是研究指出的差距，開始設計前需與現行 source 核對。
- 此研究不是今天的 GUI 全功能驗收；不要從舊截圖推導 runtime 正常或斷言缺陷。

## 必讀來源與衝突處理
1. /Users/yuuzu/HanaokaYuuzu/App/Tauri/yuzora/CLAUDE.md
2. /Users/yuuzu/HanaokaYuuzu/App/Tauri/yuzora/AGENTS.md
3. /Users/yuuzu/HanaokaYuuzu/App/Tauri/yuzora/.yuuzu/CONTEXT.html 與相關 .yuuzu/adr/
4. /Users/yuuzu/HanaokaYuuzu/App/Tauri/yuzora/docs/html/yuzora-herdr-uiux-research-2026-09-06.html
5. /Users/yuuzu/HanaokaYuuzu/App/Tauri/yuzora/docs/design/README.md
6. /Users/yuuzu/HanaokaYuuzu/App/Tauri/yuzora/src/styles.css、components.json 與受影響的現行 React components
7. /Users/yuuzu/HanaokaYuuzu/App/Tauri/yuzora/docs/design/project/Yuuzu Workbench.dc.html 與其本機 imports
8. /Users/yuuzu/HanaokaYuuzu/App/Tauri/yuzora/docs/design/project/_ds/the-browser-company-design-system-6aff1620-8268-43e3-9c43-01c0bf16dd46/readme.md 及 tokens/*.css

Domain/runtime 語意以現行 production code、CONTEXT 和 accepted ADR 為準；視覺以現行 styles/persisted palette，對照原型和 tokens。發現差異時列出來，不任意覆蓋。閱讀完整 prototype 才進入視覺實作，不複製其內部架構。
此設計系統是 repo 內依公開品牌特徵重建的參考，不是 The Browser Company 官方 Figma/品牌規格。
原型與 screenshots 內的 AgentZone、ACP 對話、PINNED/RECENT 等可能是舊模型；只取視覺，不恢復已淘汰的 runtime/導覽。
若工具無 repo 存取權，使用這份摘要完成明確標為「概念設計」的方案；不要聲稱已讀來源或已驗證 implementation。需要精確對照時列出待附的檔案／截圖。

## 保留的視覺語言
- 暖紙色：--paper-1 #fbfaf6、--paper-2 #f4f1ea；暖墨色：--ink-1 #2e2c28。
- Dark mode 使用現行 production 暖深灰／紫灰層次，如 --paper-1 #1a191f；terminal 有獨立 --term-* 色票。
- 保留 --yz-bg 的低彩度多點漸層與 --yz-glass / --yz-panel 等霧面層次；內容閱讀面保持穩定對比。
- 保留 Space 的色彩識別與持久化 palette。--yz-accent #86b81f 是 CSS fallback，不是強制唯一配色；參考系統的藍色也不是新的固定主色。
- Hanken Grotesk 用於日常 UI；Newsreader 用於 Space 名稱／少量情境標題；JetBrains Mono 用於程式碼、路徑、快捷鍵與技術資料。保留既有中日文字體 fallback，驗證繁中文字形，不把 Newsreader 強套到所有工作列表。
- 4/8px 間距節奏、柔和分層圓角（6/10/14/20/28px）、溫暖陰影與圓角側欄；深層內容不要層層套大卡片。
- 沿用 Lucide；一致的圓角線條和 stroke。Space 的使用者身份圖示可保留，狀態與 action 不靠裝飾 emoji 表達。
- 延續 120/220/380ms 與現有 easing；日常互動偏快，spring 用於少量 chrome。Terminal input、輸出更新、焦點與 resize 不因動畫延遲；尊重 reduced motion。
- 品牌漸層表達 Space／身份／少量 onboarding；working/blocked/done 等狀態用穩定語意色＋文字，不與 agent 品牌色混用。
- 不為符合通用設計模板更換字體、圖示庫、palette 或引入大面積霓虹、grain、parallax、慣性捲動。既有品牌決策優先於泛用 redesign skill 建議。

## ADE 的具體目標
使用者開啟工作面後應能快速回答：
1. 我在什麼 Space、branch/worktree、Named Session？
2. 哪些 agent 在工作，哪些需要我回覆，哪些已回覆待查看？
3. 選取 agent 後，其 terminal、輸出與相關檔案／diff 在哪裡？
4. 我能做的下一步是什麼；若不能做，原因和恢復方式是什麼？
5. 切 Space／Session／工具頁後，工作上下文和 terminal process 是否保持一致？

預設探索「纖細 Spaces rail → 情境側欄 → 主要工作畫布 → 可收合 contextual inspector」，保留按需 terminal drawer。這是候選版面，不是已決議架構；提出替代版面時說明如何改善上述任務。
ADE 密度靠資訊分層、可收合區域、清楚的優先級；不要靠縮小關鍵字體或把所有功能塞入常駐面板。

## 不可破壞的語意與技術邊界
- Named Session 是 runtime namespace；Terminal Session 是 I/O 生命週期；Agent Session 是 agent 的語意對話／恢復身份。三者不能因視覺簡化而合併。
- Herdr 擁有 workspace/tab/pane、PTY/process、agent identity/state；Yuzora 透過 typed IPC/Bridge/store 投影，不從 terminal 文本或 wsl.exe 猜身份。
- working、blocked、idle、done、unknown 必須區分。done 是 idle 且未查看，不保證任務成功；unknown 不當成 done。產品可使用清楚的中文標籤，規格保留原始 enum 對照。
- App 關閉／切頁／斷線不等於停止 Herdr process；observer 不等於 controller。控制權轉移需真實狀態與失敗處理。
- 跨 Named Session 聚合是 Yuzora 的候選 UX 能力，不能假裝現行後端已持續監聽所有 session，也不能將不同 runtime 的 ID 混用。
- Windows-native Herdr 是 runtime；WSL Pi plugin 目前 Experimental。WSL shell 不代表另一個 runtime；不承諾所有 WSL agents 都可 resume/control。
- 工作意圖／任務摘要可以是展示欄位；若 source 沒有 Task/Run entity 或 authoritative metadata，不自行創造持久化 task engine，也不從 terminal output 猜「任務成功」。
- 每個擬議 action 分別標註：現有能力／新 UI 組合／需要 backend 或 Herdr 能力／純概念。產品 UI 用人類可理解的限制說明，原始 API 名留在設計規格。
- Generic components 優先使用 shadcn/ui（@/components/ui），先檢查 registry/docs 與現有元件。App-owned lists/reading panes 用 ScrollArea；CodeMirror、xterm、textarea、原生 webview 和結構性 clipping 保留其規則。
- Tauri typed IPC、Bridge 協調、per-session stores、i18n 邊界維持清楚。原型不當成 production architecture。
- 不遺失 Editor、Git、Database、SSH/SFTP、Preview、Settings 既有入口及流程，不對 live repo/DB/remote 實際操作。

## 設計與驗收要求
- 同一語言覆蓋 light/dark、loading、empty、error、unsupported、stopped、stale、observer/controller、blocked、unseen done 與 unknown。
- 一般正文／關鍵操作文字以 13–14px 作設計起點，metadata 11–12px；小於 12px 的關鍵資訊必須提出可讀性理由與實機證據。Terminal/editor 字級尊重使用者設定。這是提案規格，不是既有 token 數值聲明。
- 對比以 WCAG AA 檢查（一般文字 4.5:1，大字 3:1；可操作控件與必要圖形 3:1）；以實際背景合成值驗證半透明層。
- 所有常用操作有可見焦點、合理 Tab 順序、快捷鍵或非拖曳替代。Tooltip 不是 accessible name；不能用顏色單獨傳達狀態。
- Desktop 密度不機械套 44px 行高：一般指標 target 以至少24px/合理間隔為起點，觸控場景再調大；需把 WCAG 例外和產品目標分開。
- 比較 1440×900、1280×800、1024×768 與 200% zoom，窄於可用範圍時說明最小窗寬和折疊順序；不要為了 mobile 模板改掉桌面 ADE 主流程。
- Reference screenshots、概念稿、mock 原型、實機證據分開標記。模擬 agent 狀態／動作明示為 demo，不展示假成功／假 latency 數字。

