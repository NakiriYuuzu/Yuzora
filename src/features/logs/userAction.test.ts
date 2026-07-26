import { afterEach, beforeEach, expect, it } from "vitest"
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks"

import { logUserAction, NO_WORKSPACE_REASON } from "./userAction"
import { useWorkspaceStore } from "@/state/workspaceStore"

// logUserAction 用 `.catch(() => undefined)` 吞掉所有失敗——包括 mockIPC handler
// 裡 `expect` 丟出的 AssertionError。因此**斷言一律寫在 handler 外面**，先把
// payload 收下來再比對；寫在 handler 裡的斷言永遠不會讓測試變紅。
interface CapturedEvent {
    level: string
    kind: string
    source: string
    workspace_path: string | null
    event: string
    message: string
    metadata: Record<string, unknown>
}

let captured: CapturedEvent[] = []

function captureIPC() {
    mockIPC((cmd, payload) => {
        if (cmd === "log_event") {
            captured.push((payload as { event: CapturedEvent }).event)
        }
    })
}

beforeEach(() => {
    captured = []
    useWorkspaceStore.setState({ workspacePath: null })
})

afterEach(() => {
    clearMocks()
    useWorkspaceStore.setState({ workspacePath: null })
})

it("logUserAction invokes log_event with a user_action envelope", async () => {
    let command: string | null = null
    mockIPC((cmd, payload) => {
        command = cmd
        captured.push((payload as { event: CapturedEvent }).event)
    })
    await expect(logUserAction("file.open", "opened a.ts")).resolves.toBeUndefined()

    expect(command).toBe("log_event")
    expect(captured).toHaveLength(1)
    expect(captured[0].level).toBe("info")
    expect(captured[0].kind).toBe("user_action")
    expect(captured[0].source).toBe("ui")
    expect(captured[0].event).toBe("file.open")
    expect(captured[0].message).toBe("opened a.ts")
})

it("logUserAction forwards the provided metadata object", async () => {
    captureIPC()
    useWorkspaceStore.setState({ workspacePath: "/w" })
    await logUserAction("cursor.move", "moved cursor", { path: "a.ts", line: 10 })

    expect(captured[0].metadata).toEqual({ path: "a.ts", line: 10 })
})

// --- issue #40 AC 第 2 條：structured workspace 或明確的 null reason -----------

it("logUserAction 帶上目前工作區的 structured workspace_path", async () => {
    captureIPC()
    useWorkspaceStore.setState({ workspacePath: "/Users/tester/projects/yuzora" })

    await logUserAction("git.commit", "committed")

    expect(captured[0].workspace_path).toBe("/Users/tester/projects/yuzora")
    // 有工作區時不該再掛 null reason。
    expect(captured[0].metadata).not.toHaveProperty("workspace_null_reason")
})

it("工作區切換後，後續的 user action 帶的是新的 workspace_path", async () => {
    captureIPC()
    useWorkspaceStore.setState({ workspacePath: "/first" })
    await logUserAction("a", "a")
    useWorkspaceStore.setState({ workspacePath: "/second" })
    await logUserAction("b", "b")

    expect(captured.map((event) => event.workspace_path)).toEqual(["/first", "/second"])
})

it("沒有開啟工作區時 workspace_path 是 null，且 metadata 說明 null reason", async () => {
    captureIPC()
    useWorkspaceStore.setState({ workspacePath: null })

    await logUserAction("app.start", "started")

    expect(captured[0].workspace_path).toBeNull()
    expect(captured[0].metadata).toEqual({ workspace_null_reason: NO_WORKSPACE_REASON })
})

it("null reason 不會覆蓋呼叫端自己的 metadata", async () => {
    captureIPC()
    await logUserAction("app.start", "started", { attempt: 2 })

    expect(captured[0].metadata).toEqual({
        attempt: 2,
        workspace_null_reason: NO_WORKSPACE_REASON,
    })
})

it("呼叫端可顯式覆寫 workspace，不受目前工作區影響", async () => {
    captureIPC()
    useWorkspaceStore.setState({ workspacePath: "/current" })

    await logUserAction("ssh.open", "opened", {}, "/explicit")

    expect(captured[0].workspace_path).toBe("/explicit")
})

it("顯式傳入 null 時仍會記下 null reason", async () => {
    captureIPC()
    useWorkspaceStore.setState({ workspacePath: "/current" })

    await logUserAction("ssh.open", "opened", {}, null)

    expect(captured[0].workspace_path).toBeNull()
    expect(captured[0].metadata).toEqual({ workspace_null_reason: NO_WORKSPACE_REASON })
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
