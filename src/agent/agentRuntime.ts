import {
    agentPresetForCommand,
    resolveRuntimeCommand,
    type AgentRuntimeAvailability,
} from "@/lib/agentPresets"
import { agentDetectRuntimes } from "@/lib/ipc"
import { cachedBuiltinPiAdapterCommand } from "@/lib/platform"

// #15：Windows 只裝 Node（無 Bun）時 curated preset 的 `bunx ...` 必失敗於
// Rust preflight。spawn 前先偵測 runtime，bun 缺席但 npx 可用就改寫為
// `npx -y ...`；沒有可用 runtime 時以可行動訊息提前失敗（與 Rust preflight 錯誤
// 同風格，走同一條 spawn 錯誤顯示路徑），並同時登記 prerequisite 狀態供
// AgentZone 顯示三個 runtime 的偵測結果。
export const NO_AGENT_RUNTIME_MESSAGE =
    "No JavaScript runtime for the agent command was found on the app PATH " +
    "(Bun, Node.js/npx and Deno were all probed; Deno is not yet supported for " +
    "ACP adapters): install Bun (bun.sh) or Node.js (nodejs.org, provides npx), " +
    "or customize the agent command in Settings"

// 只有 Deno 的機器：訊息必須說清楚「偵測到了但還不支援」，否則使用者會以為
// Yuzora 沒看到他已經裝好的 runtime。
export const DENO_UNSUPPORTED_RUNTIME_MESSAGE =
    "Deno was found on the app PATH, but Yuzora has not verified the ACP adapters " +
    "against Deno yet: install Bun (bun.sh) or Node.js (nodejs.org, provides npx), " +
    "or customize the agent command in Settings"

// 內建 pi adapter 是 `node "<bundled path>"`：Rust preflight 對含引號的指令
// fail open，缺 node 時不會被擋下，只會靜默 hang 到 initialize timeout。
export const BUILTIN_NODE_MISSING_MESSAGE =
    "The bundled pi adapter needs Node.js, which was not found on the app PATH: " +
    "install Node.js (nodejs.org), or switch the pi runtime to the community " +
    "adapter in Settings"

export type AgentRuntimePrerequisiteReason =
    | "no-runtime"
    | "deno-unsupported"
    | "builtin-node-missing"

export interface AgentRuntimePrerequisite {
    reason: AgentRuntimePrerequisiteReason
    /** spawn 當下的實際偵測結果——UI 逐一列出三個 runtime，不猜。 */
    runtimes: AgentRuntimeAvailability
    /** 被擋下的 spawn 指令（診斷用；curated／builtin 才會走到這裡）。 */
    command: string
}

// 連線層的全域單例狀態（與 acpConnection 的冷啟動註冊表同模式）：prerequisite
// 不屬於任何一個 session，spawn 前就要能顯示，因此不走 agentStore。
let prerequisiteSnapshot: AgentRuntimePrerequisite | null = null
const prerequisiteListeners = new Set<() => void>()

function publishPrerequisite(next: AgentRuntimePrerequisite | null): void {
    prerequisiteSnapshot = next
    for (const listener of [...prerequisiteListeners]) listener()
}

export function subscribeAgentRuntimePrerequisite(listener: () => void): () => void {
    prerequisiteListeners.add(listener)
    return () => {
        prerequisiteListeners.delete(listener)
    }
}

/** useSyncExternalStore 相容：同一批狀態回傳同一個物件實體（無狀態時為 null）。 */
export function agentRuntimePrerequisite(): AgentRuntimePrerequisite | null {
    return prerequisiteSnapshot
}

export function clearAgentRuntimePrerequisite(): void {
    if (prerequisiteSnapshot) publishPrerequisite(null)
}

// prerequisite 失敗仍走既有的 spawn 錯誤通道（agentStore.connectionError／startup
// timing log）；型別上可辨識，讓 router 把它排除在指數 backoff 之外（見
// isAgentRuntimePrerequisiteError）。
export class AgentRuntimePrerequisiteError extends Error {
    readonly prerequisite: AgentRuntimePrerequisite

