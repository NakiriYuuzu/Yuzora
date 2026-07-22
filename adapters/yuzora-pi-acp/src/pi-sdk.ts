// pi SDK 解析（SDK 供應決策＝resolve 使用者的 global pi 安裝；記錄於 spec P2）：
// 與 TUI 同一份 pi——session 檔格式、extension 相容性、provider 設定天然一致；
// 版本 drift 以 initialize 版本回報＋contract fixtures 把關，不 bundle 進 dist。
// 解析順序：YUZORA_PI_SDK env（測試／特殊安裝）→ PATH 上的 pi 執行檔 realpath
// 反推 package root（<root>/dist/cli.js ← bin symlink）。
import { realpathSync, existsSync } from "node:fs"
import { delimiter, dirname, join } from "node:path"
import { pathToFileURL } from "node:url"

/** 本 adapter 驗證過的 pi 版本前綴；之外的版本照常啟動但回報 warning。 */
export const TESTED_PI_VERSION_PREFIX = "0.80."

export interface PiSdk {
    root: string
    version: string
    // 只用 root exports（不 deep-import dist 內部）；型別以最小手寫介面表達，
    // 與確切版本解耦——真正的相容性由 contract fixtures 驗。
    createAgentSession: (options: Record<string, unknown>) => Promise<{
        session: PiAgentSession
        extensionsResult: unknown
        modelFallbackMessage?: string
    }>
    SessionManager: {
        create: (cwd: string) => unknown
        open: (path: string, sessionDir?: string, cwdOverride?: string) => unknown
        list: (cwd: string) => Promise<{ id: string; path: string; cwd: string }[]>
    }
    initTheme?: (...args: unknown[]) => unknown
}

export interface PiAgentSession {
    sessionId: string
    sessionFile: string | undefined
    model: { provider: string; id: string; name?: string } | undefined
    thinkingLevel: string
    isStreaming: boolean
    messages: unknown[]
    promptTemplates: readonly { name: string; description?: string }[]
    modelRuntime: { getAvailable: () => Promise<{ provider: string; id: string; name?: string }[]> }
    resourceLoader: { getSkills: () => { skills: { name: string; description?: string }[] } }
    extensionRunner: { getRegisteredCommands: () => { invocationName: string; description?: string }[] }
    prompt: (text: string, options?: Record<string, unknown>) => Promise<void>
    abort: () => Promise<void>
    setModel: (model: unknown) => Promise<void>
    setThinkingLevel: (level: string) => void
    getAvailableThinkingLevels: () => string[]
    subscribe: (listener: (event: Record<string, unknown>) => void) => () => void
    bindExtensions: (bindings: Record<string, unknown>) => Promise<void>
    dispose: () => void
    // P5 soak 增補（context/pricing、內建 slash commands、steering）：
    getSessionStats: () => {
        cost: number
        tokens: { input: number; output: number; cacheRead: number; cacheWrite: number; total: number }
        totalMessages: number
        userMessages: number
        assistantMessages: number
        toolCalls: number
        sessionFile: string | undefined
        sessionId: string
        contextUsage?: { tokens: number | null; contextWindow: number; percent: number | null }
    }
    getContextUsage: () => { tokens: number | null; contextWindow: number; percent: number | null } | undefined
    compact: (customInstructions?: string) => Promise<{ summary: string; tokensBefore: number }>
    setSessionName: (name: string) => void
    steeringMode: "all" | "one-at-a-time"
    followUpMode: "all" | "one-at-a-time"
    setSteeringMode: (mode: "all" | "one-at-a-time") => void
    setFollowUpMode: (mode: "all" | "one-at-a-time") => void
    setAutoCompactionEnabled: (enabled: boolean) => void
    autoCompactionEnabled: boolean
    exportToHtml: (outputPath?: string) => Promise<string>
}

function findPiOnPath(): string | undefined {
    const pathValue = process.env.PATH ?? ""
    for (const dir of pathValue.split(delimiter)) {
        if (!dir) continue
        const candidate = join(dir, "pi")
        if (existsSync(candidate)) return candidate
    }
    return undefined
}

export function resolvePiPackageRoot(): string {
    const override = process.env.YUZORA_PI_SDK
    if (override) return override
    const bin = findPiOnPath()
    if (!bin) {
        throw new Error(
            "pi executable not found on PATH. Install pi first: npm install -g @earendil-works/pi-coding-agent"
        )
    }
    // bin symlink → <root>/dist/cli.js
    const real = realpathSync(bin)
    const root = dirname(dirname(real))
    if (!existsSync(join(root, "dist", "index.js"))) {
        throw new Error(`resolved pi package root has no dist/index.js: ${root} (from ${bin})`)
    }
    return root
}

export async function loadPiSdk(): Promise<PiSdk> {
    const root = resolvePiPackageRoot()
    const moduleUrl = pathToFileURL(join(root, "dist", "index.js")).href
    const sdk = await import(moduleUrl) as Record<string, unknown> & Omit<PiSdk, "root" | "version">
    const version = typeof sdk.VERSION === "string" ? sdk.VERSION : "unknown"
    if (typeof sdk.createAgentSession !== "function" || !sdk.SessionManager) {
        throw new Error(`pi SDK at ${root} does not export createAgentSession/SessionManager (version ${version})`)
    }
    return {
        root,
        version,
        createAgentSession: sdk.createAgentSession as PiSdk["createAgentSession"],
        SessionManager: sdk.SessionManager as PiSdk["SessionManager"],
        ...(typeof sdk.initTheme === "function" ? { initTheme: sdk.initTheme as PiSdk["initTheme"] } : {})
    }
}
