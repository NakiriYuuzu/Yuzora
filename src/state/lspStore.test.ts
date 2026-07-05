import { describe, it, expect, beforeEach } from "vitest"

import { useLspStore, deriveDisplayState } from "./lspStore"
import type { LspProcessStatus, LspServerInfo } from "../lib/types"

function info(status: LspProcessStatus): LspServerInfo {
    return {
        workspace: "/ws",
        language: "typescript",
        serverId: "ts-server",
        command: "tsserver",
        path: null,
        status,
        lastStartupLog: null,
        lastError: null,
        restartCount: 0
    }
}

beforeEach(() => useLspStore.getState().reset())

describe("deriveDisplayState", () => {
    it("non-full grade → syntaxOnly", () => {
        expect(deriveDisplayState(info({ status: "starting" }), true, "limited")).toBe("syntaxOnly")
        expect(deriveDisplayState(null, false, "veryLongLine")).toBe("syntaxOnly")
    })
    it("missing → missing", () => {
        expect(deriveDisplayState(info({ status: "missing", installHint: "brew install" }), false, "full")).toBe("missing")
    })
    it("crashed → failed", () => {
        expect(deriveDisplayState(info({ status: "crashed", reason: "boom" }), false, "full")).toBe("failed")
    })
    it("stopped → syntaxOnly (deliberate stop, even if once initialized)", () => {
        expect(deriveDisplayState(info({ status: "stopped" }), true, "full")).toBe("syntaxOnly")
    })
    it("initialized → ready", () => {
        expect(deriveDisplayState(info({ status: "starting" }), true, "full")).toBe("ready")
    })
    it("otherwise → starting", () => {
        expect(deriveDisplayState(info({ status: "starting" }), false, "full")).toBe("starting")
        expect(deriveDisplayState(null, false, "full")).toBe("starting")
    })
})

describe("useLspStore", () => {
    it("setServerInfo / setInitialized / displayFor derive the display state", () => {
        const s = useLspStore.getState()
        s.setServerInfo(info({ status: "starting" }))
        s.setInitialized("typescript", true)

        const d = useLspStore.getState().displayFor("/ws/a.ts", "full")
        expect(d.language).toBe("typescript")
        expect(d.serverId).toBe("ts-server")
        expect(d.state).toBe("ready")
    })

    it("displayFor for an unsupported file type reports syntaxOnly", () => {
        const d = useLspStore.getState().displayFor("/ws/a.txt", "full")
        expect(d.state).toBe("syntaxOnly")
    })

    it("reset clears servers and initialized", () => {
        const s = useLspStore.getState()
        s.setServerInfo(info({ status: "starting" }))
        s.setInitialized("typescript", true)
        useLspStore.getState().reset()
        expect(useLspStore.getState().servers).toEqual({})
        expect(useLspStore.getState().initialized).toEqual({})
    })
})
