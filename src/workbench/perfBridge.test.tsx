import { afterEach, beforeEach, expect, it, vi } from "vitest"
import { cleanup, render } from "@testing-library/react"

const perfSnapshot = vi.fn()
vi.mock("../lib/ipc", () => ({
    perfSnapshot: () => perfSnapshot()
}))

const logDiagnosticsSample = vi.fn(
    async (_message: string, _metadata: Record<string, number | boolean>) => undefined
)
vi.mock("../features/logs/diagnostics", () => ({
    DIAGNOSTICS_SAMPLE_EVENT: "diagnostics.sample",
    logDiagnosticsSample: (message: string, metadata: Record<string, number | boolean>) =>
        logDiagnosticsSample(message, metadata)
}))

const terminalOutputMetricsSnapshot = vi.fn(() => ({}) as Record<string, TerminalMetrics>)
vi.mock("../terminal/terminalOutputQueue", () => ({
    terminalOutputMetricsSnapshot: () => terminalOutputMetricsSnapshot()
}))

import { LAG_WINDOW_MS, PerfBridge, REPORT_WINDOW_MS } from "./PerfBridge"
import {
    isNotableSample,
    NOTABLE_LAG_MS,
    type StallTelemetrySnapshot
} from "./stallTelemetry"

const HEARTBEAT_POLLS = REPORT_WINDOW_MS / 2000
import { SAMPLING_WINDOW, usePerfStore } from "../state/perfStore"

interface TerminalMetrics {
    pendingBytes: number
    hiddenBytes: number
    droppedBytes: number
    lastFlushLatencyMs: number
    flushCount: number
}

const SNAPSHOT = {
    cpuPercent: 12,
    memoryBytes: 370_000_000,
    appCpuPercent: 4,
    appMemoryBytes: 102_000_000,
    descendantCount: 3,
    webviewCpuPercent: 6,
    webviewMemoryBytes: 220_000_000,
    webviewCount: 2,
    managedToolsCpuPercent: 2,
    managedToolsMemoryBytes: 48_000_000,
    managedToolsCount: 1
}

/** 最近一次 diagnostics.sample 的 metadata。 */
function lastReport(): Record<string, number | boolean> {
    const last = logDiagnosticsSample.mock.calls.at(-1)
    if (!last) throw new Error("尚未寫出任何 diagnostics.sample")
    return last[1]
}

beforeEach(() => {
    vi.useFakeTimers()
    perfSnapshot.mockReset()
    perfSnapshot.mockResolvedValue(SNAPSHOT)
    logDiagnosticsSample.mockClear()
    terminalOutputMetricsSnapshot.mockReset()
    terminalOutputMetricsSnapshot.mockReturnValue({})
    usePerfStore.getState().reset()
    vi.spyOn(document, "hasFocus").mockReturnValue(true)
})

afterEach(() => {
    cleanup()
    vi.runOnlyPendingTimers()
    vi.useRealTimers()
    vi.restoreAllMocks()
})

it("polls perf_snapshot every 2000ms and feeds the store", async () => {
    render(<PerfBridge />)
    // No immediate poll — the first sample lands one interval in.
    expect(perfSnapshot).not.toHaveBeenCalled()

    await vi.advanceTimersByTimeAsync(2000)
    expect(perfSnapshot).toHaveBeenCalledTimes(1)
    expect(usePerfStore.getState().snapshot).toEqual(SNAPSHOT)

    await vi.advanceTimersByTimeAsync(2000)
    expect(perfSnapshot).toHaveBeenCalledTimes(2)
})

it("skips the poll while the window is unfocused", async () => {
    vi.mocked(document.hasFocus).mockReturnValue(false)
    render(<PerfBridge />)

    await vi.advanceTimersByTimeAsync(2000)
    expect(perfSnapshot).not.toHaveBeenCalled()
})

it("clears the interval on unmount", async () => {
    const { unmount } = render(<PerfBridge />)
    await vi.advanceTimersByTimeAsync(2000)
    expect(perfSnapshot).toHaveBeenCalledTimes(1)

    unmount()
    await vi.advanceTimersByTimeAsync(4000)
    expect(perfSnapshot).toHaveBeenCalledTimes(1)
})

// --- issue #40 §3.5：採樣失敗不可被靜默吞掉 ---------------------------------

it("採樣失敗會記進 bounded diagnostic state，而不是被 catch 吞掉", async () => {
    perfSnapshot.mockRejectedValue(new Error("perf_snapshot boom"))
    render(<PerfBridge />)

    await vi.advanceTimersByTimeAsync(2000)

    const health = usePerfStore.getState().samplingHealth()
    expect(health.attempts).toBe(1)
    expect(health.failures).toBe(1)
    expect(health.lastError).toBe("perf_snapshot boom")
    // 失敗時不得把舊快照抹成 null——那樣「沒資料」與「採樣失敗」又分不出來。
    expect(usePerfStore.getState().snapshot).toBeNull()
})

