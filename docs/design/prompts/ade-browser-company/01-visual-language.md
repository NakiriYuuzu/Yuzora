# Prompt 01｜保留品牌，重整視覺層級
你是 Yuzora 的 design-system designer。請以現行 production tokens＋原型的視覺語言完成 ADE visual spec。

## 任務
- 逐項核對 src/styles.css、原型 .yz-root/.yzdark、_ds tokens。區分實際生效值、fallback 與參考值。
- 保留暖紙色／暖深灰、Space/palette 身份、霧面 chrome、圓角、柔和陰影、既有三種字體及 Lucide。
- 重新安排 app shell／context sidebar／work canvas／inspector／menu 的層次；重點在可讀性和資訊優先級。
- 明確界定 Newsreader 的使用位置，不把 editorial display 字級搬到高密度工作列表。
- 保留現有使用者配色選項。Agent brand、Space identity、runtime state 三種顏色語意分離。
- 指定玻璃效果可以出現的 surfaces、需要實底的 code/logs/DB reading surfaces、blur fallback。
- 定義 density、row height、hit area、icon、focus、hover/pressed、selected、attention、divider 與 surface 規格。

## 輸出
1. Token mapping 表：現有 token、用途、維持值或提案、light/dark、變更原因；未量測的數值標示為提案。
2. 元件狀態 specimen：button/icon button、tabs、agent row、attention row、session selector、pane header、empty/error、popover/inspector。
3. 三組外觀對照：idle 專注、blocked 需要處理、多 agent 工作；都包含 light/dark。
4. Brand retention checklist，列出會破壞目前風格的具體改動。
5. Contrast／zoom／reduced-motion 驗證方法，不冒稱已達標。

## 特別限制
不要引入新字體、icon library、品牌主色、全頁動畫或新的 UI framework；有必要改 token 時逐項說明，不做全域盲目 replace。不要把泛用 skill 的 font-swap／去除全部 sidebar／添加 grain 建議凌駕既有品牌。
