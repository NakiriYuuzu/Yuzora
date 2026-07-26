import { useEffect } from "react"

import { logDiagnosticsSample } from "../features/logs/diagnostics"
import { perfSnapshot } from "../lib/ipc"
import { usePerfStore } from "../state/perfStore"
import { terminalOutputMetricsSnapshot } from "../terminal/terminalOutputQueue"
import {
    createStallTelemetry,
    isNotableSample,
    LAG_INTERVAL_MS,
    LAG_RING_SIZE,
    type SamplingDelta
} from "./stallTelemetry"

const POLL_INTERVAL_MS = 2000

// --- 診斷落盤節奏（issue #40；參數自行驗算，未照抄 spec 建議值）-------------
//
// HEARTBEAT_POLLS = 30 → 60 s 一筆。8 小時工作階段 = 480 筆 ≈ 216 KB（每筆約
// 450 B）；log 保留 14 天、總量上限 100 MB，即使天天如此也只佔約 3%。
//
// NOTABLE_MIN_POLLS = 5 → 有異常時最快 10 s 一筆，上限 360 筆/小時 ≈ 1.3 MB/天。
// 沒有這個下限的話，異常持續期間會退化成每 2 s 一筆（1800 筆/小時），把 heartbeat
// 的成本估算整個推翻——這是 #39「閾值組合未驗算」的同一類陷阱。
//
// 「異常」的 lag 門檻是 NOTABLE_LAG_MS（見 stallTelemetry.ts）——**不能是 0**，
// 否則 notable 恆為真、heartbeat 實際退化成 10 s（8 小時 2880 筆 ≈ 1.3 MB、
// 14 天約 18 MB），而且「異常」這個標記完全喪失鑑別力。
//
// 交叉檢查：stallTelemetry 的 lag ring 覆蓋 LAG_RING_SIZE × LAG_INTERVAL_MS
// = 64 s，必須 ≥ HEARTBEAT_POLLS × POLL_INTERVAL_MS = 60 s，否則樣本會在被回報
// 之前就被擠出環形緩衝。下面的常數斷言與 perfBridge 的測試各釘一次。
const HEARTBEAT_POLLS = 30
const NOTABLE_MIN_POLLS = 5

export const REPORT_WINDOW_MS = HEARTBEAT_POLLS * POLL_INTERVAL_MS
export const LAG_WINDOW_MS = LAG_RING_SIZE * LAG_INTERVAL_MS

function errorMessage(error: unknown): string {
    return error instanceof Error ? error.message : String(error)
}

