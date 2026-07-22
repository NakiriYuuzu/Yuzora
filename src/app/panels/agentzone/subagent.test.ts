import { describe, expect, it } from "vitest"

import { subagentInvocation } from "./subagent"

describe("subagentInvocation", () => {
    it("claude Agent/Task：subagent_type＋prompt＋title=任務短述", () => {
        expect(subagentInvocation("Audit error paths", {
            description: "Audit error paths",
            prompt: "Read git.rs and audit every error path.",
            subagent_type: "Explore"
        })).toEqual({
            kind: "spawn",
            agentType: "Explore",
            task: "Audit error paths",
            prompt: "Read git.rs and audit every error path."
        })
    })

    it("pi Agent：同構 rawInput＋model／run_in_background", () => {
        expect(subagentInvocation("Agent", {
            description: "Scan repo layout",
            prompt: "Map the repository structure.",
            subagent_type: "general-purpose",
            model: "haiku",
            run_in_background: true
        })).toEqual({
            kind: "spawn",
            agentType: "general-purpose",
            task: "Scan repo layout",
            prompt: "Map the repository structure.",
            model: "haiku",
            background: true
        })
    })

    it("pi 管理 tool：get_subagent_result／steer_subagent 以 agent_id 識別", () => {
        expect(subagentInvocation("get_subagent_result", { agent_id: "ag_1", wait: true })).toEqual({
            kind: "manage",
            agentId: "ag_1",
            task: "get_subagent_result"
        })
        expect(subagentInvocation("steer_subagent", { agent_id: "ag_2", message: "focus on tests" })?.kind).toBe("manage")
    })

    it("codex collab：receiverThreadIds／agentsStates 指紋", () => {
        expect(subagentInvocation("spawn_agent", {
            prompt: "Review the diff.",
            senderThreadId: "t-0",
            receiverThreadIds: ["t-1"],
            agentsStates: { "t-1": "running" },
            status: "running"
        })).toEqual({
            kind: "collab",
            agentType: "spawn_agent",
            task: "spawn_agent",
            prompt: "Review the diff."
        })
    })

    it("一般 tool（bash／read／question）不誤判", () => {
        expect(subagentInvocation("bash", { command: "ls" })).toBeNull()
        expect(subagentInvocation("read", { path: "/tmp/x" })).toBeNull()
        // prompt 存在但沒有 subagent_type（如 question tool 的自由欄位）不算 spawn。
        expect(subagentInvocation("question", { prompt: "which?" })).toBeNull()
        expect(subagentInvocation(undefined, undefined)).toBeNull()
    })
})
