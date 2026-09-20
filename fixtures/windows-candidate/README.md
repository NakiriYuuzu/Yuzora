# Windows 候選版 HTML 測試資料

這個目錄是獨立、只含假資料的測試工作區。將**本目錄**（不是 repository 根目錄）複製到 Windows 或 WSL 後，用 Yuzora 開啟它，對 `預覽 測試.html` 右鍵「在此預覽」。不需要 Dev Server。

預期：米白背景、綠色卡片與勾號、`JS module 與相對 import 已載入`、可點擊的計數器、同源 iframe 及 open shadow DOM 按鈕。子目錄的 `.htm` 也應套用根路徑 CSS 並載入相對 JS module／圖片。

`form-card` 中只有假資料，用於驗證複製結果不含密碼、即時欄位與 textarea 內容；請勿放入真實密碼。展開「超大元素」可驗證複製內容限制。

越界測試：先在此目錄的**上一層**建立 `outside.txt`，內容填 `OUTSIDE-DEMO-ONLY`。確認同目錄的正常 CSS／module 能載入後，按「測試越界資源拒絕」應顯示 PASS。該按鈕結果只驗證這一條請求；不能代替所有資源安全測試。

完整手動案例見 `docs/testing/windows-v0.0.15.md`。