it("成功採樣記成 ok，並保留最後一次成功的快照", async () => {
    render(<PerfBridge />)
    await vi.advanceTimersByTimeAsync(2000)

    perfSnapshot.mockRejectedValue(new Error("boom"))
    await vi.advanceTimersByTimeAsync(2000)

    const health = usePerfStore.getState().samplingHealth()
    expect(health.attempts).toBe(2)
    expect(health.failures).toBe(1)
    expect(usePerfStore.getState().snapshot).toEqual(SNAPSHOT)
})

it("失焦跳過會被計數，可與採樣失敗區分", async () => {
    vi.mocked(document.hasFocus).mockReturnValue(false)
    render(<PerfBridge />)

    await vi.advanceTimersByTimeAsync(2000 * 3)

    const health = usePerfStore.getState().samplingHealth()
    expect(health.skippedNoFocus).toBe(3)
    expect(health.failures).toBe(0)
    expect(health.lastError).toBeNull()
})

it("採樣結果視窗有界：poll 數遠超過視窗時仍只保留 SAMPLING_WINDOW 筆", async () => {
    render(<PerfBridge />)
    await vi.advanceTimersByTimeAsync(2000 * (SAMPLING_WINDOW + 20))

    expect(usePerfStore.getState().samplingHealth().attempts).toBe(SAMPLING_WINDOW)
})

it("採樣恢復後，失敗指示會在一個視窗內自行過期", async () => {
    perfSnapshot.mockRejectedValue(new Error("boom"))
    render(<PerfBridge />)
    await vi.advanceTimersByTimeAsync(2000)
    expect(usePerfStore.getState().samplingHealth().failures).toBe(1)

    perfSnapshot.mockResolvedValue(SNAPSHOT)
    await vi.advanceTimersByTimeAsync(2000 * SAMPLING_WINDOW)

    const health = usePerfStore.getState().samplingHealth()
    expect(health.failures).toBe(0)
    // 視窗內已經沒有失敗，錯誤訊息也必須跟著清掉（不可永久掛著）。
    expect(health.lastError).toBeNull()
})

// --- issue #40 §3.4/§3.6：診斷落盤 -------------------------------------------

it("每 60 秒寫一筆 diagnostics.sample，帶 perf 分類、採樣健康度與 terminal queue 指標", async () => {
    terminalOutputMetricsSnapshot.mockReturnValue({
        "pty-1": {
            pendingBytes: 100,
            hiddenBytes: 20,
            droppedBytes: 7,
            lastFlushLatencyMs: 3,
            flushCount: 5
        },
        "pty-2": {
            pendingBytes: 200,
            hiddenBytes: 0,
            droppedBytes: 1,
            lastFlushLatencyMs: 9,
            flushCount: 4
        }
    })
    render(<PerfBridge />)

    await vi.advanceTimersByTimeAsync(REPORT_WINDOW_MS - 2000)
    expect(logDiagnosticsSample).not.toHaveBeenCalled()

    await vi.advanceTimersByTimeAsync(2000)
    expect(logDiagnosticsSample).toHaveBeenCalledTimes(1)

    const metadata = lastReport()
    expect(metadata.perf_memory_bytes).toBe(370_000_000)
    expect(metadata.perf_webview_memory_bytes).toBe(220_000_000)
    expect(metadata.perf_webview_count).toBe(2)
    expect(metadata.perf_descendant_count).toBe(3)
    expect(metadata.sampling_attempts).toBe(SAMPLING_WINDOW)
    expect(metadata.sampling_failures).toBe(0)
    // #39 已對齊成 UTF-8 bytes 的 terminal queue metrics，跨 session 加總。
    expect(metadata.terminal_sessions).toBe(2)
    expect(metadata.terminal_pending_bytes).toBe(300)
    expect(metadata.terminal_hidden_bytes).toBe(20)
    expect(metadata.terminal_dropped_bytes).toBe(8)
    expect(metadata.terminal_flush_count).toBe(9)
    expect(metadata.terminal_last_flush_latency_ms).toBe(9)
    // long task 在 jsdom 下量不到，但欄位必須在且標記 unsupported。
    expect(metadata.long_task_supported).toBe(false)
    expect(metadata.long_task_count).toBe(0)
})

