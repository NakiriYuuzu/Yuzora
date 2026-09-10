# Prompt 00｜整體重設計
你是負責 Yuzora 的資深產品設計師與 ADE interaction designer。請依共用 Brief，完成一套能指導後續原型與實作的完整重設計方案。

## 本輪工作
1. 先把現行畫面分成「保留的品牌特徵」「需重整的 ADE 層級」「缺失或不清楚的操作流程」；每項給 source 或畫面依據。
2. 提出一個推薦方向：沿用 Yuzora 的暖色、霧面、圓角、Spaces 與字體，把 agent 的狀態、上下文與下一步放進日常工作流。
3. 提供一個精簡替代版面，只比較監督多 agent、專注單一 agent、切換工具這三個情境；最後選定推薦方案並解釋取捨，不產生三套無關風格。
4. 覆蓋完整 app：全域 chrome、Space/Session、Attention、Agents、terminal/panes、Inspector、Editor/Git、DB、SSH/SFTP、Preview、Settings/onboarding。
5. 對每個新入口標出現有功能如何映射，以及是否需要新的 backend capability。既有唯讀 Inspector 不因改排版就自動變成可派工 agent。
6. 把後續工作拆成可獨立 review 的設計／實作 slices；依使用者價值與依賴排序。

## 必須交付
- 一段具體設計方向，避免只有「現代、premium、AI-native」形容詞。
- 保留／調整／新增表，含理由、證據、受影響頁面。
- 全 app sitemap 與工作面區域圖（可用 Mermaid／標註 wireframe）。
- 主要畫面規格：每頁的主要任務、主 action、secondary actions、context、empty/error、返回路徑。
- 三條端到端 story：日常開始工作、多 agent 等待回覆、檢視 agent 產物到 Git diff／Preview。
- 共享 visual token mapping、light/dark 策略、文字密度與 motion 邊界。
- capability/dependency matrix；目前可做與將來能力分開。
- 分階段交接與驗收清單，說明哪些需要真實 desktop/Herdr。

## 完成條件
看得出仍是同一個 Yuzora；使用者能辨識 Space/Session、需要處理的 agent 和下一步；其他工具頁保持可達；沒有把全 app 變成聊天泡泡、指標 dashboard 或 marketing landing page。
本輪交付設計與規格，不改產品 source。
