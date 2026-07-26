// Frontend stall telemetry（issue #40 §3.4）：long-task 與 event-loop lag。
//
// 目的是讓 Windows 的 UI hang 有可交叉定位的數字——WER 只給 `AppHangB1` 與一個
// 時間戳，本模組提供同一時間軸上「renderer 卡了多久、卡了幾次」。
//
// --- 參數驗算（不照抄 spec 的建議值） ---------------------------------------
//
// LAG_INTERVAL_MS = 500
//   每秒 2 次 wakeup，成本可忽略。往下調（100 ms）會是每秒 10 次、量測本身就開始
//   貢獻負載；往上調（2000 ms）會與 PerfBridge 的 poll 對齊而產生 aliasing，且
//   短於 2 s 的停頓可能一次都取不到樣。Windows 的 AppHangB1 門檻是 5 s 無回應，
//   500 ms 取樣讓一次 hang 至少留下 10 個樣本。
//
// LAG_RING_SIZE = 128
//   ring 覆蓋的時間 = 128 × 500 ms = 64 s。**必須 ≥ 消費端的回報週期**（PerfBridge
//   的 60 s heartbeat），否則樣本會在被讀到之前就被擠掉——那正是 #39 的「兩個閾值
//   互相架空」同一類的錯誤。64 s > 60 s，留 4 s 餘裕。記憶體 128 × 8 B ≈ 1 KB。
//   `stallTelemetry.test.ts` 有一條測試把這個關係釘死。
//
// long-task 門檻
//   **不是可調參數**：`longtask` entry type 由 Long Tasks API 規範固定在 50 ms，
//   PerformanceObserver 不接受門檻設定。因此本模組只做計數與加總，不設 threshold。

export const LAG_INTERVAL_MS = 500
export const LAG_RING_SIZE = 128

/**
 * 「這次取樣視窗算不算異常」的 lag 門檻。
 *
 * **不能是 0**：真實 timer 幾乎不可能剛好準時，閒置機器實測 500 ms 取樣的 lag
 * 是 0–2 ms、20 個樣本裡 19 個非零。用 `> 0` 判定的話 `isNotableSample` 恆為真，
 * PerfBridge 的 60 s heartbeat 會實際退化成 10 s，而且「異常」這個標記完全喪失
 * 鑑別力。250 ms（半個取樣間隔）相對閒置基線有兩個數量級的餘裕，不可能是排程
 * 雜訊。低於門檻的 lag 仍留在環形緩衝並在下一次 heartbeat 照常回報，不會遺失。
 */
export const NOTABLE_LAG_MS = LAG_INTERVAL_MS / 2

/** 距上次回報以來的採樣結果增量（PerfBridge 產生，exactly-once）。 */
export interface SamplingDelta {
    attempts: number
    failures: number
    empty: number
    skippedNoFocus: number
}

/**
 * 這次取樣視窗是否「值得立刻落盤」，而不是等滿一個 heartbeat。
 *
 * 抽成純函式是必要的：vitest 的 fake timers 讓 `Date.now()` 與 timer queue 完全
 * 同步、callback 剛好在排定時刻觸發 → lag 恆為 0，因此「lag 門檻是否有意義」
 * **在整合測試裡永遠測不出來**（會全綠而行為錯誤）。唯一的檢驗方式是把真實
 * 觀測值直接餵給這個函式。
 */
export function isNotableSample(
    stall: StallTelemetrySnapshot,
    delta: SamplingDelta
): boolean {
    return (
        stall.longTaskCount > 0 ||
        stall.eventLoopLagMaxMs >= NOTABLE_LAG_MS ||
        delta.failures > 0 ||
        delta.empty > 0
    )
}

export interface StallTelemetrySnapshot {
    /** 這個 webview 是否支援 Long Tasks API。false 時 longTask* 全為 0 且無意義。 */
    longTaskSupported: boolean
    longTaskCount: number
    longTaskTotalMs: number
    longTaskMaxMs: number
    /** ring 內實際保留的樣本數，上限 LAG_RING_SIZE。 */
    eventLoopLagSamples: number
    eventLoopLagMaxMs: number
    eventLoopLagMeanMs: number
}

export interface StallTelemetry {
    start: () => void
    stop: () => void
    /** 目前累計值（不清零）。 */
    snapshot: () => StallTelemetrySnapshot
    /** 讀取並清零——回報一次視窗後呼叫，讓下一個視窗從頭累計。 */
    drain: () => StallTelemetrySnapshot
}

interface PerformanceEntryLike {
    duration: number
}

interface PerformanceObserverLike {
    observe: (options: { type: string; buffered?: boolean }) => void
    disconnect: () => void
}

type PerformanceObserverCtor = new (
    callback: (list: { getEntries: () => PerformanceEntryLike[] }) => void
) => PerformanceObserverLike

/**
 * Long Tasks API 是否可用。
 *
 * **只檢查建構函式存在是不夠的**：jsdom（Node 的 perf_hooks）有
 * `PerformanceObserver`，`supportedEntryTypes` 卻是
 * `["dns","function","gc","http","http2","mark","measure","net","resource"]`
 * ——沒有 `longtask`，而且 `observe({ type: "longtask" })` **不會 throw**，只是
 * 永遠不送 entry。Safari/WKWebView 同樣有 PerformanceObserver 而沒有 longtask。
 * 少了 supportedEntryTypes 這道檢查，就會回報 `supported: true` 卻永遠 0 筆——
 * 一個看起來正常、實際上什麼都沒量到的假訊號。
 */
