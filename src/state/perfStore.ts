import { create } from "zustand"

import type { PerfSnapshot } from "../lib/types"
import type { StallTelemetrySnapshot } from "../workbench/stallTelemetry"

// issue #40 §3.5：採樣結果不可被靜默吞掉。以**固定長度的滾動視窗**記錄最近
// SAMPLING_WINDOW 次 poll 的結果，因此：
//   1. 狀態有界（不會像無限累加的計數器一樣永遠成長）；
//   2. 狀態會過期——採樣恢復正常後，失敗指示會在一個視窗內自行消失。
// 第 2 點是刻意的：一個「永不過期」的錯誤狀態一旦取得對「每次都更新」的狀態的
// 顯示優先權，就會永久蓋住真實情況（issue #15 的教訓）。
export const SAMPLING_WINDOW = 30

/**
 * 一次 poll 的結果。
 *
 * `empty` 是 `perf_snapshot` 回 `Ok(None)`（`aggregate_tree` 找不到自身 pid）的
 * 情形——**不是** `ok`。兩者混為一談的話，log 會寫下「perf 全 0、失敗 0 次」，
 * 與「這台機器真的用 0 bytes 且一切正常」無法區分，而 StatusBar 的 chip 會整個
 * 消失且不帶任何警示——正是 §3.5／AC7 要消滅的形狀，只是換了一條路徑。
 */
export type PerfSampleOutcome = "ok" | "failed" | "empty" | "skipped_no_focus"

export interface PerfSamplingHealth {
    /** 視窗內保留的 poll 結果數，上限 SAMPLING_WINDOW。 */
    attempts: number
    failures: number
    /** 後端回 `Ok(None)`（有回應但沒有資料）的次數。 */
    empty: number
    skippedNoFocus: number
    /** 視窗內最後一次失敗的原因；沒有失敗時為 null。 */
    lastError: string | null
}

interface PerfState {
    // Latest sample from perf_snapshot, or null before the first poll / when the
    // backend returns none. StatusBar hides the chip while null.
    snapshot: PerfSnapshot | null
    outcomes: PerfSampleOutcome[]
    lastError: string | null
    stall: StallTelemetrySnapshot | null
    setSnapshot: (snapshot: PerfSnapshot | null) => void
    recordOutcome: (outcome: PerfSampleOutcome, error?: string) => void
    setStall: (stall: StallTelemetrySnapshot | null) => void
    samplingHealth: () => PerfSamplingHealth
    reset: () => void
}

const perfInitialState = {
    snapshot: null as PerfSnapshot | null,
    outcomes: [] as PerfSampleOutcome[],
    lastError: null as string | null,
    stall: null as StallTelemetrySnapshot | null
}

export const usePerfStore = create<PerfState>()((set, get) => ({
    ...perfInitialState,

    setSnapshot: (snapshot) => set({ snapshot }),

    recordOutcome: (outcome, error) =>
        set((state) => {
            const outcomes = [...state.outcomes, outcome].slice(-SAMPLING_WINDOW)
            return {
                outcomes,
                // lastError 跟著視窗走：視窗內已經沒有失敗時就清掉，避免一則舊錯誤
                // 訊息在採樣恢復後還繼續掛著。
                lastError: outcomes.includes("failed")
                    ? (outcome === "failed" ? (error ?? "unknown error") : state.lastError)
                    : null
            }
        }),

    setStall: (stall) => set({ stall }),

    // 注意：這是**滾動視窗的當下組成**，不是「距上次讀取以來的增量」。拿它去做
    // 跨報表累加會重複計數（視窗 60 s、回報間隔最短 10 s → 同一次失敗最多算 6
    // 次）。要做落盤累計請用 PerfBridge 自己的 since-last-report 計數器。
    samplingHealth: () => {
        const { outcomes, lastError } = get()
        const count = (kind: PerfSampleOutcome) =>
            outcomes.filter((outcome) => outcome === kind).length
        return {
            attempts: outcomes.length,
            failures: count("failed"),
            empty: count("empty"),
            skippedNoFocus: count("skipped_no_focus"),
            lastError
        }
    },

    reset: () => set(perfInitialState)
}))
