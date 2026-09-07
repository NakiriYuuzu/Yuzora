# 舊版 WSL bridge 清理

新版本不會載入或部署舊 bridge。清理工具預設只預覽；加入 `--apply`（WSL）或 `-Apply`（Windows）才移除符合所有權條件的項目。可重複執行，已移除的 adapter／registration 會回報 absent。

在指定 WSL 發行版內執行：

```sh
sh cleanup-wsl-adapter.sh --apply "$HOME/.pi/agent"
```

若使用 PI_CODING_AGENT_DIR，請傳入該 Agent 目錄。只刪除雜湊符合舊版的兩個 adapter 檔案及相符 receipt；修改過的檔案保留。不刪除官方 integration、Claude／Codex hooks 或 Agent Session 資料。

在 Windows 執行 `cleanup-windows-registration.ps1 -LegacyResourceRoot <舊版資源根目錄> -HerdrPath <既有 herdr.exe> -Apply`。工具比對已發布 manifest 雜湊與 registration root，只 unlink Yuzora plugin。HERDR 未執行時會停止清理，保留其狀態；工具不啟動或停止 server。請在移除舊版安裝目錄前執行。

舊 Windows runtime 安裝檔由 Windows 的舊版解除安裝／升級流程處理；本工具不遞迴刪除安裝目錄、不終止 HERDR／Agent、不移除外部 runtime 或 WSL。
