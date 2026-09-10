# Yuzora ADE 重設計 Prompts

目標：**讓整體工作流程更像 ADE，同時保留現有 The Browser Company / Arc / Dia 視覺語言。**

建立日期：2026-09-06。對照版本：Yuzora 0.0.9-beta.3 / HEAD 9d0634c。這是設計與交接工具包，沒有修改產品 UI。

## 快速使用

最省事的方式：開啟 [Prompts 閱讀與複製頁](../../../html/yuzora-ade-redesign-prompts-2026-09-06.html)，選擇任務後按「複製完整 Prompt」。複製內容已包含 shared brief，因此可以直接貼給可讀 repo 的 agent；無 repo 權限的設計工具會依摘要產出概念設計，不能聲稱已驗證 code。

使用 Markdown 時，依序提供 [shared-brief.md](shared-brief.md) 和下表其中一份 Prompt；單獨提供分階段檔案會缺少品牌、domain 與授權範圍。不要一次要求執行所有階段。

| Prompt | 用途 |
|---|---|
| [00-master.md](00-master.md) | 一次產出整體重設計方向、資訊架構與分階段交接 |
| [01-visual-language.md](01-visual-language.md) | 保留現有 TBC 風格，調整適合 ADE 的層級和密度 |
| [02-shell-navigation.md](02-shell-navigation.md) | 釐清 Space、Session、工作內容與上下文的層級 |
| [03-agents-attention-terminal.md](03-agents-attention-terminal.md) | 形成觀察、處理、回到工作內容的完整流程 |
| [04-workbench-tools.md](04-workbench-tools.md) | 讓所有子系統共用同一套 ADE 外殼與上下文 |
| [05-interactive-prototype.md](05-interactive-prototype.md) | 讓設計方案可實際操作和比較 |
| [06-implementation-handoff.md](06-implementation-handoff.md) | 把設計轉成小範圍可驗證的改動計畫 |
| [07-design-review.md](07-design-review.md) | 檢查是否既像 ADE，又保留既有風格 |

## 建議順序

1. 先用 **00 總設計**，形成全 app 方向與資訊架構。
2. 用 **01–04** 深化樣式、shell、ADE 核心操作與工具頁；沿用前一階段方案。
3. 用 **05** 建立可操作的隔離 mock 原型。
4. 用 **07** 審查風格、流程與證據，再用 **06** 形成實作計畫。
5. 產品實作由後續另行委派的工程任務執行。這些 prompts 本身不授權修改 production、Git 寫入或 live runtime 操作。

如果只需要一份可貼上的完整 prompt，使用閱讀頁預設的 **00**。如果已有設計方案，直接挑對應分階段 prompt，附上既有產物即可，無須重新探索所有方向。

## 品牌保留重點

奶油紙色與暖深灰、低彩度 Space 漸層、霧面 chrome、柔和圓角和陰影、Hanken Grotesk / Newsreader / JetBrains Mono、Lucide，以及使用者現有 palette 選項。

現行 --yz-accent 的 lime 是 fallback；不要用設計參考中的藍色覆蓋使用者配色。舊 prototype 的 ACP/AgentZone 行為不屬於品牌樣式，不應隨外觀帶回來。

## 來源

- [研究報告](../../../html/yuzora-herdr-uiux-research-2026-09-06.html)
- [設計 handoff](../../README.md)
- [現行 styles.css](../../../../src/styles.css)
- [現行 components.json](../../../../components.json)
- [原型](../../project/Yuuzu%20Workbench.dc.html)
- [本機 TBC 參考設計系統](../../project/_ds/the-browser-company-design-system-6aff1620-8268-43e3-9c43-01c0bf16dd46/readme.md)
- [領域定義](../../../../.yuuzu/CONTEXT.html)
- [專案規則](../../../../CLAUDE.md)

所有後續設計數值、版面與新操作都是待驗證提案；來源與可用能力若有變動，以當下檔案及 runtime capabilities 重新核對。