it("採樣失敗時提早回報，不必等滿一個 heartbeat", async () => {
    perfSnapshot.mockRejectedValue(new Error("boom"))
    render(<PerfBridge />)

    // NOTABLE_MIN_POLLS = 5 → 10 秒後就會寫一筆。
    await vi.advanceTimersByTimeAsync(2000 * 5)

    expect(logDiagnosticsSample).toHaveBeenCalledTimes(1)
    expect(lastReport().sampling_failures).toBe(5)
})

it("異常持續時，回報頻率被 NOTABLE_MIN_POLLS 夾住（不會退化成每 2 秒一筆）", async () => {
    perfSnapshot.mockRejectedValue(new Error("boom"))
    render(<PerfBridge />)

    await vi.advanceTimersByTimeAsync(2000 * 20)

    // 20 次 poll / 每 5 次一筆 = 4 筆；沒有下限的話會是 20 筆。
    expect(logDiagnosticsSample).toHaveBeenCalledTimes(4)
})

it("一切正常時不會提早回報（只走 60 秒 heartbeat）", async () => {
    render(<PerfBridge />)
    await vi.advanceTimersByTimeAsync(2000 * 10)
    expect(logDiagnosticsSample).not.toHaveBeenCalled()
})

it("lag 環形緩衝覆蓋的時間必須 ≥ 回報視窗，否則樣本會在被讀到前就被擠掉", () => {
    // 參數交叉檢查（#39 的「兩個閾值互相架空」同一類錯誤的防線）。
    expect(LAG_WINDOW_MS).toBeGreaterThanOrEqual(REPORT_WINDOW_MS)
})

// --- 必修 2：notable 門檻 -----------------------------------------------------
//
// **這一組是 fake timers 盲區的防護。** vitest 的 fake timers 讓 `Date.now()` 與
// timer queue 完全同步，callback 剛好在排定時刻觸發 → lag 恆為 0。因此
// 「`eventLoopLagMaxMs > 0` 就算異常」這種門檻在 fake timers 下**永遠測不出問題**，
// 整合測試會全綠而實際行為錯誤。唯一能檢驗門檻的方法是把**真實觀測值**餵給純
// 函式 `isNotableSample`。

const IDLE_STALL: StallTelemetrySnapshot = {
    longTaskSupported: true,
    longTaskCount: 0,
    longTaskTotalMs: 0,
    longTaskMaxMs: 0,
    eventLoopLagSamples: 20,
    // 閒置機器上 500 ms 取樣的實測值：[2,1,1,1,1,1,1,2,1,1,1,1,1,2,1,1,1,1,0,1]
    // ——20 個樣本裡 19 個非零，最大 2 ms。真實 timer 幾乎不可能剛好準時。
    eventLoopLagMaxMs: 2,
    eventLoopLagMeanMs: 1.15
}

const NO_DELTA = { attempts: 20, failures: 0, empty: 0, skippedNoFocus: 0 }

it("閒置機器的真實 lag（最大 2 ms）不算異常——否則 heartbeat 會退化成 10 s", () => {
    // 用 `> 0` 判定的話這裡會是 true，60 s heartbeat 實際變成每 10 s 一筆。
    expect(isNotableSample(IDLE_STALL, NO_DELTA)).toBe(false)
})

it("lag 門檻對閒置雜訊有兩個數量級的餘裕", () => {
    expect(NOTABLE_LAG_MS).toBeGreaterThan(IDLE_STALL.eventLoopLagMaxMs * 100)
})

it("真正的 stall（≥ 半個取樣間隔）算異常", () => {
    expect(
        isNotableSample({ ...IDLE_STALL, eventLoopLagMaxMs: NOTABLE_LAG_MS }, NO_DELTA)
    ).toBe(true)
    expect(isNotableSample({ ...IDLE_STALL, eventLoopLagMaxMs: 900 }, NO_DELTA)).toBe(true)
})

it("剛好低於門檻不算異常（門檻是有效的邊界，不是裝飾）", () => {
    expect(
        isNotableSample({ ...IDLE_STALL, eventLoopLagMaxMs: NOTABLE_LAG_MS - 1 }, NO_DELTA)
    ).toBe(false)
})

it("long task 與採樣失敗／無資料一律算異常", () => {
    expect(isNotableSample({ ...IDLE_STALL, longTaskCount: 1 }, NO_DELTA)).toBe(true)
    expect(isNotableSample(IDLE_STALL, { ...NO_DELTA, failures: 1 })).toBe(true)
    expect(isNotableSample(IDLE_STALL, { ...NO_DELTA, empty: 1 })).toBe(true)
})

// --- 必修 1：落盤的採樣計數是增量，不是滾動視窗 -------------------------------

