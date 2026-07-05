// Shared logging helpers. Every LSP protocol message that crosses the
// WebSocket is printed to the browser console with a greppable prefix so
// Playwright can capture it as evidence (see brief: "證據以 console log +
// network/WS 訊息紀錄為準").

export function logOut(method: string | undefined, payload: unknown) {
    console.log('[LSP>]', method ?? '(?)', JSON.stringify(payload))
}

export function logIn(method: string | undefined, payload: unknown) {
    console.log('[LSP<]', method ?? '(?)', JSON.stringify(payload))
}

// Expose test objects on window so Playwright can drive/inspect them
// programmatically (lead's note: hover/completion 可用程式化事件觸發).
export function expose(key: string, value: unknown) {
    const w = window as unknown as { __spike?: Record<string, unknown> }
    w.__spike = w.__spike ?? {}
    w.__spike[key] = value
}
