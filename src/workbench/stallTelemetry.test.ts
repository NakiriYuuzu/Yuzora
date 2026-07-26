import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import { createStallTelemetry, LAG_INTERVAL_MS, LAG_RING_SIZE } from "./stallTelemetry"

beforeEach(() => {
    vi.useFakeTimers()
})

afterEach(() => {
    vi.useRealTimers()
    vi.restoreAllMocks()
})

// --- long task -------------------------------------------------------------
//
// 誠實範圍：jsdom 有 PerformanceObserver，但它的 supportedEntryTypes **不含**
// longtask（見下方第一條測試），所以：
//   (a) 「偵測不到 longtask 時判成 unsupported」在真實 jsdom 環境下驗證；
//   (b) 「有 entry 時計數正確」用注入的假 PerformanceObserver 驗證。
// (b) 證明的是 callback 的計算邏輯，**不**證明真實 webview 會送來 longtask entry
// ——那是實機驗收項。另有一條測試斷言我們訂閱的 entry type 確實是 "longtask"，
// 這是離線能做到的最接近「這條路是活的」的檢查。

class FakeObserver {
    static instances: FakeObserver[] = []
    static supportedEntryTypes: string[] = ["longtask", "measure"]
    observed: { type: string; buffered?: boolean }[] = []
    disconnected = false
    constructor(public callback: (list: { getEntries: () => { duration: number }[] }) => void) {
        FakeObserver.instances.push(this)
    }
    observe(options: { type: string; buffered?: boolean }) {
        this.observed.push(options)
    }
    disconnect() {
        this.disconnected = true
    }
    emit(durations: number[]) {
        this.callback({ getEntries: () => durations.map((duration) => ({ duration })) })
    }
}

function scopeWith(ctor: unknown): typeof globalThis {
    return { PerformanceObserver: ctor } as unknown as typeof globalThis
}

// 可控的牆鐘。fake timer 讓 callback 剛好在排定時刻觸發，所以「event loop 被
// 佔住」只能靠獨立推進牆鐘來模擬（見 createStallTelemetry 的 now 參數）。
function fakeClock() {
    let value = 0
    return {
        now: () => value,
        advance: (ms: number) => {
            value += ms
        },
    }
}

