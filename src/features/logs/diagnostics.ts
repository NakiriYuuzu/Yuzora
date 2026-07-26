import { invoke } from "@/lib/ipc"

// issue #40：把 renderer 端的診斷指標寫進同一份 daily JSONL，讓它們自動帶上
// `run_id`（由 Rust 的 LogSink 統一補上）並能與 Rust 端的事件同軸比對。
//
// `run-summary.json`（Rust `run_summary.rs`）會把這些 record 摺成每個 run 的
// long-task／lag／perf 峰值。**metadata 的 key 名是兩側的契約**——Rust 側常數
// 見 `run_summary::DIAGNOSTICS_SAMPLE_EVENT` 與 `absorb_diagnostics`，
// `diagnostics.test.ts` 會逐一比對 key 名，改名時兩邊要一起改。
export const DIAGNOSTICS_SAMPLE_EVENT = "diagnostics.sample"

// kind=debug（診斷資料，不是使用者動作），level=info（必須在預設門檻下落盤，
// 否則 run-summary 平時永遠是空的）。
export function logDiagnosticsSample(
    message: string,
    metadata: Record<string, number | boolean>
): Promise<void> {
    return invoke("log_event", {
        event: {
            level: "info",
            kind: "debug",
            source: "diagnostics",
            workspace_path: null,
            event: DIAGNOSTICS_SAMPLE_EVENT,
            message,
            metadata
        }
    })
        .then(() => undefined)
        .catch(() => undefined)
}
