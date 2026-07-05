import { it, expect, vi, beforeEach } from "vitest"

const lspStart = vi.fn()
const lspSend = vi.fn()

vi.mock("../lib/ipc", () => ({
    lspStart: (...a: unknown[]) => lspStart(...a),
    lspSend: (...a: unknown[]) => lspSend(...a)
}))

import { createTauriTransport } from "./transport"

beforeEach(() => {
    lspStart.mockReset().mockResolvedValue({ language: "typescript" })
    lspSend.mockReset().mockResolvedValue(undefined)
})

it("constructing calls lspStart with the workspace, language and a dispatch fn", () => {
    createTauriTransport("/ws", "typescript")
    expect(lspStart).toHaveBeenCalledTimes(1)
    const [ws, lang, cb] = lspStart.mock.calls[0]
    expect(ws).toBe("/ws")
    expect(lang).toBe("typescript")
    expect(typeof cb).toBe("function")
})

it("send forwards to lspSend with the exact same workspace/language string", () => {
    const h = createTauriTransport("/ws", "typescript")
    h.transport.send('{"jsonrpc":"2.0"}')
    expect(lspSend).toHaveBeenCalledWith("/ws", "typescript", '{"jsonrpc":"2.0"}')
})

it("dispatches incoming messages to subscribed handlers, and stops after unsubscribe", () => {
    const h = createTauriTransport("/ws", "typescript")
    const handler = vi.fn()
    h.transport.subscribe(handler)
    const dispatch = lspStart.mock.calls[0][2] as (m: string) => void
    dispatch('{"id":1}')
    expect(handler).toHaveBeenCalledWith('{"id":1}')
    h.transport.unsubscribe(handler)
    dispatch('{"id":2}')
    expect(handler).toHaveBeenCalledTimes(1)
})

it("send swallows an lspSend rejection without producing an unhandled rejection (W7)", async () => {
    const proc = (globalThis as unknown as {
        process: {
            on(ev: string, cb: (e: unknown) => void): void
            off(ev: string, cb: (e: unknown) => void): void
        }
    }).process
    lspSend.mockRejectedValue(new Error("server gone"))
    const rejections: unknown[] = []
    const onUnhandled = (e: unknown) => rejections.push(e)
    proc.on("unhandledRejection", onUnhandled)

    const h = createTauriTransport("/ws", "typescript")
    h.transport.send("{}")
    // let the rejected promise settle and any unhandled-rejection fire
    await new Promise((r) => setTimeout(r, 10))

    proc.off("unhandledRejection", onUnhandled)
    expect(rejections).toEqual([])
})

it("info resolves to the LspServerInfo returned by lspStart", async () => {
    const info = { language: "typescript", serverId: "ts" }
    lspStart.mockResolvedValue(info)
    const h = createTauriTransport("/ws", "typescript")
    await expect(h.info).resolves.toBe(info)
})
