import { agentPresetForCommand, resolveRuntimeCommand } from "@/lib/agentPresets"
import { agentDetectRuntimes } from "@/lib/ipc"

// #15：Windows 只裝 Node（無 Bun）時 curated preset 的 `bunx ...` 必失敗於
// Rust preflight。spawn 前先偵測 runtime，bun 缺席但 npx 可用就改寫為
// `npx -y ...`；兩者皆無則以可行動訊息提前失敗（與 Rust preflight 錯誤同風格，
// 走同一條 spawn 錯誤顯示路徑）。
export const NO_AGENT_RUNTIME_MESSAGE =
    "No JavaScript runtime for the agent command was found on the app PATH: " +
    "install Bun (bun.sh) or Node.js (nodejs.org, provides npx), " +
    "or customize the agent command in Settings"

export async function resolveAgentSpawnCommand(command: string): Promise<string> {
    // custom command 一律原樣穿透（fingerprint／trusted replay 不受影響）
    if (agentPresetForCommand(command) === "custom") return command
    let runtimes
    try {
        runtimes = await agentDetectRuntimes()
    } catch {
        // 偵測不可用（舊 backend、測試替身）→ 維持既有行為交給 Rust preflight
        return command
    }
    if (!runtimes || typeof runtimes.bunx !== "boolean") return command
    const resolution = resolveRuntimeCommand(command, runtimes)
    if (resolution.kind === "unavailable") throw new Error(NO_AGENT_RUNTIME_MESSAGE)
    return resolution.command
}
