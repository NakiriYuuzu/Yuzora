import { beforeEach, describe, expect, it } from "vitest"

import { SAMPLING_WINDOW, usePerfStore } from "@/state/perfStore"

describe("perfStore", () => {
    beforeEach(() => {
        usePerfStore.getState().reset()
    })

    it("starts with a null snapshot", () => {
        expect(usePerfStore.getState().snapshot).toBeNull()
    })

    it("setSnapshot stores the latest sample", () => {
        usePerfStore.getState().setSnapshot({
            cpuPercent: 12.5,
            memoryBytes: 184_000_000,
            appCpuPercent: 4.5,
            appMemoryBytes: 102_000_000,
            descendantCount: 3,
            webviewCpuPercent: 0,
            webviewMemoryBytes: 0,
            webviewCount: 0,
            managedToolsCpuPercent: 0,
            managedToolsMemoryBytes: 0,
            managedToolsCount: 0,
        })
        expect(usePerfStore.getState().snapshot).toEqual({
            cpuPercent: 12.5,
            memoryBytes: 184_000_000,
            appCpuPercent: 4.5,
            appMemoryBytes: 102_000_000,
            descendantCount: 3,
            webviewCpuPercent: 0,
            webviewMemoryBytes: 0,
            webviewCount: 0,
            managedToolsCpuPercent: 0,
            managedToolsMemoryBytes: 0,
            managedToolsCount: 0,
        })
    })

    it("reset clears the snapshot back to null", () => {
        usePerfStore.getState().setSnapshot({
            cpuPercent: 3,
            memoryBytes: 1,
            appCpuPercent: 3,
            appMemoryBytes: 1,
            descendantCount: 0,
            webviewCpuPercent: 0,
            webviewMemoryBytes: 0,
            webviewCount: 0,
            managedToolsCpuPercent: 0,
            managedToolsMemoryBytes: 0,
            managedToolsCount: 0,
        })
        usePerfStore.getState().reset()
        expect(usePerfStore.getState().snapshot).toBeNull()
    })

    // --- issue #40 §3.5：有界的採樣健康度 --------------------------------

    it("samplingHealth 一開始是全零且沒有錯誤", () => {
        expect(usePerfStore.getState().samplingHealth()).toEqual({
            attempts: 0,
            failures: 0,
            empty: 0,
            skippedNoFocus: 0,
            lastError: null
        })
    })

    it("recordOutcome 分別累計 ok / failed / empty / skipped_no_focus", () => {
        const { recordOutcome } = usePerfStore.getState()
        recordOutcome("ok")
        recordOutcome("failed", "boom")
        recordOutcome("skipped_no_focus")
        recordOutcome("empty")
        recordOutcome("ok")

        expect(usePerfStore.getState().samplingHealth()).toEqual({
            attempts: 5,
            failures: 1,
            empty: 1,
            skippedNoFocus: 1,
            lastError: "boom"
        })
    })

    it("empty（Ok(None)）與 ok 是不同的結果，不可混為一談", () => {
        // 後端有回應但沒有資料 ≠ 一切正常。混淆會讓 log 寫下「全 0、失敗 0 次」，
        // 與「真的用 0 bytes」無法區分。
        usePerfStore.getState().recordOutcome("empty")
        const health = usePerfStore.getState().samplingHealth()
        expect(health.empty).toBe(1)
        expect(health.failures).toBe(0)
        expect(health.attempts).toBe(1)
    })

    it("失敗沒帶訊息時仍留下可辨識的 lastError", () => {
        usePerfStore.getState().recordOutcome("failed")
        expect(usePerfStore.getState().samplingHealth().lastError).toBe("unknown error")
    })

    it("視窗有界：超過 SAMPLING_WINDOW 次之後只保留最近的結果", () => {
        for (let index = 0; index < SAMPLING_WINDOW + 15; index += 1) {
            usePerfStore.getState().recordOutcome("ok")
        }
        expect(usePerfStore.getState().samplingHealth().attempts).toBe(SAMPLING_WINDOW)
    })

    it("失敗滑出視窗後，failures 與 lastError 一起清掉", () => {
        usePerfStore.getState().recordOutcome("failed", "boom")
        expect(usePerfStore.getState().samplingHealth().failures).toBe(1)

        for (let index = 0; index < SAMPLING_WINDOW; index += 1) {
            usePerfStore.getState().recordOutcome("ok")
        }

        const health = usePerfStore.getState().samplingHealth()
        expect(health.failures).toBe(0)
        // 一個「永不過期」的錯誤狀態會永久蓋住真實情況（issue #15 的教訓）。
        expect(health.lastError).toBeNull()
    })

    it("視窗內仍有其他失敗時，lastError 不會被誤清", () => {
        const { recordOutcome } = usePerfStore.getState()
        recordOutcome("failed", "first")
        recordOutcome("ok")
        recordOutcome("failed", "second")
        recordOutcome("ok")

        expect(usePerfStore.getState().samplingHealth().lastError).toBe("second")
    })

    it("reset 會清掉採樣結果與 stall 狀態", () => {
        usePerfStore.getState().recordOutcome("failed", "boom")
        usePerfStore.getState().setStall({
            longTaskSupported: true,
            longTaskCount: 2,
            longTaskTotalMs: 150,
            longTaskMaxMs: 90,
            eventLoopLagSamples: 4,
            eventLoopLagMaxMs: 30,
            eventLoopLagMeanMs: 10
        })
        usePerfStore.getState().reset()

        expect(usePerfStore.getState().stall).toBeNull()
        expect(usePerfStore.getState().samplingHealth()).toEqual({
            attempts: 0,
            failures: 0,
            empty: 0,
            skippedNoFocus: 0,
            lastError: null
        })
    })
})