describe("long task observer", () => {
    beforeEach(() => {
        FakeObserver.instances = []
        FakeObserver.supportedEntryTypes = ["longtask", "measure"]
    })

    it("真實 jsdom 環境：有 PerformanceObserver 但沒有 longtask，必須判成 unsupported", () => {
        // jsdom（Node perf_hooks）**有** PerformanceObserver，supportedEntryTypes 是
        // ["dns","function","gc","http","http2","mark","measure","net","resource"]，
        // 而且 observe({type:"longtask"}) 不會 throw、只是永遠不送 entry。
        // 若把判斷放寬成「建構函式存在就算支援」，這裡會回報 supported=true 卻
        // 永遠 0 筆——正是要防的假訊號。
        const ctor = (
            globalThis as unknown as {
                PerformanceObserver?: { supportedEntryTypes?: readonly string[] }
            }
        ).PerformanceObserver
        expect(typeof ctor).toBe("function")
        expect(ctor?.supportedEntryTypes).not.toContain("longtask")

        const telemetry = createStallTelemetry()
        telemetry.start()
        const snapshot = telemetry.snapshot()
        telemetry.stop()

        expect(snapshot.longTaskSupported).toBe(false)
        expect(snapshot.longTaskCount).toBe(0)
    })

    it("完全沒有 PerformanceObserver 時也 fail soft", () => {
        const telemetry = createStallTelemetry({} as unknown as typeof globalThis)
        expect(() => telemetry.start()).not.toThrow()
        expect(telemetry.snapshot().longTaskSupported).toBe(false)
        telemetry.stop()
    })

    it("訂閱的 entry type 是 longtask 且要求補送 buffered entries", () => {
        const telemetry = createStallTelemetry(scopeWith(FakeObserver))
        telemetry.start()

        expect(FakeObserver.instances).toHaveLength(1)
        // 訂閱錯 entry type = 整條 long-task 遙測是死碼，真實 webview 也收不到東西。
        expect(FakeObserver.instances[0].observed).toEqual([
            { type: "longtask", buffered: true },
        ])
        telemetry.stop()
    })

    it("累計 long task 的次數、總時長與最大值", () => {
        const telemetry = createStallTelemetry(scopeWith(FakeObserver))
        telemetry.start()
        FakeObserver.instances[0].emit([60, 120])
        FakeObserver.instances[0].emit([80])

        const snapshot = telemetry.snapshot()
        expect(snapshot.longTaskSupported).toBe(true)
        expect(snapshot.longTaskCount).toBe(3)
        expect(snapshot.longTaskTotalMs).toBe(260)
        expect(snapshot.longTaskMaxMs).toBe(120)
        telemetry.stop()
    })

    it("supportedEntryTypes 不含 longtask 時視為 unsupported（Safari/WKWebView）", () => {
        FakeObserver.supportedEntryTypes = ["measure", "navigation"]
        const telemetry = createStallTelemetry(scopeWith(FakeObserver))
        telemetry.start()

        expect(telemetry.snapshot().longTaskSupported).toBe(false)
        expect(FakeObserver.instances).toHaveLength(0)
        telemetry.stop()
    })

    it("supportedEntryTypes 缺席時 fail closed，不做樂觀嘗試", () => {
        // 樂觀分支會讓假訊號整個繞回來：observe 實測**不會** throw（見上一條），
        // 所以「先試試看」的結果是 supported=true 且永遠 0 筆——看起來一切正常。
        // 回報「不支援」是誠實的，回報「支援但都沒事」是說謊。
        // 獨立 class（不繼承 FakeObserver）：靜態屬性是沿著原型鏈找的，用 delete
        // 移除子類別的同名屬性只會露出父類別的那一份。
        class NoSupportedList {
            constructor(public callback: unknown) {
                observeCalls += 1
            }
            observe() {}
            disconnect() {}
        }
        let observeCalls = 0

        const telemetry = createStallTelemetry(scopeWith(NoSupportedList))
        telemetry.start()

        expect(telemetry.snapshot().longTaskSupported).toBe(false)
        expect(observeCalls).toBe(0)
        telemetry.stop()
    })

    it("supportedEntryTypes 不是陣列時同樣 fail closed", () => {
        class WeirdSupported extends FakeObserver {
            static override supportedEntryTypes = "longtask" as unknown as string[]
        }

        const telemetry = createStallTelemetry(scopeWith(WeirdSupported))
        telemetry.start()

        expect(telemetry.snapshot().longTaskSupported).toBe(false)
        telemetry.stop()
    })

    it("observe 丟例外時 fail soft，不讓遙測拖垮呼叫端", () => {
        class ThrowingObserver extends FakeObserver {
            override observe(): void {
                throw new Error("longtask unsupported")
            }
        }
        const telemetry = createStallTelemetry(scopeWith(ThrowingObserver))
        expect(() => telemetry.start()).not.toThrow()
        expect(telemetry.snapshot().longTaskSupported).toBe(false)
        telemetry.stop()
    })

    it("stop 會 disconnect observer", () => {
        const telemetry = createStallTelemetry(scopeWith(FakeObserver))
        telemetry.start()
        telemetry.stop()
        expect(FakeObserver.instances[0].disconnected).toBe(true)
    })
})

// --- event loop lag --------------------------------------------------------

