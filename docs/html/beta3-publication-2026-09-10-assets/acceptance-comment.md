發布核准紀錄：維護者已合併 PR #92，並於本次發布操作中明確確認發布版本為 `0.0.9-beta.3`。

- 已驗收候選 head：`a01f9556032b552af9715cc709bce324574164e5`
- 已合併 main commit：`c3472730b7a705ca796ac5197ccaf13472e18c51`
- 兩者 tree 完全相同：`c0847648d07f9da5bd4f6228ef60335d58a177b4`
- 候選 CI：[34388896993](https://github.com/NakiriYuuzu/Yuzora/actions/runs/34388896993)，attempt 2，全部 jobs success。
- 實機驗收：[Windows／Ubuntu-26.04 資料夾、Space、terminal 與重開恢復結果](https://github.com/NakiriYuuzu/Yuzora/pull/92#issuecomment-5607408963)。既有 server 與工作保留；完整結果及未重跑的平台範圍見該留言。

已驗收候選 installer SHA-256：

| 產物 | SHA-256 |
| --- | --- |
| Windows NSIS | `734bd587bfb714e254f52c3c4899fc6038dbb487abac222e5259b25dbfd1fa87` |
| Windows MSI | `6161413efaf4ca7f2442e512ecb44a3f2d5495e509da61eff72bcc17258de832` |
| macOS universal DMG | `0d4f8710c4b1ffcd1c1ed9e5e65fd5829f7d626f4b81230130a0e2e43446aa4d` |

本次依既有 Beta 發布流程，等待 exact main push CI 成功，由 Release workflow 建立 tag、重建並驗證三個 installer 後發布 GitHub Pre-release。正式 Release build 的 installer hash 將另外記錄，不與候選 hash 混用。

保留已合併 Changelog 的已知限制；本核准不宣稱未執行的完整平台／Agent GUI 矩陣已通過。Beta 僅供手動下載，不提供 OTA，也不更動 Stable Latest。沒有更新 workflow、放寬相容性或略過建置／payload gate。
