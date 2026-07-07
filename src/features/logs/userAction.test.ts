import { afterEach, expect, it } from "vitest"
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks"

import { logUserAction } from "./userAction"

afterEach(() => clearMocks())

it("logUserAction invokes log_event with a user_action envelope and defaults metadata to {}", async () => {
    mockIPC((cmd, payload) => {
        expect(cmd).toBe("log_event")
        expect(payload).toEqual({
            event: {
                level: "info",
                kind: "user_action",
                source: "ui",
                workspace_path: null,
                event: "file.open",
                message: "opened a.ts",
                metadata: {}
            }
        })
    })
    await expect(logUserAction("file.open", "opened a.ts")).resolves.toBeUndefined()
})

it("logUserAction forwards the provided metadata object", async () => {
    mockIPC((_cmd, payload) => {
        const p = payload as { event: { metadata: unknown } }
        expect(p.event.metadata).toEqual({ path: "a.ts", line: 10 })
    })
    await logUserAction("cursor.move", "moved cursor", { path: "a.ts", line: 10 })
})

it("logUserAction always sets level:info, kind:user_action, source:ui, workspace_path:null", async () => {
    mockIPC((_cmd, payload) => {
        const p = payload as { event: Record<string, unknown> }
        expect(p.event.level).toBe("info")
        expect(p.event.kind).toBe("user_action")
        expect(p.event.source).toBe("ui")
        expect(p.event.workspace_path).toBeNull()
    })
    await logUserAction("git.commit", "committed")
})

it("logUserAction resolves to undefined even when invoke resolves with a value", async () => {
    mockIPC(() => "unexpected raw ipc result")
    await expect(logUserAction("ok", "ok")).resolves.toBeUndefined()
})

it("logUserAction swallows an invoke rejection and still resolves to undefined", async () => {
    mockIPC(() => {
        throw new Error("log_event boom")
    })
    await expect(logUserAction("agent.spawn", "spawned agent")).resolves.toBeUndefined()
})