function longTaskEntryTypeSupported(scope: typeof globalThis): boolean {
    const ctor = (scope as { PerformanceObserver?: unknown }).PerformanceObserver
    if (typeof ctor !== "function") return false
    const supported = (ctor as { supportedEntryTypes?: readonly string[] }).supportedEntryTypes
    // `supportedEntryTypes` 缺席時 **fail closed**（判為不支援），不做樂觀嘗試。
    // 樂觀分支會讓假訊號整個繞回來：observe 不 throw → longTaskSupported = true
    // → 永遠 0 筆，看起來一切正常。回報「不支援」是誠實的，回報「支援但都沒事」
    // 是說謊。這個屬性從 Chrome 73／Safari 12.1／Firefox 65 起就有，WebView2
    // （Chromium）與 WKWebView 都具備，fail closed 不會誤傷實際的目標平台。
    return Array.isArray(supported) && supported.includes("longtask")
}

/**
 * @param scope PerformanceObserver 的來源（測試可注入假件）。
 * @param now   單調時鐘。**必須可注入**：fake timer 會讓 callback 剛好在排定
 *              時刻觸發，`Date.now()` 因此永遠等於預期值、lag 恆為 0，量測邏輯
 *              就完全測不到。注入時鐘才能模擬「event loop 被佔住」——牆鐘走的比
 *              timer 的名目延遲多，正是真實 stall 的樣子。
 */
export function createStallTelemetry(
    scope: typeof globalThis = globalThis,
    now: () => number = Date.now
): StallTelemetry {
    const lagRing = new Float64Array(LAG_RING_SIZE)
    let lagWrite = 0
    let lagFilled = 0
    let longTaskCount = 0
    let longTaskTotalMs = 0
    let longTaskMaxMs = 0
    let longTaskSupported = false
    let observer: PerformanceObserverLike | null = null
    let timer: ReturnType<typeof setTimeout> | null = null
    let expected = 0

    function recordLag(lagMs: number) {
        // 環形寫入：永遠只覆蓋最舊的一格，總配置量固定為 LAG_RING_SIZE。
        lagRing[lagWrite] = lagMs
        lagWrite = (lagWrite + 1) % LAG_RING_SIZE
        if (lagFilled < LAG_RING_SIZE) lagFilled += 1
    }

    function scheduleLagProbe() {
        expected = now() + LAG_INTERVAL_MS
        timer = setTimeout(() => {
            // 實際回呼時間 − 預期時間 = event loop 被佔住的時間。負值（timer 早到）
            // 夾成 0，避免時鐘校正把 lag 統計拉成負的。
            recordLag(Math.max(0, now() - expected))
            scheduleLagProbe()
        }, LAG_INTERVAL_MS)
    }

    function startLongTaskObserver() {
        if (!longTaskEntryTypeSupported(scope)) return
        const Ctor = (scope as unknown as { PerformanceObserver: PerformanceObserverCtor })
            .PerformanceObserver
        try {
            const created = new Ctor((list) => {
                for (const entry of list.getEntries()) {
                    longTaskCount += 1
                    longTaskTotalMs += entry.duration
                    if (entry.duration > longTaskMaxMs) longTaskMaxMs = entry.duration
                }
            })
            // `buffered: true` 讓 observer 掛上之前就已經發生的 long task 也補送過來
            // ——app 啟動時的長任務正好落在這個空窗裡。
            created.observe({ type: "longtask", buffered: true })
            observer = created
            longTaskSupported = true
        } catch {
            // 真正擋住假訊號的是上面的 supportedEntryTypes 檢查，**不是**這個
            // try/catch：實測 jsdom 對 `observe({type:"longtask"})` 並不 throw，
            // 只是永遠不送 entry。這裡純粹是對「建構函式或 observe 以其他方式
            // 失敗」的防禦，不是宣稱的那道安全網。
            observer = null
            longTaskSupported = false
        }
    }

    function read(): StallTelemetrySnapshot {
        let max = 0
        let total = 0
        for (let index = 0; index < lagFilled; index += 1) {
            const value = lagRing[index]
            total += value
            if (value > max) max = value
        }
        return {
            longTaskSupported,
            longTaskCount,
            longTaskTotalMs,
            longTaskMaxMs,
            eventLoopLagSamples: lagFilled,
            eventLoopLagMaxMs: max,
            eventLoopLagMeanMs: lagFilled === 0 ? 0 : total / lagFilled
        }
    }

    return {
        start() {
            if (timer !== null) return
            startLongTaskObserver()
            scheduleLagProbe()
        },
        stop() {
            if (timer !== null) clearTimeout(timer)
            timer = null
            observer?.disconnect()
            observer = null
        },
        snapshot: read,
        drain() {
            const value = read()
            longTaskCount = 0
            longTaskTotalMs = 0
            longTaskMaxMs = 0
            lagWrite = 0
            lagFilled = 0
            return value
        }
    }
}