    constructor(message: string, prerequisite: AgentRuntimePrerequisite) {
        super(message)
        this.name = "AgentRuntimePrerequisiteError"
        this.prerequisite = prerequisite
    }
}

/**
 * 缺 runtime 是「使用者幾秒內就能修好的外部條件」，不是 agent 掛掉——比照
 * cold-start-cancelled 豁免 router 的重試退讓，否則按幾次 Retry 就被罰站到 30 秒，
 * 而被 disable 的正是 prerequisite banner 自己的 Retry 鈕。跨模組實體可能不同
 * （測試替身、重複載入），因此比照 isAgentAuthRequiredError 併用 name 比對。
 */
export function isAgentRuntimePrerequisiteError(error: unknown): boolean {
    return error instanceof AgentRuntimePrerequisiteError
        || (error instanceof Error && error.name === "AgentRuntimePrerequisiteError")
}

// 登記 prerequisite（UI 用）後回傳要 throw 的錯誤。
function failWithPrerequisite(
    message: string,
    prerequisite: AgentRuntimePrerequisite,
): Error {
    publishPrerequisite(prerequisite)
    return new AgentRuntimePrerequisiteError(message, prerequisite)
}

/**
 * 內建 pi adapter 的指令（`node "<bundled path>"`）比對。preset 判定會把它算成
 * "custom"（它不等於任何 latestCommand），但它其實是 curated 路徑，仍需 runtime
 * 檢查——以「與 cache 完全相同」為條件，使用者自己寫的 `node ...` custom 指令
 * 不會被誤判。
 */
function isBuiltinAdapterCommand(command: string): boolean {
    const builtin = cachedBuiltinPiAdapterCommand()
    return builtin !== null && command.trim() === builtin.trim()
}

/**
 * 每一條「沒有登記 prerequisite」的結束路徑都必須清掉殘留狀態，否則舊的
 * prerequisite banner 會蓋掉之後真正的連線錯誤（例：curated 缺 runtime 失敗後，
 * 使用者改成打錯字的 custom 指令，Rust preflight 的 `'foo-acp' was not found`
 * 會被舊 banner 蓋掉，把人導去裝 Node.js）。因此 clear 收斂成這裡唯一一處：
 * 內層新增任何 early return 都自動被涵蓋，忘不掉。
 */
export async function resolveAgentSpawnCommand(command: string): Promise<string> {
    const resolved = await resolveSpawnCommand(command)
    clearAgentRuntimePrerequisite()
    return resolved
}

async function resolveSpawnCommand(command: string): Promise<string> {
    const builtin = isBuiltinAdapterCommand(command)
    // custom command 一律原樣穿透（fingerprint／trusted replay 不受影響）
    if (!builtin && agentPresetForCommand(command) === "custom") return command
    let runtimes
    try {
        runtimes = await agentDetectRuntimes()
    } catch {
        // 偵測不可用（舊 backend、測試替身）→ 維持既有行為交給 Rust preflight
        return command
    }
    if (!runtimes || typeof runtimes.bunx !== "boolean") return command
    if (builtin) {
        if (!runtimes.node) {
            throw failWithPrerequisite(BUILTIN_NODE_MISSING_MESSAGE, {
                reason: "builtin-node-missing",
                runtimes,
                command,
            })
        }
        return command
    }
    const resolution = resolveRuntimeCommand(command, runtimes)
    if (resolution.kind === "unsupported-runtime") {
        throw failWithPrerequisite(DENO_UNSUPPORTED_RUNTIME_MESSAGE, {
            reason: "deno-unsupported",
            runtimes,
            command,
        })
    }
    if (resolution.kind === "unavailable") {
        throw failWithPrerequisite(NO_AGENT_RUNTIME_MESSAGE, {
            reason: "no-runtime",
            runtimes,
            command,
        })
    }
    return resolution.command
}
