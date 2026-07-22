import { isTauri } from "@tauri-apps/api/core"
import { resolveResource } from "@tauri-apps/api/path"

// Re-exported so the rest of the app detects the Tauri shell through this module
// instead of importing `@tauri-apps/api/core` directly (kept to ipc.ts + here).
export { isTauri }

/**
 * True only inside the Tauri shell on macOS — the sole case where the native
 * traffic lights overlay the UI (titleBarStyle: Overlay) and the rail must
 * reserve space for them. Windows/Linux keep their native title bar.
 */
export function showsNativeTrafficLights(): boolean {
    return isTauri() && /Mac/.test(navigator.userAgent)
}

/**
 * 內建 pi adapter（yuzora-pi-acp，隨 app 打包於 resources）的 spawn 指令。
 * P5 的 pi runtime 選擇器據此提供 builtin 選項；bundle 外（dev server、測試）
 * 或 resource 缺失時回 null，呼叫端退回 community runtime。
 */
export async function builtinPiAdapterCommand(): Promise<string | null> {
    if (!isTauri()) return null
    try {
        const path = await resolveResource("adapters/yuzora-pi-acp/index.mjs")
        return path ? `node "${path}"` : null
    } catch {
        return null
    }
}

// resolveResource 是 async，但 command 路由（settingsStorage.resolveAgentCommandRoute）
// 是同步呼叫鏈——啟動時（AgentBridge 的 startup effect，先於 prewarm）resolve 一次
// 進 cache，之後同步讀。null＝尚未 init／非 Tauri／resource 缺失 → 退回 community。
let builtinPiAdapterCommandCache: string | null = null

export async function initBuiltinPiAdapterCommand(): Promise<void> {
    builtinPiAdapterCommandCache = await builtinPiAdapterCommand()
}

export function cachedBuiltinPiAdapterCommand(): string | null {
    return builtinPiAdapterCommandCache
}

/** 測試替身；生產程式碼不呼叫。 */
export function setCachedBuiltinPiAdapterCommandForTests(command: string | null): void {
    builtinPiAdapterCommandCache = command
}
