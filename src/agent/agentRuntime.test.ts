import { beforeEach, describe, expect, it, vi } from "vitest"
import { NO_AGENT_RUNTIME_MESSAGE, resolveAgentSpawnCommand } from "./agentRuntime"
import { agentDetectRuntimes } from "@/lib/ipc"

vi.mock("@/lib/ipc", () => ({
    agentDetectRuntimes: vi.fn(),
}))

const detect = vi.mocked(agentDetectRuntimes)

describe("resolveAgentSpawnCommand (#15)", () => {
    beforeEach(() => {
        detect.mockReset()
    })

    it("passes custom commands through without probing runtimes", async () => {
        await expect(resolveAgentSpawnCommand("fake-acp-agent")).resolves.toBe("fake-acp-agent")
        expect(detect).not.toHaveBeenCalled()
    })

    it("keeps a curated command on bunx when bunx is available", async () => {
        detect.mockResolvedValue({ bunx: true, deno: false, node: true, npx: true })
        await expect(resolveAgentSpawnCommand("bunx pi-acp@latest"))
            .resolves.toBe("bunx pi-acp@latest")
    })

    it("rewrites a curated command to npx -y when bunx is missing", async () => {
        detect.mockResolvedValue({ bunx: false, deno: false, node: true, npx: true })
        await expect(resolveAgentSpawnCommand("bunx pi-acp@latest"))
            .resolves.toBe("npx -y pi-acp@latest")
    })

    it("fails with an actionable message when no runtime exists", async () => {
        detect.mockResolvedValue({ bunx: false, deno: false, node: false, npx: false })
        await expect(resolveAgentSpawnCommand("bunx pi-acp@latest"))
            .rejects.toThrow(NO_AGENT_RUNTIME_MESSAGE)
    })

    it("falls back to the original command when detection is unavailable", async () => {
        // 舊 backend／測試替身沒有 agent_detect_runtimes：維持既有行為交給 Rust preflight。
        detect.mockRejectedValue(new Error("unknown command"))
        await expect(resolveAgentSpawnCommand("bunx pi-acp@latest"))
            .resolves.toBe("bunx pi-acp@latest")

        detect.mockResolvedValue(undefined as never)
        await expect(resolveAgentSpawnCommand("bunx pi-acp@latest"))
            .resolves.toBe("bunx pi-acp@latest")
    })
})
