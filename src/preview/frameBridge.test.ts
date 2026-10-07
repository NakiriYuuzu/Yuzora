import { expect, it, vi } from "vitest"
import script from "../../src-tauri/src/preview_interaction.js?raw"

const binding = { id: "nextTab", key: "TAB", ctrl: true, meta: false, alt: false, shift: false }
type Frame = { closed: boolean; postMessage: (data: unknown, origin: string) => void }

function bridge() {
    let message!: (event: { source: unknown; data: unknown }) => void
    const windowPort: Record<string, unknown> = {
        addEventListener: vi.fn((type: string, listener: typeof message) => {
            expect(type).toBe("message")
            message = listener
        }),
    }
    windowPort.top = windowPort
    const documentPort = { addEventListener: vi.fn() }
    new Function("window", "document", script)(windowPort, documentPort)
    const api = windowPort.__yuzoraBrowser as {
        poll(bindings: unknown[]): { commands: string[]; selection: null; selecting: boolean }
    }
    return {
        api,
        documentPort,
        windowPort,
        ready: (source: unknown) => message({ source, data: { type: "yuzora-tab-ready" } }),
        command: (source: unknown, id = "nextTab") => message({ source, data: { type: "yuzora-tab-command", id } }),
    }
}

function frame(): Frame {
    return { closed: false, postMessage: vi.fn() }
}

it("delivers current and empty bindings to live frames without duplicating ready entries or key listeners", () => {
    const b = bridge(), a = frame(), sibling = frame()
    b.ready(a)
    b.ready(a)
    b.ready(sibling)
    expect(b.api.poll([binding])).toEqual({ commands: [], selection: null, selecting: false })
    expect(a.postMessage).toHaveBeenCalledExactlyOnceWith({ type: "yuzora-tab-bindings", bindings: [binding] }, "*")
    b.api.poll([])
    expect(a.postMessage).toHaveBeenLastCalledWith({ type: "yuzora-tab-bindings", bindings: [] }, "*")
    expect(a.postMessage).toHaveBeenCalledTimes(2)
    expect(sibling.postMessage).toHaveBeenCalledTimes(2)
    expect(b.documentPort.addEventListener).toHaveBeenCalledTimes(1)
    expect(b.windowPort.addEventListener).toHaveBeenCalledTimes(1)
})

it("stops sending to closed frames whose postMessage does not throw and keeps live siblings", () => {
    const b = bridge(), closed = frame(), live = frame()
    b.ready(closed)
    b.ready(live)
    b.api.poll([binding])
    closed.closed = true
    for (let i = 0; i < 100; i++) b.api.poll([binding])
    expect(closed.postMessage).toHaveBeenCalledTimes(1)
    expect(live.postMessage).toHaveBeenCalledTimes(101)
    b.command(closed)
    b.command(live)
    expect(b.api.poll([binding]).commands).toEqual(["nextTab"])
})

it("reclaims a full registry before a replacement ready message even without an intervening poll", () => {
    const b = bridge(), old = Array.from({ length: 64 }, frame)
    old.forEach(b.ready)
    b.api.poll([binding])
    old.forEach(item => { item.closed = true })
    const next = frame()
    b.ready(next)
    b.command(next)
    expect(b.api.poll([binding]).commands).toEqual(["nextTab"])
    expect(next.postMessage).toHaveBeenCalledTimes(1)
    old.forEach(item => expect(item.postMessage).toHaveBeenCalledTimes(1))
})

it("retains the 64 live frame limit, accepts duplicates, and rejects unregistered commands", () => {
    const b = bridge(), live = Array.from({ length: 64 }, frame), overflow = frame()
    live.forEach(b.ready)
    b.ready(live[0])
    b.ready(overflow)
    b.api.poll([binding])
    b.command(overflow)
    b.command(live[0])
    expect(b.api.poll([binding]).commands).toEqual(["nextTab"])
    expect(overflow.postMessage).not.toHaveBeenCalled()
    live.forEach(item => expect(item.postMessage).toHaveBeenCalledTimes(2))
})

it("preserves queued commands across frame closure and the configured-command and eight-command limits", () => {
    const b = bridge(), child = frame()
    b.ready(child)
    b.api.poll([binding])
    b.command(child, "unconfigured")
    for (let i = 0; i < 12; i++) b.command(child)
    child.closed = true
    expect(b.api.poll([binding]).commands).toEqual(Array(8).fill("nextTab"))
    expect(b.api.poll([binding]).commands).toEqual([])
})

it("forgets inaccessible or throwing frames while allowing later live registrations", () => {
    const b = bridge(), unreadable = frame(), throwing = frame(), live = frame()
    b.ready(unreadable)
    Object.defineProperty(unreadable, "closed", { get() { throw new Error("unavailable") } })
    throwing.postMessage = vi.fn(() => { throw new Error("gone") })
    b.ready(throwing)
    b.ready(live)
    expect(() => b.api.poll([binding])).not.toThrow()
    b.api.poll([binding])
    expect(unreadable.postMessage).not.toHaveBeenCalled()
    expect(throwing.postMessage).toHaveBeenCalledTimes(1)
    expect(live.postMessage).toHaveBeenCalledTimes(2)
    expect(() => b.ready(unreadable)).not.toThrow()
    const late = frame()
    late.closed = true
    b.ready(late)
    b.api.poll([binding])
    expect(late.postMessage).not.toHaveBeenCalled()
})

it("continues admitting new frames through repeated closure cycles in one page", () => {
    const b = bridge()
    b.api.poll([binding])
    for (let cycle = 0; cycle < 120; cycle++) {
        const children = Array.from({ length: 8 }, frame)
        children.forEach(b.ready)
        b.command(children[0])
        expect(b.api.poll([binding]).commands).toEqual(["nextTab"])
        children.forEach(item => { item.closed = true })
        b.api.poll([binding])
        children.forEach(item => expect(item.postMessage).toHaveBeenCalledTimes(1))
    }
    expect(b.documentPort.addEventListener).toHaveBeenCalledTimes(1)
})

it("keeps the order and command membership of interleaved survivors when compacting", () => {
    const b = bridge(), children = Array.from({ length: 64 }, frame)
    children.forEach(b.ready)
    b.api.poll([binding])
    children.forEach((child, index) => { child.closed = index % 2 === 0 })
    b.api.poll([binding])
    b.command(children[0])
    b.command(children[63])
    expect(b.api.poll([binding]).commands).toEqual(["nextTab"])
    const replacements = Array.from({ length: 32 }, frame)
    replacements.forEach(b.ready)
    const overflow = frame()
    b.ready(overflow)
    b.command(replacements[31])
    b.command(overflow)
    expect(b.api.poll([binding]).commands).toEqual(["nextTab"])
    children.forEach((child, index) => expect(child.postMessage).toHaveBeenCalledTimes(index % 2 === 0 ? 1 : 4))
    replacements.forEach(child => expect(child.postMessage).toHaveBeenCalledTimes(1))
    expect(overflow.postMessage).not.toHaveBeenCalled()
})
