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

const settle = () => new Promise((r) => setTimeout(r, 0))

it("send forwards to lspSend with the exact same workspace/language string", async () => {
    const h = createTauriTransport("/ws", "typescript")
    h.transport.send('{"jsonrpc":"2.0"}')
    await settle()
    expect(lspSend).toHaveBeenCalledWith("/ws", "typescript", '{"jsonrpc":"2.0"}')
})

it("send waits for lspStart to settle so the first message cannot overtake server startup", async () => {
    let resolveStart: (v: unknown) => void = () => {}
    lspStart.mockReturnValue(new Promise((r) => (resolveStart = r)))
    const h = createTauriTransport("/ws", "typescript")
    h.transport.send('{"method":"initialize"}')
    await settle()
    expect(lspSend).not.toHaveBeenCalled()
    resolveStart({ language: "typescript" })
    await settle()
    expect(lspSend).toHaveBeenCalledWith("/ws", "typescript", '{"method":"initialize"}')
})

it("serializes concurrent sends: a later message never reaches lspSend before the earlier one settles (#56)", async () => {
    const resolvers: Array<() => void> = []
    lspSend.mockImplementation(() => new Promise<void>((r) => resolvers.push(() => r())))
    const h = createTauriTransport("/ws", "typescript")
    h.transport.send('{"v":3}')
    h.transport.send('{"v":4}')
    await settle()
    // v4 must not be invoked while v3 is still in flight.
    expect(lspSend).toHaveBeenCalledTimes(1)
    expect(lspSend).toHaveBeenNthCalledWith(1, "/ws", "typescript", '{"v":3}')
    resolvers[0]()
    await settle()
    expect(lspSend).toHaveBeenCalledTimes(2)
    expect(lspSend).toHaveBeenNthCalledWith(2, "/ws", "typescript", '{"v":4}')
})

it("a rejected send does not stall the chain — later messages still go out", async () => {
    lspSend.mockRejectedValueOnce(new Error("pipe full")).mockResolvedValue(undefined)
    const h = createTauriTransport("/ws", "typescript")
    h.transport.send('{"v":1}')
    h.transport.send('{"v":2}')
    await settle()
    await settle()
    expect(lspSend).toHaveBeenCalledTimes(2)
    expect(lspSend).toHaveBeenNthCalledWith(2, "/ws", "typescript", '{"v":2}')
})

it("sends still go out (and stay ordered) when lspStart rejected", async () => {
    lspStart.mockRejectedValue(new Error("no adapter"))
    const h = createTauriTransport("/ws", "typescript")
    h.transport.send('{"v":1}')
    await settle()
    expect(lspSend).toHaveBeenCalledWith("/ws", "typescript", '{"v":1}')
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
