// P2 gate 驗證：單一 adapter process 上（鏡射 agentRouter「command+cwd 共用
// connection、多 session 多工」的使用形態）——同 cwd 兩 session 並行 prompt 各自
// 獨立、事件依 sessionId 正確分流、寫檔互不干擾；再於異 cwd 開第三個 session
// 驗證隔離。用法：
//   PI_ACP_PI_COMMAND=... bun fixtures/pi-acp-contract/multi-session.ts --command "node adapters/yuzora-pi-acp/dist/index.mjs"
import { mkdtemp, readFile } from "node:fs/promises"
import { tmpdir } from "node:os"
import { join } from "node:path"

import { AcpDriver } from "./driver"

function asRecord(value: unknown): Record<string, unknown> {
    return value && typeof value === "object" ? value as Record<string, unknown> : {}
}

const checks: { name: string; pass: boolean; detail?: string }[] = []
function check(name: string, pass: boolean, detail?: string) {
    checks.push({ name, pass, ...(detail ? { detail } : {}) })
    console.log(`${pass ? "PASS" : "FAIL"} ${name}${detail ? ` — ${detail}` : ""}`)
}

async function main() {
    const commandFlagIndex = process.argv.indexOf("--command")
    const command = commandFlagIndex !== -1
        ? process.argv[commandFlagIndex + 1]
        : "node adapters/yuzora-pi-acp/dist/index.mjs"
    const cwdA = await mkdtemp(join(tmpdir(), "yuzora-pi-multi-a-"))
    const cwdB = await mkdtemp(join(tmpdir(), "yuzora-pi-multi-b-"))
    // agentRouter 以 cwd 分 connection；此處驗的是「同一 connection 多 session」
    // ＋「另一 connection 異 cwd」兩型。
    const driverA = new AcpDriver(command, cwdA)
    const driverB = new AcpDriver(command, cwdB)
    try {
        await driverA.request("initialize", { protocolVersion: 1, clientCapabilities: {} }, 60_000)
        await driverB.request("initialize", { protocolVersion: 1, clientCapabilities: {} }, 60_000)

        const s1 = asRecord(await driverA.request("session/new", { cwd: cwdA, mcpServers: [] }, 120_000))
        const s2 = asRecord(await driverA.request("session/new", { cwd: cwdA, mcpServers: [] }, 120_000))
        const s3 = asRecord(await driverB.request("session/new", { cwd: cwdB, mcpServers: [] }, 120_000))
        const id1 = String(s1.sessionId)
        const id2 = String(s2.sessionId)
        const id3 = String(s3.sessionId)
        check("three sessions created with distinct ids", new Set([id1, id2, id3]).size === 3, `${id1} / ${id2} / ${id3}`)

        const promptText = (name: string, content: string) =>
            `Use your write tool to create a file named ${name} whose content is exactly: ${content}\nDo nothing else. Then reply with the single word: done`

        // 同 cwd 兩 session 並行。
        const [r1, r2] = await Promise.all([
            driverA.request("session/prompt", {
                sessionId: id1,
                prompt: [{ type: "text", text: promptText("alpha.txt", "one") }]
            }, 300_000),
            driverA.request("session/prompt", {
                sessionId: id2,
                prompt: [{ type: "text", text: promptText("beta.txt", "two") }]
            }, 300_000)
        ])
        check("s1 prompt end_turn", asRecord(r1).stopReason === "end_turn", String(asRecord(r1).stopReason))
        check("s2 prompt end_turn", asRecord(r2).stopReason === "end_turn", String(asRecord(r2).stopReason))

        const alpha = (await readFile(join(cwdA, "alpha.txt"), "utf8").catch(() => "(missing)")).trim()
        const beta = (await readFile(join(cwdA, "beta.txt"), "utf8").catch(() => "(missing)")).trim()
        check("alpha.txt written by s1", alpha === "one", alpha)
        check("beta.txt written by s2", beta === "two", beta)

        // 事件分流：每個 tool_call update 的 sessionId 必須指向持有該檔案任務的 session。
        const toolTargets = new Map<string, Set<string>>()
        for (const line of driverA.lines) {
            const msg = line.msg
            if (line.dir !== "a2c" || msg.method !== "session/update") continue
            const params = asRecord(msg.params)
            const update = asRecord(params.update)
            if (update.sessionUpdate !== "tool_call" && update.sessionUpdate !== "tool_call_update") continue
            const sessionId = String(params.sessionId)
            const rawInput = asRecord(update.rawInput)
            const path = typeof rawInput.path === "string" ? rawInput.path
                : typeof rawInput.file_path === "string" ? rawInput.file_path : undefined
            if (!path) continue
            const set = toolTargets.get(sessionId) ?? new Set<string>()
            set.add(path.includes("alpha") ? "alpha" : path.includes("beta") ? "beta" : path)
            toolTargets.set(sessionId, set)
        }
        const s1Targets = [...(toolTargets.get(id1) ?? [])]
        const s2Targets = [...(toolTargets.get(id2) ?? [])]
        check("s1 tool events only touch alpha", s1Targets.every((target) => target === "alpha") && s1Targets.length > 0, s1Targets.join(","))
        check("s2 tool events only touch beta", s2Targets.every((target) => target === "beta") && s2Targets.length > 0, s2Targets.join(","))

        // 異 cwd 隔離：s3 正常運作且 cwdB 不出現 cwdA 的檔案。
        const r3 = await driverB.request("session/prompt", {
            sessionId: id3,
            prompt: [{ type: "text", text: promptText("gamma.txt", "three") }]
        }, 300_000)
        check("s3 prompt end_turn (different cwd)", asRecord(r3).stopReason === "end_turn", String(asRecord(r3).stopReason))
        const gamma = (await readFile(join(cwdB, "gamma.txt"), "utf8").catch(() => "(missing)")).trim()
        check("gamma.txt written in cwd B", gamma === "three", gamma)
        const leaked = await readFile(join(cwdB, "alpha.txt"), "utf8").then(() => true).catch(() => false)
        check("no cross-cwd leakage", !leaked)
    } finally {
        await driverA.close()
        await driverB.close()
    }
    const failed = checks.filter((entry) => !entry.pass)
    console.log(failed.length === 0 ? `ALL ${checks.length} CHECKS PASSED` : `${failed.length} CHECKS FAILED`)
    process.exit(failed.length === 0 ? 0 : 1)
}

main().catch((error) => {
    console.error(error)
    process.exit(1)
})
