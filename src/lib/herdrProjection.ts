import { LOCAL_HOST_ID } from "./runtimeIdentity"

/** Stable across reconnects; a display name or a connection generation is not an owner. */
export function herdrProjectionKey(scope: string, tabId?: string | null, terminalId?: string | null): string | null {
    const identity = tabId?.trim() || terminalId?.trim()
    if (!identity) return null
    const runtime: [string, string] = scope.startsWith("[") ? JSON.parse(scope) : [LOCAL_HOST_ID, scope]
    return JSON.stringify([runtime[0], runtime[1], tabId?.trim() ? "tab" : "terminal", identity])
}

export function herdrProjectionDismissed(dismissed: Record<string, true>, scope: string, tabId?: string | null, terminalId?: string | null): boolean {
    const primary = herdrProjectionKey(scope, tabId, terminalId)
    const legacy = herdrProjectionKey(scope, null, terminalId)
    return Boolean((primary && dismissed[primary]) || (legacy && dismissed[legacy]))
}
