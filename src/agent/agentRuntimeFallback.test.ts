import { afterEach, describe, expect, it, vi } from "vitest"
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks"
import { emit } from "@tauri-apps/api/event"

// listen 的每次註冊與釋放都要能數得到——失敗那次的 listener 不得留下。
// mockIPC 內部自行吃掉 plugin:event|* 呼叫，所以在模組層計數而非數 IPC。
const listenerCounts = vi.hoisted(() => ({ registered: 0, released: 0 }))
vi.mock("@tauri-apps/api/event", async (importOriginal) => {
    const actual = await importOriginal<typeof import("@tauri-apps/api/event")>()
    return {
        ...actual,
        listen: async (...args: Parameters<typeof actual.listen>) => {
            const unlisten = await actual.listen(...args)
            listenerCounts.registered += 1
            return () => {
                listenerCounts.released += 1
                unlisten()
            }
        },
    }
})

import { createAcpConnection } from "./acpConnection"
import { createFakeAcpAgentBridge } from "./fakeAcpAgent"
import {
    NO_AGENT_RUNTIME_MESSAGE,
    clearAgentRuntimePrerequisite,
    agentRuntimePrerequisite,
} from "./agentRuntime"

// #15 §3.4：runtime fallback 之後，整條 spawn → initialize → session/new →
// session/prompt 必須真的走得完（不是只驗指令字串），失敗後可 retry，且失敗那次
// 不留下 listener。
//
// 涵蓋範圍的誠實界線：這裡用的是 `src/agent/fakeAcpAgent.ts` 的行內 JSON-RPC 假件
// ＋ mockIPC 的假 `agent_spawn`，驅動的是**真實的 acpConnection 層**。因此本測試
// 證明的是「改寫後的指令會被送進 agent_spawn」「JSON-RPC 三段走得完」「失敗時
// listener 有被釋放」；它**不**證明 `npx -y` 在真實 shell 起得來（Windows 引號
// 屬 #35），也不涉及子行程清理——prerequisite 失敗發生在 spawn 之前，根本沒有
// 子行程可談，所以這裡不做任何「無殘留 process」的宣稱。
describe("curated agent runtime fallback end to end (#15)", () => {
    afterEach(() => {
        clearAgentRuntimePrerequisite()
        clearMocks()
    })

    it("completes a full session over the npx fallback and leaves nothing behind on failure", async () => {
        const processId = "agent-fallback"
        let runtimes = { bunx: false, deno: false, node: false, npx: false }
        const spawnedCommands: string[] = []
        const fake = createFakeAcpAgentBridge(
            (line) => emit("agent://stdout", { id: processId, line })
        )

        mockIPC((cmd, payload) => {
            if (cmd === "agent_detect_runtimes") return runtimes
            if (cmd === "agent_spawn") {
                spawnedCommands.push((payload as { command: string }).command)
                return processId
            }
            if (cmd === "agent_write") return fake.write((payload as { chunk: string }).chunk)
            if (cmd === "agent_kill") return undefined
            if (cmd === "agent_stderr_tail") return []
            return undefined
        }, { shouldMockEvents: true })

        const connection = createAcpConnection({
            command: "bunx pi-acp@0.0.32",
            bootstrapTimeoutMs: 30_000,
            initializeTimeoutMs: 5_000,
        })

        // 第一次：三者皆無 → 在 spawn 之前失敗（沒有 agent_spawn，因此也沒有子行程）。
        await expect(connection.newSession("/w")).rejects.toThrow(NO_AGENT_RUNTIME_MESSAGE)
        expect(spawnedCommands).toEqual([])
        expect(agentRuntimePrerequisite()?.reason).toBe("no-runtime")
        expect(listenerCounts.registered).toBeGreaterThan(0)
        expect(listenerCounts.released).toBe(listenerCounts.registered)

        // 第二次（retry）：裝好 Node → 以 `npx -y` fallback 完成整條流程。
        runtimes = { bunx: false, deno: false, node: true, npx: true }
        await expect(connection.newSession("/w")).resolves.toMatchObject({ sessionId: "fake-session" })
        expect(spawnedCommands).toEqual(["npx -y pi-acp@0.0.32"])
        expect(agentRuntimePrerequisite()).toBeNull()

        expect(fake.messages.map((message) => message.method).filter(Boolean))
            .toEqual(expect.arrayContaining(["initialize", "session/new"]))
        await expect(connection.prompt("fake-session", [{ type: "text", text: "hello" }]))
            .resolves.toBe("end_turn")
        expect(fake.messages.some((message) => message.method === "session/prompt")).toBe(true)
    })
})