it("連續回報的 sampling_failures 加總 = 實際失敗次數（不重複計數）", async () => {
    perfSnapshot.mockRejectedValue(new Error("boom"))
    render(<PerfBridge />)

    // 20 次 poll 全部失敗 → 每 5 次一筆，共 4 筆。
    await vi.advanceTimersByTimeAsync(2000 * 20)

    const reported = logDiagnosticsSample.mock.calls.map((call) => call[1].sampling_failures)
    expect(reported).toEqual([5, 5, 5, 5])
    // Rust 端 absorb_diagnostics 對 count 類做 saturating_add：
    expect(reported.reduce((a, b) => (a as number) + (b as number), 0)).toBe(20)
})

it("單一一次失敗只會被計一次（滾動視窗會重複計到 6 次）", async () => {
    render(<PerfBridge />)
    // 先跑 4 次成功，再讓第 5 次失敗 → 第 5 次 poll 觸發 notable 回報。
    await vi.advanceTimersByTimeAsync(2000 * 4)
    perfSnapshot.mockRejectedValue(new Error("boom"))
    await vi.advanceTimersByTimeAsync(2000)
    perfSnapshot.mockResolvedValue(SNAPSHOT)
    // 再跑滿一個完整 heartbeat，確認那次失敗不會又被算進後面的報表。
    await vi.advanceTimersByTimeAsync(2000 * HEARTBEAT_POLLS)

    const total = logDiagnosticsSample.mock.calls.reduce(
        (sum, call) => sum + (call[1].sampling_failures as number),
        0
    )
    expect(total).toBe(1)
})

it("sampling_attempts 是距上次回報的 poll 數，不是視窗長度", async () => {
    perfSnapshot.mockRejectedValue(new Error("boom"))
    render(<PerfBridge />)
    await vi.advanceTimersByTimeAsync(2000 * 10)

    const attempts = logDiagnosticsSample.mock.calls.map((call) => call[1].sampling_attempts)
    expect(attempts).toEqual([5, 5])
})

// --- 必修 3：Ok(None) --------------------------------------------------------

it("perf_snapshot 回 null 記成 empty 而非 ok，且不覆寫最後一次成功的快照", async () => {
    render(<PerfBridge />)
    await vi.advanceTimersByTimeAsync(2000)
    expect(usePerfStore.getState().snapshot).toEqual(SNAPSHOT)

    perfSnapshot.mockResolvedValue(null)
    await vi.advanceTimersByTimeAsync(2000)

    const health = usePerfStore.getState().samplingHealth()
    expect(health.empty).toBe(1)
    expect(health.failures).toBe(0)
    // 一視同仁記成 ok 的話，log 會寫「perf 全 0、失敗 0 次」而 chip 無聲消失。
    expect(usePerfStore.getState().snapshot).toEqual(SNAPSHOT)
})

it("一直回 null 時會提早回報並帶 sampling_empty，不會被當成正常", async () => {
    perfSnapshot.mockResolvedValue(null)
    render(<PerfBridge />)
    await vi.advanceTimersByTimeAsync(2000 * 5)

    expect(logDiagnosticsSample).toHaveBeenCalledTimes(1)
    expect(lastReport().sampling_empty).toBe(5)
    expect(lastReport().sampling_failures).toBe(0)
})

// --- 必修 6：terminal dropped bytes 在 session 關閉後不得倒退 -------------------

it("session 關閉後 terminal_dropped_bytes 仍含它的貢獻（單調遞增）", async () => {
    terminalOutputMetricsSnapshot.mockReturnValue({
        "pty-1": {
            pendingBytes: 0,
            hiddenBytes: 0,
            droppedBytes: 5000,
            lastFlushLatencyMs: 1,
            flushCount: 10
        }
    })
    render(<PerfBridge />)
    await vi.advanceTimersByTimeAsync(REPORT_WINDOW_MS)
    expect(lastReport().terminal_dropped_bytes).toBe(5000)

    // pty-1 關閉（registry 移除），pty-2 開啟並累積 3000。
    terminalOutputMetricsSnapshot.mockReturnValue({
        "pty-2": {
            pendingBytes: 0,
            hiddenBytes: 0,
            droppedBytes: 3000,
            lastFlushLatencyMs: 1,
            flushCount: 4
        }
    })
    await vi.advanceTimersByTimeAsync(REPORT_WINDOW_MS)

    // 直接加總活著的 session 會得到 3000（低報 37.5%）。
    expect(lastReport().terminal_dropped_bytes).toBe(8000)
    expect(lastReport().terminal_flush_count).toBe(14)
    expect(lastReport().terminal_sessions).toBe(1)
})
