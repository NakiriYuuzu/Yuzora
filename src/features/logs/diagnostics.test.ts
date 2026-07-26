import { afterEach, beforeEach, expect, it } from "vitest"
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks"

import { DIAGNOSTICS_SAMPLE_EVENT, logDiagnosticsSample } from "./diagnostics"

// 與 userAction.test.ts 同樣的理由：logDiagnosticsSample 會吞掉 rejection，
// handler 內的斷言不會讓測試變紅，所以先收 payload 再比對。
interface CapturedEvent {
    level: string
    kind: string
    source: string
    workspace_path: string | null
    event: string
    message: string
    metadata: Record<string, unknown>
}

let captured: CapturedEvent[] = []

beforeEach(() => {
    captured = []
    mockIPC((cmd, payload) => {
        if (cmd === "log_event") captured.push((payload as { event: CapturedEvent }).event)
    })
})

afterEach(() => clearMocks())

it("寫入 kind=debug、level=info 的 diagnostics.sample record", async () => {
    await logDiagnosticsSample("renderer diagnostics sample", { long_task_count: 3 })

    expect(captured).toHaveLength(1)
    expect(captured[0].event).toBe(DIAGNOSTICS_SAMPLE_EVENT)
    expect(captured[0].source).toBe("diagnostics")
    expect(captured[0].kind).toBe("debug")
    // level 必須是 info：debug 在預設門檻（info）下不落盤，run-summary 會永遠空白。
    expect(captured[0].level).toBe("info")
    expect(captured[0].metadata).toEqual({ long_task_count: 3 })
})

it("event 名與 Rust run_summary::DIAGNOSTICS_SAMPLE_EVENT 一致", () => {
    // 契約：Rust 端用這個字串認出診斷 record，改名時兩邊要一起改。
    expect(DIAGNOSTICS_SAMPLE_EVENT).toBe("diagnostics.sample")
})

it("invoke 失敗時 resolve 成 undefined，不讓診斷寫入影響 poll 迴圈", async () => {
    clearMocks()
    mockIPC(() => {
        throw new Error("log_event boom")
    })
    await expect(logDiagnosticsSample("x", {})).resolves.toBeUndefined()
})
