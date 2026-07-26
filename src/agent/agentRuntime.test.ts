import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import {
    BUILTIN_NODE_MISSING_MESSAGE,
    DENO_UNSUPPORTED_RUNTIME_MESSAGE,
    NO_AGENT_RUNTIME_MESSAGE,
    agentRuntimePrerequisite,
    clearAgentRuntimePrerequisite,
    resolveAgentSpawnCommand,
    subscribeAgentRuntimePrerequisite,
} from "./agentRuntime"
import { agentDetectRuntimes } from "@/lib/ipc"
import { setCachedBuiltinPiAdapterCommandForTests } from "@/lib/platform"

vi.mock("@/lib/ipc", () => ({
    agentDetectRuntimes: vi.fn(),
}))

const detect = vi.mocked(agentDetectRuntimes)

describe("resolveAgentSpawnCommand (#15)", () => {
    beforeEach(() => {
        detect.mockReset()
        clearAgentRuntimePrerequisite()
    })

    afterEach(() => {
        setCachedBuiltinPiAdapterCommandForTests(null)
        clearAgentRuntimePrerequisite()
    })

    it("passes custom commands through without probing runtimes", async () => {
        await expect(resolveAgentSpawnCommand("fake-acp-agent")).resolves.toBe("fake-acp-agent")
        expect(detect).not.toHaveBeenCalled()
    })

    it("keeps a curated command on bunx when bunx is available", async () => {
        detect.mockResolvedValue({ bunx: true, deno: false, node: true, npx: true })
        await expect(resolveAgentSpawnCommand("bunx pi-acp@0.0.32"))
            .resolves.toBe("bunx pi-acp@0.0.32")
    })

    it("rewrites a curated command to npx -y when bunx is missing", async () => {
        detect.mockResolvedValue({ bunx: false, deno: false, node: true, npx: true })
        await expect(resolveAgentSpawnCommand("bunx pi-acp@0.0.32"))
            .resolves.toBe("npx -y pi-acp@0.0.32")
    })

    it("fails with an actionable message when no runtime exists", async () => {
        detect.mockResolvedValue({ bunx: false, deno: false, node: false, npx: false })
        await expect(resolveAgentSpawnCommand("bunx pi-acp@0.0.32"))
            .rejects.toThrow(NO_AGENT_RUNTIME_MESSAGE)
    })

    it("falls back to the original command when detection is unavailable", async () => {
        // 舊 backend／測試替身沒有 agent_detect_runtimes：維持既有行為交給 Rust preflight。
        detect.mockRejectedValue(new Error("unknown command"))
        await expect(resolveAgentSpawnCommand("bunx pi-acp@0.0.32"))
            .resolves.toBe("bunx pi-acp@0.0.32")

        detect.mockResolvedValue(undefined as never)
        await expect(resolveAgentSpawnCommand("bunx pi-acp@0.0.32"))
            .resolves.toBe("bunx pi-acp@0.0.32")
    })

    it("reports a deno-specific prerequisite instead of a generic failure", async () => {
        detect.mockResolvedValue({ bunx: false, deno: true, node: false, npx: false })
        await expect(resolveAgentSpawnCommand("bunx pi-acp@0.0.32"))
            .rejects.toThrow(DENO_UNSUPPORTED_RUNTIME_MESSAGE)
        expect(agentRuntimePrerequisite()).toEqual({
            reason: "deno-unsupported",
            runtimes: { bunx: false, deno: true, node: false, npx: false },
            command: "bunx pi-acp@0.0.32",
        })
    })

    it("records the probed runtimes with the no-runtime prerequisite", async () => {
        detect.mockResolvedValue({ bunx: false, deno: false, node: false, npx: false })
        await expect(resolveAgentSpawnCommand("bunx pi-acp@0.0.32"))
            .rejects.toThrow(NO_AGENT_RUNTIME_MESSAGE)
        expect(agentRuntimePrerequisite()).toEqual({
            reason: "no-runtime",
            runtimes: { bunx: false, deno: false, node: false, npx: false },
            command: "bunx pi-acp@0.0.32",
        })
    })

    it("names all three runtimes in the no-runtime message", () => {
        // 第 5 條：訊息不得只提 Bun 與 Node.js——Deno 有被探測，就要說明它的處置。
        for (const runtime of ["Bun", "Node.js", "Deno"]) {
            expect(NO_AGENT_RUNTIME_MESSAGE).toContain(runtime)
        }
    })

    it("publishes and clears the prerequisite to subscribers", async () => {
        const seen: (ReturnType<typeof agentRuntimePrerequisite>)[] = []
        const unsubscribe = subscribeAgentRuntimePrerequisite(() => {
            seen.push(agentRuntimePrerequisite())
        })
        try {
            detect.mockResolvedValue({ bunx: false, deno: false, node: false, npx: false })
            await expect(resolveAgentSpawnCommand("bunx pi-acp@0.0.32")).rejects.toThrow()
            expect(seen.at(-1)?.reason).toBe("no-runtime")

            // 成功解析後必須自行解除，否則使用者裝好 runtime 也會一直看到提示。
            detect.mockResolvedValue({ bunx: true, deno: false, node: true, npx: true })
            await expect(resolveAgentSpawnCommand("bunx pi-acp@0.0.32"))
                .resolves.toBe("bunx pi-acp@0.0.32")
            expect(seen.at(-1)).toBeNull()
            expect(agentRuntimePrerequisite()).toBeNull()
        } finally {
            unsubscribe()
        }
    })

    it("blocks the bundled pi adapter when node is missing", async () => {
        // builtin 指令是 `node "<path>"`：Rust preflight 對含引號的指令 fail open，
        // 前端不擋就會靜默 hang 到 initialize timeout。
        const builtin = 'node "/Applications/Yuzora.app/adapters/yuzora-pi-acp/index.mjs"'
        setCachedBuiltinPiAdapterCommandForTests(builtin)
        detect.mockResolvedValue({ bunx: false, deno: false, node: false, npx: false })
        await expect(resolveAgentSpawnCommand(builtin))
            .rejects.toThrow(BUILTIN_NODE_MISSING_MESSAGE)
        expect(agentRuntimePrerequisite()).toEqual({
            reason: "builtin-node-missing",
            runtimes: { bunx: false, deno: false, node: false, npx: false },
            command: builtin,
        })
    })

    it("passes the bundled pi adapter through and clears a standing prerequisite", async () => {
        // 真實情境：builtin 缺 node → banner → 使用者裝好 node → Retry。這一步是
        // 唯一會清掉 banner 的地方，所以測試必須從「已有 prerequisite」出發，
        // 否則 beforeEach 已清成 null，toBeNull() 是空斷言。
        const builtin = 'node "/Applications/Yuzora.app/adapters/yuzora-pi-acp/index.mjs"'
        setCachedBuiltinPiAdapterCommandForTests(builtin)
        detect.mockResolvedValue({ bunx: false, deno: false, node: false, npx: false })
        await expect(resolveAgentSpawnCommand(builtin)).rejects.toThrow(BUILTIN_NODE_MISSING_MESSAGE)
        expect(agentRuntimePrerequisite()?.reason).toBe("builtin-node-missing")

        detect.mockResolvedValue({ bunx: false, deno: false, node: true, npx: false })
        await expect(resolveAgentSpawnCommand(builtin)).resolves.toBe(builtin)
        expect(agentRuntimePrerequisite()).toBeNull()
    })

    // Blocking 回退守門：這三條 early return 都不登記 prerequisite，因此都必須清掉
    // 殘留狀態——否則舊 banner 會蓋掉之後真正的連線錯誤。
    it("clears a standing prerequisite on every non-prerequisite exit path", async () => {
        const seedPrerequisite = async () => {
            detect.mockResolvedValue({ bunx: false, deno: false, node: false, npx: false })
            await expect(resolveAgentSpawnCommand("bunx pi-acp@0.0.32")).rejects.toThrow()
            expect(agentRuntimePrerequisite()).not.toBeNull()
        }

        // (1) custom command 穿透
        await seedPrerequisite()
        await expect(resolveAgentSpawnCommand("foo-acp")).resolves.toBe("foo-acp")
        expect(agentRuntimePrerequisite()).toBeNull()

        // (2) 偵測 throw（舊 backend）
        await seedPrerequisite()
        detect.mockRejectedValue(new Error("unknown command"))
        await expect(resolveAgentSpawnCommand("bunx pi-acp@0.0.32"))
            .resolves.toBe("bunx pi-acp@0.0.32")
        expect(agentRuntimePrerequisite()).toBeNull()

        // (3) 回傳形狀不對
        await seedPrerequisite()
        detect.mockResolvedValue(undefined as never)
        await expect(resolveAgentSpawnCommand("bunx pi-acp@0.0.32"))
            .resolves.toBe("bunx pi-acp@0.0.32")
        expect(agentRuntimePrerequisite()).toBeNull()
    })

    it("does not treat a user's own node command as the bundled adapter", async () => {
        // custom 指令不得被 runtime 檢查攔截（第 4／6 條）：只有與 builtin cache
        // 完全相同的指令才算 builtin。
        setCachedBuiltinPiAdapterCommandForTests('node "/Applications/Yuzora.app/adapters/yuzora-pi-acp/index.mjs"')
        detect.mockResolvedValue({ bunx: false, deno: false, node: false, npx: false })
        await expect(resolveAgentSpawnCommand('node "/home/me/my-agent.mjs"'))
            .resolves.toBe('node "/home/me/my-agent.mjs"')
        expect(detect).not.toHaveBeenCalled()
        expect(agentRuntimePrerequisite()).toBeNull()
    })
})