describe("event loop lag", () => {
    let telemetryClock = fakeClock()

    beforeEach(() => {
        telemetryClock = fakeClock()
    })

    function makeTelemetry() {
        return createStallTelemetry({} as unknown as typeof globalThis, telemetryClock.now)
    }

    /** 讓 N 次 lag 取樣發生，每次牆鐘比 timer 的名目延遲多走 `extraMs`。 */
    async function advanceSamples(count: number, extraMs = 0) {
        for (let index = 0; index < count; index += 1) {
            telemetryClock.advance(LAG_INTERVAL_MS + extraMs)
            await vi.advanceTimersByTimeAsync(LAG_INTERVAL_MS)
        }
    }

    it("量測預期與實際回呼時間的差值", async () => {
        const telemetry = makeTelemetry()
        telemetry.start()
        // timer 比預期晚 300 ms 才跑到 = event loop 被佔住 300 ms。
        await advanceSamples(1, 300)

        const snapshot = telemetry.snapshot()
        expect(snapshot.eventLoopLagSamples).toBe(1)
        expect(snapshot.eventLoopLagMaxMs).toBe(300)
        telemetry.stop()
    })

    it("timer 準時抵達時 lag 是 0，不會出現負值", async () => {
        const telemetry = makeTelemetry()
        telemetry.start()
        await advanceSamples(3)

        const snapshot = telemetry.snapshot()
        expect(snapshot.eventLoopLagSamples).toBe(3)
        expect(snapshot.eventLoopLagMaxMs).toBe(0)
        expect(snapshot.eventLoopLagMeanMs).toBe(0)
        telemetry.stop()
    })

    it("環形緩衝有界：取樣數遠超過容量時仍只保留 LAG_RING_SIZE 筆", async () => {
        // 回歸防線：#39 的無界成長事故。緩衝若改成無界陣列，這裡會是 300 而變紅。
        const telemetry = makeTelemetry()
        telemetry.start()
        await advanceSamples(LAG_RING_SIZE + 172)

        expect(telemetry.snapshot().eventLoopLagSamples).toBe(LAG_RING_SIZE)
        telemetry.stop()
    })

    it("最舊的樣本會被擠出去（是環形緩衝，不是只夾住計數）", async () => {
        const telemetry = makeTelemetry()
        telemetry.start()
        // 先塞一個很大的 lag，再用正常樣本填滿整圈把它擠掉。
        await advanceSamples(1, 5_000)
        expect(telemetry.snapshot().eventLoopLagMaxMs).toBe(5_000)

        await advanceSamples(LAG_RING_SIZE)
        expect(telemetry.snapshot().eventLoopLagMaxMs).toBe(0)
        telemetry.stop()
    })

    it("平均值以視窗內保留的樣本計算", async () => {
        const telemetry = makeTelemetry()
        telemetry.start()
        await advanceSamples(1, 100)
        await advanceSamples(1, 300)

        const snapshot = telemetry.snapshot()
        expect(snapshot.eventLoopLagSamples).toBe(2)
        expect(snapshot.eventLoopLagMeanMs).toBe(200)
        telemetry.stop()
    })

    it("stop 之後不再取樣", async () => {
        const telemetry = makeTelemetry()
        telemetry.start()
        await advanceSamples(2)
        telemetry.stop()
        await advanceSamples(5)

        expect(telemetry.snapshot().eventLoopLagSamples).toBe(2)
    })
})

describe("drain", () => {
    it("回傳目前值並清零，讓下一個回報視窗重新累計", async () => {
        const clock = fakeClock()
        FakeObserver.instances = []
        const telemetry = createStallTelemetry(scopeWith(FakeObserver), clock.now)
        telemetry.start()
        FakeObserver.instances[0].emit([90])
        clock.advance(LAG_INTERVAL_MS + 40)
        await vi.advanceTimersByTimeAsync(LAG_INTERVAL_MS)

        const drained = telemetry.drain()
        expect(drained.longTaskCount).toBe(1)
        expect(drained.longTaskMaxMs).toBe(90)
        expect(drained.eventLoopLagMaxMs).toBe(40)

        const after = telemetry.snapshot()
        expect(after.longTaskCount).toBe(0)
        expect(after.longTaskTotalMs).toBe(0)
        expect(after.longTaskMaxMs).toBe(0)
        expect(after.eventLoopLagSamples).toBe(0)
        expect(after.eventLoopLagMaxMs).toBe(0)
        telemetry.stop()
    })
})