// Polls the app + its managed child processes' cpu/memory every 2s and feeds the
// StatusBar chip. Skips a round while the window is unfocused so background
// polling stays quiet (same throttle as GitBridge's remote check) — but the skip
// is now *counted* rather than invisible, so "沒資料" 與 "採樣失敗" 可以分辨。
export function PerfBridge() {
    useEffect(() => {
        const telemetry = createStallTelemetry()
        telemetry.start()

        let pollsSinceReport = 0
        // 落盤用的增量計數。**不能**改用 perfStore 的 samplingHealth()：那是 30 筆
        // 滾動視窗的當下組成，與最短 5 筆的回報間隔重疊，跨報表累加會把同一次
        // 失敗最多計 6 次（Rust 端 `absorb_diagnostics` 對 count 類做 saturating_add）。
        let delta: SamplingDelta = { attempts: 0, failures: 0, empty: 0, skippedNoFocus: 0 }
        // terminal 的 droppedBytes／flushCount 是**每個 queue 各自的累計量**，而
        // `terminalOutputMetricsSnapshot()` 只看得到活著的 session（#39 的 registry
        // 在 unregister 時移除）。直接加總會在 session 關閉時倒退，讓 Rust 端的
        // 「取最大值」低報。改為記錄每個 session id 的高水位，總和因此單調遞增。
        const terminalHighWater = new Map<string, { dropped: number; flushes: number }>()

        const countOutcome = (kind: keyof SamplingDelta) => {
            delta.attempts += 1
            if (kind !== "attempts") delta[kind] += 1
        }

        const report = () => {
            pollsSinceReport = 0
            const store = usePerfStore.getState()
            const stall = telemetry.drain()
            store.setStall(stall)
            const snapshot = store.snapshot
            const terminals = Object.entries(terminalOutputMetricsSnapshot())
            for (const [sessionId, metrics] of terminals) {
                const previous = terminalHighWater.get(sessionId)
                terminalHighWater.set(sessionId, {
                    dropped: Math.max(previous?.dropped ?? 0, metrics.droppedBytes),
                    flushes: Math.max(previous?.flushes ?? 0, metrics.flushCount)
                })
            }
            const liveSum = (pick: (m: (typeof terminals)[number][1]) => number) =>
                terminals.reduce((total, [, metrics]) => total + pick(metrics), 0)
            const cumulative = (pick: (v: { dropped: number; flushes: number }) => number) =>
                [...terminalHighWater.values()].reduce((total, value) => total + pick(value), 0)
            // metadata 的 key 名是與 Rust `run_summary::absorb_diagnostics` 的契約。
            void logDiagnosticsSample("renderer diagnostics sample", {
                long_task_supported: stall.longTaskSupported,
                long_task_count: stall.longTaskCount,
                long_task_total_ms: stall.longTaskTotalMs,
                long_task_max_ms: stall.longTaskMaxMs,
                event_loop_lag_samples: stall.eventLoopLagSamples,
                event_loop_lag_max_ms: stall.eventLoopLagMaxMs,
                event_loop_lag_mean_ms: stall.eventLoopLagMeanMs,
                perf_cpu_percent: snapshot?.cpuPercent ?? 0,
                perf_memory_bytes: snapshot?.memoryBytes ?? 0,
                perf_webview_memory_bytes: snapshot?.webviewMemoryBytes ?? 0,
                perf_webview_count: snapshot?.webviewCount ?? 0,
                perf_managed_tools_memory_bytes: snapshot?.managedToolsMemoryBytes ?? 0,
                perf_descendant_count: snapshot?.descendantCount ?? 0,
                // 全部是「距上次回報以來」的增量，Rust 端累加後等於實際次數。
                sampling_attempts: delta.attempts,
                sampling_failures: delta.failures,
                sampling_empty: delta.empty,
                sampling_skipped_no_focus: delta.skippedNoFocus,
                // #39 已對齊成 UTF-8 bytes 的 terminal queue metrics，直接沿用。
                // pending/hidden 是瞬時 gauge（只看活著的 session），dropped/flush
                // 是單調累計（含已關閉的 session）。
                terminal_sessions: terminals.length,
                terminal_pending_bytes: liveSum((m) => m.pendingBytes),
                terminal_hidden_bytes: liveSum((m) => m.hiddenBytes),
                terminal_dropped_bytes: cumulative((v) => v.dropped),
                terminal_flush_count: cumulative((v) => v.flushes),
                terminal_last_flush_latency_ms: terminals.reduce(
                    (max, [, m]) => Math.max(max, m.lastFlushLatencyMs),
                    0
                )
            })
            delta = { attempts: 0, failures: 0, empty: 0, skippedNoFocus: 0 }
        }

        const maybeReport = () => {
            pollsSinceReport += 1
            if (
                pollsSinceReport >= HEARTBEAT_POLLS ||
                (isNotableSample(telemetry.snapshot(), delta) &&
                    pollsSinceReport >= NOTABLE_MIN_POLLS)
            ) {
                report()
            }
        }

        const poll = () => {
            if (!document.hasFocus()) {
                usePerfStore.getState().recordOutcome("skipped_no_focus")
                countOutcome("skippedNoFocus")
                maybeReport()
                return
            }
            void perfSnapshot()
                .then((snapshot) => {
                    const store = usePerfStore.getState()
                    if (snapshot === null) {
                        // `Ok(None)`：後端有回應但沒有資料（aggregate_tree 找不到自身
                        // pid）。**不覆寫**最後一次成功的快照——與失敗路徑一致，
                        // 讓 chip 保留可讀的數值並掛上警示，而不是無聲消失。
                        store.recordOutcome("empty")
                        countOutcome("empty")
                        return
                    }
                    store.setSnapshot(snapshot)
                    store.recordOutcome("ok")
                    countOutcome("attempts")
                })
                // §3.5：原本是 `.catch(() => {})`，失敗完全看不見。改成落進有界的
                // 診斷狀態，StatusBar 會顯示，週期性 report 也會把它寫進 log。
                .catch((error: unknown) => {
                    usePerfStore.getState().recordOutcome("failed", errorMessage(error))
                    countOutcome("failures")
                })
                .finally(() => {
                    maybeReport()
                })
        }

        // **已知限制（latent，本次刻意不處理）**：量測工具與被量測對象在同一條
        // event loop 上。hang 期間這個 interval 不會跑、也就不落盤；恢復後才會記
        // 下一筆等於 hang 全長的 lag——這部分是可用的。但使用者遇到 AppHangB1 的
        // 典型反應是**強制結束**，那個視窗的 `diagnostics.sample` 就從未寫出，
        // 整條前端 stall 遙測對「hang 到被 kill」這個最重要的情境是空的。
        // 要涵蓋它需要把量測搬到 renderer 之外（Rust 側的 watchdog），超出 #40。
        const id = setInterval(poll, POLL_INTERVAL_MS)
        return () => {
            clearInterval(id)
            telemetry.stop()
        }
    }, [])

    return null
}
