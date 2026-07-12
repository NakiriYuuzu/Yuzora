import { describe, expect, it } from "vitest"
// NOTE: 不用 node:fs + `new URL("./x", import.meta.url)`：
// 1) 專案沒有 @types/node，`node:fs` 會撞上 tsc bundler 解析對 URI-like specifier 的已知限制；
// 2) 兩參數 `new URL(literal, import.meta.url)` 字面量會被 Vite 的 asset-URL 靜態分析
//    重寫成 dev-server URL，讓檔案讀取在 jsdom 測試環境下失效。
// 改用 Vite 原生的 `?raw` 匯入直接取得原始檔案內容字串，兩個問題都繞開。
import src from "./acpConnection.ts?raw"

describe("acp protocol dependency", () => {
  it("imports from the maintained @agentclientprotocol/sdk, not the deprecated scope", () => {
    expect(src).toContain('from "@agentclientprotocol/sdk"')
    expect(src).not.toContain("@zed-industries/agent-client-protocol")
  })
})
